//! Fixtures for the in-crate tests.
//!
//! Compiled only under `cfg(test)`, so nothing here reaches a built artifact.
//!
//! A route test builds the real application with [`app`] and drives it through
//! `oneshot`, so it exercises routing, extractors, middleware and serialization
//! — everything a request meets except a socket. What it does not exercise is a
//! database: the services underneath are `mockall` doubles, which is what lets
//! a test say what a handler should ask for rather than arrange rows to imply
//! it.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

use crate::{
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat},
    services::auth::MockAuthProvider,
    state::AppState,
};

#[must_use]
pub fn config() -> Config {
    Config {
        environment: Environment::Development,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        log_format: LogFormat::Pretty,
        log_filter: "warn".to_owned(),
        request_timeout: Duration::from_secs(5),
        body_limit_bytes: 64 * 1024,
        cors_origins: CorsOrigins::Disabled,
        // Deliberately unreachable, and port 1 refuses immediately rather than
        // hanging. The pool connects lazily, so building the state costs
        // nothing and only a test that reaches the real DAOs pays for it.
        database_url: DatabaseUrl::new("postgres://unused@127.0.0.1:1/unused"),
        database_pool_size: 1,
        database_connect_timeout: Duration::from_millis(50),
        session_ttl: Duration::from_hours(1),
        auth_success_redirect: "http://localhost:4321/".to_owned(),
    }
}

/// State over [`config`], with an auth provider that panics if it is used.
///
/// Layer the doubles a test needs on top with `with_auth`, `with_sessions` and
/// the `with_*_dao` pair; whatever is left untouched is a mock with no
/// expectations, so reaching it is a failure rather than a silent success.
#[must_use]
pub fn state() -> AppState {
    AppState::new(config(), Arc::new(MockAuthProvider::new()))
}

/// The whole application, middleware included.
pub fn app(state: AppState) -> Router {
    crate::app(state)
}

/// Drives one request and reads the body as JSON. An empty body is
/// [`Value::Null`], which is how a `204` reads.
pub async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = raw(app, request).await;
    let status = response.status();

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("failed to read body")
        .to_bytes();

    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response was not valid JSON")
    };

    (status, body)
}

pub async fn raw(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("router failure")
}

#[must_use]
pub fn get(uri: &str) -> Request<Body> {
    request(Method::GET, uri, None)
}

#[must_use]
pub fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    request(Method::GET, uri, Some(cookie))
}

#[must_use]
pub fn delete_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    request(Method::DELETE, uri, Some(cookie))
}

fn request(method: Method, uri: &str, cookie: Option<&str>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(uri);

    match cookie {
        Some(cookie) => builder.header(header::COOKIE, cookie),
        None => builder,
    }
    .body(Body::empty())
    .expect("bad request")
}

/// The `Set-Cookie` header for `name`, whole, so attributes can be asserted on.
#[must_use]
pub fn set_cookie_named(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
        .map(ToOwned::to_owned)
}

/// Just the `name=value` pair of a `Set-Cookie`, ready to send back up.
#[must_use]
pub fn cookie_pair(response: &Response, name: &str) -> Option<String> {
    set_cookie_named(response, name).map(|cookie| {
        cookie
            .split(';')
            .next()
            .expect("a cookie always has a first field")
            .to_owned()
    })
}

#[must_use]
pub fn location(response: &Response) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}
