use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum VercelError {
    Unconfigured,
    Auth(String),
    RateLimited,
    /// Vercel answered with an error and this message.
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

pub type VercelResult<T> = Result<T, VercelError>;

impl fmt::Display for VercelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("vercel: not signed in"),
            Self::Auth(message) => write!(formatter, "vercel: sign-in failed: {message}"),
            Self::RateLimited => {
                formatter.write_str("vercel: too many requests; try again in a minute")
            }
            Self::Api(message) => write!(formatter, "vercel: {message}"),
            Self::Http { status, message } => {
                write!(formatter, "vercel: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => write!(formatter, "vercel: {message}"),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "vercel: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Vercel"),
            ),
            Self::Response(message) => {
                write!(formatter, "vercel: unexpected response: {message}")
            }
        }
    }
}

impl From<reqwest::Error> for VercelError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("vercel: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for VercelError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<VercelError> for PluginError {
    fn from(error: VercelError) -> Self {
        let message = error.to_string();
        match error {
            VercelError::Unconfigured => PluginError::new("unconfigured", message),
            VercelError::Auth(_) => PluginError::new("auth", message),
            VercelError::RateLimited => PluginError::new("rate-limited", message).with_status(429),
            VercelError::Unreachable(kind) => PluginError::unreachable(kind, "Vercel"),
            VercelError::Http { status, .. } => PluginError::http(status, message),
            VercelError::BadArg(_) => PluginError::new("bad-params", message),
            VercelError::NotFound(_) => PluginError::new("not-found", message),
            VercelError::Keychain(_) => PluginError::new("keychain", message),
            VercelError::Api(_) | VercelError::Transport(_) => PluginError::new("vercel", message),
            VercelError::Response(_) => PluginError::new("response", message),
        }
    }
}
