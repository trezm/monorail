//! A string that must not reach a log line.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Wraps a credential so it cannot be printed by accident.
///
/// `Debug` prints `[redacted]` and there is deliberately no `Display`, so the
/// value only leaves through [`Self::expose`] — which is greppable, unlike
/// `{}`. Same reasoning as [`DatabaseUrl`](crate::config::DatabaseUrl), which
/// predates this type and keeps its own redaction because it prints a useful
/// partial value rather than nothing.
///
/// This is not memory protection. It hides a secret from `tracing`, panics and
/// error reports, not from a process dump.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_never_prints_the_value() {
        let secret = Secret::new("hunter2");

        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert!(!format!("{:?}", vec![secret.clone()]).contains("hunter2"));
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn serde_round_trips_transparently() {
        let json = serde_json::to_string(&Secret::new("token")).expect("serializes");

        assert_eq!(json, "\"token\"");
        assert_eq!(
            serde_json::from_str::<Secret>(&json)
                .expect("deserializes")
                .expose(),
            "token"
        );
    }
}
