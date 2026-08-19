//! Shared application state.
//!
//! `Config` sits behind an `Arc` because axum clones the state per request and
//! it owns heap data; `Instant` is `Copy`, so it does not need to. `Database`
//! is already a handle around a shared pool, so cloning it is cheap on its own.
//!
//! Every collaborator is held as a trait object, the DAO included, so a test can
//! install a `mockall` double at whichever layer it wants to cut: a route test
//! mocks [`SessionStore`], a service test mocks the DAO underneath it.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::TimeDelta;

use crate::{
    config::Config,
    dao::sessions::{PgSessionDao, SessionDao},
    db::Database,
    services::{
        auth::AuthProvider,
        session::{DaoSessionStore, SessionStore},
    },
};

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    database: Database,
    auth: Arc<dyn AuthProvider>,
    session_dao: Arc<dyn SessionDao>,
    sessions: Arc<dyn SessionStore>,
    started_at: Instant,
}

impl AppState {
    /// Builds the state, including the connection pool.
    ///
    /// Infallible and synchronous: the pool connects lazily, so nothing here
    /// touches the network. See [`Database::new`].
    ///
    /// The provider is passed in rather than built here because building one
    /// can fail, and because it is what lets a test install a double.
    #[must_use]
    pub fn new(config: Config, auth: Arc<dyn AuthProvider>) -> Self {
        let database = Database::new(&config);
        let ttl = session_ttl(&config);

        let session_dao: Arc<dyn SessionDao> = Arc::new(PgSessionDao::new(database.clone()));
        let sessions = Arc::new(DaoSessionStore::new(session_dao.clone(), ttl));

        Self {
            config: Arc::new(config),
            database,
            auth,
            session_dao,
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

    /// Swaps in a different auth provider.
    #[must_use]
    pub fn with_auth(mut self, auth: Arc<dyn AuthProvider>) -> Self {
        self.auth = auth;
        self
    }

    /// Swaps in a different session store, for a test that cuts at the service
    /// seam and does not care what is under it.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Arc<dyn SessionStore>) -> Self {
        self.sessions = sessions;
        self
    }

    /// Swaps in a different session DAO and rebuilds the session store over
    /// it, for a test that wants the real store with only the rows mocked.
    #[must_use]
    pub fn with_session_dao(mut self, session_dao: Arc<dyn SessionDao>) -> Self {
        self.sessions = Arc::new(DaoSessionStore::new(
            session_dao.clone(),
            session_ttl(&self.config),
        ));
        self.session_dao = session_dao;
        self
    }

    #[must_use]
    pub fn sessions(&self) -> &dyn SessionStore {
        self.sessions.as_ref()
    }

    #[must_use]
    pub fn session_dao(&self) -> &dyn SessionDao {
        self.session_dao.as_ref()
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

/// A configured TTL that does not fit `TimeDelta` is absurd rather than fatal,
/// so it falls back to the default fortnight instead of failing startup.
fn session_ttl(config: &Config) -> TimeDelta {
    TimeDelta::from_std(config.session_ttl)
        .unwrap_or_else(|_| TimeDelta::try_days(14).unwrap_or_default())
}

/// Hand-written because `dyn AuthProvider` is not `Debug`, and adding that
/// bound would force every double in a test to implement it.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}
