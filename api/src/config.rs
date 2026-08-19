//! Process configuration, read once at startup from the environment.
//!
//! Every setting has a usable default so the service starts with no environment
//! at all. See `.env.example` for the full list.

use std::{
    convert::Infallible,
    env::{self, VarError},
    fmt::Display,
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
    time::Duration,
};

use crate::constants;

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
    /// Where the Railway client sends its GraphQL requests.
    pub railway_endpoint: String,
    /// Per-request budget for a single call to Railway.
    pub railway_timeout: Duration,
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
            railway_endpoint: parsed(
                constants::RAILWAY_ENDPOINT,
                constants::DEFAULT_RAILWAY_ENDPOINT.to_owned(),
            )?,
            railway_timeout: Duration::from_secs(parsed(
                constants::RAILWAY_TIMEOUT_SECS,
                constants::DEFAULT_RAILWAY_TIMEOUT_SECS,
            )?),
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
        })
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
