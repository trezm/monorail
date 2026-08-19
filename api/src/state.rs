//! Shared application state.
//!
//! `Config` sits behind an `Arc` because axum clones the state per request and
//! it owns heap data; `Instant` is `Copy`, so it does not need to. `Database`
//! is already a handle around a shared pool, so cloning it is cheap on its own.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{config::Config, db::Database};

#[derive(Debug, Clone)]
pub struct AppState {
    config: Arc<Config>,
    database: Database,
    started_at: Instant,
}

impl AppState {
    /// Builds the state, including the connection pool.
    ///
    /// Infallible and synchronous: the pool connects lazily, so nothing here
    /// touches the network. See [`Database::new`].
    #[must_use]
    pub fn new(config: Config) -> Self {
        let database = Database::new(&config);

        Self {
            config: Arc::new(config),
            database,
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
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}
