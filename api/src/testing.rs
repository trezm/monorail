//! Fixtures and helpers for route-handler unit tests.
//!
//! The service traits carry `#[cfg_attr(test, mockall::automock)]`, so a route
//! module's `#[cfg(test)]` tests configure `MockAuthProvider`,
//! `MockRailwayApi` and `MockSessionStore` with exactly the behaviour under
//! test, assemble them into an [`AppState`] here, and drive the module's
//! `router()` with `oneshot` — no middleware, no network, no Postgres. A mock
//! panics on any call it was not told to expect, so "never reaches Railway"
//! is asserted with `.never()` or by setting no expectation at all.
//!
//! `tests/api.rs` cannot use any of this — an external test crate compiles
//! the library without `cfg(test)` — so it declares its own mocks with
//! `mockall::mock!`.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

use crate::{
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat},
    routes::auth::SESSION_COOKIE,
    secret::Secret,
    services::{
        auth::{MockAuthProvider, TokenSet},
        railway::MockRailwayApi,
        session::{MockSessionStore, Session, User},
    },
    state::AppState,
};

/// A development-mode config whose database is deliberately unreachable —
/// port 1 refuses immediately rather than hanging, and the pool connects
/// lazily, so only a test that actually queries pays for it.
pub fn config() -> Config {
    Config {
        environment: Environment::Development,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        log_format: LogFormat::Pretty,
        log_filter: "warn".to_owned(),
        request_timeout: std::time::Duration::from_secs(5),
        body_limit_bytes: 64 * 1024,
        cors_origins: CorsOrigins::Disabled,
        database_url: DatabaseUrl::new("postgres://unused@127.0.0.1:1/unused"),
        database_pool_size: 1,
        database_connect_timeout: std::time::Duration::from_millis(50),
        session_ttl: std::time::Duration::from_hours(1),
        auth_success_redirect: "http://localhost:4321/".to_owned(),
    }
}

/// Assembles the mocks into an [`AppState`].
pub fn state(
    auth: MockAuthProvider,
    sessions: MockSessionStore,
    railway: MockRailwayApi,
) -> AppState {
    state_with_config(config(), auth, sessions, railway)
}

/// The same, under a different [`Config`] — for a test that needs, say, a
/// production environment.
pub fn state_with_config(
    config: Config,
    auth: MockAuthProvider,
    sessions: MockSessionStore,
    railway: MockRailwayApi,
) -> AppState {
    AppState::new(config, Arc::new(auth), Arc::new(railway)).with_sessions(Arc::new(sessions))
}

/// A state whose services expect nothing: reaching any of them fails the
/// test. For requests that must be rejected before a service is consulted.
pub fn untouched_state() -> AppState {
    state(
        MockAuthProvider::new(),
        MockSessionStore::new(),
        MockRailwayApi::new(),
    )
}

/// The token set a fresh login carries: good for another hour.
pub fn fresh_tokens() -> TokenSet {
    TokenSet {
        access_token: Secret::new("access-stub"),
        refresh_token: None,
        id_token: None,
        scope: "openid email".to_owned(),
        expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
    }
}

/// A token set whose access token is already spent, carrying whatever refresh
/// token the test wants a renewal to trade in.
pub fn expired_tokens(refresh_token: Option<Secret>) -> TokenSet {
    TokenSet {
        access_token: Secret::new("access-spent"),
        refresh_token,
        id_token: None,
        scope: "openid email".to_owned(),
        expires_at: chrono::Utc::now() - chrono::TimeDelta::seconds(1),
    }
}

/// A live session carrying `tokens`, for [`logged_in`] to answer with.
pub fn session(tokens: TokenSet) -> Session {
    Session {
        id: uuid::Uuid::nil(),
        user: User {
            id: uuid::Uuid::nil(),
            railway_user_id: "user_stub".to_owned(),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
        tokens,
        expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
    }
}

/// Expects lookups and answers them with a session carrying `tokens`,
/// returning the `Cookie` header value a logged-in browser would send.
pub fn logged_in(sessions: &mut MockSessionStore, tokens: TokenSet) -> String {
    let session = session(tokens);

    sessions
        .expect_lookup()
        .returning(move |_| Ok(Some(session.clone())));

    format!("{SESSION_COOKIE}=session-token")
}

/// Sends the request and decodes the JSON body, `Null` when there is none.
pub async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = raw(app, request).await;
    let status = response.status();

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("failed to read body")
        .to_bytes();

    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response was not valid JSON")
    };

    (status, body)
}

pub async fn raw(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("router failure")
}

pub fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("bad request")
}

pub fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("bad request")
}

pub fn post_json(uri: &str, cookie: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    builder
        .body(Body::from(body.to_string()))
        .expect("bad request")
}

pub fn post_empty(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    builder.body(Body::empty()).expect("bad request")
}

/// The `Set-Cookie` value for `name`, if the response carries one.
pub fn set_cookie_named(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
        .map(ToOwned::to_owned)
}
