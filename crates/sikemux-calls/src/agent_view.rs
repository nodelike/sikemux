use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AgentViewKind {
    Chat,
    Terminal,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentViewParams {
    pub agent_id: String,
    pub to: AgentViewKind,
}
