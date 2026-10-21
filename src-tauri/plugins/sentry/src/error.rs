use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum SentryError {
    Unconfigured,
    Auth(String),
    Forbidden(String),
    NotFound(String),
    RateLimited,
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    Keychain(String),
    Transport(String),
    /// Sentry could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
    Response(String),
}

pub type SentryResult<T> = Result<T, SentryError>;

impl fmt::Display for SentryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("sentry: not signed in"),
            Self::Auth(message) => write!(formatter, "sentry: sign-in failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "sentry: not allowed: {message}"),
            Self::NotFound(message) => write!(formatter, "sentry: {message}"),
            Self::RateLimited => {
                formatter.write_str("sentry: too many requests; try again in a minute")
            }
            Self::Http { status, message } => {
                write!(formatter, "sentry: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "sentry: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Sentry"),
            ),
            Self::Response(message) => {
                write!(formatter, "sentry: unexpected response: {message}")
            }
        }
    }
}

impl From<reqwest::Error> for SentryError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("sentry: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for SentryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<SentryError> for PluginError {
    fn from(error: SentryError) -> Self {
        let message = error.to_string();
        match error {
            SentryError::Unconfigured => PluginError::new("unconfigured", message),
            SentryError::Auth(_) => PluginError::new("auth", message),
            SentryError::Forbidden(_) => PluginError::new("forbidden", message).with_status(403),
            SentryError::NotFound(_) => PluginError::new("not-found", message),
            SentryError::RateLimited => PluginError::new("rate-limited", message).with_status(429),
            SentryError::Unreachable(kind) => PluginError::unreachable(kind, "Sentry"),
            SentryError::Http { status, .. } => PluginError::http(status, message),
            SentryError::BadArg(_) => PluginError::new("bad-params", message),
            SentryError::Keychain(_) => PluginError::new("keychain", message),
            SentryError::Transport(_) => PluginError::new("sentry", message),
            SentryError::Response(_) => PluginError::new("response", message),
        }
    }
}
