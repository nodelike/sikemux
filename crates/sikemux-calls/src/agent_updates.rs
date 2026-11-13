use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UpdateAgents {
    /// The agents to update; none updates every one with a newer release out.
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// How an update the person asked for is going. One that finished is no longer listed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Updating {
    /// Waiting for another agent's update to finish.
    Queued,
    /// The updater runs; `said` is the last line it printed.
    Running {
        said: Option<String>,
    },
    Failed {
        reason: String,
    },
}
