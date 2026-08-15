//! Shared application state.
//!
//! One `Arc` wraps the whole struct, so `AppState` is cheap to clone (axum
//! clones it per request) and adding a field does not add another allocation.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{config::Config, widget::WidgetStore};

#[derive(Debug, Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: Config,
    widgets: WidgetStore,
    started_at: Instant,
}

impl AppState {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                widgets: WidgetStore::new(),
                started_at: Instant::now(),
            }),
        }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    #[must_use]
    pub fn widgets(&self) -> &WidgetStore {
        &self.inner.widgets
    }

    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.inner.started_at.elapsed()
    }
}
