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
pub const DATABASE_URL: &str = "API_DATABASE_URL";
/// Unprefixed, and what diesel-cli and most hosted Postgres providers use.
/// Loses to `API_DATABASE_URL`.
pub const DATABASE_URL_FALLBACK: &str = "DATABASE_URL";
pub const DATABASE_POOL_SIZE: &str = "API_DATABASE_POOL_SIZE";
pub const DATABASE_CONNECT_TIMEOUT_SECS: &str = "API_DATABASE_CONNECT_TIMEOUT_SECS";
pub const RAILWAY_CLIENT_ID: &str = "API_RAILWAY_CLIENT_ID";
pub const RAILWAY_CLIENT_SECRET: &str = "API_RAILWAY_CLIENT_SECRET";
pub const RAILWAY_REDIRECT_URI: &str = "API_RAILWAY_REDIRECT_URI";
pub const RAILWAY_SCOPES: &str = "API_RAILWAY_SCOPES";
pub const RAILWAY_ISSUER: &str = "API_RAILWAY_ISSUER";
pub const RAILWAY_TIMEOUT_SECS: &str = "API_RAILWAY_TIMEOUT_SECS";

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_LOG_FILTER: &str = "info,tower_http=debug,monorail_api=debug";
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Development-only default; it matches the Postgres in `compose.yaml`. Staging
/// and production have no default and must set the variable explicitly, so a
/// missing one fails at startup rather than quietly pointing at localhost.
pub const DEFAULT_DATABASE_URL: &str = "postgres://monorail:monorail@localhost:5432/monorail";
pub const DEFAULT_DATABASE_POOL_SIZE: u32 = 10;
pub const DEFAULT_DATABASE_CONNECT_TIMEOUT_SECS: u64 = 5;

/// Railway's `OpenID` Connect issuer. Overridable so a test can point the flow at
/// a local server; there is no other reason to change it.
pub const DEFAULT_RAILWAY_ISSUER: &str = "https://backboard.railway.com";

/// `openid` is mandatory. `email`/`profile` fill in the account shown to the
/// user; the `:member` pair is what lets a token act on Railway resources
/// later, and widens the consent screen accordingly.
pub const DEFAULT_RAILWAY_SCOPES: &str = "openid email profile project:member workspace:member";

/// The scope `OpenID` Connect requires, and without which the flow returns no
/// identity at all.
pub const REQUIRED_RAILWAY_SCOPE: &str = "openid";

pub const DEFAULT_RAILWAY_TIMEOUT_SECS: u64 = 10;
