//! What a shared chat's host and its viewers say to each other, sealed, through the
//! accounts server.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat::ToolDetail;
use crate::protocol::ChatAttachment;

/// What a share sends its viewers, a frame at a time: the chat as it stands, then each
/// change to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShareFrame {
    /// Starts a viewer over.
    Snapshot {
        chat: SharedChat,
        attachment: ChatAttachment,
    },
    /// The chat's changes, as the core sends them to its own clients.
    Event { event: SharedEvent },
}

/// The one event of the core's that a share passes on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SharedEvent {
    /// A list of [`crate::chat::ChatChange`] as JSON, numbered by `seq`.
    Chat {
        agent_id: String,
        seq: u64,
        changes: Value,
    },
}

/// The shared chat, as a viewer names it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedChat {
    pub agent_id: String,
    pub provider: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The home folder on the host, so a viewer can shorten its paths.
    pub home: Option<PathBuf>,
}

/// What a viewer asks the host, numbered by `id` so its answer finds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShareAsk {
    /// The turns before `before` in the chat run `feed`.
    Page {
        id: u64,
        feed: String,
        before: u64,
    },
    ToolDetail {
        id: u64,
        tool_call_id: String,
    },
    /// A picture pasted into the chat.
    File {
        id: u64,
        path: PathBuf,
    },
}

impl ShareAsk {
    pub fn id(&self) -> u64 {
        match self {
            Self::Page { id, .. } | Self::ToolDetail { id, .. } | Self::File { id, .. } => *id,
        }
    }
}

/// The host's answer to the question numbered `id`: one of the rest, or `error`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareReply {
    pub id: u64,
    /// A [`crate::chat::ChatPage`] as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_detail: Option<ToolDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<SharedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedFile {
    /// The file's bytes in base64.
    pub data: String,
}
