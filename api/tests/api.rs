//! End-to-end tests against the real router.
//!
//! `oneshot` drives the assembled `Router` directly, so these exercise routing,
//! extractors, middleware and serialization without binding a port.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt as _;
use monorail_api::{
    AppState, RailwayAuth, Secret,
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat, OAuthConfig},
    routes::auth::{PENDING_COOKIE, SESSION_COOKIE},
    services::{
        auth::{AuthProvider, AuthResult, CsrfState, Pkce, RailwayIdentity, TokenSet},
        session::{Session, SessionResult, SessionStore, SessionToken, User},
    },
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
        railway_oauth: test_oauth(),
        session_ttl: std::time::Duration::from_hours(1),
        auth_success_redirect: "http://localhost:4321/".to_owned(),
    }
}

/// Points at an issuer nothing listens on. No test here reaches the provider;
/// the ones that do install [`StubAuth`] instead.
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
    let config = test_config();
    let auth = RailwayAuth::new(config.railway_oauth.clone()).expect("client should build");

    monorail_api::app(AppState::new(config, Arc::new(auth)))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
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

async fn raw(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("router failure")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("bad request")
}

fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("bad request")
}

/// Every `Set-Cookie` on a response, as raw header values.
fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .collect()
}

fn set_cookie_named(response: &Response, name: &str) -> Option<String> {
    set_cookies(response)
        .into_iter()
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// An [`AuthProvider`] that answers without a network or an OAuth app. The
/// authorize URL still carries the real state and challenge, so the pending
/// cookie it produces is the one the callback has to match.
struct StubAuth;

const STUB_SUBJECT: &str = "user_stub";

#[async_trait::async_trait]
impl AuthProvider for StubAuth {
    fn authorize_url(&self, state: &CsrfState, pkce: &Pkce) -> String {
        format!(
            "https://provider.test/oauth/auth?state={}&code_challenge={}",
            state.as_str(),
            pkce.challenge()
        )
    }

    async fn exchange_code(&self, code: &str, _pkce: &Pkce) -> AuthResult<TokenSet> {
        assert_eq!(code, "code-ok", "the handler should forward the query code");

        Ok(TokenSet {
            access_token: Secret::new("access-stub"),
            refresh_token: None,
            id_token: None,
            scope: "openid email".to_owned(),
            expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
        })
    }

    async fn refresh(&self, _refresh_token: &Secret) -> AuthResult<TokenSet> {
        unreachable!("the login flow never refreshes")
    }

    async fn identity(&self, _access_token: &Secret) -> AuthResult<RailwayIdentity> {
        Ok(RailwayIdentity {
            subject: STUB_SUBJECT.to_owned(),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
        })
    }
}

/// A [`SessionStore`] in a map, so the route tests need no Postgres.
#[derive(Default)]
struct MemorySessions {
    rows: Mutex<HashMap<Vec<u8>, Session>>,
}

#[async_trait::async_trait]
impl SessionStore for MemorySessions {
    async fn begin(
        &self,
        identity: &RailwayIdentity,
        tokens: TokenSet,
    ) -> SessionResult<(SessionToken, Session)> {
        let token = SessionToken::generate();
        let session = Session {
            id: uuid::Uuid::nil(),
            user: User {
                id: uuid::Uuid::nil(),
                railway_user_id: identity.subject.clone(),
                email: identity.email.clone(),
                name: identity.name.clone(),
                avatar_url: identity.avatar_url.clone(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            tokens,
            expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
        };

        self.rows
            .lock()
            .expect("lock")
            .insert(token.digest(), session.clone());

        Ok((token, session))
    }

    async fn lookup(&self, token: &SessionToken) -> SessionResult<Option<Session>> {
        Ok(self
            .rows
            .lock()
            .expect("lock")
            .get(&token.digest())
            .cloned())
    }

    async fn end(&self, token: &SessionToken) -> SessionResult<()> {
        self.rows.lock().expect("lock").remove(&token.digest());
        Ok(())
    }
}

/// The router with a login that works, and sessions that do not need a database.
fn app_with_login() -> (Router, Arc<MemorySessions>) {
    let sessions = Arc::new(MemorySessions::default());
    let state =
        AppState::new(test_config(), Arc::new(StubAuth)).with_sessions(sessions.clone());

    (monorail_api::app(state), sessions)
}

/// Reproduces what the login handler set, so the callback can be driven
/// directly without following a redirect through a real provider.
fn pending_cookie(state: &str, verifier: &str) -> String {
    format!("{PENDING_COOKIE}={state}.{verifier}")
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

#[tokio::test]
async fn login_redirects_to_the_provider_and_remembers_the_attempt() {
    let (app, _) = app_with_login();

    let response = raw(&app, get("/auth/railway/login")).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        location(&response).starts_with("https://provider.test/oauth/auth?state="),
        "got {}",
        location(&response)
    );

    let cookie = set_cookie_named(&response, PENDING_COOKIE).expect("pending cookie should be set");

    assert!(cookie.contains("HttpOnly"), "got {cookie}");
    assert!(cookie.contains("SameSite=Lax"), "got {cookie}");
    assert!(cookie.contains("Path=/auth"), "got {cookie}");
    assert!(cookie.contains("Max-Age=600"), "got {cookie}");
}

/// Development runs over plain http, so `Secure` would stop the cookie being
/// sent at all. Anywhere else it must be there.
#[tokio::test]
async fn the_session_cookie_is_secure_outside_development() {
    let mut config = test_config();
    config.environment = Environment::Production;

    let app = monorail_api::app(
        AppState::new(config, Arc::new(StubAuth))
            .with_sessions(Arc::new(MemorySessions::default())),
    );

    let response = raw(&app, get("/auth/railway/login")).await;
    let cookie = set_cookie_named(&response, PENDING_COOKIE).expect("pending cookie should be set");

    assert!(cookie.contains("Secure"), "got {cookie}");
}

#[tokio::test]
async fn a_callback_without_the_pending_cookie_is_rejected() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        get("/auth/railway/callback?code=code-ok&state=whatever"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn a_callback_whose_state_does_not_match_is_rejected() {
    let (app, sessions) = app_with_login();

    let (status, body) = send(
        &app,
        get_with_cookie(
            "/auth/railway/callback?code=code-ok&state=attacker",
            &pending_cookie("issued", "verifier"),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(
        sessions.rows.lock().expect("lock").is_empty(),
        "no session should be opened"
    );
}

#[tokio::test]
async fn a_declined_consent_screen_is_not_reported_as_an_outage() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        get_with_cookie(
            "/auth/railway/callback?error=access_denied&error_description=user%20said%20no",
            &pending_cookie("issued", "verifier"),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "forbidden");
}

#[tokio::test]
async fn a_completed_callback_opens_a_session_and_clears_the_pending_cookie() {
    let (app, sessions) = app_with_login();

    let response = raw(
        &app,
        get_with_cookie(
            "/auth/railway/callback?code=code-ok&state=issued",
            &pending_cookie("issued", "verifier"),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "http://localhost:4321/");
    assert_eq!(sessions.rows.lock().expect("lock").len(), 1);

    assert!(
        set_cookie_named(&response, SESSION_COOKIE)
            .expect("session cookie should be set")
            .contains("HttpOnly")
    );

    let cleared =
        set_cookie_named(&response, PENDING_COOKIE).expect("pending cookie should be cleared");
    assert!(cleared.contains("Max-Age=0"), "got {cleared}");
}

#[tokio::test]
async fn the_profile_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(&app, get("/api/v1/auth/me")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn an_unknown_session_cookie_is_not_a_login() {
    let (app, _) = app_with_login();

    let (status, _) = send(
        &app,
        get_with_cookie("/api/v1/auth/me", &format!("{SESSION_COOKIE}=made-up")),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_logged_in_browser_reads_its_own_profile_and_can_log_out() {
    let (app, sessions) = app_with_login();

    let login = raw(
        &app,
        get_with_cookie(
            "/auth/railway/callback?code=code-ok&state=issued",
            &pending_cookie("issued", "verifier"),
        ),
    )
    .await;

    let session_cookie = set_cookie_named(&login, SESSION_COOKIE)
        .expect("session cookie should be set")
        .split(';')
        .next()
        .expect("cookie should have a value")
        .to_owned();

    let (status, body) = send(&app, get_with_cookie("/api/v1/auth/me", &session_cookie)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "jane@example.test");
    assert_eq!(body["name"], "Jane Developer");
    assert!(
        body.get("railway_user_id").is_none(),
        "the profile should not mirror the row"
    );

    let logout = raw(
        &app,
        Request::builder()
            .method("POST")
            .uri("/auth/logout")
            .header(header::COOKIE, &session_cookie)
            .body(Body::empty())
            .expect("bad request"),
    )
    .await;

    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(sessions.rows.lock().expect("lock").is_empty());

    let (status, _) = send(&app, get_with_cookie("/api/v1/auth/me", &session_cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
