use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum DockerError {
    /// No engine answers on any socket Sikemux knows of.
    NotRunning(String),
    /// `DOCKER_HOST` or the current context points somewhere other than a socket on this Mac.
    Unsupported(String),
    NotFound(String),
    /// The engine refused because of the object's state, such as removing a running container.
    Conflict(String),
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    Transport(String),
    Response(String),
}

pub type DockerResult<T> = Result<T, DockerError>;

impl fmt::Display for DockerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning(message) => write!(formatter, "docker: {message}"),
            Self::Unsupported(message) => write!(formatter, "docker: {message}"),
            Self::NotFound(message) => write!(formatter, "docker: {message}"),
            Self::Conflict(message) => write!(formatter, "docker: {message}"),
            Self::Http { status, message } => write!(formatter, "docker: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Transport(message) => write!(formatter, "docker: {message}"),
            Self::Response(message) => write!(formatter, "docker: unexpected response: {message}"),
        }
    }
}

impl From<serde_json::Error> for DockerError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<hyper::Error> for DockerError {
    fn from(error: hyper::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

impl From<DockerError> for PluginError {
    fn from(error: DockerError) -> Self {
        let message = error.to_string();
        match error {
            DockerError::NotRunning(_) => PluginError::new("not-running", message),
            DockerError::Unsupported(_) => PluginError::new("unsupported", message),
            DockerError::NotFound(_) => PluginError::new("not-found", message),
            DockerError::Conflict(_) => PluginError::new("conflict", message).with_status(409),
            DockerError::Http { status, .. } => PluginError::http(status, message),
            DockerError::BadArg(_) => PluginError::new("bad-params", message),
            DockerError::Transport(_) => PluginError::new("docker", message),
            DockerError::Response(_) => PluginError::new("response", message),
        }
    }
}
