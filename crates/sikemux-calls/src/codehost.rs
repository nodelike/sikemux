use serde::{Deserialize, Serialize};
use sikemux_plugin_api::codehost::TrackedKind;

#[derive(Serialize, Deserialize)]
pub struct PullsParams {
    pub project: String,
    pub state: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CodeHostProjectParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
pub struct TrackedContextParams {
    pub project: String,
    pub kind: TrackedKind,
    pub number: u64,
}
