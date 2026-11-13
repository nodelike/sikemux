use serde::{Deserialize, Serialize};
use sikemux_view::chat::RotationBrief;
use sikemux_wire::protocol::ChatContext;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepHistoryParams {
    pub agent_id: String,
    /// Off, the chat is summarised before the next message.
    pub keep: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandOffParams {
    pub agent_id: String,
    /// The agent taking the chat over, such as `codex`.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// A new chat takes it over, and this one stays as it is.
    #[serde(default)]
    pub fork: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHandedOff {
    /// The chat that carries on: the same one, or the fork.
    pub agent_id: String,
    pub project: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropHandoffParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationBriefParams {
    pub agent_id: String,
    /// When the fresh session started, as its line says.
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationBriefRead {
    pub brief: RotationBrief,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideChatParams {
    /// The chat to open one beside.
    pub agent_id: String,
    /// The first message, sent at once when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideChat {
    pub agent_id: String,
}

/// A page of a chat's rows: those before or after an event, or around the row holding one.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPageParams {
    pub agent_id: String,
    pub feed: String,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub around: Option<u64>,
    /// A subagent's rows rather than the chat's own.
    pub session: Option<String>,
    pub turns: Option<u32>,
}

impl ChatPageParams {
    /// The page of `feed` nothing narrows yet.
    pub fn new(agent_id: &str, feed: String) -> Self {
        Self {
            agent_id: agent_id.to_owned(),
            feed,
            before: None,
            after: None,
            around: None,
            session: None,
            turns: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailParams {
    pub agent_id: String,
    pub tool_call_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLeftOutParams {
    pub agent_id: String,
    pub message_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditParams {
    pub agent_id: String,
    pub message_id: String,
    pub text: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptChatParams {
    pub agent_id: String,
    #[serde(default)]
    pub message_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub context: Vec<ChatContext>,
}

impl PromptChatParams {
    /// Plain words for `agent_id`, with nothing attached.
    pub fn new(agent_id: String, text: String) -> Self {
        Self {
            agent_id,
            message_id: None,
            text,
            paths: Vec::new(),
            context: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedParams {
    pub agent_id: String,
    pub ids: Vec<String>,
    /// A message sent with them, after them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<QueuedMessageParams>,
}

/// A message as the person wrote it.
#[derive(Serialize, Deserialize)]
pub struct QueuedMessageParams {
    pub text: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub context: Vec<ChatContext>,
}

/// The most files one message carries.
pub const MAX_ATTACHMENTS: usize = 32;
