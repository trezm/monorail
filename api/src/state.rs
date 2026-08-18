//! Shared application state.
//!
//! `Config` sits behind an `Arc` because axum clones the state per request and
//! it owns heap data; `Instant` is `Copy`, so it does not need to.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct AppState {
    config: Arc<Config>,
    started_at: Instant,
}

impl AppState {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            started_at: Instant::now(),
        }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}
