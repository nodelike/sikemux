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
    /// What a person sent the agent.
    Prompt,
    TurnStarted,
    TurnCompleted,
    PermissionRequest,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: ChatEventKind,
    pub payload: Value,
}
