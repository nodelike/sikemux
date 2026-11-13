use serde::{Deserialize, Serialize};
use sikemux_view::workspace::AgentBuddy;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentName {
    pub agent_id: String,
    pub name: String,
    pub project: String,
    pub parent: Option<String>,
}

/// A closed agent the person named, which can be opened again as itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgent {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub session_id: String,
    pub title: Option<String>,
    pub buddy: Option<AgentBuddy>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameAgentParams {
    pub agent_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgentsParams {
    pub cwd: PathBuf,
}
