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
