use crate::search::SearchParams;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct StreamParams {
    /// None stops every stream on the connection, as a reloaded page does.
    pub stream: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct SearchStreamParams {
    pub stream: u64,
    #[serde(flatten)]
    pub search: SearchParams,
}
