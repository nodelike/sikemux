use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum LinearError {
    Unconfigured,
    Auth(String),
    RateLimited,
    /// Linear answered with errors and this message.
    Api(String),
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    NotFound(String),
    Keychain(String),
    Transport(String),
    /// The service could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
    Response(String),
}

pub type LinearResult<T> = Result<T, LinearError>;

impl fmt::Display for LinearError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("linear: no workspace is signed in"),
            Self::Auth(message) => write!(formatter, "linear: sign-in failed: {message}"),
            Self::RateLimited => {
                formatter.write_str("linear: too many requests; try again in a minute")
            }
            Self::Api(message) => write!(formatter, "linear: {message}"),
            Self::Http { status, message } => {
                write!(formatter, "linear: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "linear: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Linear"),
            ),
            Self::Response(message) => {
                write!(formatter, "linear: unexpected response: {message}")
            }
        }
    }
}

impl From<reqwest::Error> for LinearError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("linear: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for LinearError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<LinearError> for PluginError {
    fn from(error: LinearError) -> Self {
        let message = error.to_string();
        match error {
            LinearError::Unconfigured => PluginError::new("unconfigured", message),
            LinearError::Auth(_) => PluginError::new("auth", message),
            LinearError::RateLimited => PluginError::new("rate-limited", message).with_status(429),
            LinearError::Unreachable(kind) => PluginError::unreachable(kind, "Linear"),
            LinearError::Http { status, .. } => PluginError::http(status, message),
            LinearError::BadArg(_) => PluginError::new("bad-params", message),
            LinearError::NotFound(_) => PluginError::new("not-found", message),
            LinearError::Keychain(_) => PluginError::new("keychain", message),
            LinearError::Api(_) | LinearError::Transport(_) => PluginError::new("linear", message),
            LinearError::Response(_) => PluginError::new("response", message),
        }
    }
}
