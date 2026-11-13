use serde::{Deserialize, Serialize};

/// The chat a goal's button acts on.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGoalParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalReportsParams {
    pub project: String,
    pub id: u64,
}
