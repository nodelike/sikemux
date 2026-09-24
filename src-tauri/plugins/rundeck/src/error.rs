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
    Http { status: u16, message: String },
    BadArg(&'static str),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for RundeckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(message) => write!(formatter, "rundeck: {message}"),
            Self::Unconfigured => formatter.write_str("rundeck: not configured"),
            Self::Auth(message) => write!(formatter, "rundeck: auth failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "rundeck: not allowed: {message}"),
            Self::Transport(message) => write!(formatter, "rundeck: {message}"),
            Self::Http { status, message } => {
                write!(formatter, "rundeck: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
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
        Self::Transport(error.to_string())
    }
}

impl From<RundeckError> for PluginError {
    fn from(error: RundeckError) -> Self {
        let category = match &error {
            RundeckError::Api(_) => "rundeck",
            RundeckError::Unconfigured => "unconfigured",
            RundeckError::Auth(_) => "auth",
            RundeckError::Forbidden(_) => "forbidden",
            RundeckError::Transport(_) => "rundeck",
            RundeckError::Http { .. } => "http",
            RundeckError::BadArg(_) => "bad-params",
            RundeckError::Io(_) => "io",
            RundeckError::Json(_) => "json",
        };
        let status = match &error {
            RundeckError::Http { status, .. } => Some(*status),
            RundeckError::Forbidden(_) => Some(403),
            _ => None,
        };
        let plugin_error = PluginError::new(category, error.to_string());
        match status {
            Some(status) => plugin_error.with_status(status),
            None => plugin_error,
        }
    }
}

pub type RundeckResult<T> = Result<T, RundeckError>;
