//! A Railway service's per-environment instance, and actions on it.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::ApiResult,
    extract::{CurrentSession, Json, Path, Query},
    services::railway::ServiceInstance,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services/{service_id}/instance", get(instance))
        .route("/services/{service_id}/spin-down", post(spin_down))
        .route("/services/{service_id}/spin-up", post(spin_up))
}

/// The environment rides in the query string rather than the path because it
/// is the axis the UI's dropdown varies, not a resource this route owns.
#[derive(Debug, Deserialize)]
struct InstanceQuery {
    environment: String,
}

/// `404` when the service has no instance in that environment — a service does
/// not have to be deployed everywhere its project has environments.
async fn instance(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<ServiceInstance>> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let instance = state
        .railway()
        .service_instance(&access_token, &service_id, &query.environment)
        .await?;

    Ok(Json(instance))
}

/// Removes the service's latest deployment in that environment — a spin-down,
/// not a delete: the service and its configuration stay. `204` because removal
/// leaves nothing to describe; the UI refetches the instance for the
/// deployment's new state.
async fn spin_down(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<StatusCode> {
    let access_token = state.credentials().access_token(&token, session).await?;
    state
        .railway()
        .spin_down(&access_token, &service_id, &query.environment)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Redeploys what a spin-down removed — the inverse of `spin_down`, and `204`
/// for the same reason: the UI refetches the instance rather than trusting a
/// snapshot of a deployment that is only starting.
async fn spin_up(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<StatusCode> {
    let access_token = state.credentials().access_token(&token, session).await?;
    state
        .railway()
        .spin_up(&access_token, &service_id, &query.environment)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
