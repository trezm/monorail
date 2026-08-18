//! Postgres access.
//!
//! One [`Database`] lives in [`AppState`](crate::state::AppState) and is shared
//! by every handler. It owns a [`bb8`] pool of [`AsyncPgConnection`]s, which
//! speak the wire protocol over `tokio-postgres` — there is no libpq to link,
//! so the Bazel build stays hermetic and the eventual deployment image needs no
//! system packages.
//!
//! Handlers check out a connection per query and drop it as soon as they are
//! done:
//!
//! ```ignore
//! let mut conn = state.db().conn().await?;
//! let rows = some_table::table.load::<Row>(&mut conn).await?;
//! ```
//!
//! Holding one across an `.await` that does something else — an HTTP call, a
//! lock — is how a pool of ten deadlocks under eleven concurrent requests.

use std::fmt;

use anyhow::Context as _;
use diesel_async::{
    AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl as _,
    pooled_connection::{AsyncDieselConnectionManager, bb8},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};

use crate::{config::Config, error::ApiError};

/// Every `.sql` file under `api/migrations`, baked into the binary at compile
/// time so a deployed artifact carries its own schema history and needs no
/// diesel-cli alongside it.
///
/// Under Bazel this needs `compile_data` and a `CARGO_MANIFEST_DIR` in
/// `rustc_env`; see `api/BUILD.bazel`.
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// A pool of async Postgres connections.
pub type Pool = bb8::Pool<AsyncPgConnection>;

/// A connection checked out of the [`Pool`], returned to it on drop.
pub type PooledConnection<'a> = bb8::PooledConnection<'a, AsyncPgConnection>;

pub type DbResult<T> = Result<T, DbError>;

/// What can go wrong when talking to Postgres.
///
/// The split is the one that matters to a caller: [`Self::Unavailable`] means
/// the request never reached the database and is worth retrying,
/// [`Self::Query`] means it did and the statement itself failed.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// No connection could be checked out: the pool timed out, or Postgres
    /// refused or dropped the connection.
    #[error("could not obtain a database connection")]
    Unavailable(#[source] anyhow::Error),

    /// The statement reached Postgres and failed.
    #[error(transparent)]
    Query(#[from] diesel::result::Error),
}

impl From<bb8::RunError> for DbError {
    fn from(error: bb8::RunError) -> Self {
        Self::Unavailable(error.into())
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Unavailable(source) => Self::Unavailable(source),
            // Deliberately not mapping `Error::NotFound` to a 404: whether an
            // empty result is "this resource does not exist" or a broken
            // invariant depends on the query, and only the call site knows
            // which. Match on it there and return `ApiError::not_found`.
            DbError::Query(source) => Self::Internal(source.into()),
        }
    }
}

/// The handle to Postgres shared across the application.
#[derive(Clone)]
pub struct Database {
    pool: Pool,
}

impl Database {
    /// Builds the pool.
    ///
    /// Nothing connects here — bb8 opens connections lazily on first use — so
    /// this is infallible and synchronous. That is what lets tests build a real
    /// [`AppState`](crate::state::AppState) without a database anywhere near
    /// them. Call [`Self::ping`] to find out whether the database is actually
    /// reachable; [`run`](crate::run) does exactly that before it binds a port.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let manager =
            AsyncDieselConnectionManager::<AsyncPgConnection>::new(config.database_url.as_str());

        let pool = Pool::builder()
            .max_size(config.database_pool_size)
            .connection_timeout(config.database_connect_timeout)
            .build_unchecked(manager);

        Self { pool }
    }

    /// The underlying pool, for the rare caller that needs bb8 itself.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Checks out a connection, waiting up to `database_connect_timeout`.
    pub async fn conn(&self) -> DbResult<PooledConnection<'_>> {
        Ok(self.pool.get().await?)
    }

    /// Round-trips a trivial statement. Used by the readiness probe, which
    /// wants to know that a *usable* connection exists — a pool that has never
    /// dialled out looks healthy right up until the first real query.
    pub async fn ping(&self) -> DbResult<()> {
        let mut conn = self.conn().await?;
        diesel::sql_query("SELECT 1").execute(&mut conn).await?;
        Ok(())
    }

    /// Applies every migration in [`MIGRATIONS`] that has not run yet, and
    /// returns the versions it applied.
    ///
    /// Diesel's migration machinery is synchronous, so the harness parks it on
    /// a blocking thread via `block_in_place`. That requires the multi-threaded
    /// tokio runtime — which is what `#[tokio::main]` gives the binary, but not
    /// what `#[tokio::test]` gives a test. Annotate any test that migrates with
    /// `#[tokio::test(flavor = "multi_thread")]`.
    ///
    /// Runs on a connection opened outside the pool: applying a migration can
    /// take as long as the DDL takes, and a pool slot held that long is a slot
    /// requests are queueing for.
    pub async fn migrate(&self) -> anyhow::Result<Vec<String>> {
        let conn = self
            .pool
            .dedicated_connection()
            .await
            .context("could not connect to run migrations")?;

        let mut harness = AsyncMigrationHarness::new(conn);

        let applied = harness
            .run_pending_migrations(MIGRATIONS)
            .map_err(|error| anyhow::anyhow!(error))
            .context("failed to apply pending migrations")?;

        Ok(applied.iter().map(ToString::to_string).collect())
    }
}

/// Hand-written so the connection string — which carries a password — cannot
/// reach a log through a `#[derive(Debug)]` on this or anything holding it.
impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("state", &self.pool.state())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use diesel::{migration::MigrationSource, pg::Pg};

    use super::MIGRATIONS;

    /// Guards the build wiring, not the SQL. `embed_migrations!` resolves its
    /// directory at compile time, so under Bazel it depends on `compile_data`
    /// and `rustc_env` in `api/BUILD.bazel` being right. Get those wrong and
    /// the binary links fine and migrates nothing.
    #[test]
    fn migrations_are_embedded() {
        let migrations = MigrationSource::<Pg>::migrations(&MIGRATIONS)
            .expect("embedded migrations should be readable");

        let names: Vec<_> = migrations
            .iter()
            .map(|migration| migration.name().to_string())
            .collect();

        assert!(
            names
                .iter()
                .any(|name| name.ends_with("diesel_initial_setup")),
            "expected the initial setup migration, found {names:?}"
        );
    }
}
