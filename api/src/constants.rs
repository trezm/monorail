//! Environment variable names and their defaults.

pub const ENV: &str = "API_ENV";
pub const HOST: &str = "API_HOST";
pub const PORT: &str = "API_PORT";
/// Unprefixed, and what most container platforms inject. Loses to `API_PORT`.
pub const PORT_FALLBACK: &str = "PORT";
pub const LOG_FORMAT: &str = "API_LOG_FORMAT";
pub const LOG_FILTER: &str = "API_LOG_FILTER";
pub const REQUEST_TIMEOUT_SECS: &str = "API_REQUEST_TIMEOUT_SECS";
pub const BODY_LIMIT_BYTES: &str = "API_BODY_LIMIT_BYTES";
pub const CORS_ALLOWED_ORIGINS: &str = "API_CORS_ALLOWED_ORIGINS";
/// Railway's GraphQL endpoint. Override for a self-hosted install or to point
/// a test at a local stub.
pub const RAILWAY_ENDPOINT: &str = "API_RAILWAY_ENDPOINT";
/// Per-request budget for a single call to Railway.
pub const RAILWAY_TIMEOUT_SECS: &str = "API_RAILWAY_TIMEOUT_SECS";

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_LOG_FILTER: &str = "info,tower_http=debug,monorail_api=debug";
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_RAILWAY_ENDPOINT: &str = "https://backboard.railway.com/graphql/v2";
/// Comfortably inside `DEFAULT_REQUEST_TIMEOUT_SECS`, so a stalled call to
/// Railway cannot outlive the request that triggered it.
pub const DEFAULT_RAILWAY_TIMEOUT_SECS: u64 = 15;
