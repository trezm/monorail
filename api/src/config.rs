//! Process configuration, read once at startup from the environment.
//!
//! Every setting has a usable default so the service starts with no environment
//! at all. See `.env.example` for the full list.

use std::{
    env::{self, VarError},
    fmt::Display,
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
    time::Duration,
};

/// Prefix for every variable this service reads.
const PREFIX: &str = "API_";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{key} is not valid UTF-8")]
    NotUnicode { key: String },
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
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" | "local" => Ok(Self::Development),
            "stage" | "staging" => Ok(Self::Staging),
            "prod" | "production" => Ok(Self::Production),
            other => Err(format!(
                "expected one of development/staging/production, got `{other}`"
            )),
        }
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
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let environment = parsed("ENV", Environment::Development)?;

        Ok(Self {
            environment,
            host: parsed("HOST", IpAddr::V4(Ipv4Addr::UNSPECIFIED))?,
            // `PORT` (unprefixed) is what most container platforms inject, so it
            // acts as a fallback when `API_PORT` is absent.
            port: match raw("PORT")? {
                Some(_) => parsed("PORT", 8080)?,
                None => parse_var("PORT", 8080)?,
            },
            log_format: parsed(
                "LOG_FORMAT",
                if environment.is_development() {
                    LogFormat::Pretty
                } else {
                    LogFormat::Json
                },
            )?,
            log_filter: raw("LOG_FILTER")?
                .unwrap_or_else(|| "info,tower_http=debug,monorail_api=debug".to_owned()),
            request_timeout: Duration::from_secs(parsed("REQUEST_TIMEOUT_SECS", 30_u64)?),
            body_limit_bytes: parsed("BODY_LIMIT_BYTES", 2 * 1024 * 1024_usize)?,
            cors_origins: match raw("CORS_ALLOWED_ORIGINS")?.as_deref().map(str::trim) {
                None | Some("") => CorsOrigins::Disabled,
                Some("*") => CorsOrigins::Any,
                Some(list) => CorsOrigins::List(
                    list.split(',')
                        .map(str::trim)
                        .filter(|origin| !origin.is_empty())
                        .map(ToOwned::to_owned)
                        .collect(),
                ),
            },
        })
    }
}

/// Reads `API_{key}` as a raw string.
fn raw(key: &str) -> Result<Option<String>, ConfigError> {
    read(&format!("{PREFIX}{key}"))
}

fn read(full_key: &str) -> Result<Option<String>, ConfigError> {
    match env::var(full_key) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::NotUnicode {
            key: full_key.to_owned(),
        }),
    }
}

/// Reads and parses `API_{key}`, falling back to `default` when unset.
fn parsed<T>(key: &str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    parse_var(&format!("{PREFIX}{key}"), default)
}

fn parse_var<T>(full_key: &str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    let Some(value) = read(full_key)? else {
        return Ok(default);
    };

    value
        .trim()
        .parse()
        .map_err(|source: T::Err| ConfigError::Invalid {
            key: full_key.to_owned(),
            value: value.clone(),
            source: source.to_string().into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_aliases() {
        assert_eq!("dev".parse(), Ok(Environment::Development));
        assert_eq!("PRODUCTION".parse(), Ok(Environment::Production));
        assert_eq!(" staging ".parse(), Ok(Environment::Staging));
        assert!("nope".parse::<Environment>().is_err());
    }

    #[test]
    fn parses_log_formats() {
        assert_eq!("json".parse(), Ok(LogFormat::Json));
        assert_eq!("Pretty".parse(), Ok(LogFormat::Pretty));
        assert!("xml".parse::<LogFormat>().is_err());
    }
}
