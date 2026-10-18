use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum GitlabError {
    Unconfigured,
    Auth(String),
    Forbidden(String),
    RateLimited {
        resets_in_secs: u64,
    },
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    NotFound(String),
    Unsupported(&'static str),
    Keychain(String),
    Transport(String),
    /// The service could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
    Response(String),
}

impl fmt::Display for GitlabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("gitlab: not signed in"),
            Self::Auth(message) => write!(formatter, "gitlab: sign-in failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "gitlab: not allowed: {message}"),
            Self::RateLimited { resets_in_secs } => write!(
                formatter,
                "gitlab: the rate limit is used up; try again in {}",
                in_words(*resets_in_secs)
            ),
            Self::Http { status, message } => {
                write!(formatter, "gitlab: http {status}: {message}")
            }
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Unsupported(what) => write!(formatter, "GitLab cannot {what}"),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "gitlab: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "GitLab"),
            ),
            Self::Response(message) => {
                write!(formatter, "gitlab: unexpected response: {message}")
            }
        }
    }
}

/// reqwest's own message stops at "error sending request"; why it could not
/// be sent, such as a DNS or certificate failure, is further down the chain.
impl From<reqwest::Error> for GitlabError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("gitlab: {error}");
            return Self::Unreachable(kind);
        }
        let mut message = error.to_string();
        let mut cause = std::error::Error::source(&error);
        while let Some(next) = cause {
            let text = next.to_string();
            if !message.contains(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }
            cause = next.source();
        }
        Self::Transport(message)
    }
}

impl From<serde_json::Error> for GitlabError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<GitlabError> for PluginError {
    fn from(error: GitlabError) -> Self {
        let message = error.to_string();
        match error {
            GitlabError::Unconfigured => PluginError::new("unconfigured", message),
            GitlabError::Auth(_) => PluginError::new("auth", message),
            GitlabError::Forbidden(_) => PluginError::forbidden(message),
            GitlabError::RateLimited { .. } => {
                PluginError::new("rate-limited", message).with_status(429)
            }
            GitlabError::Http { status, .. } => PluginError::http(status, message),
            GitlabError::BadArg(_) => PluginError::new("bad-params", message),
            GitlabError::NotFound(_) => PluginError::new("not-found", message).with_status(404),
            GitlabError::Unsupported(_) => PluginError::new("unsupported", message),
            GitlabError::Keychain(_) => PluginError::new("keychain", message),
            GitlabError::Unreachable(kind) => PluginError::unreachable(kind, "GitLab"),
            GitlabError::Transport(_) => PluginError::new("gitlab", message),
            GitlabError::Response(_) => PluginError::new("response", message),
        }
    }
}

/// `40s`, `12 min` or `2 h 5 min`, for telling someone how long to wait.
pub fn in_words(secs: u64) -> String {
    match secs {
        0..=59 => format!("{}s", secs.max(1)),
        60..=3599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

pub type GitlabResult<T> = Result<T, GitlabError>;
