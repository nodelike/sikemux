use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestMessageParams {
    pub agent_id: String,
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct StopShareParams {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub id: String,
    pub agent_id: String,
    pub url: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    pub viewers: u32,
    pub state: ShareState,
    /// Why the server would not open it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareState {
    /// Waiting for the accounts server, which is out of reach or has not
    /// answered yet.
    Connecting,
    Open,
    Refused,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareChatParams {
    pub agent_id: String,
    pub lifetime_ms: u64,
    pub title: Option<String>,
}
