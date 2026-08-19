//! Railway login, over OAuth 2.0 and `OpenID` Connect.
//!
//! Railway is a standards-compliant provider; the endpoints below come from its
//! discovery document at `{issuer}/oauth/.well-known/openid-configuration`. The
//! flow is authorization code with PKCE:
//!
//! 1. [`AuthProvider::authorize_url`] sends the browser to Railway carrying a
//!    [`CsrfState`] and the challenge half of a [`Pkce`] pair.
//! 2. Railway redirects back with a code, which
//!    [`AuthProvider::exchange_code`] trades for a [`TokenSet`].
//! 3. [`AuthProvider::identity`] reads the user behind that token.
//!
//! The ID token's signature is not checked and no JWKS client exists here. The
//! token set arrives over TLS from a direct, client-authenticated call to the
//! token endpoint rather than through the browser, which `OpenID` Connect Core
//! §3.1.3.7 exempts from signature validation. Identity comes from the
//! userinfo endpoint, so nothing depends on parsing the JWT at all.

use std::fmt;

use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::{
    config::OAuthConfig,
    error::ApiError,
    secret::{Secret, random_token},
};

pub type AuthResult<T> = Result<T, AuthError>;

/// What can go wrong while authenticating someone.
///
/// Independent of [`ApiError`] so the flow stays usable outside HTTP; the `From`
/// impl below is the one place that decides how each case reaches a client.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The callback did not carry back the state this server issued. Either the
    /// login went stale or someone else started it.
    #[error("this login could not be verified; start again")]
    InvalidState,

    /// The provider rejected the authorization code — replayed, expired, or
    /// issued for different parameters.
    #[error("the authorization code was rejected")]
    InvalidGrant,

    /// The user said no at the consent screen.
    #[error("access was denied")]
    Denied,

    /// The provider was unreachable, or answered something unusable. Retryable,
    /// and nothing about the request was wrong.
    #[error("the identity provider is unavailable")]
    Provider(#[source] anyhow::Error),

    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::InvalidState | AuthError::InvalidGrant => {
                Self::BadRequest(error.to_string())
            }
            AuthError::Denied => Self::Forbidden,
            AuthError::Provider(source) => Self::Unavailable(source),
            AuthError::Backend(source) => Self::Internal(source),
        }
    }
}

/// The `state` parameter: an opaque value echoed back by the provider, so a
/// callback can be tied to the login that started it.
#[derive(Clone, PartialEq, Eq)]
pub struct CsrfState(String);

impl CsrfState {
    #[must_use]
    pub fn generate() -> Self {
        Self(random_token())
    }

    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compares against a value that arrived from a client, in time independent
    /// of how many leading characters happen to match.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        constant_time_eq(self.0.as_bytes(), candidate.as_bytes())
    }
}

/// Redacts: a leaked state lets an attacker complete someone else's login.
impl fmt::Debug for CsrfState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CsrfState([redacted])")
    }
}

/// A PKCE verifier and the challenge derived from it (RFC 7636).
///
/// The challenge goes to the provider in the authorization redirect; the
/// verifier stays on this side until the code exchange proves the two came from
/// the same login. `S256` is the only method Railway advertises.
#[derive(Clone)]
pub struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    #[must_use]
    pub fn generate() -> Self {
        Self::from_verifier(random_token())
    }

    /// Rebuilds the pair from a verifier held somewhere across the redirect.
    #[must_use]
    pub fn from_verifier(verifier: impl Into<String>) -> Self {
        let verifier = verifier.into();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

        Self {
            verifier,
            challenge,
        }
    }

    #[must_use]
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    #[must_use]
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// Redacts the verifier; the challenge is public by design but useless alone.
impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// What the token endpoint hands back.
///
/// `expires_at` is absolute: the wire format is a relative `expires_in`, which
/// is meaningless once stored.
#[derive(Debug, Clone)]
pub struct TokenSet {
    pub access_token: Secret,
    /// Only issued when the login asked for the `offline_access` scope.
    pub refresh_token: Option<Secret>,
    pub id_token: Option<Secret>,
    /// Space-separated, and not necessarily what was asked for — a provider may
    /// grant less.
    pub scope: String,
    pub expires_at: DateTime<Utc>,
}

impl TokenSet {
    #[must_use]
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }
}

/// The Railway account behind an access token, from the userinfo endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailwayIdentity {
    /// Railway's stable identifier for the user. The only claim guaranteed to
    /// be present, so it is what a local account is keyed on.
    #[serde(rename = "sub")]
    pub subject: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "picture", default)]
    pub avatar_url: Option<String>,
}

/// Authenticates a user against an OAuth 2.0 provider.
///
/// `#[async_trait]` boxes the returned futures so the trait stays
/// dyn-compatible: implementations are held as `Arc<dyn AuthProvider>` and
/// chosen at runtime, which is also what lets a test swap in a stub.
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync + 'static {
    /// Where to send the browser to start a login.
    fn authorize_url(&self, state: &CsrfState, pkce: &Pkce) -> String;

    /// Trades an authorization code for tokens. `pkce` must be the pair whose
    /// challenge went out with the authorization request.
    async fn exchange_code(&self, code: &str, pkce: &Pkce) -> AuthResult<TokenSet>;

    /// Trades a refresh token for a fresh set. Providers may rotate the refresh
    /// token, so the returned one replaces the one passed in.
    async fn refresh(&self, refresh_token: &Secret) -> AuthResult<TokenSet>;

    /// Reads the account behind an access token.
    async fn identity(&self, access_token: &Secret) -> AuthResult<RailwayIdentity>;
}

/// [`AuthProvider`] against Railway.
///
/// Every endpoint is derived from the configured issuer, so a test can point
/// this at a local server without a network or a real OAuth app.
pub struct RailwayAuth {
    config: OAuthConfig,
    http: reqwest::Client,
}

impl RailwayAuth {
    pub fn new(config: OAuthConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("could not build the OAuth HTTP client")?;

        Ok(Self { config, http })
    }

    fn endpoint(&self, path: &str) -> Url {
        self.config
            .issuer
            .join(path)
            .expect("issuer is a base URL and the path is a literal")
    }

    /// Both grants hit the same endpoint with the same client authentication
    /// and differ only in their form body.
    async fn token_request(&self, form: &[(&str, &str)]) -> AuthResult<TokenSet> {
        let response = self
            .http
            .post(self.endpoint("oauth/token"))
            .basic_auth(
                &self.config.client_id,
                Some(self.config.client_secret.expose()),
            )
            .form(form)
            .send()
            .await
            .map_err(|error| {
                AuthError::Provider(anyhow::Error::new(error).context("token request failed"))
            })?;

        let status = response.status();
        let body = response.bytes().await.map_err(|error| {
            AuthError::Provider(anyhow::Error::new(error).context("could not read token response"))
        })?;

        if !status.is_success() {
            return Err(token_error(status, &body));
        }

        let wire: TokenResponse = serde_json::from_slice(&body).map_err(|error| {
            AuthError::Provider(
                anyhow::Error::new(error).context("token response was not the expected shape"),
            )
        })?;

        Ok(wire.into_token_set(Utc::now()))
    }
}

#[async_trait::async_trait]
impl AuthProvider for RailwayAuth {
    fn authorize_url(&self, state: &CsrfState, pkce: &Pkce) -> String {
        let mut url = self.endpoint("oauth/auth");

        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("scope", &self.config.scopes.join(" "))
            .append_pair("state", state.as_str())
            .append_pair("code_challenge", pkce.challenge())
            .append_pair("code_challenge_method", "S256");

        url.into()
    }

    async fn exchange_code(&self, code: &str, pkce: &Pkce) -> AuthResult<TokenSet> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.config.redirect_uri),
            ("code_verifier", pkce.verifier()),
        ])
        .await
    }

    async fn refresh(&self, refresh_token: &Secret) -> AuthResult<TokenSet> {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
        ])
        .await
    }

    async fn identity(&self, access_token: &Secret) -> AuthResult<RailwayIdentity> {
        let response = self
            .http
            .get(self.endpoint("oauth/me"))
            .bearer_auth(access_token.expose())
            .send()
            .await
            .map_err(|error| {
                AuthError::Provider(anyhow::Error::new(error).context("userinfo request failed"))
            })?;

        let status = response.status();

        if !status.is_success() {
            return Err(AuthError::Provider(anyhow::anyhow!(
                "userinfo endpoint answered {status}"
            )));
        }

        response.json().await.map_err(|error| {
            AuthError::Provider(
                anyhow::Error::new(error).context("userinfo response was not the expected shape"),
            )
        })
    }
}

/// The token endpoint's success body. Private: [`TokenSet`] is the type callers
/// see, and it stores an absolute expiry instead of a relative one.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Secret,
    #[serde(default)]
    refresh_token: Option<Secret>,
    #[serde(default)]
    id_token: Option<Secret>,
    #[serde(default)]
    scope: String,
    expires_in: i64,
}

impl TokenResponse {
    fn into_token_set(self, now: DateTime<Utc>) -> TokenSet {
        let lifetime = TimeDelta::try_seconds(self.expires_in).unwrap_or_else(TimeDelta::zero);

        TokenSet {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            id_token: self.id_token,
            scope: self.scope,
            expires_at: now + lifetime,
        }
    }
}

/// RFC 6749 §5.2. Only the codes that mean something different to a caller are
/// distinguished; the rest is an unhealthy provider.
#[derive(Debug, Deserialize)]
struct OAuthErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn token_error(status: reqwest::StatusCode, body: &[u8]) -> AuthError {
    let Ok(parsed) = serde_json::from_slice::<OAuthErrorResponse>(body) else {
        return AuthError::Provider(anyhow::anyhow!("token endpoint answered {status}"));
    };

    match parsed.error.as_str() {
        "invalid_grant" => AuthError::InvalidGrant,
        "access_denied" => AuthError::Denied,
        other => AuthError::Provider(anyhow::anyhow!(
            "token endpoint rejected the request: {other}{}",
            parsed
                .error_description
                .map(|description| format!(" ({description})"))
                .unwrap_or_default()
        )),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{Router, http::HeaderMap, routing::get};

    use super::{
        AuthError, AuthProvider, CsrfState, OAuthConfig, Pkce, RailwayAuth, RailwayIdentity,
        Secret, TokenResponse, token_error,
    };
    use crate::{error::ApiError, extract::Json};

    /// RFC 7636 Appendix B: the one published verifier/challenge pair, which
    /// pins both the SHA-256 and the base64url-without-padding encoding.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn config(issuer: &str) -> OAuthConfig {
        OAuthConfig {
            issuer: issuer.parse().expect("issuer should parse"),
            client_id: "client-id".to_owned(),
            client_secret: Secret::new("client-secret"),
            redirect_uri: "http://localhost:8080/auth/railway/callback".to_owned(),
            scopes: vec!["openid".to_owned(), "project:member".to_owned()],
            timeout: Duration::from_secs(5),
        }
    }

    /// Serves `router` on an ephemeral port and points a provider at it, so the
    /// HTTP paths are exercised without a network or a real OAuth app.
    async fn provider_serving(router: Router) -> RailwayAuth {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("should bind an ephemeral port");
        let issuer = format!(
            "http://{}/",
            listener.local_addr().expect("should have an addr")
        );

        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });

        RailwayAuth::new(config(&issuer)).expect("client should build")
    }

    fn token_body() -> serde_json::Value {
        serde_json::json!({
            "access_token": "access-123",
            "refresh_token": "refresh-123",
            "id_token": "id-123",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "openid project:member",
        })
    }

    #[test]
    fn pkce_matches_the_rfc_7636_test_vector() {
        assert_eq!(Pkce::from_verifier(RFC_VERIFIER).challenge(), RFC_CHALLENGE);
    }

    #[test]
    fn generated_secrets_differ_and_stay_out_of_debug_output() {
        let (first, second) = (Pkce::generate(), Pkce::generate());
        assert_ne!(first.verifier(), second.verifier());
        assert_ne!(first.challenge(), second.challenge());
        assert!(!format!("{first:?}").contains(first.verifier()));

        let state = CsrfState::generate();
        assert_ne!(state.as_str(), CsrfState::generate().as_str());
        assert!(!format!("{state:?}").contains(state.as_str()));
    }

    #[test]
    fn state_matches_only_an_identical_value() {
        let state = CsrfState::new("abc123");

        assert!(state.matches("abc123"));
        assert!(!state.matches("abc124"));
        assert!(!state.matches("abc1234"));
        assert!(!state.matches(""));
    }

    #[test]
    fn authorize_url_carries_every_required_parameter() {
        let provider = RailwayAuth::new(config("https://backboard.railway.com/"))
            .expect("client should build");

        let url = url::Url::parse(&provider.authorize_url(
            &CsrfState::new("state-value"),
            &Pkce::from_verifier(RFC_VERIFIER),
        ))
        .expect("authorize URL should parse");

        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(
            url.as_str().split('?').next(),
            Some("https://backboard.railway.com/oauth/auth")
        );
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "client-id");
        assert_eq!(
            query["redirect_uri"],
            "http://localhost:8080/auth/railway/callback"
        );
        assert_eq!(query["scope"], "openid project:member");
        assert_eq!(query["state"], "state-value");
        assert_eq!(query["code_challenge"], RFC_CHALLENGE);
        assert_eq!(query["code_challenge_method"], "S256");
    }

    /// An issuer with a path must keep it: `Url::join` on a base without a
    /// trailing slash would drop the last segment.
    #[test]
    fn authorize_url_preserves_an_issuer_path() {
        let provider =
            RailwayAuth::new(config("https://example.test/idp/")).expect("client should build");

        let url = provider.authorize_url(&CsrfState::new("s"), &Pkce::from_verifier("v"));

        assert!(
            url.starts_with("https://example.test/idp/oauth/auth?"),
            "got {url}"
        );
    }

    #[test]
    fn relative_expiry_becomes_absolute() {
        let now = chrono::Utc::now();
        let response: TokenResponse =
            serde_json::from_value(token_body()).expect("body should deserialize");

        let tokens = response.into_token_set(now);

        assert_eq!(tokens.access_token.expose(), "access-123");
        assert_eq!(
            tokens.refresh_token.as_ref().map(Secret::expose),
            Some("refresh-123")
        );
        assert_eq!(tokens.scope, "openid project:member");
        assert_eq!(tokens.expires_at, now + chrono::TimeDelta::seconds(3600));
        assert!(!tokens.is_expired_at(now));
        assert!(tokens.is_expired_at(now + chrono::TimeDelta::seconds(3601)));
    }

    #[test]
    fn a_token_set_never_debug_prints_its_tokens() {
        let tokens = serde_json::from_value::<TokenResponse>(token_body())
            .expect("body should deserialize")
            .into_token_set(chrono::Utc::now());

        let rendered = format!("{tokens:?}");

        assert!(!rendered.contains("access-123"), "got {rendered}");
        assert!(!rendered.contains("refresh-123"), "got {rendered}");
        assert!(!rendered.contains("id-123"), "got {rendered}");
    }

    #[test]
    fn oauth_error_codes_map_to_distinct_failures() {
        let status = reqwest::StatusCode::BAD_REQUEST;

        assert!(matches!(
            token_error(status, br#"{"error":"invalid_grant"}"#),
            AuthError::InvalidGrant
        ));
        assert!(matches!(
            token_error(status, br#"{"error":"access_denied"}"#),
            AuthError::Denied
        ));
        assert!(matches!(
            token_error(
                status,
                br#"{"error":"server_error","error_description":"boom"}"#
            ),
            AuthError::Provider(_)
        ));
        assert!(matches!(
            token_error(status, b"<html>gateway timeout</html>"),
            AuthError::Provider(_)
        ));
    }

    #[test]
    fn failures_reach_clients_with_the_right_status() {
        use axum::http::StatusCode;

        assert_eq!(
            ApiError::from(AuthError::InvalidState).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::from(AuthError::InvalidGrant).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::from(AuthError::Denied).status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ApiError::from(AuthError::Provider(anyhow::anyhow!("down"))).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            ApiError::from(AuthError::Backend(anyhow::anyhow!("bug"))).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn exchange_code_sends_the_grant_and_reads_the_tokens() {
        let seen = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&seen);

        let router = Router::new().route(
            "/oauth/token",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let captured = Arc::clone(&captured);
                async move {
                    *captured.lock().expect("lock") = Some((headers, body));
                    Json(token_body())
                }
            }),
        );

        let tokens = provider_serving(router)
            .await
            .exchange_code("code-abc", &Pkce::from_verifier(RFC_VERIFIER))
            .await
            .expect("exchange should succeed");

        assert_eq!(tokens.access_token.expose(), "access-123");

        let (headers, body) = seen.lock().expect("lock").take().expect("request captured");
        let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();

        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "code-abc");
        assert_eq!(form["code_verifier"], RFC_VERIFIER);
        assert_eq!(
            form["redirect_uri"],
            "http://localhost:8080/auth/railway/callback"
        );
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ=")
        );
    }

    #[tokio::test]
    async fn a_rejected_grant_is_not_reported_as_an_outage() {
        let router = Router::new().route(
            "/oauth/token",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid_grant" })),
                )
            }),
        );

        let error = provider_serving(router)
            .await
            .exchange_code("stale", &Pkce::generate())
            .await
            .expect_err("a rejected code should fail");

        assert!(matches!(error, AuthError::InvalidGrant), "got {error:?}");
    }

    #[tokio::test]
    async fn identity_reads_the_userinfo_claims() {
        let seen = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&seen);

        let router = Router::new().route(
            "/oauth/me",
            get(move |headers: HeaderMap| {
                let captured = Arc::clone(&captured);
                async move {
                    *captured.lock().expect("lock") = Some(headers);
                    Json(serde_json::json!({
                        "sub": "user_abc123",
                        "email": "jane@example.test",
                        "name": "Jane Developer",
                        "picture": "https://avatars.example.test/jane",
                    }))
                }
            }),
        );

        let identity = provider_serving(router)
            .await
            .identity(&Secret::new("access-123"))
            .await
            .expect("userinfo should succeed");

        assert_eq!(
            identity,
            RailwayIdentity {
                subject: "user_abc123".to_owned(),
                email: Some("jane@example.test".to_owned()),
                name: Some("Jane Developer".to_owned()),
                avatar_url: Some("https://avatars.example.test/jane".to_owned()),
            }
        );
        assert_eq!(
            seen.lock()
                .expect("lock")
                .as_ref()
                .and_then(|headers| headers.get("authorization"))
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-123")
        );
    }

    /// `sub` is the only claim Railway guarantees, so the rest must be optional.
    #[tokio::test]
    async fn identity_tolerates_a_bare_subject() {
        let router = Router::new().route(
            "/oauth/me",
            get(|| async { Json(serde_json::json!({ "sub": "user_only" })) }),
        );

        let identity = provider_serving(router)
            .await
            .identity(&Secret::new("access-123"))
            .await
            .expect("userinfo should succeed");

        assert_eq!(identity.subject, "user_only");
        assert_eq!(identity.email, None);
    }
}
