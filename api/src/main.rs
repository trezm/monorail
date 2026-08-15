//! Binary entrypoint. Everything substantive lives in the library so it can be
//! tested in-process; this file only wires the process together.

use monorail_api::{Config, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Developer convenience: a local `.env` never overrides real environment.
    // Absent file is not an error.
    let _ = dotenvy::dotenv();

    let config = Config::from_env()?;
    telemetry::init(&config)?;

    tracing::info!(
        environment = config.environment.as_str(),
        version = env!("CARGO_PKG_VERSION"),
        "starting"
    );

    monorail_api::run(config).await
}
