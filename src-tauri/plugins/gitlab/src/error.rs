use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum GitlabError {
    Unconfigured,
    Auth(String),
    Forbidden(String),
    RateLimited { resets_in_secs: u64 },
    Http { status: u16, message: String },
    BadArg(String),
    NotFound(String),
    Unsupported(&'static str),
    Keychain(String),
    Transport(String),
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
        let (category, status) = match &error {
            GitlabError::Unconfigured => ("unconfigured", None),
            GitlabError::Auth(_) => ("auth", None),
            GitlabError::Forbidden(_) => ("forbidden", Some(403)),
            GitlabError::RateLimited { .. } => ("rate-limited", Some(429)),
            GitlabError::Http { status, .. } => ("http", Some(*status)),
            GitlabError::BadArg(_) => ("bad-params", None),
            GitlabError::NotFound(_) => ("not-found", Some(404)),
            GitlabError::Unsupported(_) => ("unsupported", None),
            GitlabError::Keychain(_) => ("keychain", None),
            GitlabError::Transport(_) => ("gitlab", None),
            GitlabError::Response(_) => ("response", None),
        };
        let plugin_error = PluginError::new(category, error.to_string());
        match status {
            Some(status) => plugin_error.with_status(status),
            None => plugin_error,
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
