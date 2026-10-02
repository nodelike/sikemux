use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum GithubError {
    Unconfigured,
    Auth(String),
    Forbidden(String),
    RateLimited { resets_in_secs: u64 },
    Http { status: u16, message: String },
    BadArg(String),
    NotFound(String),
    Keychain(String),
    Transport(String),
    Response(String),
}

impl fmt::Display for GithubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("github: not signed in"),
            Self::Auth(message) => write!(formatter, "github: sign-in failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "github: not allowed: {message}"),
            Self::RateLimited { resets_in_secs } => write!(
                formatter,
                "github: the rate limit is used up; it resets in {}",
                in_words(*resets_in_secs)
            ),
            Self::Http { status, message } => write!(formatter, "github: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "github: {message}"),
            Self::Response(message) => write!(formatter, "github: unexpected response: {message}"),
        }
    }
}

/// reqwest's own message stops at "error sending request"; why it could not
/// be sent, such as a DNS or certificate failure, is further down the chain.
impl From<reqwest::Error> for GithubError {
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

impl From<serde_json::Error> for GithubError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<GithubError> for PluginError {
    fn from(error: GithubError) -> Self {
        let (category, status) = match &error {
            GithubError::Unconfigured => ("unconfigured", None),
            GithubError::Auth(_) => ("auth", None),
            GithubError::Forbidden(_) => ("forbidden", Some(403)),
            GithubError::RateLimited { .. } => ("rate-limited", Some(429)),
            GithubError::Http { status, .. } => ("http", Some(*status)),
            GithubError::BadArg(_) => ("bad-params", None),
            GithubError::NotFound(_) => ("not-found", Some(404)),
            GithubError::Keychain(_) => ("keychain", None),
            GithubError::Transport(_) => ("github", None),
            GithubError::Response(_) => ("response", None),
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

pub type GithubResult<T> = Result<T, GithubError>;
