//! Applies pending migrations, then exits.
//!
//! Separate from the server so migrating is an explicit step: one job per
//! deploy, rather than every replica racing to run them at startup. Reads the
//! same `API_DATABASE_URL` as the server and the same embedded
//! [`MIGRATIONS`](monorail_api::db::MIGRATIONS), so there is no second source
//! of truth and no diesel-cli to install.

use monorail_api::{Config, Database, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let config = Config::from_env()?;
    telemetry::init(&config)?;

    let applied = Database::new(&config).migrate().await?;

    if applied.is_empty() {
        tracing::info!("database schema is up to date");
    } else {
        tracing::info!(versions = ?applied, count = applied.len(), "applied migrations");
    }

    Ok(())
}
