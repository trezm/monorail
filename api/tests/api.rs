//! End-to-end tests across the crate's public boundary.
//!
//! What is left here is the wiring a route test cannot see: that `app` assembles
//! from outside the crate, that the middleware stack is in place, and that a
//! request which reaches no handler still answers on the error envelope.
//!
//! Behaviour of a route belongs in a `#[cfg(test)]` module beside the handler,
//! where the services under it can be `mockall` doubles — see
//! `src/routes/auth.rs`. Those tests drive the same assembled `Router` this one
//! does, so moving a case here buys fidelity nothing and costs it a database.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt as _;
use monorail_api::{
    AppState, RailwayAuth, Secret,
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat, OAuthConfig},
};
use serde_json::Value;
use tower::ServiceExt as _;

fn test_config() -> Config {
    Config {
        environment: Environment::Development,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        log_format: LogFormat::Pretty,
        log_filter: "warn".to_owned(),
        request_timeout: std::time::Duration::from_secs(5),
        body_limit_bytes: 64 * 1024,
        cors_origins: CorsOrigins::Disabled,
        // Deliberately unreachable, and port 1 refuses immediately rather than
        // hanging. The pool connects lazily, so building the state costs
        // nothing and only a test that queries pays for this — which is exactly
        // what `readiness_reports_unavailable_without_a_database` asserts.
        database_url: DatabaseUrl::new("postgres://unused@127.0.0.1:1/unused"),
        database_pool_size: 1,
        database_connect_timeout: std::time::Duration::from_millis(50),
        session_ttl: std::time::Duration::from_hours(1),
        auth_success_redirect: "http://localhost:4321/".to_owned(),
    }
}

/// Points at an issuer nothing listens on. No test here starts a login.
fn test_oauth() -> OAuthConfig {
    OAuthConfig {
        issuer: "http://127.0.0.1:1/".parse().expect("issuer should parse"),
        client_id: "test-client".to_owned(),
        client_secret: Secret::new("test-secret"),
        redirect_uri: "http://127.0.0.1:1/auth/railway/callback".to_owned(),
        scopes: vec!["openid".to_owned()],
        timeout: std::time::Duration::from_millis(50),
    }
}

fn app() -> Router {
    let auth = RailwayAuth::new(test_oauth()).expect("client should build");

    monorail_api::app(AppState::new(test_config(), Arc::new(auth)))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response: Response = app.clone().oneshot(request).await.expect("router failure");
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

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("bad request")
}

#[tokio::test]
async fn liveness_reports_ok() {
    let (status, body) = send(&app(), get("/health/live")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

/// Readiness round-trips a real query, so with no database it must fail — and
/// fail as a `503` on the standard envelope rather than a `500` or a panic.
/// The only test in the suite that reaches the real DAO stack.
#[tokio::test]
async fn readiness_reports_unavailable_without_a_database() {
    let (status, body) = send(&app(), get("/health/ready")).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "service_unavailable");
}

#[tokio::test]
async fn root_reports_service_identity() {
    let (status, body) = send(&app(), get("/")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "monorail-api");
    assert_eq!(body["environment"], "development");
}

#[tokio::test]
async fn every_response_carries_a_request_id() {
    let response = app()
        .oneshot(get("/health/live"))
        .await
        .expect("router failure");

    assert!(response.headers().contains_key("x-request-id"));
}

#[tokio::test]
async fn unknown_routes_use_the_error_envelope() {
    let (status, body) = send(&app(), get("/does-not-exist")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}
