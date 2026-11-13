use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UpdateAgents {
    /// The agents to update; none updates every one with a newer release out.
    #[serde(default)]
    pub kinds: Vec<String>,
}
