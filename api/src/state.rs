//! Shared application state.
//!
//! `Config` sits behind an `Arc` because axum clones the state per request and
//! it owns heap data; `Instant` is `Copy`, so it does not need to. `Database`
//! is already a handle around a shared pool, so cloning it is cheap on its own.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{config::Config, db::Database, services::auth::AuthProvider};

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    database: Database,
    auth: Arc<dyn AuthProvider>,
    started_at: Instant,
}

impl AppState {
    /// Builds the state, including the connection pool.
    ///
    /// Infallible and synchronous: the pool connects lazily, so nothing here
    /// touches the network. See [`Database::new`].
    ///
    /// The provider is passed in rather than built here because building one
    /// can fail, and because it is what lets a test install a stub.
    #[must_use]
    pub fn new(config: Config, auth: Arc<dyn AuthProvider>) -> Self {
        let database = Database::new(&config);

        Self {
            config: Arc::new(config),
            database,
            auth,
            started_at: Instant::now(),
        }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn db(&self) -> &Database {
        &self.database
    }

    #[must_use]
    pub fn auth(&self) -> &dyn AuthProvider {
        self.auth.as_ref()
    }

    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Hand-written because `dyn AuthProvider` is not `Debug`, and adding that
/// bound would force every stub in a test to implement it.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}
