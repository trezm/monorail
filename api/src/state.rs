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
    auth: Option<Arc<dyn AuthProvider>>,
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
            auth: None,
            started_at: Instant::now(),
        }
    }

    /// Attaches an identity provider.
    ///
    /// Separate from [`Self::new`] because building one can fail and reaches
    /// the network, neither of which belongs in state construction — and
    /// because it is what lets a test install a stub provider.
    #[must_use]
    pub fn with_auth(mut self, auth: Arc<dyn AuthProvider>) -> Self {
        self.auth = Some(auth);
        self
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn db(&self) -> &Database {
        &self.database
    }

    /// `None` when Railway OAuth is not configured; handlers that need it
    /// should answer `503` rather than pretend.
    #[must_use]
    pub fn auth(&self) -> Option<&dyn AuthProvider> {
        self.auth.as_deref()
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
            .field("auth", &self.auth.is_some())
            .finish_non_exhaustive()
    }
}
