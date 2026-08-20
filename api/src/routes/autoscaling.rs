//! A service's horizontal autoscaling rules.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get},
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    extract::{CurrentUser, Json, Path},
    services::autoscaling::{NewRule, Rule},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services/{service_id}/autoscaling", get(list).post(create))
        .route(
            "/services/{service_id}/autoscaling/{rule_id}",
            delete(remove),
        )
}

/// An object rather than a bare array, for the same reason `ProjectList` is.
#[derive(Debug, Serialize)]
pub struct RuleList {
    pub rules: Vec<Rule>,
}

async fn list(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<Json<RuleList>> {
    let rules = state.autoscaling().list(user.id, &service_id).await?;

    Ok(Json(RuleList { rules }))
}

/// `409` when the service already has a rule for the metric; the thresholds
/// are validated here so the database's CHECK constraints stay what they are —
/// invariants, not the error path.
async fn create(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<NewRule>,
) -> ApiResult<(StatusCode, Json<Rule>)> {
    if body.environment_id.trim().is_empty() {
        return Err(ApiError::UnprocessableEntity(
            "environment_id must not be empty".to_owned(),
        ));
    }
    if body.poll_frequency_secs <= 0 {
        return Err(ApiError::UnprocessableEntity(
            "poll_frequency_secs must be positive".to_owned(),
        ));
    }
    // NaN is rejected explicitly: it compares false both ways, so it would
    // otherwise be stored as a threshold nothing can ever cross.
    if body.min_threshold.is_nan() || body.min_threshold < 0.0 {
        return Err(ApiError::UnprocessableEntity(
            "min_threshold must not be negative".to_owned(),
        ));
    }
    if body.max_threshold.is_nan() || body.max_threshold <= body.min_threshold {
        return Err(ApiError::UnprocessableEntity(
            "max_threshold must be greater than min_threshold".to_owned(),
        ));
    }

    let rule = state
        .autoscaling()
        .create(user.id, &service_id, body)
        .await?;

    Ok((StatusCode::CREATED, Json(rule)))
}

/// `404` for an unknown rule and for another account's alike — this endpoint
/// does not confirm other people's rules exist.
async fn remove(
    State(state): State<AppState>,
    Path((service_id, rule_id)): Path<(String, Uuid)>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<StatusCode> {
    if state
        .autoscaling()
        .remove(user.id, &service_id, rule_id)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("autoscaling rule", rule_id))
    }
}
