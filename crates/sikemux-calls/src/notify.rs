use serde::{Deserialize, Serialize};

/// The push token of the Live Activity the calling phone shows this host's agents in; none once it ended.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityParams {
    pub token: Option<String>,
}
