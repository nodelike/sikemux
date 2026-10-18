use std::fmt;
use std::io;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum RundeckError {
    Api(String),
    Unconfigured,
    Auth(String),
    Forbidden(String),
    Transport(String),
    Unreachable(sikemux_plugin_api::Unreachable, String),
    Http { status: u16, message: String },
    BadArg(&'static str),
    Unclear(String),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for RundeckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(message) => write!(formatter, "rundeck: {message}"),
            Self::Unconfigured => formatter.write_str("not signed in to Rundeck"),
            Self::Auth(message) => write!(formatter, "rundeck: auth failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "rundeck: not allowed: {message}"),
            Self::Transport(message) => write!(formatter, "rundeck: {message}"),
            Self::Unreachable(_, message) => formatter.write_str(message),
            Self::Http { status, message } => {
                write!(formatter, "rundeck: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Unclear(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Json(error) => write!(formatter, "json: {error}"),
        }
    }
}

impl From<io::Error> for RundeckError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RundeckError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<reqwest::Error> for RundeckError {
    fn from(error: reqwest::Error) -> Self {
        let Some(kind) = sikemux_plugin_api::unreachable(&error) else {
            return Self::Transport(error.to_string());
        };
        eprintln!("rundeck: {error}");
        let host = error
            .url()
            .and_then(|url| url.host_str())
            .unwrap_or("its address");
        let how = match kind {
            sikemux_plugin_api::Unreachable::Timeout => "it did not answer in time",
            sikemux_plugin_api::Unreachable::Offline => "the connection failed",
        };
        Self::Unreachable(
            kind,
            format!("Can't reach Rundeck at {host} ({how}). Check the network or VPN."),
        )
    }
}

impl From<RundeckError> for PluginError {
    fn from(error: RundeckError) -> Self {
        let message = error.to_string();
        match error {
            RundeckError::Api(_) | RundeckError::Transport(_) => {
                PluginError::new("rundeck", message)
            }
            RundeckError::Unreachable(kind, _) => {
                PluginError::new(sikemux_plugin_api::network::category(kind), message)
            }
            RundeckError::Unconfigured => PluginError::new("unconfigured", message),
            RundeckError::Auth(_) => PluginError::new("auth", message),
            RundeckError::Forbidden(_) => PluginError::forbidden(message),
            RundeckError::Http { status, .. } => PluginError::http(status, message),
            RundeckError::BadArg(_) | RundeckError::Unclear(_) => {
                PluginError::new("bad-params", message)
            }
            RundeckError::Io(_) => PluginError::new("io", message),
            RundeckError::Json(_) => PluginError::new("json", message),
        }
    }
}

pub type RundeckResult<T> = Result<T, RundeckError>;
