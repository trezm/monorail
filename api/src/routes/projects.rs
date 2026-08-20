//! The signed-in user's Railway projects.

use axum::{Router, extract::State, routing::get};
use serde::Serialize;

use crate::{
    error::ApiResult,
    extract::{CurrentSession, Json},
    services::railway::Project,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/projects", get(list))
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
