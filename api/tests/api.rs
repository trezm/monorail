//! End-to-end happy paths against the real router.
//!
//! `oneshot` drives the assembled `Router` in-process, so these exercise
//! routing, extractors, middleware and serialization together — without
//! binding a port. One test per endpoint walks the happy path, plus the
//! behaviour only the assembled application shows: middleware and the
//! fallback. Every other case — auth required, validation, error mapping —
//! is a unit test with mocks in the route module that owns the handler.
//!
//! The mocks are declared here with `mockall::mock!` because an external test
//! crate compiles the library without `cfg(test)` and cannot see the
//! `automock`-generated ones. The session mock is backed by a real map: the
//! login flows need state that survives across requests, which per-call
//! expectations cannot give.

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
    AppState, Secret,
    config::{Config, CorsOrigins, DatabaseUrl, Environment, LogFormat},
    routes::auth::{PENDING_COOKIE, SESSION_COOKIE},
    services::{
        auth::{AuthProvider, AuthResult, CsrfState, Pkce, RailwayIdentity, TokenSet},
        railway::{
            Deployment, Environment as RailwayEnvironment, Project, RailwayApi, RailwayResult,
            Service, ServiceInstance, ServiceSource,
        },
        session::{Session, SessionResult, SessionStore, SessionToken, User},
    },
};
use serde_json::Value;
use tower::ServiceExt as _;

mockall::mock! {
    Auth {}

    #[async_trait::async_trait]
    impl AuthProvider for Auth {
        fn authorize_url(&self, state: &CsrfState, pkce: &Pkce) -> String;
        async fn exchange_code(&self, code: &str, pkce: &Pkce) -> AuthResult<TokenSet>;
        async fn refresh(&self, refresh_token: &Secret) -> AuthResult<TokenSet>;
        async fn identity(&self, access_token: &Secret) -> AuthResult<RailwayIdentity>;
    }
}

mockall::mock! {
    Sessions {}

    #[async_trait::async_trait]
    impl SessionStore for Sessions {
        async fn begin(
            &self,
            identity: &RailwayIdentity,
            tokens: TokenSet,
        ) -> SessionResult<(SessionToken, Session)>;
        async fn lookup(&self, token: &SessionToken) -> SessionResult<Option<Session>>;
        async fn renew(&self, token: &SessionToken, tokens: &TokenSet) -> SessionResult<()>;
        async fn end(&self, token: &SessionToken) -> SessionResult<()>;
    }
}

mockall::mock! {
    Railway {}

    #[async_trait::async_trait]
    impl RailwayApi for Railway {
        async fn projects(&self, access_token: &Secret) -> RailwayResult<Vec<Project>>;
        async fn create_service(
            &self,
            access_token: &Secret,
            project_id: &str,
            source: &ServiceSource,
        ) -> RailwayResult<Service>;
        async fn environments(
            &self,
            access_token: &Secret,
            project_id: &str,
        ) -> RailwayResult<Vec<RailwayEnvironment>>;
        async fn service_instance(
            &self,
            access_token: &Secret,
            service_id: &str,
            environment_id: &str,
        ) -> RailwayResult<ServiceInstance>;
        async fn spin_down(
            &self,
            access_token: &Secret,
            service_id: &str,
            environment_id: &str,
        ) -> RailwayResult<()>;
        async fn spin_up(
            &self,
            access_token: &Secret,
            service_id: &str,
            environment_id: &str,
        ) -> RailwayResult<Deployment>;
    }
}

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
        // nothing — no test here queries it.
        database_url: DatabaseUrl::new("postgres://unused@127.0.0.1:1/unused"),
        database_pool_size: 1,
        database_connect_timeout: std::time::Duration::from_millis(50),
        session_ttl: std::time::Duration::from_hours(1),
        auth_success_redirect: "http://localhost:4321/".to_owned(),
    }
}

type SessionRows = Arc<Mutex<HashMap<Vec<u8>, Session>>>;

/// A [`SessionStore`] over a shared map. `renew` is left unexpected on
/// purpose: no happy path renews, so a renewal here is a failure.
fn memory_sessions() -> (MockSessions, SessionRows) {
    let rows = SessionRows::default();
    let mut sessions = MockSessions::new();

    let map = rows.clone();
    sessions.expect_begin().returning(move |identity, tokens| {
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

        map.lock()
            .expect("lock")
            .insert(token.digest(), session.clone());

        Ok((token, session))
    });

    let map = rows.clone();
    sessions
        .expect_lookup()
        .returning(move |token| Ok(map.lock().expect("lock").get(&token.digest()).cloned()));

    let map = rows.clone();
    sessions.expect_end().returning(move |token| {
        map.lock().expect("lock").remove(&token.digest());
        Ok(())
    });

    (sessions, rows)
}

/// An [`AuthProvider`] whose login always succeeds. The authorize URL still
/// carries the real state and challenge, so the pending cookie it produces is
/// the one the callback has to match. `refresh` is left unexpected: no happy
/// path renews a token.
fn login_auth() -> MockAuth {
    let mut auth = MockAuth::new();

    auth.expect_authorize_url().returning(|state, pkce| {
        format!(
            "https://provider.test/oauth/auth?state={}&code_challenge={}",
            state.as_str(),
            pkce.challenge()
        )
    });

    auth.expect_exchange_code()
        .withf(|code, _| code == "code-ok")
        .returning(|_, _| {
            Ok(TokenSet {
                access_token: Secret::new("access-stub"),
                refresh_token: None,
                id_token: None,
                scope: "openid email".to_owned(),
                expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
            })
        });

    auth.expect_identity().returning(|_| {
        Ok(RailwayIdentity {
            subject: "user_stub".to_owned(),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
        })
    });

    auth
}

/// The assembled application over the given Railway mock, with a login that
/// works and sessions that need no database.
fn app(railway: MockRailway) -> (Router, SessionRows) {
    let (sessions, rows) = memory_sessions();
    let state = AppState::new(test_config(), Arc::new(login_auth()), Arc::new(railway))
        .with_sessions(Arc::new(sessions));

    (monorail_api::app(state), rows)
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

fn post_json(uri: &str, cookie: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .body(Body::from(body.to_string()))
        .expect("bad request")
}

fn post_empty(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("bad request")
}

fn set_cookie_named(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
        .map(ToOwned::to_owned)
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
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

/// Reproduces what the login handler set, so the callback can be driven
/// directly without following a redirect through a real provider.
fn pending_cookie(state: &str, verifier: &str) -> String {
    format!("{PENDING_COOKIE}={state}.{verifier}")
}

#[tokio::test]
async fn liveness_reports_ok() {
    let (app, _) = app(MockRailway::new());

    let (status, body) = send(&app, get("/health/live")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn root_reports_service_identity() {
    let (app, _) = app(MockRailway::new());

    let (status, body) = send(&app, get("/")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "monorail-api");
    assert_eq!(body["environment"], "development");
}

#[tokio::test]
async fn every_response_carries_a_request_id() {
    let (app, _) = app(MockRailway::new());

    let response = raw(&app, get("/health/live")).await;

    assert!(response.headers().contains_key("x-request-id"));
}

/// The fallback lives on the assembled router, not in any route module, so
/// only a test here can see it.
#[tokio::test]
async fn unknown_routes_use_the_error_envelope() {
    let (app, _) = app(MockRailway::new());

    let (status, body) = send(&app, get("/does-not-exist")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn login_redirects_to_the_provider_and_remembers_the_attempt() {
    let (app, _) = app(MockRailway::new());

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

#[tokio::test]
async fn a_completed_callback_opens_a_session_and_clears_the_pending_cookie() {
    let (app, sessions) = app(MockRailway::new());

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
    assert_eq!(sessions.lock().expect("lock").len(), 1);

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
async fn a_logged_in_browser_reads_its_own_profile_and_can_log_out() {
    let (app, sessions) = app(MockRailway::new());
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
    assert!(sessions.lock().expect("lock").is_empty());

    let (status, _) = send(&app, get_with_cookie("/api/v1/users/me", &session_cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_logged_in_browser_reads_its_projects_and_their_services() {
    let mut railway = MockRailway::new();
    railway
        .expect_projects()
        .withf(|access_token| access_token.expose() == "access-stub")
        .times(1)
        .returning(|_| {
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
        });

    let (app, _) = app(railway);
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
}

#[tokio::test]
async fn a_service_is_created_from_a_docker_image() {
    let mut railway = MockRailway::new();
    railway
        .expect_create_service()
        .withf(|access_token, project_id, source| {
            access_token.expose() == "access-stub"
                && project_id == "project-1"
                && *source == ServiceSource::DockerImage("nginx:latest".to_owned())
        })
        .times(1)
        .returning(|_, _, _| {
            Ok(Service {
                id: "service-new".to_owned(),
                name: "shiny-new-service".to_owned(),
                created_at: None,
            })
        });

    let (app, _) = app(railway);
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_json(
            "/api/v1/projects/project-1/services",
            &session_cookie,
            &serde_json::json!({ "source": { "docker_image": "nginx:latest" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["id"], "service-new");
    assert_eq!(body["name"], "shiny-new-service");
}

#[tokio::test]
async fn a_logged_in_browser_reads_a_projects_environments() {
    let mut railway = MockRailway::new();
    railway
        .expect_environments()
        .times(1)
        .returning(|_, project_id| {
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
        });

    let (app, _) = app(railway);
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
async fn a_logged_in_browser_reads_a_services_instance() {
    let mut railway = MockRailway::new();
    railway
        .expect_service_instance()
        .times(1)
        .returning(|_, service_id, environment_id| {
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
        });

    let (app, _) = app(railway);
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
async fn a_logged_in_browser_spins_a_service_down() {
    let mut railway = MockRailway::new();
    railway
        .expect_spin_down()
        .withf(|_, service_id, environment_id| {
            service_id == "service-1" && environment_id == "env-1"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let (app, _) = app(railway);
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-down?environment=env-1",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
}

#[tokio::test]
async fn a_logged_in_browser_spins_a_service_back_up() {
    let mut railway = MockRailway::new();
    railway
        .expect_spin_up()
        .withf(|_, service_id, environment_id| {
            service_id == "service-1" && environment_id == "env-1"
        })
        .times(1)
        .returning(|_, _, _| {
            Ok(Deployment {
                id: "deploy-2".to_owned(),
                status: "BUILDING".to_owned(),
                created_at: None,
            })
        });

    let (app, _) = app(railway);
    let session_cookie = log_in(&app).await;

    let (status, body) = send(
        &app,
        post_empty(
            "/api/v1/services/service-1/spin-up?environment=env-1",
            &session_cookie,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["id"], "deploy-2");
    assert_eq!(body["status"], "BUILDING");
}
