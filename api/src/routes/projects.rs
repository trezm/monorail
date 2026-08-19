//! The signed-in user's Railway projects.

use axum::{Router, extract::State, routing::get};
use chrono::{TimeDelta, Utc};
use serde::Serialize;

use crate::{
    error::{ApiError, ApiResult},
    extract::{CurrentSession, Json},
    secret::Secret,
    services::{
        auth::AuthError,
        railway::Project,
        session::{Session, SessionToken},
    },
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

/// How much of an access token's remaining life is too little to start a
/// request with. Renewing a token that expires mid-flight costs one extra call
/// and avoids a `401` the user would see as a spurious logout.
const EXPIRY_SKEW_SECS: i64 = 60;

async fn list(
    State(state): State<AppState>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<ProjectList>> {
    let access_token = access_token(&state, &token, session).await?;
    let projects = state.railway().projects(&access_token).await?;

    Ok(Json(ProjectList { projects }))
}

/// The session's Railway access token, renewed first if it is spent.
///
/// A session lasts two weeks and the token it was opened with lasts about an
/// hour, so without this the dashboard stops working long before the login
/// does. A login granted without a refresh token cannot be renewed, and one the
/// provider no longer honours is equally final: both end as a `401`, which is
/// what sends the browser back through a login.
async fn access_token(
    state: &AppState,
    token: &SessionToken,
    session: Session,
) -> ApiResult<Secret> {
    let skew = TimeDelta::try_seconds(EXPIRY_SKEW_SECS).unwrap_or_else(TimeDelta::zero);

    if !session.tokens.is_expired_at(Utc::now() + skew) {
        return Ok(session.tokens.access_token);
    }

    let previous = session.tokens;
    let refresh_token = previous
        .refresh_token
        .as_ref()
        .ok_or(ApiError::Unauthorized)?;

    let mut renewed = match state.auth().refresh(refresh_token).await {
        Ok(renewed) => renewed,
        // A grant the provider has stopped honouring is the user's cue to log
        // in again, not a bad request from the browser that asked.
        Err(AuthError::InvalidGrant) => return Err(ApiError::Unauthorized),
        Err(error) => return Err(error.into()),
    };

    // Rotation is optional: a provider that returns no new refresh token means
    // the old one still stands, and overwriting it with nothing would end the
    // session at the next expiry.
    if renewed.refresh_token.is_none() {
        renewed.refresh_token = previous.refresh_token;
    }

    state.sessions().renew(token, &renewed).await?;
    tracing::debug!(user_id = %session.user.id, "renewed a Railway access token");

    Ok(renewed.access_token)
}
