//! An agent as the roster lists it, for what reads the list without building it.

use sikemux_wire::protocol::AgentStatus;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    Chat,
    Asleep,
    Terminal,
    Stopped,
    /// Through the linked host it runs on.
    Remote,
}

#[derive(Clone)]
pub(crate) struct Listed {
    pub agent_id: String,
    pub name: Option<String>,
    pub project: String,
    pub cli: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub parent: Option<String>,
    pub active_at: Option<u64>,
    pub rank: u32,
    pub reach: Reach,
}
