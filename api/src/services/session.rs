//! Login sessions, and the local account behind one.
//!
//! A session is what turns a completed OAuth flow into something a browser can
//! present on later requests. The cookie carries an opaque
//! [`SessionToken`]; only its SHA-256 digest is written down, so a dump of the
//! `sessions` table yields no usable session.
//!
//! The Railway tokens ride along in the row because the point of logging in
//! with Railway is to act on Railway afterwards, and an access token that lives
//! an hour has to outlive the request that fetched it. `id_token` is not
//! persisted — its claims are already in `users` — but `scope` is, so a caller
//! can tell what a stored token may do without re-running consent.

use chrono::{DateTime, TimeDelta, Utc};
use diesel::{
    ExpressionMethods as _, Identifiable, OptionalExtension as _, QueryDsl as _, Queryable,
    Selectable, SelectableHelper as _,
};
use diesel_async::{AsyncConnection as _, RunQueryDsl as _};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    constants::ACCESS_TOKEN_EXPIRY_SKEW_SECS,
    db::{Database, DbError},
    error::ApiError,
    schema::{sessions, users},
    secret::{Secret, random_token},
    services::auth::{AuthError, AuthProvider, RailwayIdentity, TokenSet},
};

pub type SessionResult<T> = Result<T, SessionError>;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Database(#[from] DbError),

    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl From<diesel::result::Error> for SessionError {
    fn from(error: diesel::result::Error) -> Self {
        Self::Database(DbError::Query(error))
    }
}

impl From<SessionError> for ApiError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Database(source) => source.into(),
            SessionError::Backend(source) => Self::Internal(source),
        }
    }
}

/// The value a browser presents to prove it is logged in.
///
/// Opaque and unguessable rather than signed: the row it points at is the
/// authority, so revoking a session is a `DELETE` rather than a key rotation.
#[derive(Debug, Clone)]
pub struct SessionToken(Secret);

impl SessionToken {
    #[must_use]
    pub fn generate() -> Self {
        Self(Secret::new(random_token()))
    }

    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Secret::new(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose()
    }

    /// What is stored, in place of the token itself.
    ///
    /// A plain digest with no salt or stretching, deliberately: the input is
    /// 256 bits of uniform randomness, so there is no dictionary to defend
    /// against and a slow hash would only tax every authenticated request.
    #[must_use]
    pub fn digest(&self) -> Vec<u8> {
        Sha256::digest(self.0.expose().as_bytes()).to_vec()
    }
}

/// A Railway account this service has seen.
///
/// Keyed on the `sub` claim rather than the email address: `sub` is the only
/// claim Railway guarantees, and an email is both absent without the `email`
/// scope and mutable when present.
#[derive(Debug, Clone, PartialEq, Eq, Queryable, Selectable, Identifiable, Serialize)]
#[diesel(table_name = users, check_for_backend(diesel::pg::Pg))]
pub struct User {
    pub id: Uuid,
    pub railway_user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A live login: who it belongs to, and the Railway tokens it was opened with.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub user: User,
    pub tokens: TokenSet,
    pub expires_at: DateTime<Utc>,
}

/// Opens, reads and closes login sessions.
///
/// A trait for the same reason [`ContainerManager`](super::container::ContainerManager)
/// is one — handlers depend on the behaviour, not on Postgres — and because it
/// is what lets the route tests run with `MockSessionStore` and no database.
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync + 'static {
    /// Records the identity behind a completed login and opens a session for
    /// it, returning the token to hand the browser. The token is not
    /// recoverable afterwards.
    async fn begin(
        &self,
        identity: &RailwayIdentity,
        tokens: TokenSet,
    ) -> SessionResult<(SessionToken, Session)>;

    /// The session behind a token, or `None` if there is none or it has
    /// expired. Expiry is enforced here rather than trusted from the cookie.
    async fn lookup(&self, token: &SessionToken) -> SessionResult<Option<Session>>;

    /// Replaces the Railway tokens on a live session, after a refresh.
    ///
    /// Does not extend the session itself: how long a login lasts is this
    /// service's decision and not the provider's. A token naming no session is
    /// not an error — the session ended while the refresh was in flight, and
    /// the caller has nothing left to write to.
    async fn renew(&self, token: &SessionToken, tokens: &TokenSet) -> SessionResult<()>;

    /// Revokes a session. Absent is success: logging out twice is not an error.
    async fn end(&self, token: &SessionToken) -> SessionResult<()>;
}

pub type CredentialResult<T> = Result<T, CredentialError>;

/// What can go wrong while producing a usable Railway credential.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The access token is spent and there is nothing left to renew it with —
    /// no refresh token was issued, or the one that was is no longer honoured.
    /// Only a new login fixes it.
    #[error("this login can no longer act on Railway; sign in again")]
    Spent,

    #[error(transparent)]
    Provider(#[from] AuthError),

    #[error(transparent)]
    Store(#[from] SessionError),
}

impl From<CredentialError> for ApiError {
    fn from(error: CredentialError) -> Self {
        match error {
            // A `401` and not a `403`: nothing about the request was wrong, and
            // starting a new login is what fixes it.
            CredentialError::Spent => Self::Unauthorized,
            CredentialError::Provider(source) => source.into(),
            CredentialError::Store(source) => source.into(),
        }
    }
}

/// The Railway credential behind a session, kept usable.
///
/// A session lasts two weeks and the access token it was opened with lasts
/// about an hour, so something has to renew the second without ending the
/// first. That needs both the store that holds the tokens and the provider that
/// reissues them, which is why it composes the two rather than living on
/// either: neither knows about the other, and it would be the wrong dependency
/// in both directions.
pub struct Credentials<'a> {
    sessions: &'a dyn SessionStore,
    auth: &'a dyn AuthProvider,
}

impl<'a> Credentials<'a> {
    #[must_use]
    pub fn new(sessions: &'a dyn SessionStore, auth: &'a dyn AuthProvider) -> Self {
        Self { sessions, auth }
    }

    /// The session's access token, renewed and written back first if it is
    /// spent.
    ///
    /// Renewal is per-request and unsynchronised: two requests arriving on the
    /// same expired session both refresh, and the second write wins. Both
    /// tokens work, so the cost is a wasted call rather than a broken session.
    pub async fn access_token(
        &self,
        token: &SessionToken,
        session: Session,
    ) -> CredentialResult<Secret> {
        let skew =
            TimeDelta::try_seconds(ACCESS_TOKEN_EXPIRY_SKEW_SECS).unwrap_or_else(TimeDelta::zero);

        if !session.tokens.is_expired_at(Utc::now() + skew) {
            return Ok(session.tokens.access_token);
        }

        let previous = session.tokens;
        let refresh_token = previous
            .refresh_token
            .as_ref()
            .ok_or(CredentialError::Spent)?;

        let mut renewed = match self.auth.refresh(refresh_token).await {
            Ok(renewed) => renewed,
            // A grant the provider has stopped honouring is the user's cue to
            // log in again, not a bad request from the browser that asked.
            Err(AuthError::InvalidGrant) => return Err(CredentialError::Spent),
            Err(error) => return Err(error.into()),
        };

        // Rotation is optional: a provider that returns no new refresh token
        // means the old one still stands, and overwriting it with nothing would
        // end the session at the next expiry.
        if renewed.refresh_token.is_none() {
            renewed.refresh_token = previous.refresh_token;
        }

        self.sessions.renew(token, &renewed).await?;
        tracing::debug!(user_id = %session.user.id, "renewed a Railway access token");

        Ok(renewed.access_token)
    }
}

/// [`SessionStore`] over the application's Postgres pool.
#[derive(Debug, Clone)]
pub struct PgSessionStore {
    database: Database,
    ttl: TimeDelta,
}

impl PgSessionStore {
    #[must_use]
    pub fn new(database: Database, ttl: TimeDelta) -> Self {
        Self { database, ttl }
    }
}

#[async_trait::async_trait]
impl SessionStore for PgSessionStore {
    /// One transaction: a login either produces a user and the session that
    /// goes with it, or neither.
    async fn begin(
        &self,
        identity: &RailwayIdentity,
        tokens: TokenSet,
    ) -> SessionResult<(SessionToken, Session)> {
        let mut conn = self.database.conn().await?;
        let now = Utc::now();
        let token = SessionToken::generate();
        let expires_at = now + self.ttl;

        let (user, id) = conn
            .transaction::<_, SessionError, _>(async |conn| {
                let user = diesel::insert_into(users::table)
                    .values((
                        users::railway_user_id.eq(&identity.subject),
                        users::email.eq(&identity.email),
                        users::name.eq(&identity.name),
                        users::avatar_url.eq(&identity.avatar_url),
                    ))
                    .on_conflict(users::railway_user_id)
                    .do_update()
                    .set((
                        users::email.eq(&identity.email),
                        users::name.eq(&identity.name),
                        users::avatar_url.eq(&identity.avatar_url),
                        users::updated_at.eq(now),
                    ))
                    .returning(User::as_returning())
                    .get_result::<User>(conn)
                    .await?;

                let id = diesel::insert_into(sessions::table)
                    .values((
                        sessions::token_hash.eq(token.digest()),
                        sessions::user_id.eq(user.id),
                        sessions::access_token.eq(tokens.access_token.expose()),
                        sessions::refresh_token
                            .eq(tokens.refresh_token.as_ref().map(Secret::expose)),
                        sessions::scope.eq(&tokens.scope),
                        sessions::access_token_expires_at.eq(tokens.expires_at),
                        sessions::expires_at.eq(expires_at),
                    ))
                    .returning(sessions::id)
                    .get_result::<Uuid>(conn)
                    .await?;

                Ok((user, id))
            })
            .await?;

        Ok((
            token,
            Session {
                id,
                user,
                tokens,
                expires_at,
            },
        ))
    }

    async fn lookup(&self, token: &SessionToken) -> SessionResult<Option<Session>> {
        let mut conn = self.database.conn().await?;

        let row = sessions::table
            .inner_join(users::table)
            .filter(sessions::token_hash.eq(token.digest()))
            .filter(sessions::expires_at.gt(Utc::now()))
            .select((
                sessions::id,
                sessions::access_token,
                sessions::refresh_token,
                sessions::scope,
                sessions::access_token_expires_at,
                sessions::expires_at,
                User::as_select(),
            ))
            .first::<(
                Uuid,
                String,
                Option<String>,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
                User,
            )>(&mut conn)
            .await
            .optional()?;

        Ok(row.map(
            |(id, access_token, refresh_token, scope, token_expires_at, expires_at, user)| {
                Session {
                    id,
                    user,
                    tokens: TokenSet {
                        access_token: Secret::new(access_token),
                        refresh_token: refresh_token.map(Secret::new),
                        id_token: None,
                        scope,
                        expires_at: token_expires_at,
                    },
                    expires_at,
                }
            },
        ))
    }

    async fn renew(&self, token: &SessionToken, tokens: &TokenSet) -> SessionResult<()> {
        let mut conn = self.database.conn().await?;

        diesel::update(sessions::table.filter(sessions::token_hash.eq(token.digest())))
            .set((
                sessions::access_token.eq(tokens.access_token.expose()),
                sessions::refresh_token.eq(tokens.refresh_token.as_ref().map(Secret::expose)),
                sessions::scope.eq(&tokens.scope),
                sessions::access_token_expires_at.eq(tokens.expires_at),
            ))
            .execute(&mut conn)
            .await?;

        Ok(())
    }

    async fn end(&self, token: &SessionToken) -> SessionResult<()> {
        let mut conn = self.database.conn().await?;

        diesel::delete(sessions::table.filter(sessions::token_hash.eq(token.digest())))
            .execute(&mut conn)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};

    use super::{PgSessionStore, SessionStore, SessionToken};
    use crate::{
        config::Config,
        db::Database,
        secret::Secret,
        services::auth::{RailwayIdentity, TokenSet},
    };

    #[test]
    fn a_token_hashes_to_a_stable_digest_it_does_not_reveal() {
        let token = SessionToken::new("session-value");

        assert_eq!(token.digest(), SessionToken::new("session-value").digest());
        assert_ne!(token.digest(), SessionToken::new("session-valuf").digest());
        assert_eq!(token.digest().len(), 32);
        assert!(!format!("{token:?}").contains("session-value"));
    }

    #[test]
    fn generated_tokens_do_not_repeat() {
        assert_ne!(
            SessionToken::generate().expose(),
            SessionToken::generate().expose()
        );
    }

    /// Exercises the queries against a real Postgres, which is the only thing
    /// that can catch a hand-written `schema.rs` drifting from `migrations/`.
    /// Ignored by default so `bazel test //...` needs no database:
    ///
    /// ```text
    /// tools/stack.sh db && bazel run //api:migrate
    /// cargo test -p monorail-api -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "needs the local Postgres from tools/stack.sh"]
    async fn a_session_round_trips_through_postgres() {
        let config = Config::from_env().expect("development defaults should parse");
        let store = PgSessionStore::new(Database::new(&config), TimeDelta::hours(1));

        let identity = RailwayIdentity {
            subject: format!("user_{}", crate::secret::random_token()),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
        };
        let tokens = TokenSet {
            access_token: Secret::new("access"),
            refresh_token: Some(Secret::new("refresh")),
            id_token: None,
            scope: "openid email".to_owned(),
            expires_at: Utc::now() + TimeDelta::hours(1),
        };

        let (token, opened) = store
            .begin(&identity, tokens)
            .await
            .expect("a session should open");

        let found = store
            .lookup(&token)
            .await
            .expect("lookup should succeed")
            .expect("the session should be found");

        assert_eq!(found.id, opened.id);
        assert_eq!(found.user.railway_user_id, identity.subject);
        assert_eq!(found.user.email.as_deref(), Some("jane@example.test"));
        assert_eq!(found.tokens.access_token.expose(), "access");
        assert_eq!(found.tokens.scope, "openid email");

        let (second, reopened) = store
            .begin(&identity, found.tokens.clone())
            .await
            .expect("a second session should open");
        assert_eq!(
            reopened.user.id, opened.user.id,
            "logging in again should reuse the account, not create a second one"
        );

        store.end(&token).await.expect("logout should succeed");
        assert!(
            store
                .lookup(&token)
                .await
                .expect("lookup should succeed")
                .is_none(),
            "a revoked session should not resolve"
        );

        store.end(&second).await.expect("cleanup should succeed");
    }
}
