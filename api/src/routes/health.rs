//! Liveness and readiness probes, plus a service-identity root.
//!
//! These are mounted outside `/api/v1` because orchestrators and load balancers
//! should not have to track the API version.

use axum::{Router, routing::get};
use serde::Serialize;

use crate::{error::ApiResult, extract::Json, state::AppState};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
}

#[derive(Debug, Serialize)]
pub struct ServiceInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub environment: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: u64,
}

async fn root(state: axum::extract::State<AppState>) -> Json<ServiceInfo> {
    Json(ServiceInfo {
        name: NAME,
        version: VERSION,
        environment: state.config().environment.as_str(),
    })
}

/// Liveness: the process is running and can serve. Never checks dependencies —
/// a dependency outage should not get the container killed.
async fn live(state: axum::extract::State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok",
        version: VERSION,
        uptime_seconds: state.uptime().as_secs(),
    })
}

/// Readiness: the process can serve *useful* traffic right now. Checks the
/// things a request would need — currently just Postgres — and answers `503` so
/// the load balancer stops sending traffic until they come back.
///
/// The check is a real round-trip, not a look at pool statistics: a pool that
/// has never dialled out reports itself perfectly healthy.
async fn ready(state: axum::extract::State<AppState>) -> ApiResult<Json<Health>> {
    state.db().ping().await?;

    Ok(Json(Health {
        status: "ready",
        version: VERSION,
        uptime_seconds: state.uptime().as_secs(),
    }))
}
