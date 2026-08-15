//! Drop-in replacements for axum's extractors that reject with [`ApiError`].
//!
//! Using these instead of `axum::Json` / `axum::extract::Query` / `axum::extract::Path`
//! means a malformed request produces the same JSON error envelope as a failure
//! raised inside a handler.

use axum::{
    extract::{FromRequest, FromRequestParts},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::error::ApiError;

#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct Query<T>(pub T);

#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct Path<T>(pub T);
