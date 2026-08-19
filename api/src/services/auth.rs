//! Caller credentials passed through to a backend.

use std::fmt;

/// A bearer token authenticating the caller against a backend.
///
/// Every [`ServiceManager`](crate::services::service::ServiceManager) and
/// [`ProjectManager`](crate::services::project::ProjectManager) method takes
/// one, so each call is made *as* a particular caller rather than as one set
/// of ambient credentials shared by the whole process. Where the token comes
/// from — a request header, a config value, a vault — is the caller's problem.
///
/// This is a distinct type rather than a `&str` for two reasons: a token
/// cannot be passed where a URL or a name is expected, and [`Debug`] redacts
/// it. The codebase already redacts `Authorization` on the way in and out
/// (see the sensitive-header layers in `crate::app`); this keeps a token from
/// leaking through a `?`-formatted log line in between.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccessToken(String);

impl AccessToken {
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The raw token. Only for handing to a backend — never log the result.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

impl From<String> for AccessToken {
    fn from(token: String) -> Self {
        Self(token)
    }
}

#[cfg(test)]
mod tests {
    use super::AccessToken;

    #[test]
    fn debug_does_not_leak_the_token() {
        let token = AccessToken::new("super-secret-value");
        let rendered = format!("{token:?}");

        assert!(!rendered.contains("super-secret-value"), "{rendered}");
        assert_eq!(rendered, "AccessToken(<redacted>)");
    }

    #[test]
    fn expose_returns_the_raw_token() {
        assert_eq!(AccessToken::new("abc").expose(), "abc");
    }
}
