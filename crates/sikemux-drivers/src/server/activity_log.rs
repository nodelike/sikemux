//! What a driver counts of a session's use of the model.

use serde::{Deserialize, Serialize};

/// A context this large, read mostly without the cache, is a cold call.
const COLD_CONTEXT: u64 = 100_000;

/// One call to the model, with how much context it carried and how much of that the cache held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCall {
    pub at_ms: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl ModelCall {
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    pub fn cold(&self) -> bool {
        self.context() > COLD_CONTEXT && self.cache_read * 2 < self.context()
    }
}

/// The session was compacted: by its agent as it filled, by the person, or by the core while idle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compaction {
    pub at_ms: u64,
    pub trigger: String,
    pub before: u64,
    pub after: u64,
    pub ms: u64,
    /// What writing the summary cost, when nothing else called the model beside it.
    pub cost: Option<ModelCall>,
}
