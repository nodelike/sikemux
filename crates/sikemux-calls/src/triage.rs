use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageParams {
    pub project: String,
    /// What the person typed so far.
    pub text: String,
}
