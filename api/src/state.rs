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

use chrono::TimeDelta;

use crate::{
    config::Config,
    db::Database,
    services::{
        auth::AuthProvider,
        railway::RailwayApi,
        session::{PgSessionStore, SessionStore},
    },
};

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    database: Database,
    auth: Arc<dyn AuthProvider>,
    railway: Arc<dyn RailwayApi>,
    sessions: Arc<dyn SessionStore>,
    started_at: Instant,
}

impl AppState {
    /// Builds the state, including the connection pool.
    ///
    /// Infallible and synchronous: the pool connects lazily, so nothing here
    /// touches the network. See [`Database::new`].
    ///
    /// The login provider and the Railway API client are passed in rather than
    /// built here because building either can fail, and because it is what lets
    /// a test install a stub.
    #[must_use]
    pub fn new(config: Config, auth: Arc<dyn AuthProvider>, railway: Arc<dyn RailwayApi>) -> Self {
        let database = Database::new(&config);
        let ttl = TimeDelta::from_std(config.session_ttl)
            .unwrap_or_else(|_| TimeDelta::try_days(14).unwrap_or_default());
        let sessions = Arc::new(PgSessionStore::new(database.clone(), ttl));

        Self {
            config: Arc::new(config),
            database,
            auth,
            railway,
            sessions,
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

    /// Swaps in a different session store, for a test that needs one that is
    /// not Postgres.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Arc<dyn SessionStore>) -> Self {
        self.sessions = sessions;
        self
    }

    #[must_use]
    pub fn sessions(&self) -> &dyn SessionStore {
        self.sessions.as_ref()
    }

    #[must_use]
    pub fn auth(&self) -> &dyn AuthProvider {
        self.auth.as_ref()
    }

    #[must_use]
    pub fn railway(&self) -> &dyn RailwayApi {
        self.railway.as_ref()
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
