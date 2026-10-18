use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum SignozError {
    Unconfigured,
    Auth(String),
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

impl fmt::Display for SignozError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("signoz: not configured"),
            Self::Auth(message) => write!(formatter, "signoz: sign-in failed: {message}"),
            Self::Http { status, message } => write!(formatter, "signoz: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "signoz: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "SigNoz"),
            ),
            Self::Response(message) => write!(formatter, "signoz: unexpected response: {message}"),
        }
    }
}

impl From<reqwest::Error> for SignozError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("signoz: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for SignozError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<SignozError> for PluginError {
    fn from(error: SignozError) -> Self {
        let message = error.to_string();
        match error {
            SignozError::Unconfigured => PluginError::new("unconfigured", message),
            SignozError::Auth(_) => PluginError::new("auth", message),
            SignozError::Http { status, .. } => PluginError::http(status, message),
            SignozError::BadArg(_) => PluginError::new("bad-params", message),
            SignozError::NotFound(_) => PluginError::new("not-found", message),
            SignozError::Keychain(_) => PluginError::new("keychain", message),
            SignozError::Unreachable(kind) => PluginError::unreachable(kind, "SigNoz"),
            SignozError::Transport(_) => PluginError::new("signoz", message),
            SignozError::Response(_) => PluginError::new("response", message),
        }
    }
}

pub type SignozResult<T> = Result<T, SignozError>;
