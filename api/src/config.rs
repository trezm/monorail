//! Process configuration, read once at startup from the environment.
//!
//! Every setting has a usable default so the service starts with no environment
//! at all. See `.env.example` for the full list.

use std::{
    convert::Infallible,
    env::{self, VarError},
    fmt::{self, Display},
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
    time::Duration,
};

use url::Url;

use crate::{constants, secret::Secret};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{key} is not valid UTF-8")]
    NotUnicode { key: String },
    #[error("{key} must be set outside development")]
    Missing { key: String },
    #[error("{key} must be set")]
    Required { key: String },
    #[error("{key}=\"{value}\" could not be parsed: {source}")]
    Invalid {
        key: String,
        value: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Staging,
    Production,
}

impl Environment {
    #[must_use]
    pub fn is_development(self) -> bool {
        self == Self::Development
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

impl FromStr for Environment {
    type Err = Infallible;

    /// Anything unrecognised is development.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "stage" | "staging" => Self::Staging,
            "prod" | "production" => Self::Production,
            _ => Self::Development,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, multi-line. Good for a terminal.
    Pretty,
    /// One JSON object per line. Good for a log aggregator.
    Json,
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pretty" | "text" => Ok(Self::Pretty),
            "json" => Ok(Self::Json),
            other => Err(format!("expected one of pretty/json, got `{other}`")),
        }
    }
}

/// Which origins the browser CORS layer will accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsOrigins {
    /// No `Access-Control-Allow-Origin` header is sent; browsers block cross-origin calls.
    Disabled,
    /// Reflects any origin. Convenient locally, rarely what you want in production.
    Any,
    /// An explicit allow-list.
    List(Vec<String>),
}

/// A Postgres connection string.
///
/// A newtype rather than a `String` for one reason: the password lives in it,
/// and [`Config`] derives `Debug`. This type's `Debug` redacts, so no
/// `?config` in a log line — or a panic message, or an error report — can spill
/// the credential. `Display` is intentionally absent so there is no accidental
/// unredacted path; the connection string itself comes out of [`Self::as_str`].
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseUrl(String);

impl DatabaseUrl {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `postgres://user:****@host:5432/db`. Anything that does not parse as a
    /// URL with credentials is returned unchanged — it has no password to hide.
    #[must_use]
    pub fn redacted(&self) -> String {
        let Some((scheme, rest)) = self.0.split_once("://") else {
            return self.0.clone();
        };
        let Some((authority, tail)) = rest.split_once('@') else {
            return self.0.clone();
        };

        let credentials = match authority.split_once(':') {
            Some((user, _password)) => format!("{user}:****"),
            None => authority.to_owned(),
        };

        format!("{scheme}://{credentials}@{tail}")
    }
}

impl fmt::Debug for DatabaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl FromStr for DatabaseUrl {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Environment,
    pub host: IpAddr,
    pub port: u16,
    pub log_format: LogFormat,
    /// A `tracing_subscriber::EnvFilter` directive, e.g. `info,monorail_api=debug`.
    pub log_filter: String,
    /// Per-request wall-clock budget before the server answers `408`.
    pub request_timeout: Duration,
    /// Maximum accepted request body size, in bytes.
    pub body_limit_bytes: usize,
    pub cors_origins: CorsOrigins,
    /// libpq-style connection string, e.g. `postgres://user:pw@host:5432/db`.
    pub database_url: DatabaseUrl,
    /// Upper bound on concurrent Postgres connections held by this process.
    /// Postgres itself caps total connections (`max_connections`, 100 by
    /// default), so this times the replica count has to stay under that.
    pub database_pool_size: u32,
    /// How long a request waits for a pooled connection before giving up. This
    /// covers queueing behind a busy pool as well as opening a new connection.
    pub database_connect_timeout: Duration,
    /// How long a session cookie stays valid. Enforced against the stored
    /// session, not the cookie's own `Max-Age`, which a client controls.
    pub session_ttl: Duration,
    /// Where a completed login sends the browser.
    pub auth_success_redirect: String,
}

/// Everything a login against Railway needs.
///
/// The client id, client secret and redirect URI have no defensible default
/// and are required. There is nothing to do with this service without a login,
/// so a missing one is a startup error rather than a disabled feature.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// Base URL of the provider, always with a trailing slash so joining a
    /// relative endpoint path cannot eat a path segment.
    pub issuer: Url,
    pub client_id: String,
    pub client_secret: Secret,
    /// Must byte-for-byte match a redirect URI registered on the OAuth app.
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    /// Wall-clock budget for one call to the provider.
    pub timeout: Duration,
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let environment = parsed(constants::ENV, Environment::Development)?;
        let cors = parsed(constants::CORS_ALLOWED_ORIGINS, String::new())?;

        Ok(Self {
            environment,
            host: parsed(constants::HOST, IpAddr::V4(Ipv4Addr::UNSPECIFIED))?,
            port: parsed(
                constants::PORT,
                parsed(constants::PORT_FALLBACK, constants::DEFAULT_PORT)?,
            )?,
            log_format: parsed(
                constants::LOG_FORMAT,
                if environment.is_development() {
                    LogFormat::Pretty
                } else {
                    LogFormat::Json
                },
            )?,
            log_filter: parsed(
                constants::LOG_FILTER,
                constants::DEFAULT_LOG_FILTER.to_owned(),
            )?,
            request_timeout: Duration::from_secs(parsed(
                constants::REQUEST_TIMEOUT_SECS,
                constants::DEFAULT_REQUEST_TIMEOUT_SECS,
            )?),
            body_limit_bytes: parsed(
                constants::BODY_LIMIT_BYTES,
                constants::DEFAULT_BODY_LIMIT_BYTES,
            )?,
            cors_origins: match cors.as_str() {
                "" => CorsOrigins::Disabled,
                "*" => CorsOrigins::Any,
                list => CorsOrigins::List(
                    list.split(',')
                        .map(str::trim)
                        .filter(|origin| !origin.is_empty())
                        .map(ToOwned::to_owned)
                        .collect(),
                ),
            },
            database_url: database_url(environment)?,
            database_pool_size: parsed(
                constants::DATABASE_POOL_SIZE,
                constants::DEFAULT_DATABASE_POOL_SIZE,
            )?,
            database_connect_timeout: Duration::from_secs(parsed(
                constants::DATABASE_CONNECT_TIMEOUT_SECS,
                constants::DEFAULT_DATABASE_CONNECT_TIMEOUT_SECS,
            )?),
            session_ttl: Duration::from_secs(parsed(
                constants::SESSION_TTL_SECS,
                constants::DEFAULT_SESSION_TTL_SECS,
            )?),
            auth_success_redirect: parsed(
                constants::AUTH_SUCCESS_REDIRECT,
                constants::DEFAULT_AUTH_SUCCESS_REDIRECT.to_owned(),
            )?,
        })
    }
}

/// Resolves the connection string, preferring the prefixed name.
///
/// Development falls back to the local `compose.yaml` database so the service
/// still starts with no environment at all. Anywhere else an unset variable is
/// an error: defaulting to localhost in production would turn a deployment
/// mistake into a confusing connection refused at the first request.
fn database_url(environment: Environment) -> Result<DatabaseUrl, ConfigError> {
    let configured = match present(constants::DATABASE_URL)? {
        Some(url) => Some(url),
        None => present(constants::DATABASE_URL_FALLBACK)?,
    };

    match configured {
        Some(url) => Ok(DatabaseUrl::new(url)),
        None if environment.is_development() => {
            Ok(DatabaseUrl::new(constants::DEFAULT_DATABASE_URL))
        }
        None => Err(ConfigError::Missing {
            key: constants::DATABASE_URL.to_owned(),
        }),
    }
}

impl OAuthConfig {
    /// Reads the login settings from the process environment.
    ///
    /// Separate from [`Config::from_env`] because only the server logs anyone
    /// in. `//api:migrate` needs a database and nothing else, and asking a
    /// migration job for OAuth credentials it never sends would be a way to
    /// leak them.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            issuer: issuer()?,
            client_id: required(constants::RAILWAY_CLIENT_ID)?,
            client_secret: Secret::new(required(constants::RAILWAY_CLIENT_SECRET)?),
            redirect_uri: required(constants::RAILWAY_REDIRECT_URI)?,
            scopes: scopes()?,
            timeout: Duration::from_secs(parsed(
                constants::RAILWAY_TIMEOUT_SECS,
                constants::DEFAULT_RAILWAY_TIMEOUT_SECS,
            )?),
        })
    }
}

/// Normalises the issuer to end in `/`. Without that, `Url::join("oauth/token")`
/// replaces the issuer's last path segment instead of appending to it.
fn issuer() -> Result<Url, ConfigError> {
    let configured: String = parsed(
        constants::RAILWAY_ISSUER,
        constants::DEFAULT_RAILWAY_ISSUER.to_owned(),
    )?;

    let base = format!("{}/", configured.trim_end_matches('/'));

    base.parse()
        .map_err(|source: url::ParseError| ConfigError::Invalid {
            key: constants::RAILWAY_ISSUER.to_owned(),
            value: configured,
            source: source.to_string().into(),
        })
}

/// Scopes are space-separated on the wire; commas are accepted too because
/// every other list-valued variable here uses them.
fn scopes() -> Result<Vec<String>, ConfigError> {
    let configured: String = parsed(
        constants::RAILWAY_SCOPES,
        constants::DEFAULT_RAILWAY_SCOPES.to_owned(),
    )?;

    let scopes: Vec<String> = configured
        .split([',', ' ', '\t', '\n'])
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if !scopes
        .iter()
        .any(|scope| scope == constants::REQUIRED_RAILWAY_SCOPE)
    {
        return Err(ConfigError::Invalid {
            key: constants::RAILWAY_SCOPES.to_owned(),
            value: configured,
            source: format!(
                "OpenID Connect requires the `{}` scope",
                constants::REQUIRED_RAILWAY_SCOPE
            )
            .into(),
        });
    }

    Ok(scopes)
}

/// Reads `key`, failing when it is absent.
fn required(key: &str) -> Result<String, ConfigError> {
    present(key)?.ok_or_else(|| ConfigError::Required {
        key: key.to_owned(),
    })
}

/// Reads `key`, treating unset and empty-after-trim alike. An exported-but-empty
/// variable is what a template that did not get substituted looks like, and
/// taking it literally is never useful.
fn present(key: &str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value.trim().to_owned()).filter(|value| !value.is_empty())),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::NotUnicode {
            key: key.to_owned(),
        }),
    }
}

/// Reads and parses `key`, falling back to `default` when unset.
fn parsed<T>(key: &str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    let value = match env::var(key) {
        Ok(value) => value,
        Err(VarError::NotPresent) => return Ok(default),
        Err(VarError::NotUnicode(_)) => {
            return Err(ConfigError::NotUnicode {
                key: key.to_owned(),
            });
        }
    };

    value
        .trim()
        .parse()
        .map_err(|source: T::Err| ConfigError::Invalid {
            key: key.to_owned(),
            value: value.clone(),
            source: source.to_string().into(),
        })
}
