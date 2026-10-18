use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum JiraError {
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

impl fmt::Display for JiraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("jira: no site is signed in"),
            Self::Auth(message) => write!(formatter, "jira: sign-in failed: {message}"),
            Self::Http { status, message } => write!(formatter, "jira: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "jira: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Jira"),
            ),
            Self::Response(message) => write!(formatter, "jira: unexpected response: {message}"),
        }
    }
}

impl From<reqwest::Error> for JiraError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("jira: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for JiraError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<JiraError> for PluginError {
    fn from(error: JiraError) -> Self {
        let message = error.to_string();
        match error {
            JiraError::Unconfigured => PluginError::new("unconfigured", message),
            JiraError::Auth(_) => PluginError::new("auth", message),
            JiraError::Http { status, .. } => PluginError::http(status, message),
            JiraError::BadArg(_) => PluginError::new("bad-params", message),
            JiraError::NotFound(_) => PluginError::new("not-found", message),
            JiraError::Keychain(_) => PluginError::new("keychain", message),
            JiraError::Unreachable(kind) => PluginError::unreachable(kind, "Jira"),
            JiraError::Transport(_) => PluginError::new("jira", message),
            JiraError::Response(_) => PluginError::new("response", message),
        }
    }
}

pub type JiraResult<T> = Result<T, JiraError>;
