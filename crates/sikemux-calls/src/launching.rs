use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct SmartPickParams {
    pub launcher: String,
    pub project: String,
}

/// Takes up an agent's session again in the project it ran in, recent or not, and
/// answers the agent that holds it once its session is ready.
/// The agent a saved session came back as, and the project it is in.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTaken {
    pub agent_id: String,
    pub project: String,
}

/// A saved agent to start; one moved to another agent before it ran waits for the person.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    pub agent_id: String,
    #[serde(default)]
    pub waits_for_person: bool,
    /// Drops a chat still waiting for the person first, as it would otherwise keep the launch
    /// it was made with and not what its record says now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub again: bool,
}
