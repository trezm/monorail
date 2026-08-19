//! HTTP API service.
//!
//! The router lives in a library rather than the binary so integration tests can
//! build the real application with [`app`] and drive it in-process, without
//! binding a port.

pub mod config;
pub mod constants;
pub mod db;
pub mod error;
pub mod extract;
pub mod routes;
pub mod secret;
pub mod services;
pub mod shutdown;
pub mod state;
pub mod telemetry;

use std::{iter::once, net::SocketAddr, time::Duration};

use anyhow::Context as _;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{
        StatusCode,
        header::{AUTHORIZATION, COOKIE, SET_COOKIE},
    },
};
use std::sync::Arc;

use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::{
    LatencyUnit,
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    sensitive_headers::{SetSensitiveRequestHeadersLayer, SetSensitiveResponseHeadersLayer},
    timeout::TimeoutLayer,
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::{Level, Span};

pub use config::Config;
pub use db::Database;
pub use error::{ApiError, ApiResult};
pub use secret::Secret;
pub use services::auth::{AuthProvider, RailwayAuth};
pub use state::AppState;

/// Builds the fully-configured application.
///
/// Layer order matters. `ServiceBuilder` applies top-to-bottom on the way in, so
/// the request id is assigned before tracing opens a span (letting every log
/// line carry it), and the timeout sits inside the trace layer so timeouts are
/// still logged.
pub fn app(state: AppState) -> Router {
    let config = state.config().clone();

    let middleware = ServiceBuilder::new()
        // Redact credentials before anything can log them.
        .layer(SetSensitiveRequestHeadersLayer::new(
            once(AUTHORIZATION).chain(once(COOKIE)),
        ))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                )
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        // Turn a panicking handler into a 500 instead of dropping the connection.
        .layer(CatchPanicLayer::new())
        .layer(cors(&config))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ))
        .layer(CompressionLayer::new())
        .layer(SetSensitiveResponseHeadersLayer::new(once(SET_COOKIE)));

    Router::new()
        .merge(routes::health::router())
        .nest("/api/v1", routes::api_v1())
        .fallback(not_found)
        .layer(middleware)
        .layer(DefaultBodyLimit::max(config.body_limit_bytes))
        .with_state(state)
}

/// Binds the listener and serves until a shutdown signal arrives.
///
/// Postgres is reached before the listener is bound, so a bad connection string
/// or an unreachable database is a startup failure with a readable message
/// rather than a process that accepts traffic and answers `503` to all of it.
/// Migrations are not run here — that is `//api:migrate`.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let addr = SocketAddr::from((config.host, config.port));
    let oauth = config.railway_oauth.clone();
    let mut state = AppState::new(config);

    if let Some(oauth) = oauth {
        tracing::info!(issuer = %oauth.issuer, scopes = ?oauth.scopes, "Railway login enabled");
        state = state.with_auth(Arc::new(RailwayAuth::new(oauth)?));
    } else {
        tracing::warn!(
            "Railway login is disabled; set {} to enable it",
            constants::RAILWAY_CLIENT_ID
        );
    }

    state.db().ping().await.with_context(|| {
        format!(
            "database at {} is unreachable",
            state.config().database_url.redacted()
        )
    })?;
    tracing::info!(database = %state.config().database_url.redacted(), "database connected");

    let app = app(state);

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    let bound = listener.local_addr().context("failed to read local addr")?;
    tracing::info!(address = %bound, "server listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown::signal())
    .await
    .context("server error")?;

    tracing::info!("server stopped");
    Ok(())
}

fn make_span<B>(request: &Request<B>) -> Span {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .unwrap_or("unknown");

    tracing::info_span!(
        "http_request",
        method = %request.method(),
        path = %request.uri().path(),
        version = ?request.version(),
        request_id = %request_id,
    )
}

fn cors(config: &Config) -> CorsLayer {
    use config::CorsOrigins;

    let layer = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .max_age(Duration::from_secs(60 * 60));

    match &config.cors_origins {
        CorsOrigins::Disabled => CorsLayer::new(),
        CorsOrigins::Any => layer.allow_origin(Any),
        CorsOrigins::List(origins) => {
            let parsed: Vec<_> = origins
                .iter()
                .filter_map(|origin| match origin.parse() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!(%origin, "ignoring unparseable CORS origin");
                        None
                    }
                })
                .collect();

            layer.allow_origin(parsed)
        }
    }
}

async fn not_found(request: Request) -> ApiError {
    ApiError::NotFound(format!(
        "no route for {} {}",
        request.method(),
        request.uri().path()
    ))
}
