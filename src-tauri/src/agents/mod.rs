pub(crate) mod accounts;
pub(crate) mod config;
pub(crate) mod executable;
pub(crate) mod sessions;
pub(crate) mod watch;

use serde::{Deserialize, Serialize};
use sikemux_core::agent_accounts::status::AgentStatus;

pub(crate) use config::global_skill_folders;
pub use sikemux_core::agent_catalog::AgentKind;
pub(crate) use sikemux_core::sessions::context::session_transcript_path;
pub use watch::{note_streaming_session, watch_count};

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
    status: AgentStatus,
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
