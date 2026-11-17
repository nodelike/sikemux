use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestParams {
    pub since: Option<u64>,
    /// Only what is about this project, by its path.
    pub project: Option<String>,
}
