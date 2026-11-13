use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::SessionId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PortOwner {
    /// A terminal, in the app's pane `pane_id` when it has one.
    Terminal {
        session_id: SessionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    /// A task run, which `agent_id` started when an agent did.
    Task {
        session_id: SessionId,
        execution_id: String,
        task_id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// An agent, in its terminal `session_id` or in a chat.
    Agent {
        agent_id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<SessionId>,
    },
}

impl PortOwner {
    /// `agent_id` started it, by itself or in a task it ran.
    pub fn started_by(&self, agent_id: &str) -> bool {
        match self {
            PortOwner::Agent {
                agent_id: owner, ..
            } => owner == agent_id,
            PortOwner::Task {
                agent_id: owner, ..
            } => owner.as_deref() == Some(agent_id),
            PortOwner::Terminal { .. } => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    pub address: String,
    pub pid: u32,
    pub process: String,
    /// The project whose terminal, task or agent listens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub owner: PortOwner,
}

/// A port as a project shows it, with its preview first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPort {
    #[serde(flatten)]
    pub listening: ListeningPort,
    pub url: String,
    /// What `sikemux.json`'s preview points at.
    pub preview: bool,
    /// The checkout whose own port it is, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
}

/// What listens now, for a check before a task starts.
#[derive(Serialize, Deserialize)]
pub struct PortsParams {
    pub project: Option<String>,
}
