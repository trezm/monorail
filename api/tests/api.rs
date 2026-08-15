//! End-to-end tests against the real router.
//!
//! `oneshot` drives the assembled `Router` directly, so these exercise routing,
//! extractors, middleware and serialization without binding a port.

use std::net::{IpAddr, Ipv4Addr};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt as _;
use monorail_api::{
    AppState,
    config::{Config, CorsOrigins, Environment, LogFormat},
};
use serde_json::{Value, json};
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
    }
}

fn app() -> Router {
    monorail_api::app(AppState::new(test_config()))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("router failure");
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

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("bad request")
}

#[tokio::test]
async fn liveness_reports_ok() {
    let (status, body) = send(&app(), get("/health/live")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn readiness_reports_ready() {
    let (status, body) = send(&app(), get("/health/ready")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
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

#[tokio::test]
async fn widget_lifecycle() {
    let app = app();

    // Empty to start.
    let (status, body) = send(&app, get("/api/v1/widgets")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pagination"]["total"], 0);
    assert_eq!(body["data"].as_array().expect("data array").len(), 0);

    // Create.
    let (status, created) = send(
        &app,
        json_request(
            "POST",
            "/api/v1/widgets",
            &json!({"name": "sprocket", "description": "spins"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "sprocket");

    let id = created["id"].as_str().expect("id").to_owned();

    // Read back.
    let (status, fetched) = send(&app, get(&format!("/api/v1/widgets/{id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["id"], id);

    // Partial update leaves description alone.
    let (status, updated) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/v1/widgets/{id}"),
            &json!({"name": "cog"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["name"], "cog");
    assert_eq!(updated["description"], "spins");

    // Explicit null clears it.
    let (status, cleared) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/v1/widgets/{id}"),
            &json!({"description": null}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["description"], Value::Null);

    // Delete, then it is gone.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/widgets/{id}"))
                .body(Body::empty())
                .expect("bad request"),
        )
        .await
        .expect("router failure");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (status, _) = send(&app, get(&format!("/api/v1/widgets/{id}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_sets_a_location_header() {
    let response = app()
        .oneshot(json_request(
            "POST",
            "/api/v1/widgets",
            &json!({"name": "sprocket"}),
        ))
        .await
        .expect("router failure");

    assert_eq!(response.status(), StatusCode::CREATED);
    let location = response
        .headers()
        .get(header::LOCATION)
        .expect("location header")
        .to_str()
        .expect("ascii");
    assert!(location.starts_with("/api/v1/widgets/"));
}

#[tokio::test]
async fn malformed_json_is_a_400() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/widgets")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .expect("bad request");

    let (status, body) = send(&app(), request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn semantically_invalid_json_is_a_422() {
    // Well-formed JSON, wrong shape: `name` must be a string.
    let (status, body) = send(
        &app(),
        json_request("POST", "/api/v1/widgets", &json!({"name": 42})),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "unprocessable_entity");
}

#[tokio::test]
async fn blank_names_are_rejected() {
    let (status, body) = send(
        &app(),
        json_request("POST", "/api/v1/widgets", &json!({"name": "  "})),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message")
            .contains("blank")
    );
}

#[tokio::test]
async fn non_uuid_path_segments_are_rejected() {
    let (status, body) = send(&app(), get("/api/v1/widgets/not-a-uuid")).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn out_of_range_limit_is_rejected() {
    let (status, _) = send(&app(), get("/api/v1/widgets?limit=1000")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = send(&app(), get("/api/v1/widgets?limit=0")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn list_paginates() {
    let app = app();

    for i in 0..3 {
        let (status, _) = send(
            &app,
            json_request("POST", "/api/v1/widgets", &json!({"name": format!("w{i}")})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) = send(&app, get("/api/v1/widgets?limit=2&offset=0")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pagination"]["total"], 3);
    assert_eq!(body["data"].as_array().expect("data array").len(), 2);
}
