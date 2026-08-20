//! A Railway service's per-environment instance.

use axum::{Router, extract::State, routing::get};
use serde::Deserialize;

use crate::{
    error::ApiResult,
    extract::{CurrentSession, Json, Path, Query},
    services::railway::ServiceInstance,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/services/{service_id}/instance", get(instance))
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
