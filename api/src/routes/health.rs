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

/// Readiness: the process can serve *useful* traffic right now. Check the things
/// a request would need — database pool, cache, migrations — and return an error
/// so the load balancer stops sending traffic.
async fn ready(state: axum::extract::State<AppState>) -> ApiResult<Json<Health>> {
    // Add dependency checks here, e.g.:
    //     state.db().ping().await.map_err(|e| ApiError::Internal(e.into()))?;

    Ok(Json(Health {
        status: "ready",
        version: VERSION,
        uptime_seconds: state.uptime().as_secs(),
    }))
}
