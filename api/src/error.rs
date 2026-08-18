//! A single error type for handlers, and its wire representation.
//!
//! Handlers return [`ApiResult<T>`]. Anything unexpected can be `?`-ed into
//! [`ApiError::Internal`] via `anyhow`, which logs the full chain and returns an
//! opaque `500` so internal detail never reaches a client.

use axum::{
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::extract::Json;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request was syntactically fine but semantically invalid.
    #[error("{0}")]
    UnprocessableEntity(String),

    /// The request could not be understood at all (bad JSON, bad path segment).
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    /// The request conflicts with current state (duplicate key, version mismatch).
    #[error("{0}")]
    Conflict(String),

    #[error("authentication required")]
    Unauthorized,

    #[error("insufficient permissions")]
    Forbidden,

    /// Anything unexpected. The message is logged, never returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    #[must_use]
    pub fn not_found(resource: &str, id: impl std::fmt::Display) -> Self {
        Self::NotFound(format!("{resource} `{id}` does not exist"))
    }

    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// A stable, machine-readable identifier. Clients should branch on this
    /// rather than on the human-readable message.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnprocessableEntity(_) => "unprocessable_entity",
            Self::BadRequest(_) => "bad_request",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Internal(_) => "internal_error",
        }
    }
}

/// The JSON body every error response carries: `{"error": {"code", "message"}}`.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();

        let message = match &self {
            // Log the whole cause chain, tell the client nothing.
            Self::Internal(source) => {
                tracing::error!(error = ?source, "request failed with an unhandled error");
                "an internal error occurred".to_owned()
            }
            other => other.to_string(),
        };

        if status.is_client_error() {
            tracing::debug!(%status, code, %message, "request rejected");
        }

        (
            status,
            Json(ErrorBody {
                error: ErrorDetail { code, message },
            }),
        )
            .into_response()
    }
}

// Extractor rejections are remapped so that malformed input produces the same
// error envelope as everything else.
impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::JsonDataError(err) => Self::UnprocessableEntity(err.body_text()),
            other => Self::BadRequest(other.body_text()),
        }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}
