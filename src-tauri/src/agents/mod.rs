pub(crate) mod accounts;
pub(crate) mod config;
pub(crate) mod executable;
pub(crate) mod models;
pub(crate) mod sessions;
pub(crate) mod status;
pub(crate) mod usage;
pub(crate) mod watch;

use serde::{Deserialize, Serialize};

pub(crate) use executable::resolve_agent_executable;
pub(crate) use sessions::context::session_transcript_path;
pub use sikemux_core::agent_catalog::AgentKind;
pub use watch::{note_streaming_session, watch_count};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSession {
    id: String,
    title: String,
    mtime: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(rename = "type")]
    kind: &'static str,
    label: &'static str,
    command: String,
    available: bool,
    error: Option<String>,
    warning: Option<String>,
    profile_id: Option<String>,
    config_path: Option<String>,
    #[serde(rename = "defaultModel")]
    default_model: Option<String>,
    #[serde(rename = "defaultEffort")]
    default_effort: Option<String>,
    status: status::AgentStatus,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileRequest {
    #[serde(rename = "type")]
    kind: AgentKind,
    profile_id: Option<String>,
    executable_path: Option<String>,
    config_path: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum AgentUsageResetAt {
    Unix(u64),
    Iso(String),
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageWindow {
    label: String,
    used_percent: f64,
    resets_at: Option<AgentUsageResetAt>,
    window_minutes: Option<u64>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    provider: &'static str,
    plan: Option<String>,
    windows: Vec<AgentUsageWindow>,
    unavailable_reason: Option<String>,
}
