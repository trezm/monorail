//! Drop-in replacements for axum's extractors that reject with [`ApiError`].
//!
//! Using these instead of `axum::Json` / `axum::extract::Query` / `axum::extract::Path`
//! means a malformed request produces the same JSON error envelope as a failure
//! raised inside a handler.
//!
//! [`CurrentUser`] is here for the same reason rather than a different one: it
//! is an extractor whose rejection is an [`ApiError`], and putting it in a
//! handler signature is what makes an endpoint require a login.

// The one module allowed to name axum's extractors: wrapping them is the
// whole point of it. Everywhere else //:clippy.toml makes that a build
// failure.
#![allow(clippy::disallowed_types)]

use axum::{
    extract::{FromRequest, FromRequestParts},
    http::request::Parts,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Serialize;

use crate::{
    dao::users::User, error::ApiError, routes::auth::SESSION_COOKIE,
    services::session::SessionToken, state::AppState,
};

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

/// The account behind the request's session cookie.
///
/// Rejects with `401` when there is no cookie, when it names no session, or
/// when that session has expired — the three are deliberately indistinguishable
/// to a caller. Expiry is checked against the stored row rather than the
/// cookie's `Max-Age`, which the client controls.
///
/// Costs one query per extraction. An endpoint that needs the user twice should
/// take it once and pass it down.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = CookieJar::from_headers(&parts.headers)
            .get(SESSION_COOKIE)
            .map(|cookie| SessionToken::new(cookie.value()))
            .ok_or(ApiError::Unauthorized)?;

        state
            .sessions()
            .lookup(&token)
            .await?
            .map(|session| Self(session.user))
            .ok_or(ApiError::Unauthorized)
    }
}
