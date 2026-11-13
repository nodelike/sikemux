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
