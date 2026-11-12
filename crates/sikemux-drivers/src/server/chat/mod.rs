pub mod cap;
pub mod feed;
pub mod find;
pub mod history;
pub mod native;
pub mod permission;
pub mod prompt;
pub mod reset;
pub mod rotation;
pub mod states;

use serde_json::Value;
use sikemux_fold::update::notice;

/// Tells the agent the option the person chose, or none when the question
/// was cancelled. It writes to the agent before it returns, so an answer
/// given before the chat lets go of its agent is never lost.
pub type Respond = Box<dyn FnOnce(Option<String>) + Send>;

/// A call to the person that a prompt answered.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Answered {
    Call,
    /// One whose words are never kept: they may be a password or a token.
    HiddenCall,
}

pub fn restart_notice(title: &str, description: &str) -> Value {
    notice("warning", title, Some(description))
}
