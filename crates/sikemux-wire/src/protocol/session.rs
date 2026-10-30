//! The terminals and tasks the core runs.

use crate::pty::shell_protocol::SshLink;
use crate::pty::task::TaskSource;
use serde::{Deserialize, Serialize};

use super::{AgentGroup, AgentState, AgentStatus, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Terminal,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub kind: SessionKind,
    pub pid: Option<u32>,
    pub running: bool,
    pub cols: u16,
    pub rows: u16,
    pub attached: usize,
    /// The workspace project it runs in, by id; none for a terminal outside
    /// every project.
    pub project: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    /// What the core calls an agent terminal's agent, such as Otter.
    #[serde(default)]
    pub agent_name: Option<String>,
    pub agent_type: Option<String>,
    pub task_execution_id: Option<String>,
    /// The last state published for an agent terminal.
    pub agent_state: Option<AgentState>,
    /// What a task session was started as, so a client that did not start it
    /// can take it over.
    pub task: Option<TaskSessionInfo>,
    /// Set once the process was reaped.
    pub exit: Option<SessionExit>,
    /// A client asked for the process to end.
    pub killed: bool,
    /// The paired device that started it. The app started the rest.
    pub started_by: Option<String>,
    /// The name the app gives an agent terminal.
    pub title: Option<String>,
    /// What the program in the terminal last set its title to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// The agent finished or asked for something while the person was not
    /// looking at it in the app.
    pub unread: bool,
    /// When the agent last started work, finished or asked for something, in
    /// Unix milliseconds.
    pub active_at: Option<u64>,
    /// For an agent terminal the device view lists, as for a chat.
    #[serde(default)]
    pub group: Option<AgentGroup>,
    #[serde(default)]
    pub status: Option<AgentStatus>,
    /// For an agent terminal, when the soonest trigger set on its agent sends it a prompt, in
    /// Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_trigger: Option<u64>,
    /// When the agent was last named with `@` or sent a message by another agent, in Unix
    /// milliseconds, so a list can greet it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub called_at: Option<u64>,
    #[serde(default)]
    pub attention_rank: Option<u32>,
    /// For an SSH terminal, where its connection last stood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshLink>,
    /// The SSH config host an SSH terminal connects to.
    #[serde(default)]
    pub ssh_alias: Option<String>,
    /// What the rail calls a plain shell outside every project, agent and
    /// SSH host, started with no command or name of its own: the lowest
    /// number no other running one has when it started.
    #[serde(default)]
    pub rail_name: Option<String>,
}

/// A task's launch request without its environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSessionInfo {
    pub execution_id: String,
    pub terminal_key: String,
    pub task_id: String,
    pub label: String,
    pub project: String,
    pub source: TaskSource,
    pub command: String,
    pub cwd: String,
    pub agent_id: Option<String>,
}

/// `code` is absent when the status could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExit {
    pub code: Option<u32>,
    pub signal: Option<String>,
}
