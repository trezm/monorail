//! The `sessions` table, and the login write that fills it.
//!
//! Rows are addressed by the SHA-256 digest of the session token, never by the
//! token itself — see [`SessionToken`](crate::services::session::SessionToken).
//! This layer only ever sees the digest.

use chrono::{DateTime, Utc};
use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, SelectableHelper as _,
};
use diesel_async::{AsyncConnection as _, RunQueryDsl as _};
use uuid::Uuid;

use crate::{
    dao::users::{NewUser, User},
    db::{Database, DbError, DbResult},
    schema::{sessions, users},
    secret::Secret,
};

/// The session columns a login writes. `user_id` is absent because the same
/// call produces the user; `id` and `created_at` are the database's.
///
/// The provider's tokens ride along because the point of logging in with
/// Railway is to act on Railway afterwards, and an access token that lives an
/// hour has to outlive the request that fetched it. The ID token is absent by
/// design: its claims are already in `users`.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub token_hash: Vec<u8>,
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub scope: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A session row with the account it belongs to, as one read.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user: User,
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub scope: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SessionDao: Send + Sync + 'static {
    /// Writes the account and the session that goes with it, returning the
    /// stored user and the new session's id.
    ///
    /// One method rather than two because it is one transaction: a login
    /// leaves a user and its session or neither. Splitting it per table would
    /// put the two writes on different pooled connections, which is a
    /// transaction boundary this layer cannot then draw.
    ///
    /// The account is written by upsert — logging in a second time refreshes
    /// the row rather than conflicting with it. `now` comes from the caller so
    /// one clock governs `updated_at` and the session's expiry.
    async fn open_login(
        &self,
        user: &NewUser,
        session: &NewSession,
        now: DateTime<Utc>,
    ) -> DbResult<(User, Uuid)>;

    /// The unexpired row for `token_hash`, joined to its user.
    ///
    /// Expiry is a `WHERE` clause rather than a check on the way out, so a
    /// stale row can never be returned by a caller that forgets to look.
    async fn find_unexpired(
        &self,
        token_hash: &[u8],
        now: DateTime<Utc>,
    ) -> DbResult<Option<SessionRecord>>;

    /// Deletes the row for `token_hash`, returning how many were removed.
    /// Zero is a normal answer: logging out twice is not an error.
    async fn delete(&self, token_hash: &[u8]) -> DbResult<usize>;
}

/// [`SessionDao`] over the application's Postgres pool.
#[derive(Debug, Clone)]
pub struct PgSessionDao {
    database: Database,
}

impl PgSessionDao {
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

/// The joined shape diesel reads, before it becomes a [`SessionRecord`].
type JoinedRow = (
    Uuid,
    String,
    Option<String>,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    User,
);

#[async_trait::async_trait]
impl SessionDao for PgSessionDao {
    async fn open_login(
        &self,
        user: &NewUser,
        session: &NewSession,
        now: DateTime<Utc>,
    ) -> DbResult<(User, Uuid)> {
        let mut conn = self.database.conn().await?;

        conn.transaction::<_, DbError, _>(async |conn| {
            let stored = diesel::insert_into(users::table)
                .values((
                    users::railway_user_id.eq(&user.railway_user_id),
                    users::email.eq(&user.email),
                    users::name.eq(&user.name),
                    users::avatar_url.eq(&user.avatar_url),
                ))
                .on_conflict(users::railway_user_id)
                .do_update()
                .set((
                    users::email.eq(&user.email),
                    users::name.eq(&user.name),
                    users::avatar_url.eq(&user.avatar_url),
                    users::updated_at.eq(now),
                ))
                .returning(User::as_returning())
                .get_result::<User>(conn)
                .await?;

            let id = diesel::insert_into(sessions::table)
                .values((
                    sessions::token_hash.eq(&session.token_hash),
                    sessions::user_id.eq(stored.id),
                    sessions::access_token.eq(session.access_token.expose()),
                    sessions::refresh_token.eq(session.refresh_token.as_ref().map(Secret::expose)),
                    sessions::scope.eq(&session.scope),
                    sessions::access_token_expires_at.eq(session.access_token_expires_at),
                    sessions::expires_at.eq(session.expires_at),
                ))
                .returning(sessions::id)
                .get_result::<Uuid>(conn)
                .await?;

            Ok((stored, id))
        })
        .await
    }

    async fn find_unexpired(
        &self,
        token_hash: &[u8],
        now: DateTime<Utc>,
    ) -> DbResult<Option<SessionRecord>> {
        let mut conn = self.database.conn().await?;

        let row = sessions::table
            .inner_join(users::table)
            .filter(sessions::token_hash.eq(token_hash))
            .filter(sessions::expires_at.gt(now))
            .select((
                sessions::id,
                sessions::access_token,
                sessions::refresh_token,
                sessions::scope,
                sessions::access_token_expires_at,
                sessions::expires_at,
                User::as_select(),
            ))
            .first::<JoinedRow>(&mut conn)
            .await
            .optional()?;

        Ok(row.map(
            |(
                id,
                access_token,
                refresh_token,
                scope,
                access_token_expires_at,
                expires_at,
                user,
            )| {
                SessionRecord {
                    id,
                    user,
                    access_token: Secret::new(access_token),
                    refresh_token: refresh_token.map(Secret::new),
                    scope,
                    access_token_expires_at,
                    expires_at,
                }
            },
        ))
    }

    async fn delete(&self, token_hash: &[u8]) -> DbResult<usize> {
        let mut conn = self.database.conn().await?;

        Ok(
            diesel::delete(sessions::table.filter(sessions::token_hash.eq(token_hash)))
                .execute(&mut conn)
                .await?,
        )
    }
}
