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
    /// The service could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
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
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Slack"),
            ),
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
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("slack: {error}");
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

impl From<serde_json::Error> for SlackError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<SlackError> for PluginError {
    fn from(error: SlackError) -> Self {
        let message = error.to_string();
        match error {
            SlackError::Unconfigured => PluginError::new("unconfigured", message),
            SlackError::Auth(_) => PluginError::new("auth", message),
            SlackError::MissingScope(_) => PluginError::forbidden(message),
            SlackError::RateLimited { .. } => {
                PluginError::new("rate-limited", message).with_status(429)
            }
            SlackError::Unreachable(kind) => PluginError::unreachable(kind, "Slack"),
            SlackError::Api(_) | SlackError::Transport(_) => PluginError::new("slack", message),
            SlackError::Http { status, .. } => PluginError::http(status, message),
            SlackError::BadArg(_) => PluginError::new("bad-params", message),
            SlackError::NotFound(_) => PluginError::new("not-found", message).with_status(404),
            SlackError::Keychain(_) => PluginError::new("keychain", message),
            SlackError::Response(_) => PluginError::new("response", message),
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
