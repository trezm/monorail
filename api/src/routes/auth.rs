//! Login with Railway.
//!
//! These sit outside `/api/v1` because they are browser redirects rather than a
//! versioned JSON API — the same reason `health` does. The signed-in user is a
//! resource rather than a redirect, so it lives in [`crate::routes::users`].
//!
//! Two cookies carry the flow. The pending cookie holds the `state` and the
//! PKCE verifier for the ten minutes between the redirect out and the callback
//! back; comparing the `state` query parameter against it is the standard
//! double-submit check, and it is scoped to `/auth` so it is not attached to
//! anything else. The session cookie holds an opaque token whose digest is the
//! row's key.
//!
//! `SameSite=Lax` is required, not a preference: the callback is a top-level
//! cross-site GET, and `Strict` would withhold the pending cookie exactly when
//! it is needed. Neither cookie uses the `__Host-` prefix, which mandates
//! `Secure` and so cannot work over the `http://localhost` a local checkout
//! runs on; `Secure` is set everywhere except development instead.
//!
//! A failed login answers on the standard error envelope rather than
//! redirecting to a friendlier page. That is honest but raw — a user who
//! declines consent sees JSON — and swapping it for a redirect carrying an
//! error code is the obvious follow-up.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::Redirect,
    routing::{delete, get},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    config::Config,
    error::ApiResult,
    extract::Query,
    services::{
        auth::{AuthError, CsrfState, Pkce},
        session::SessionToken,
    },
    state::AppState,
};

/// Holds the session token. Scoped to the whole site, since any endpoint may
/// need to know who is calling.
pub const SESSION_COOKIE: &str = "monorail_session";

/// Holds a login in progress. Scoped to [`PENDING_PATH`] so it is not sent
/// anywhere it is not needed.
pub const PENDING_COOKIE: &str = "monorail_oauth";

const PENDING_PATH: &str = "/auth";

/// Long enough to read a consent screen, short enough that an abandoned login
/// does not linger. Browser-enforced only, which is fine: replaying a stale
/// pending cookie lets someone restart their own login and nothing else.
const PENDING_TTL: time::Duration = time::Duration::minutes(10);

/// Browser redirects, not JSON.
///
/// The session is the resource: `GET /auth/railway` starts one, the callback
/// completes it, and `DELETE /auth/session` ends it. The callback keeps its
/// path because it is the redirect URI registered on the OAuth app, which has
/// to match byte for byte.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/railway", get(login))
        .route("/auth/railway/callback", get(callback))
        .route("/auth/session", delete(logout))
}

/// The callback's query string. Success and failure are mutually exclusive, but
/// both shapes arrive at the same URL, so every field is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn login(State(state): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, Redirect)> {
    let auth = state.auth();

    let csrf = CsrfState::generate();
    let pkce = Pkce::generate();
    let destination = auth.authorize_url(&csrf, &pkce);

    Ok((
        jar.add(pending_cookie(state.config(), &csrf, &pkce)),
        Redirect::to(&destination),
    ))
}

async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<CallbackParams>,
) -> ApiResult<(CookieJar, Redirect)> {
    let auth = state.auth();

    let pending = jar
        .get(PENDING_COOKIE)
        .map(|cookie| cookie.value().to_owned());
    let jar = jar.remove(removal(state.config(), PENDING_COOKIE, PENDING_PATH));

    if let Some(error) = params.error.as_deref() {
        return Err(provider_refusal(error, params.error_description.as_deref()).into());
    }

    let (issued, verifier) = pending
        .as_deref()
        .and_then(|value| value.split_once('.'))
        .ok_or(AuthError::InvalidState)?;

    let returned = params.state.as_deref().ok_or(AuthError::InvalidState)?;

    if !CsrfState::new(issued).matches(returned) {
        return Err(AuthError::InvalidState.into());
    }

    let code = params.code.as_deref().ok_or(AuthError::InvalidState)?;

    let tokens = auth
        .exchange_code(code, &Pkce::from_verifier(verifier))
        .await?;
    let identity = auth.identity(&tokens.access_token).await?;
    let (token, session) = state.sessions().begin(&identity, tokens).await?;

    tracing::info!(user_id = %session.user.id, "login completed");

    Ok((
        jar.add(session_cookie(state.config(), &token)),
        Redirect::to(&state.config().auth_success_redirect),
    ))
}

async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state
            .sessions()
            .end(&SessionToken::new(cookie.value()))
            .await?;
    }

    Ok((
        jar.remove(removal(state.config(), SESSION_COOKIE, "/")),
        StatusCode::NO_CONTENT,
    ))
}

/// The provider redirected back with a refusal rather than a code (RFC 6749
/// §4.1.2.1). A declined consent screen is the user's decision, not a fault.
fn provider_refusal(error: &str, description: Option<&str>) -> AuthError {
    match error {
        "access_denied" => AuthError::Denied,
        other => AuthError::Provider(anyhow::anyhow!(
            "the provider refused the authorization request: {other}{}",
            description
                .map(|description| format!(" ({description})"))
                .unwrap_or_default()
        )),
    }
}

/// Both halves of a login in progress, in one cookie. The two values are
/// base64url, so `.` cannot occur inside either.
fn pending_cookie(config: &Config, state: &CsrfState, pkce: &Pkce) -> Cookie<'static> {
    harden(
        config,
        Cookie::new(
            PENDING_COOKIE,
            format!("{}.{}", state.as_str(), pkce.verifier()),
        ),
        PENDING_PATH,
        Some(PENDING_TTL),
    )
}

fn session_cookie(config: &Config, token: &SessionToken) -> Cookie<'static> {
    let seconds = i64::try_from(config.session_ttl.as_secs()).unwrap_or(i64::MAX);

    harden(
        config,
        Cookie::new(SESSION_COOKIE, token.expose().to_owned()),
        "/",
        Some(time::Duration::seconds(seconds)),
    )
}

/// A cookie only clears if the name *and* path match the one that was set.
fn removal(config: &Config, name: &'static str, path: &'static str) -> Cookie<'static> {
    harden(config, Cookie::new(name, ""), path, None)
}

fn harden(
    config: &Config,
    mut cookie: Cookie<'static>,
    path: &'static str,
    max_age: Option<time::Duration>,
) -> Cookie<'static> {
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path(path);
    cookie.set_secure(!config.environment.is_development());
    cookie.set_max_age(max_age);

    cookie
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use chrono::{TimeDelta, Utc};
    use uuid::Uuid;

    use super::{PENDING_COOKIE, SESSION_COOKIE};
    use crate::{
        config::Environment,
        dao::{sessions::MockSessionDao, users::User},
        db::DbError,
        secret::Secret,
        services::{
            auth::{AuthError, MockAuthProvider, RailwayIdentity, TokenSet},
            session::{MockSessionStore, Session, SessionError, SessionToken},
        },
        state::AppState,
        testing,
    };

    const SUBJECT: &str = "user_stub";

    /// A provider whose authorize URL echoes back the state and challenge the
    /// handler generated, so the pending cookie can be checked against it.
    fn provider() -> MockAuthProvider {
        let mut auth = MockAuthProvider::new();

        auth.expect_authorize_url().returning(|state, pkce| {
            format!(
                "https://provider.test/oauth/auth?state={}&code_challenge={}",
                state.as_str(),
                pkce.challenge()
            )
        });

        auth
    }

    fn identity() -> RailwayIdentity {
        RailwayIdentity {
            subject: SUBJECT.to_owned(),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
        }
    }

    fn tokens() -> TokenSet {
        TokenSet {
            access_token: Secret::new("access-stub"),
            refresh_token: None,
            id_token: None,
            scope: "openid email".to_owned(),
            expires_at: Utc::now() + TimeDelta::seconds(3600),
        }
    }

    fn user() -> User {
        User {
            id: Uuid::nil(),
            railway_user_id: SUBJECT.to_owned(),
            email: identity().email,
            name: identity().name,
            avatar_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn session() -> Session {
        Session {
            id: Uuid::nil(),
            user: user(),
            tokens: tokens(),
            expires_at: Utc::now() + TimeDelta::seconds(3600),
        }
    }

    /// A provider that completes the code exchange, for the callback tests.
    fn completing_provider() -> MockAuthProvider {
        let mut auth = provider();

        auth.expect_exchange_code()
            .withf(|code, _| code == "code-ok")
            .returning(|_, _| Ok(tokens()));
        auth.expect_identity().returning(|_| Ok(identity()));

        auth
    }

    /// Reproduces what the login handler set, so the callback can be driven
    /// directly without following a redirect through a real provider.
    fn pending(state: &str, verifier: &str) -> String {
        format!("{PENDING_COOKIE}={state}.{verifier}")
    }

    #[tokio::test]
    async fn login_redirects_to_the_provider_and_remembers_the_attempt() {
        let app = testing::app(testing::state().with_auth(Arc::new(provider())));

        let response = testing::raw(&app, testing::get("/auth/railway")).await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            testing::location(&response).starts_with("https://provider.test/oauth/auth?state="),
            "got {}",
            testing::location(&response)
        );

        let cookie = testing::set_cookie_named(&response, PENDING_COOKIE)
            .expect("pending cookie should be set");

        assert!(cookie.contains("HttpOnly"), "got {cookie}");
        assert!(cookie.contains("SameSite=Lax"), "got {cookie}");
        assert!(cookie.contains("Path=/auth"), "got {cookie}");
        assert!(cookie.contains("Max-Age=600"), "got {cookie}");
    }

    /// Development runs over plain http, so `Secure` would stop the cookie
    /// being sent at all. Anywhere else it must be there.
    #[tokio::test]
    async fn the_pending_cookie_is_secure_outside_development() {
        let mut config = testing::config();
        config.environment = Environment::Production;

        let app = testing::app(AppState::new(config, Arc::new(provider())));

        let response = testing::raw(&app, testing::get("/auth/railway")).await;
        let cookie = testing::set_cookie_named(&response, PENDING_COOKIE)
            .expect("pending cookie should be set");

        assert!(cookie.contains("Secure"), "got {cookie}");
    }

    /// No pending cookie means nothing to check the returned state against.
    /// The store is a mock with no expectations, so opening a session here
    /// would panic rather than pass.
    #[tokio::test]
    async fn a_callback_without_the_pending_cookie_is_rejected() {
        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(provider()))
                .with_sessions(Arc::new(MockSessionStore::new())),
        );

        let (status, body) = testing::send(
            &app,
            testing::get("/auth/railway/callback?code=code-ok&state=whatever"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn a_callback_whose_state_does_not_match_is_rejected() {
        let mut sessions = MockSessionStore::new();
        sessions.expect_begin().never();

        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(provider()))
                .with_sessions(Arc::new(sessions)),
        );

        let (status, body) = testing::send(
            &app,
            testing::get_with_cookie(
                "/auth/railway/callback?code=code-ok&state=attacker",
                &pending("issued", "verifier"),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    /// A declined consent screen is the user's decision, so it is a `403` and
    /// not the `503` that an unreachable provider would give.
    #[tokio::test]
    async fn a_declined_consent_screen_is_not_reported_as_an_outage() {
        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(provider()))
                .with_sessions(Arc::new(MockSessionStore::new())),
        );

        let (status, body) = testing::send(
            &app,
            testing::get_with_cookie(
                "/auth/railway/callback?error=access_denied&error_description=user%20said%20no",
                &pending("issued", "verifier"),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "forbidden");
    }

    /// The exchange has to carry the verifier out of the pending cookie, not a
    /// freshly generated one — get that wrong and every real login fails PKCE
    /// while a test that only checks the redirect still passes.
    #[tokio::test]
    async fn a_completed_callback_forwards_the_pending_verifier_and_opens_a_session() {
        let mut auth = provider();
        auth.expect_exchange_code()
            .withf(|code, pkce| code == "code-ok" && pkce.verifier() == "verifier")
            .times(1)
            .returning(|_, _| Ok(tokens()));
        auth.expect_identity()
            .withf(|access_token| access_token.expose() == "access-stub")
            .times(1)
            .returning(|_| Ok(identity()));

        let mut sessions = MockSessionStore::new();
        sessions
            .expect_begin()
            .withf(|identity, tokens| {
                identity.subject == SUBJECT && tokens.access_token.expose() == "access-stub"
            })
            .times(1)
            .returning(|_, _| Ok((SessionToken::new("opaque-token"), session())));

        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(auth))
                .with_sessions(Arc::new(sessions)),
        );

        let response = testing::raw(
            &app,
            testing::get_with_cookie(
                "/auth/railway/callback?code=code-ok&state=issued",
                &pending("issued", "verifier"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(testing::location(&response), "http://localhost:4321/");

        let session_cookie = testing::set_cookie_named(&response, SESSION_COOKIE)
            .expect("session cookie should be set");
        assert!(
            session_cookie.contains("opaque-token"),
            "got {session_cookie}"
        );
        assert!(session_cookie.contains("HttpOnly"), "got {session_cookie}");
        assert!(session_cookie.contains("Path=/"), "got {session_cookie}");

        let cleared = testing::set_cookie_named(&response, PENDING_COOKIE)
            .expect("pending cookie should be cleared");
        assert!(cleared.contains("Max-Age=0"), "got {cleared}");
    }

    /// The provider is reachable but refuses the grant. That is the user's
    /// problem to retry, not an outage, so it stays a `400`.
    #[tokio::test]
    async fn a_rejected_authorization_code_does_not_open_a_session() {
        let mut auth = provider();
        auth.expect_exchange_code()
            .returning(|_, _| Err(AuthError::InvalidGrant));

        let mut sessions = MockSessionStore::new();
        sessions.expect_begin().never();

        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(auth))
                .with_sessions(Arc::new(sessions)),
        );

        let response = testing::raw(
            &app,
            testing::get_with_cookie(
                "/auth/railway/callback?code=code-ok&state=issued",
                &pending("issued", "verifier"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            testing::set_cookie_named(&response, SESSION_COOKIE).is_none(),
            "a failed login should set no session cookie"
        );
    }

    /// The whole point of the round trip: the state and verifier the login
    /// handler minted are the ones the callback has to accept.
    #[tokio::test]
    async fn a_login_started_here_completes_here() {
        let mut sessions = MockSessionStore::new();
        sessions
            .expect_begin()
            .times(1)
            .returning(|_, _| Ok((SessionToken::new("opaque-token"), session())));

        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(completing_provider()))
                .with_sessions(Arc::new(sessions)),
        );

        let started = testing::raw(&app, testing::get("/auth/railway")).await;
        let issued = testing::location(&started);
        let issued_state = issued
            .split("state=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .expect("the redirect should carry a state");
        let pending_cookie =
            testing::cookie_pair(&started, PENDING_COOKIE).expect("pending cookie should be set");

        let completed = testing::raw(
            &app,
            testing::get_with_cookie(
                &format!("/auth/railway/callback?code=code-ok&state={issued_state}"),
                &pending_cookie,
            ),
        )
        .await;

        assert_eq!(completed.status(), StatusCode::SEE_OTHER);
        assert!(testing::set_cookie_named(&completed, SESSION_COOKIE).is_some());
    }

    #[tokio::test]
    async fn logout_revokes_the_session_behind_the_cookie_and_clears_it() {
        let mut sessions = MockSessionStore::new();
        sessions
            .expect_end()
            .withf(|token| token.expose() == "opaque-token")
            .times(1)
            .returning(|_| Ok(()));

        let app = testing::app(testing::state().with_sessions(Arc::new(sessions)));

        let response = testing::raw(
            &app,
            testing::delete_with_cookie("/auth/session", &format!("{SESSION_COOKIE}=opaque-token")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let cleared = testing::set_cookie_named(&response, SESSION_COOKIE)
            .expect("session cookie should be cleared");
        assert!(cleared.contains("Max-Age=0"), "got {cleared}");
    }

    /// Logging out without a cookie is not an error, and must not reach the
    /// store — `expect_end().never()` is the assertion that it does not.
    #[tokio::test]
    async fn logout_without_a_session_cookie_is_still_a_success() {
        let mut sessions = MockSessionStore::new();
        sessions.expect_end().never();

        let app = testing::app(testing::state().with_sessions(Arc::new(sessions)));

        let (status, _) = testing::send(
            &app,
            testing::delete_with_cookie("/auth/session", "unrelated=value"),
        )
        .await;

        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// The one test that leaves the session store real, so handler, store and
    /// DAO are all under test and only the rows are mocked. Every other test
    /// here cuts at one seam, which proves each layer against its own double
    /// but not that the layers are wired to each other.
    #[tokio::test]
    async fn a_login_reaches_the_dao_layer_through_the_real_store() {
        let mut sessions = MockSessionDao::new();
        sessions
            .expect_open_login()
            .times(1)
            .returning(|new_user, session, _| {
                assert_eq!(new_user.railway_user_id, SUBJECT);
                assert_eq!(new_user.email.as_deref(), Some("jane@example.test"));
                assert_eq!(
                    session.token_hash.len(),
                    32,
                    "the row is keyed on the digest, not the token"
                );
                assert_eq!(session.access_token.expose(), "access-stub");

                Ok((user(), Uuid::from_u128(2)))
            });

        let app = testing::app(
            testing::state()
                .with_auth(Arc::new(completing_provider()))
                .with_session_dao(Arc::new(sessions)),
        );

        let response = testing::raw(
            &app,
            testing::get_with_cookie(
                "/auth/railway/callback?code=code-ok&state=issued",
                &pending("issued", "verifier"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            testing::set_cookie_named(&response, SESSION_COOKIE).is_some(),
            "a session cookie should reach the browser"
        );
    }

    /// A store that cannot reach Postgres must surface as a retryable `503` on
    /// the standard envelope, not a `500`.
    #[tokio::test]
    async fn a_store_outage_during_logout_is_reported_as_unavailable() {
        let mut sessions = MockSessionStore::new();
        sessions.expect_end().times(1).returning(|_| {
            Err(SessionError::Database(DbError::Unavailable(
                anyhow::anyhow!("pool exhausted"),
            )))
        });

        let app = testing::app(testing::state().with_sessions(Arc::new(sessions)));

        let (status, body) = testing::send(
            &app,
            testing::delete_with_cookie("/auth/session", &format!("{SESSION_COOKIE}=opaque-token")),
        )
        .await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "service_unavailable");
    }
}
