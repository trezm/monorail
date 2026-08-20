//! The signed-in user's Railway projects.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    error::{ApiError, ApiResult},
    extract::{CurrentSession, Json, Path},
    services::railway::{Project, Service, ServiceSource},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list))
        .route("/projects/{project_id}/services", post(create_service))
}

/// An object rather than a bare array, so a later addition — a cursor, a
/// workspace the projects belong to — is not a breaking change.
#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub projects: Vec<Project>,
}

/// Renewing the access token is [`Credentials`](crate::services::session::Credentials)'
/// job, not this one's: a session outlives by weeks the token it was opened
/// with, and every endpoint that acts on Railway needs the same answer.
async fn list(
    State(state): State<AppState>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<ProjectList>> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let projects = state.railway().projects(&access_token).await?;

    Ok(Json(ProjectList { projects }))
}

/// The body of `POST /projects/{project_id}/services`. An object holding the
/// source rather than the source alone, so a later knob — a name, a branch —
/// is an added field, not a reshaped body.
#[derive(Debug, Deserialize)]
pub struct NewService {
    pub source: ServiceSource,
}

/// Creation is a pass-through: the source is the only configurable thing, and
/// Railway owns every other setting — the name included.
async fn create_service(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    CurrentSession { token, session }: CurrentSession,
    Json(body): Json<NewService>,
) -> ApiResult<(StatusCode, Json<Service>)> {
    if body.source.value().is_empty() {
        return Err(ApiError::UnprocessableEntity(format!(
            "{} must not be empty",
            body.source.field()
        )));
    }

    let access_token = state.credentials().access_token(&token, session).await?;
    let service = state
        .railway()
        .create_service(&access_token, &project_id, &body.source)
        .await?;

    Ok((StatusCode::CREATED, Json(service)))
}
