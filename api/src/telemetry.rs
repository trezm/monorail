//! Structured logging setup.
//!
//! Pretty, human-readable output locally; one JSON object per line everywhere
//! else, so a log aggregator can index the span fields (including `request_id`).

use anyhow::Context as _;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::config::{Config, LogFormat};

/// Installs the global tracing subscriber.
///
/// Call exactly once, as early in `main` as possible. Returns an error if the
/// configured filter directive is invalid or a subscriber is already installed.
pub fn init(config: &Config) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(&config.log_filter)
        .with_context(|| format!("invalid log filter `{}`", config.log_filter))?;

    let registry = tracing_subscriber::registry().with(filter);

    match config.log_format {
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
        LogFormat::Pretty => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .pretty()
                    .with_target(true)
                    .with_file(false)
                    .with_line_number(false),
            )
            .try_init(),
    }
    .context("a tracing subscriber is already installed")
}
