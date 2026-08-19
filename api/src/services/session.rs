//! Login sessions.
//!
//! A session is what turns a completed OAuth flow into something a browser can
//! present on later requests. The cookie carries an opaque [`SessionToken`];
//! only its SHA-256 digest reaches the database, so a dump of the `sessions`
//! table yields no usable session.
//!
//! The rules live here and the rows live in [`dao`](crate::dao): this decides
//! that a session lasts `ttl` and that a token is looked up by digest, and
//! [`SessionDao`] writes whatever it is told.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    dao::{
        sessions::{NewSession, SessionDao},
        users::{NewUser, User},
    },
    db::DbError,
    error::ApiError,
    secret::{Secret, random_token},
    services::auth::{RailwayIdentity, TokenSet},
};

pub type SessionResult<T> = Result<T, SessionError>;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Database(#[from] DbError),

    #[error(transparent)]
    Backend(#[from] anyhow::Error),
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
/// is one — handlers depend on the behaviour, not on Postgres — and because a
/// mock of it is what lets the route tests run without a database.
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

    /// Revokes a session. Absent is success: logging out twice is not an error.
    async fn end(&self, token: &SessionToken) -> SessionResult<()>;
}

/// [`SessionStore`] over the [`dao`](crate::dao) layer.
#[derive(Clone)]
pub struct DaoSessionStore {
    sessions: Arc<dyn SessionDao>,
    ttl: TimeDelta,
}

impl DaoSessionStore {
    #[must_use]
    pub fn new(sessions: Arc<dyn SessionDao>, ttl: TimeDelta) -> Self {
        Self { sessions, ttl }
    }
}

#[async_trait::async_trait]
impl SessionStore for DaoSessionStore {
    /// Both writes go through a single DAO call because they are one
    /// transaction: a login leaves a user and its session, or neither.
    async fn begin(
        &self,
        identity: &RailwayIdentity,
        tokens: TokenSet,
    ) -> SessionResult<(SessionToken, Session)> {
        let now = Utc::now();
        let token = SessionToken::generate();
        let expires_at = now + self.ttl;

        let (user, id) = self
            .sessions
            .open_login(
                &NewUser {
                    railway_user_id: identity.subject.clone(),
                    email: identity.email.clone(),
                    name: identity.name.clone(),
                    avatar_url: identity.avatar_url.clone(),
                },
                &NewSession {
                    token_hash: token.digest(),
                    access_token: tokens.access_token.clone(),
                    refresh_token: tokens.refresh_token.clone(),
                    scope: tokens.scope.clone(),
                    access_token_expires_at: tokens.expires_at,
                    expires_at,
                },
                now,
            )
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
        let record = self
            .sessions
            .find_unexpired(&token.digest(), Utc::now())
            .await?;

        Ok(record.map(|record| Session {
            id: record.id,
            user: record.user,
            tokens: TokenSet {
                access_token: record.access_token,
                refresh_token: record.refresh_token,
                id_token: None,
                scope: record.scope,
                expires_at: record.access_token_expires_at,
            },
            expires_at: record.expires_at,
        }))
    }

    async fn end(&self, token: &SessionToken) -> SessionResult<()> {
        self.sessions.delete(&token.digest()).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeDelta, Utc};
    use uuid::Uuid;

    use super::{DaoSessionStore, SessionStore, SessionToken};
    use crate::{
        dao::{
            sessions::{MockSessionDao, SessionRecord},
            users::{NewUser, User},
        },
        secret::Secret,
        services::auth::{RailwayIdentity, TokenSet},
    };

    const TTL: TimeDelta = TimeDelta::hours(1);

    fn identity() -> RailwayIdentity {
        RailwayIdentity {
            subject: "user_stub".to_owned(),
            email: Some("jane@example.test".to_owned()),
            name: Some("Jane Developer".to_owned()),
            avatar_url: None,
        }
    }

    fn user() -> User {
        User {
            id: Uuid::from_u128(1),
            railway_user_id: identity().subject,
            email: identity().email,
            name: identity().name,
            avatar_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn tokens() -> TokenSet {
        TokenSet {
            access_token: Secret::new("access"),
            refresh_token: Some(Secret::new("refresh")),
            id_token: Some(Secret::new("id")),
            scope: "openid email".to_owned(),
            expires_at: Utc::now() + TTL,
        }
    }

    fn store(sessions: MockSessionDao) -> DaoSessionStore {
        DaoSessionStore::new(Arc::new(sessions), TTL)
    }

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

    /// The store, not the DAO, is what decides when a session dies — and the
    /// row is keyed on the digest, never the token the browser is handed.
    #[tokio::test]
    async fn beginning_a_session_writes_the_digest_and_a_ttl_expiry() {
        let mut sessions = MockSessionDao::new();
        sessions
            .expect_open_login()
            .times(1)
            .returning(|new_user, session, now| {
                assert_eq!(new_user.railway_user_id, "user_stub");
                assert_eq!(new_user.email.as_deref(), Some("jane@example.test"));
                assert_eq!(session.token_hash.len(), 32);
                assert_eq!(session.access_token.expose(), "access");
                assert_eq!(session.scope, "openid email");
                assert_eq!(
                    session.expires_at,
                    now + TTL,
                    "expiry is the store's decision, off the clock it passed down"
                );

                Ok((user(), Uuid::from_u128(2)))
            });

        let (token, session) = store(sessions)
            .begin(&identity(), tokens())
            .await
            .expect("a session should open");

        assert_eq!(session.id, Uuid::from_u128(2));
        assert_eq!(session.user.id, user().id);
        assert!(!token.expose().is_empty());
    }

    /// The ID token is a login artefact, not session state: its claims are
    /// already in `users`, so it must not be handed to the DAO to store.
    #[tokio::test]
    async fn beginning_a_session_does_not_persist_the_id_token() {
        let mut sessions = MockSessionDao::new();
        sessions
            .expect_open_login()
            .times(1)
            .returning(|_, session, _| {
                let stored = format!("{session:?}");
                assert!(
                    !stored.contains("id_token"),
                    "the write should carry no id token, got {stored}"
                );
                Ok((user(), Uuid::from_u128(2)))
            });

        store(sessions)
            .begin(&identity(), tokens())
            .await
            .expect("a session should open");
    }

    #[tokio::test]
    async fn looking_up_a_token_asks_for_the_digest_and_rebuilds_the_tokens() {
        let expires_at = Utc::now() + TTL;

        let mut sessions = MockSessionDao::new();
        sessions
            .expect_find_unexpired()
            .withf(move |token_hash, _| token_hash == SessionToken::new("opaque").digest())
            .times(1)
            .returning(move |_, _| {
                Ok(Some(SessionRecord {
                    id: Uuid::from_u128(2),
                    user: user(),
                    access_token: Secret::new("access"),
                    refresh_token: Some(Secret::new("refresh")),
                    scope: "openid email".to_owned(),
                    access_token_expires_at: expires_at,
                    expires_at,
                }))
            });

        let session = store(sessions)
            .lookup(&SessionToken::new("opaque"))
            .await
            .expect("lookup should succeed")
            .expect("the session should be found");

        assert_eq!(session.id, Uuid::from_u128(2));
        assert_eq!(session.tokens.access_token.expose(), "access");
        assert_eq!(session.tokens.scope, "openid email");
        assert!(
            session.tokens.id_token.is_none(),
            "no id token is stored, so none can come back"
        );
    }

    #[tokio::test]
    async fn a_row_the_dao_does_not_return_is_not_a_session() {
        let mut sessions = MockSessionDao::new();
        sessions
            .expect_find_unexpired()
            .times(1)
            .returning(|_, _| Ok(None));

        assert!(
            store(sessions)
                .lookup(&SessionToken::new("opaque"))
                .await
                .expect("lookup should succeed")
                .is_none()
        );
    }

    /// Logging out twice is not an error, so a delete that removed nothing is
    /// still a success.
    #[tokio::test]
    async fn ending_a_session_that_is_already_gone_succeeds() {
        let mut sessions = MockSessionDao::new();
        sessions
            .expect_delete()
            .withf(|token_hash| token_hash == SessionToken::new("opaque").digest())
            .times(1)
            .returning(|_| Ok(0));

        store(sessions)
            .end(&SessionToken::new("opaque"))
            .await
            .expect("logout should succeed");
    }

    /// A login is one transaction, so a session insert that fails must take the
    /// user upsert down with it. Only a real database can show this: a mocked
    /// DAO has no transaction to roll back.
    ///
    /// Forces the failure with a `token_hash` that is already taken — the
    /// column is `UNIQUE` — while the account in the same call is new.
    #[tokio::test]
    #[ignore = "needs the local Postgres from tools/stack.sh"]
    async fn a_login_that_fails_halfway_writes_no_user() {
        use crate::{
            config::Config,
            dao::sessions::{NewSession, PgSessionDao, SessionDao as _},
            db::Database,
        };

        let config = Config::from_env().expect("development defaults should parse");
        let dao = PgSessionDao::new(Database::new(&config));

        let collision = SessionToken::generate();
        let session = |token_hash: Vec<u8>| NewSession {
            token_hash,
            access_token: Secret::new("access"),
            refresh_token: None,
            scope: "openid".to_owned(),
            access_token_expires_at: Utc::now() + TTL,
            expires_at: Utc::now() + TTL,
        };

        let first = NewUser {
            railway_user_id: format!("user_{}", crate::secret::random_token()),
            email: None,
            name: None,
            avatar_url: None,
        };
        dao.open_login(&first, &session(collision.digest()), Utc::now())
            .await
            .expect("the first login should open");

        let second = NewUser {
            railway_user_id: format!("user_{}", crate::secret::random_token()),
            ..first.clone()
        };
        dao.open_login(&second, &session(collision.digest()), Utc::now())
            .await
            .expect_err("a duplicate token hash should fail the insert");

        let store = DaoSessionStore::new(Arc::new(dao), TTL);
        let identity = RailwayIdentity {
            subject: second.railway_user_id.clone(),
            ..identity()
        };

        let (token, reopened) = store
            .begin(&identity, tokens())
            .await
            .expect("the account should be free to log in");
        assert_eq!(
            reopened.user.created_at, reopened.user.updated_at,
            "the rolled-back upsert should have left no row, so this is an insert"
        );

        store.end(&token).await.expect("cleanup should succeed");
        store.end(&collision).await.expect("cleanup should succeed");
    }

    /// Exercises the real queries against a real Postgres, which is the only
    /// thing that can catch a hand-written `schema.rs` drifting from
    /// `migrations/` — or the login transaction failing to commit. Ignored by
    /// default so `bazel test //...` needs no database:
    ///
    /// ```text
    /// tools/stack.sh db && bazel run //api:migrate
    /// cargo test -p monorail-api -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "needs the local Postgres from tools/stack.sh"]
    async fn a_session_round_trips_through_postgres() {
        use crate::{config::Config, dao::sessions::PgSessionDao, db::Database};

        let config = Config::from_env().expect("development defaults should parse");
        let store = DaoSessionStore::new(Arc::new(PgSessionDao::new(Database::new(&config))), TTL);

        let identity = RailwayIdentity {
            subject: format!("user_{}", crate::secret::random_token()),
            ..identity()
        };

        let (token, opened) = store
            .begin(&identity, tokens())
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
