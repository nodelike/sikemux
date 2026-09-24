use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum SignozError {
    Unconfigured,
    Auth(String),
    Http { status: u16, message: String },
    BadArg(String),
    NotFound(String),
    Keychain(String),
    Transport(String),
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
            Self::Response(message) => write!(formatter, "signoz: unexpected response: {message}"),
        }
    }
}

impl From<reqwest::Error> for SignozError {
    fn from(error: reqwest::Error) -> Self {
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
        let (category, status) = match &error {
            SignozError::Unconfigured => ("unconfigured", None),
            SignozError::Auth(_) => ("auth", None),
            SignozError::Http { status, .. } => ("http", Some(*status)),
            SignozError::BadArg(_) => ("bad-params", None),
            SignozError::NotFound(_) => ("not-found", None),
            SignozError::Keychain(_) => ("keychain", None),
            SignozError::Transport(_) => ("signoz", None),
            SignozError::Response(_) => ("response", None),
        };
        let plugin_error = PluginError::new(category, error.to_string());
        match status {
            Some(status) => plugin_error.with_status(status),
            None => plugin_error,
        }
    }
}

pub type SignozResult<T> = Result<T, SignozError>;
