use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum SlackError {
    Unconfigured,
    Auth(String),
    /// The token lacks a scope a call needs; Slack names it.
    MissingScope(String),
    RateLimited {
        retry_after_secs: u64,
    },
    /// Slack answered `ok: false` with this error code.
    Api(String),
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    NotFound(String),
    Keychain(String),
    Transport(String),
    Response(String),
}

impl fmt::Display for SlackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("slack: no workspace is signed in"),
            Self::Auth(message) => write!(formatter, "slack: sign-in failed: {message}"),
            Self::MissingScope(scope) => write!(
                formatter,
                "slack: the token cannot do this; add the {scope} scope to the Slack app and sign in again"
            ),
            Self::RateLimited { retry_after_secs } => write!(
                formatter,
                "slack: too many requests; try again in {retry_after_secs}s"
            ),
            Self::Api(code) => write!(formatter, "slack: {}", in_words(code)),
            Self::Http { status, message } => write!(formatter, "slack: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Transport(message) => write!(formatter, "slack: {message}"),
            Self::Response(message) => write!(formatter, "slack: unexpected response: {message}"),
        }
    }
}

/// Slack's error codes as a sentence, for the ones people meet.
pub fn in_words(code: &str) -> String {
    match code {
        "channel_not_found" => "that channel does not exist, or this account cannot see it".into(),
        "not_in_channel" => "this account is not in that channel".into(),
        "thread_not_found" | "message_not_found" => {
            "that message is gone, or this account cannot see it".into()
        }
        "is_archived" => "that channel is archived".into(),
        "msg_too_long" => "the message is too long for Slack".into(),
        "no_text" => "the message is empty".into(),
        "user_not_found" | "users_not_found" => "no such person in this workspace".into(),
        other => other.replace('_', " "),
    }
}

/// reqwest's own message stops at "error sending request"; why it could not
/// be sent, such as a DNS or certificate failure, is further down the chain.
impl From<reqwest::Error> for SlackError {
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

impl From<serde_json::Error> for SlackError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<SlackError> for PluginError {
    fn from(error: SlackError) -> Self {
        let (category, status) = match &error {
            SlackError::Unconfigured => ("unconfigured", None),
            SlackError::Auth(_) => ("auth", None),
            SlackError::MissingScope(_) => ("forbidden", Some(403)),
            SlackError::RateLimited { .. } => ("rate-limited", Some(429)),
            SlackError::Api(_) => ("slack", None),
            SlackError::Http { status, .. } => ("http", Some(*status)),
            SlackError::BadArg(_) => ("bad-params", None),
            SlackError::NotFound(_) => ("not-found", Some(404)),
            SlackError::Keychain(_) => ("keychain", None),
            SlackError::Transport(_) => ("slack", None),
            SlackError::Response(_) => ("response", None),
        };
        let plugin_error = PluginError::new(category, error.to_string());
        match status {
            Some(status) => plugin_error.with_status(status),
            None => plugin_error,
        }
    }
}

pub type SlackResult<T> = Result<T, SlackError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slacks_codes_read_as_sentences() {
        assert_eq!(
            in_words("not_in_channel"),
            "this account is not in that channel"
        );
        assert_eq!(in_words("invalid_cursor"), "invalid cursor");
        assert!(SlackError::MissingScope("chat:write".into())
            .to_string()
            .contains("add the chat:write scope"));
    }
}
