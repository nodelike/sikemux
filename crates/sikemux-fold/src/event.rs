//! What a chat agent said, as the core's feed keeps it and writes it to the
//! chat's history. Clients never see these: they get the rows folded from
//! them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatEventKind {
    Status,
    Ready,
    SessionUpdate,
    Prompt,
    TurnStarted,
    TurnCompleted,
    PermissionRequest,
    Error,
    /// What the claims of an answer rest on, as the core found it once the turn that asked `why`
    /// ended: `from` the event the call was made at, `until` the turn's end, `checked` the links
    /// it checked, and `chips` what each that holds names.
    Sourced,
    /// A GIF the agent asked for in its reply: first `sessionId` and `ask` where it was asked,
    /// then `slot`, that event's index, with the `gif` found, or none.
    Gif,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: ChatEventKind,
    pub payload: Value,
}
