use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeParams {
    pub agent_ids: Vec<String>,
    /// When it wakes, in Unix milliseconds; none wakes it now, unless `typed` names a time.
    #[serde(default)]
    pub until: Option<u64>,
    /// A time in words this host reads on its own clock, such as "fri 2pm" or "in 90m".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleParams {
    pub agent_ids: Vec<String>,
    pub settled: bool,
}
