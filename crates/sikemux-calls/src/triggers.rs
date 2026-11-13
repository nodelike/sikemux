use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerParams {
    pub trigger: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldTriggerParams {
    pub trigger: u64,
    /// True pauses it, false resumes it.
    pub paused: bool,
}
