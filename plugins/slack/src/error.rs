use sikemux_plugin_api::{Service, ServiceError};

pub struct Slack;

impl Service for Slack {
    const ID: &'static str = "slack";
    const NAME: &'static str = "Slack";
    const SIGNED_OUT: &'static str = "no workspace is signed in";
}

pub type SlackError = ServiceError<Slack>;
pub type SlackResult<T> = Result<T, SlackError>;

/// The token lacks a scope a call needs; Slack names it.
pub fn missing_scope(scope: &str) -> SlackError {
    SlackError::Forbidden(format!(
        "the token cannot do this; add the {scope} scope to the Slack app and sign in again"
    ))
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
        assert!(missing_scope("chat:write")
            .to_string()
            .contains("add the chat:write scope"));
    }
}
