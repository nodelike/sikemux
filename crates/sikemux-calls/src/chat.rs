use serde::{Deserialize, Serialize};
use sikemux_view::chat::RotationBrief;

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
