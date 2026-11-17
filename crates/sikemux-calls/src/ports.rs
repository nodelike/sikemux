use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::SessionId;
use std::path::PathBuf;

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

/// A port of a checkout, and what listens on it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutPort {
    pub name: String,
    pub port: u16,
    /// The program that listens, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
}

/// A checkout's name, ports and environment, as the person and agents are shown them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutAbout {
    pub name: String,
    pub path: PathBuf,
    /// The first of its own ten, none in the project's own folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_port: Option<u16>,
    pub ports: Vec<CheckoutPort>,
    /// Each name with the port the project's own folder keeps.
    pub main_ports: Vec<(String, u16)>,
    pub env: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct CheckoutParams {
    pub cwd: PathBuf,
}

impl CheckoutAbout {
    /// Its ports no name takes, as a first and last.
    pub fn unnamed(&self) -> Option<(u16, u16)> {
        let block = sikemux_corekit::tasks::checkout::MAX_PORT_NAMES as u16;
        let first = self.first_port?;
        let next = first + self.ports.len() as u16;
        (next < first + block).then_some((next, first + block - 1))
    }
}
