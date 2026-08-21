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
use chrono::{DateTime, Utc};
use http_body_util::BodyExt as _;
use monorail_api::{
    AppState, RailwayAuth, Secret,
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat, OAuthConfig},
    routes::auth::{PENDING_COOKIE, SESSION_COOKIE},
    services::{
        auth::{AuthError, AuthProvider, AuthResult, CsrfState, Pkce, RailwayIdentity, TokenSet},
        autoscaling::{AutoscaleError, AutoscaleResult, AutoscaleStore, Metric, NewRule, Rule},
        railway::{
            Deployment, Environment as RailwayEnvironment, Measurement, MetricSample, Project,
            RailwayApi, RailwayError, RailwayResult, Service, ServiceInstance, ServiceSource,
        },
        session::{Session, SessionCredentials, SessionResult, SessionStore, SessionToken, User},
    },
};
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

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
        // The loop is only spawned by `run`, never by `app`; this documents
        // that no test here runs it.
        autoscaler_enabled: false,
        autoscaler_tick: std::time::Duration::from_secs(30),
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
    let auth = RailwayAuth::new(test_oauth()).expect("client should build");

    monorail_api::app(AppState::new(
        test_config(),
        Arc::new(auth),
        Arc::new(StubRailway::default()),
    ))
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

fn post_json(uri: &str, cookie: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    builder
        .body(Body::from(body.to_string()))
        .expect("bad request")
}

fn post_empty(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    builder.body(Body::empty()).expect("bad request")
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

impl StubAuth {
    /// The only refresh token this provider will trade in.
    const REFRESHED: &'static str = "refresh-ok";
}

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

    /// Only [`StubAuth::REFRESHED`] is honoured, so a test can tell a renewal
    /// that worked from one the provider has stopped accepting.
    async fn refresh(&self, refresh_token: &Secret) -> AuthResult<TokenSet> {
        if refresh_token.expose() != StubAuth::REFRESHED {
            return Err(AuthError::InvalidGrant);
        }

        Ok(TokenSet {
            access_token: Secret::new("access-renewed"),
            refresh_token: None,
            id_token: None,
            scope: "openid email".to_owned(),
            expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
        })
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

    async fn renew(&self, token: &SessionToken, tokens: &TokenSet) -> SessionResult<()> {
        if let Some(session) = self.rows.lock().expect("lock").get_mut(&token.digest()) {
            session.tokens = tokens.clone();
        }

        Ok(())
    }

    async fn end(&self, token: &SessionToken) -> SessionResult<()> {
        self.rows.lock().expect("lock").remove(&token.digest());
        Ok(())
    }

    async fn freshest_for_user(&self, _user_id: Uuid) -> SessionResult<Option<SessionCredentials>> {
        unreachable!("only the autoscaling loop reads sessions by user, and no route test runs it")
    }

    async fn renew_by_id(&self, _session_id: Uuid, _tokens: &TokenSet) -> SessionResult<()> {
        unreachable!("only the autoscaling loop renews by row id, and no route test runs it")
    }
}

/// A [`RailwayApi`] that answers without a network, and remembers which access
/// token it was handed — which is how the renewal tests tell a spent token from
/// a fresh one.
#[derive(Default)]
struct StubRailway {
    seen: Mutex<Vec<String>>,
    created: Mutex<Vec<(String, ServiceSource)>>,
    spun_down: Mutex<Vec<(String, String)>>,
    spun_up: Mutex<Vec<(String, String)>>,
}

impl StubRailway {
    /// The one project id creation declines, the way Railway declines a
    /// project the login cannot see.
    const REJECTED_PROJECT: &'static str = "project-forbidden";

    fn last_token(&self) -> Option<String> {
        self.seen.lock().expect("lock").last().cloned()
    }

    fn last_created(&self) -> Option<(String, ServiceSource)> {
        self.created.lock().expect("lock").last().cloned()
    }

    fn last_spun_down(&self) -> Option<(String, String)> {
        self.spun_down.lock().expect("lock").last().cloned()
    }

    fn last_spun_up(&self) -> Option<(String, String)> {
        self.spun_up.lock().expect("lock").last().cloned()
    }
}

#[async_trait::async_trait]
impl RailwayApi for StubRailway {
    async fn projects(&self, access_token: &Secret) -> RailwayResult<Vec<Project>> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        Ok(vec![
            Project {
                id: "project-1".to_owned(),
                name: "atlas".to_owned(),
                description: Some("the first one".to_owned()),
                created_at: None,
                services: vec![
                    Service {
                        id: "service-1".to_owned(),
                        name: "api".to_owned(),
                        created_at: None,
                    },
                    Service {
                        id: "service-2".to_owned(),
                        name: "worker".to_owned(),
                        created_at: None,
                    },
                ],
            },
            Project {
                id: "project-2".to_owned(),
                name: "beacon".to_owned(),
                description: None,
                created_at: None,
                services: Vec::new(),
            },
        ])
    }

    async fn create_service(
        &self,
        access_token: &Secret,
        project_id: &str,
        source: &ServiceSource,
    ) -> RailwayResult<Service> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        if project_id == Self::REJECTED_PROJECT {
            return Err(RailwayError::Rejected("Project not found".to_owned()));
        }

        self.created
            .lock()
            .expect("lock")
            .push((project_id.to_owned(), source.clone()));

        Ok(Service {
            id: "service-new".to_owned(),
            name: "shiny-new-service".to_owned(),
            created_at: None,
        })
    }

    /// Echoes the project id into the environment ids, so a test can assert the
    /// path parameter reached the stub.
    async fn environments(
        &self,
        access_token: &Secret,
        project_id: &str,
    ) -> RailwayResult<Vec<RailwayEnvironment>> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        Ok(vec![
            RailwayEnvironment {
                id: format!("{project_id}:production"),
                name: "production".to_owned(),
                created_at: None,
            },
            RailwayEnvironment {
                id: format!("{project_id}:staging"),
                name: "staging".to_owned(),
                created_at: None,
            },
        ])
    }

    /// `env-empty` is the environment nothing is deployed in.
    async fn service_instance(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<ServiceInstance> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        if environment_id == "env-empty" {
            return Err(RailwayError::NotFound(format!(
                "service `{service_id}` has no instance in `{environment_id}`"
            )));
        }

        Ok(ServiceInstance {
            id: format!("{service_id}:{environment_id}"),
            start_command: Some("bazel run //api".to_owned()),
            build_command: None,
            root_directory: None,
            healthcheck_path: Some("/health/ready".to_owned()),
            region: Some("us-west2".to_owned()),
            num_replicas: Some(2),
            restart_policy_type: Some("ON_FAILURE".to_owned()),
            restart_policy_max_retries: Some(10),
            latest_deployment: Some(Deployment {
                id: "deploy-1".to_owned(),
                status: "SUCCESS".to_owned(),
                created_at: None,
            }),
        })
    }

    async fn service_metrics(
        &self,
        access_token: &Secret,
        _service_id: &str,
        _measurement: Measurement,
        _since: DateTime<Utc>,
    ) -> RailwayResult<Vec<MetricSample>> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        Ok(Vec::new())
    }

    async fn set_replicas(
        &self,
        access_token: &Secret,
        _service_id: &str,
        _environment_id: &str,
        _replicas: i64,
    ) -> RailwayResult<()> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        Ok(())
    }

    /// `env-empty` mirrors `service_instance`; `service-parked` is one whose
    /// latest deployment is already gone.
    async fn spin_down(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<()> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        if environment_id == "env-empty" {
            return Err(RailwayError::NotFound(format!(
                "service `{service_id}` has no instance in `{environment_id}`"
            )));
        }

        if service_id == "service-parked" {
            return Err(RailwayError::Rejected(
                "the service is already spun down in this environment".to_owned(),
            ));
        }

        self.spun_down
            .lock()
            .expect("lock")
            .push((service_id.to_owned(), environment_id.to_owned()));

        Ok(())
    }

    /// `env-empty` mirrors `service_instance`; `service-running` is one with
    /// nothing removed to bring back.
    async fn spin_up(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<Deployment> {
        self.seen
            .lock()
            .expect("lock")
            .push(access_token.expose().to_owned());

        if environment_id == "env-empty" {
            return Err(RailwayError::NotFound(format!(
                "service `{service_id}` has no instance in `{environment_id}`"
            )));
        }

        if service_id == "service-running" {
            return Err(RailwayError::Rejected(
                "the service is not spun down in this environment".to_owned(),
            ));
        }

        self.spun_up
            .lock()
            .expect("lock")
            .push((service_id.to_owned(), environment_id.to_owned()));

        Ok(Deployment {
            id: "deploy-2".to_owned(),
            status: "BUILDING".to_owned(),
            created_at: None,
        })
    }
}

/// An [`AutoscaleStore`] in a vec, so the rule endpoints need no Postgres.
/// Only what the routes reach is real; the sweep half is unreachable because
/// no test here runs the loop.
#[derive(Default)]
struct MemoryAutoscale {
    rules: Mutex<Vec<Rule>>,
}

#[async_trait::async_trait]
impl AutoscaleStore for MemoryAutoscale {
    async fn create(&self, owner: Uuid, service_id: &str, rule: NewRule) -> AutoscaleResult<Rule> {
        let mut rules = self.rules.lock().expect("lock");

        if rules
            .iter()
            .any(|existing| existing.service_id == service_id && existing.metric == rule.metric)
        {
            return Err(AutoscaleError::Duplicate);
        }

        let row = Rule {
            service_id: service_id.to_owned(),
            metric: rule.metric,
            user_id: owner,
            environment_id: rule.environment_id,
            min_threshold: rule.min_threshold,
            max_threshold: rule.max_threshold,
            poll_frequency_secs: rule.poll_frequency_secs,
            last_checked: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        rules.push(row.clone());

        Ok(row)
    }

    async fn list(&self, owner: Uuid, service_id: &str) -> AutoscaleResult<Vec<Rule>> {
        Ok(self
            .rules
            .lock()
            .expect("lock")
            .iter()
            .filter(|rule| rule.user_id == owner && rule.service_id == service_id)
            .cloned()
            .collect())
    }

    async fn remove(&self, owner: Uuid, service_id: &str, metric: Metric) -> AutoscaleResult<bool> {
        let mut rules = self.rules.lock().expect("lock");
        let before = rules.len();

        rules.retain(|rule| {
            !(rule.service_id == service_id && rule.metric == metric && rule.user_id == owner)
        });

        Ok(rules.len() < before)
    }

    async fn due(&self, _now: DateTime<Utc>) -> AutoscaleResult<Vec<Rule>> {
        unreachable!("no route test runs the autoscaling loop")
    }

    async fn mark_checked(
        &self,
        _service_id: &str,
        _metric: Metric,
        _now: DateTime<Utc>,
    ) -> AutoscaleResult<()> {
        unreachable!("no route test runs the autoscaling loop")
    }
}

/// The router with a login that works, and sessions that do not need a database.
fn app_with_login() -> (Router, Arc<MemorySessions>) {
    let (app, sessions, _) = app_with_railway();

    (app, sessions)
}

/// The same, plus the Railway stub, for a test that has to see what was asked
/// of it.
fn app_with_railway() -> (Router, Arc<MemorySessions>, Arc<StubRailway>) {
    let sessions = Arc::new(MemorySessions::default());
    let railway = Arc::new(StubRailway::default());
    let state = AppState::new(test_config(), Arc::new(StubAuth), railway.clone())
        .with_sessions(sessions.clone())
        .with_autoscaling(Arc::new(MemoryAutoscale::default()));

    (monorail_api::app(state), sessions, railway)
}

/// Drives a whole login and returns the `Cookie` header value a browser would
/// send afterwards.
async fn log_in(app: &Router) -> String {
    let response = raw(
        app,
        get_with_cookie(
            "/auth/railway/callback?code=code-ok&state=issued",
            &pending_cookie("issued", "verifier"),
        ),
    )
    .await;

    set_cookie_named(&response, SESSION_COOKIE)
        .expect("session cookie should be set")
        .split(';')
        .next()
        .expect("cookie should have a value")
        .to_owned()
}

/// Opens a session directly on the store, so a test can choose the Railway
/// tokens it carries rather than take the ones the stub login mints.
async fn session_cookie_with(sessions: &MemorySessions, tokens: TokenSet) -> String {
    let identity = RailwayIdentity {
        subject: STUB_SUBJECT.to_owned(),
        email: None,
        name: None,
        avatar_url: None,
    };

    let (token, _) = sessions
        .begin(&identity, tokens)
        .await
        .expect("session should open");

    format!("{SESSION_COOKIE}={}", token.expose())
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

    let response = raw(&app, get("/auth/railway")).await;

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
        AppState::new(config, Arc::new(StubAuth), Arc::new(StubRailway::default()))
            .with_sessions(Arc::new(MemorySessions::default())),
    );

    let response = raw(&app, get("/auth/railway")).await;
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

    let (status, body) = send(&app, get("/api/v1/users/me")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn an_unknown_session_cookie_is_not_a_login() {
    let (app, _) = app_with_login();

    let (status, _) = send(
        &app,
        get_with_cookie("/api/v1/users/me", &format!("{SESSION_COOKIE}=made-up")),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_logged_in_browser_reads_its_own_profile_and_can_log_out() {
    let (app, sessions) = app_with_login();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(&app, get_with_cookie("/api/v1/users/me", &session_cookie)).await;

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
            .method("DELETE")
            .uri("/auth/session")
            .header(header::COOKIE, &session_cookie)
            .body(Body::empty())
            .expect("bad request"),
    )
    .await;

    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(sessions.rows.lock().expect("lock").is_empty());

    let (status, _) = send(&app, get_with_cookie("/api/v1/users/me", &session_cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_projects_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(&app, get("/api/v1/projects")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_logged_in_browser_reads_its_projects_and_their_services() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(&app, get_with_cookie("/api/v1/projects", &session_cookie)).await;

    assert_eq!(status, StatusCode::OK);

    let projects = body["projects"]
        .as_array()
        .expect("projects should be a list");
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0]["name"], "atlas");
    assert_eq!(projects[0]["services"][1]["name"], "worker");
    assert_eq!(
        projects[1]["services"]
            .as_array()
            .expect("services should be a list")
            .len(),
        0,
        "a project with no services is still a project"
    );

    assert_eq!(
        railway.last_token().as_deref(),
        Some("access-stub"),
        "the session's own token should reach Railway"
    );
}

#[tokio::test]
async fn creating_a_service_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            None,
            &serde_json::json!({ "source": { "docker_image": "nginx:latest" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_service_is_created_from_a_docker_image() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            Some(&session_cookie),
            &serde_json::json!({ "source": { "docker_image": "nginx:latest" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["id"], "service-new");
    assert_eq!(body["name"], "shiny-new-service");

    assert_eq!(
        railway.last_created(),
        Some((
            "project-1".to_owned(),
            ServiceSource::DockerImage("nginx:latest".to_owned())
        ))
    );
}

#[tokio::test]
async fn a_service_is_created_from_a_github_repo() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, _) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            Some(&session_cookie),
            &serde_json::json!({ "source": { "github_repo": "railwayapp/starters" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        railway.last_created(),
        Some((
            "project-1".to_owned(),
            ServiceSource::GithubRepo("railwayapp/starters".to_owned())
        ))
    );
}

#[tokio::test]
async fn a_blank_source_never_reaches_railway() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            Some(&session_cookie),
            &serde_json::json!({ "source": { "docker_image": "   " } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "unprocessable_entity");
    assert_eq!(body["error"]["message"], "docker_image must not be empty");
    assert_eq!(railway.last_created(), None);
}

/// Only the two supported sources deserialize; anything else is caught by the
/// extractor and answers on the standard envelope.
#[tokio::test]
async fn an_unsupported_source_kind_is_rejected() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            Some(&session_cookie),
            &serde_json::json!({ "source": { "helm_chart": "bitnami/nginx" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "unprocessable_entity");
    assert_eq!(railway.last_created(), None);
}

/// Railway declining the mutation is the caller's problem to fix, and its
/// message survives the trip — not a `503` pretending the provider is down.
#[tokio::test]
async fn railways_rejection_reaches_the_caller() {
    let (app, _, _) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-forbidden/services",
            Some(&session_cookie),
            &serde_json::json!({ "source": { "docker_image": "nginx:latest" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["message"], "Project not found");
}

#[tokio::test]
async fn the_environments_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(&app, get("/api/v1/projects/project-1/environments")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_logged_in_browser_reads_a_projects_environments() {
    let (app, _) = app_with_login();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        get_with_cookie("/api/v1/projects/project-1/environments", &session_cookie),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let environments = body["environments"]
        .as_array()
        .expect("environments should be a list");
    assert_eq!(environments.len(), 2);
    assert_eq!(environments[0]["name"], "production");
    assert_eq!(
        environments[0]["id"], "project-1:production",
        "the project id in the path should reach Railway"
    );
}

#[tokio::test]
async fn the_instance_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        get("/api/v1/services/service-1/instance?environment=env-1"),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_logged_in_browser_reads_a_services_instance() {
    let (app, _) = app_with_login();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        get_with_cookie(
            "/api/v1/services/service-1/instance?environment=env-1",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["id"], "service-1:env-1",
        "both identifiers should reach Railway"
    );
    assert_eq!(body["region"], "us-west2");
    assert_eq!(body["num_replicas"], 2);
    assert_eq!(body["latest_deployment"]["status"], "SUCCESS");
}

#[tokio::test]
async fn a_service_missing_from_an_environment_is_not_found() {
    let (app, _) = app_with_login();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        get_with_cookie(
            "/api/v1/services/service-1/instance?environment=env-empty",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn an_instance_request_without_an_environment_is_rejected() {
    let (app, _) = app_with_login();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        get_with_cookie("/api/v1/services/service-1/instance", &session_cookie),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

fn delete_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("bad request")
}

fn rule_body() -> Value {
    serde_json::json!({
        "environment_id": "env-1",
        "metric": "CPU",
        "min_threshold": 0.2,
        "max_threshold": 0.8,
        "poll_frequency_secs": 60,
    })
}

#[tokio::test]
async fn the_autoscaling_endpoints_require_a_session() {
    let (app, _, _) = app_with_railway();

    let (status, _) = send(&app, get("/api/v1/services/service-1/autoscaling")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &app,
        post_json("/api/v1/services/service-1/autoscaling", None, &rule_body()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_autoscaling_rule_round_trips() {
    let (app, _, _) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, created) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &rule_body(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["metric"], "CPU");
    assert_eq!(created["environment_id"], "env-1");
    assert_eq!(created["min_threshold"], 0.2);
    assert_eq!(created["max_threshold"], 0.8);
    assert_eq!(created["poll_frequency_secs"], 60);
    assert!(created["last_checked"].is_null());
    assert!(
        created.get("user_id").is_none(),
        "the owner should not be on the wire"
    );

    let (status, body) = send(
        &app,
        get_with_cookie("/api/v1/services/service-1/autoscaling", &session_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rules"][0]["metric"], "CPU");

    // A rule is addressed by its identity — the service and the metric.
    let response = raw(
        &app,
        delete_with_cookie(
            "/api/v1/services/service-1/autoscaling/CPU",
            &session_cookie,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (_, body) = send(
        &app,
        get_with_cookie("/api/v1/services/service-1/autoscaling", &session_cookie),
    )
    .await;
    assert_eq!(
        body["rules"]
            .as_array()
            .expect("rules should be a list")
            .len(),
        0
    );
}

#[tokio::test]
async fn the_spin_down_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-down?environment=env-1",
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_logged_in_browser_spins_a_service_down() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-down?environment=env-1",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    assert_eq!(
        railway.last_spun_down(),
        Some(("service-1".to_owned(), "env-1".to_owned()))
    );
}

#[tokio::test]
async fn a_second_rule_for_the_same_metric_is_a_conflict() {
    let (app, _, _) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, _) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &rule_body(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &rule_body(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict");
}

#[tokio::test]
async fn an_unusable_rule_is_rejected_with_the_reason() {
    let (app, _, _) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let mut inverted = rule_body();
    inverted["min_threshold"] = serde_json::json!(0.9);
    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &inverted,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["message"],
        "max_threshold must be greater than min_threshold"
    );

    let mut unpollable = rule_body();
    unpollable["poll_frequency_secs"] = serde_json::json!(0);
    let (status, _) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &unpollable,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let mut unknown_metric = rule_body();
    unknown_metric["metric"] = serde_json::json!("DISK");
    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/services/service-1/autoscaling",
            Some(&session_cookie),
            &unknown_metric,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "unprocessable_entity");
}

#[tokio::test]
async fn removing_an_unknown_rule_is_not_found() {
    let (app, _, _) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        delete_with_cookie(
            "/api/v1/services/service-1/autoscaling/MEMORY",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    // A segment naming no metric at all is caught by the extractor, on the
    // envelope like every other malformed path.
    let (status, body) = send(
        &app,
        delete_with_cookie(
            "/api/v1/services/service-1/autoscaling/DISK",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn spinning_down_a_service_missing_from_an_environment_is_not_found() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-down?environment=env-empty",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
    assert_eq!(railway.last_spun_down(), None);
}

/// Nothing running is the caller's situation, answered with Railway's own
/// message — not a `503` pretending the provider is down.
#[tokio::test]
async fn spinning_down_a_parked_service_is_rejected() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-parked/spin-down?environment=env-1",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["message"],
        "the service is already spun down in this environment"
    );
    assert_eq!(railway.last_spun_down(), None);
}

#[tokio::test]
async fn a_spin_down_without_an_environment_is_rejected() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-down",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    assert_eq!(railway.last_spun_down(), None);
}

#[tokio::test]
async fn the_spin_up_endpoint_requires_a_session() {
    let (app, _) = app_with_login();

    let (status, body) = send(
        &app,
        post_empty("/api/v1/services/service-1/spin-up?environment=env-1", None),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn a_logged_in_browser_spins_a_service_back_up() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-up?environment=env-1",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["id"], "deploy-2");
    assert_eq!(body["status"], "BUILDING");
    assert_eq!(
        railway.last_spun_up(),
        Some(("service-1".to_owned(), "env-1".to_owned()))
    );
}

#[tokio::test]
async fn spinning_up_a_service_missing_from_an_environment_is_not_found() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-up?environment=env-empty",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
    assert_eq!(railway.last_spun_up(), None);
}

/// Nothing spun down is the caller's situation, answered with Railway's own
/// message — not a `503` pretending the provider is down.
#[tokio::test]
async fn spinning_up_a_running_service_is_rejected() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-running/spin-up?environment=env-1",
            Some(&session_cookie),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["message"],
        "the service is not spun down in this environment"
    );
    assert_eq!(railway.last_spun_up(), None);
}

#[tokio::test]
async fn a_spin_up_without_an_environment_is_rejected() {
    let (app, _, railway) = app_with_railway();
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty("/api/v1/services/service-1/spin-up", Some(&session_cookie)),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    assert_eq!(railway.last_spun_up(), None);
}

/// A session outlives the access token it was opened with, so a stale one is
/// renewed in place rather than logging the user out.
#[tokio::test]
async fn an_expired_access_token_is_renewed_before_railway_is_read() {
    let (app, sessions, railway) = app_with_railway();

    let cookie = session_cookie_with(
        &sessions,
        TokenSet {
            access_token: Secret::new("access-spent"),
            refresh_token: Some(Secret::new(StubAuth::REFRESHED)),
            id_token: None,
            scope: "openid email".to_owned(),
            expires_at: chrono::Utc::now() - chrono::TimeDelta::seconds(1),
        },
    )
    .await;

    let (status, _) = send(&app, get_with_cookie("/api/v1/projects", &cookie)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(railway.last_token().as_deref(), Some("access-renewed"));

    let stored = sessions
        .rows
        .lock()
        .expect("lock")
        .values()
        .next()
        .expect("the session should still be there")
        .tokens
        .clone();

    assert_eq!(stored.access_token.expose(), "access-renewed");
    assert_eq!(
        stored.refresh_token.as_ref().map(Secret::expose),
        Some(StubAuth::REFRESHED),
        "a provider that rotates nothing should not lose the refresh token"
    );
}

/// A login that came back without a refresh token, or with one the provider no
/// longer honours, has nothing left to renew: both send the browser back
/// through a login rather than reporting a bad request.
#[tokio::test]
async fn an_unrenewable_access_token_asks_for_a_new_login() {
    let (app, sessions, railway) = app_with_railway();

    let expired = |refresh_token| TokenSet {
        access_token: Secret::new("access-spent"),
        refresh_token,
        id_token: None,
        scope: "openid email".to_owned(),
        expires_at: chrono::Utc::now() - chrono::TimeDelta::seconds(1),
    };

    for tokens in [expired(None), expired(Some(Secret::new("refresh-revoked")))] {
        let cookie = session_cookie_with(&sessions, tokens).await;

        let (status, body) = send(&app, get_with_cookie("/api/v1/projects", &cookie)).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    assert_eq!(
        railway.last_token(),
        None,
        "a spent token should never reach Railway"
    );
}
