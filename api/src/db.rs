//! Postgres access.
//!
//! One [`Database`] lives in [`AppState`](crate::state::AppState) and holds a
//! [`bb8`] pool of [`AsyncPgConnection`]s. Only diesel's `postgres_backend`
//! feature is enabled — `postgres` would link libpq — so all io is
//! tokio-postgres and the build stays pure Rust.
//!
//! ```ignore
//! let mut conn = state.db().conn().await?;
//! let rows = some_table::table.load::<Row>(&mut conn).await?;
//! ```
//!
//! Do not hold a connection across an unrelated `.await`: a pool of ten
//! deadlocks under eleven concurrent requests that do.

use std::fmt;

use anyhow::Context as _;
use diesel_async::{
    AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl as _,
    pooled_connection::{AsyncDieselConnectionManager, bb8},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};

use crate::{config::Config, error::ApiError};

/// Every `.sql` file under `api/migrations`, baked into the binary at compile
/// time so a deployed artifact needs no diesel-cli alongside it.
///
/// Under Bazel this needs `compile_data` and a `CARGO_MANIFEST_DIR` in
/// `rustc_env`; see `api/BUILD.bazel`.
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// A pool of async Postgres connections.
pub type Pool = bb8::Pool<AsyncPgConnection>;

/// A connection checked out of the [`Pool`], returned to it on drop.
pub type PooledConnection<'a> = bb8::PooledConnection<'a, AsyncPgConnection>;

pub type DbResult<T> = Result<T, DbError>;

/// [`Self::Unavailable`] means the query never reached Postgres and is worth
/// retrying; [`Self::Query`] means it did and the statement failed.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("could not obtain a database connection")]
    Unavailable(#[source] anyhow::Error),

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
            // `Error::NotFound` is deliberately not a 404: only the call site
            // knows whether an empty result is a missing resource or a broken
            // invariant. Match on it there and return `ApiError::not_found`.
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
    /// Builds the pool. bb8 connects lazily, so this is infallible and needs no
    /// database — which is what lets tests build a real
    /// [`AppState`](crate::state::AppState). [`run`](crate::run) calls
    /// [`Self::ping`] at startup to check reachability.
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

    /// Round-trips `SELECT 1`. A pool that has never dialled out looks healthy,
    /// so the readiness probe needs a real query rather than pool statistics.
    pub async fn ping(&self) -> DbResult<()> {
        let mut conn = self.conn().await?;
        diesel::sql_query("SELECT 1").execute(&mut conn).await?;
        Ok(())
    }

    /// Applies pending migrations and returns the versions applied.
    ///
    /// Uses a connection opened outside the pool, since DDL can hold one for a
    /// long time. Diesel's migration code is synchronous and the harness parks
    /// it with `block_in_place`, so this needs the multi-threaded runtime: a
    /// test that migrates must say `#[tokio::test(flavor = "multi_thread")]`.
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

/// Hand-written: a derived impl would print the pool's manager, and the
/// connection string in it carries a password.
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

    /// Guards the Bazel wiring, not the SQL: get `compile_data` or `rustc_env`
    /// wrong and the binary links fine and migrates nothing.
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
