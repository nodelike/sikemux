//! Chat agents: how one is started, where it stands, and what it waits on.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AgentGroup, AgentStatus};

/// Everything the core needs to start a chat agent, resolved by the app: the
/// program and its environment, and the tool servers the agent is told of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLaunch {
    pub agent_id: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub mcp_servers: Vec<Value>,
    /// The provider's session to load instead of opening a new one.
    pub resume_id: Option<String>,
    pub permission_mode: PermissionMode,
    pub model: Option<String>,
    /// The hosted model provider its turns are paid with, in place of the agent's own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    /// The account the agent signs in as, when the app names one.
    #[serde(default)]
    pub account: Option<ChatAccount>,
    /// Accounts the chat moves to, in order, when its own runs out of usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ChatAccount>,
}

/// Whether a chat agent asks before it acts outside its workspace, or never asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    WorkspaceWrite,
    Bypass,
}

impl PermissionMode {
    pub fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "workspace-write" => Ok(Self::WorkspaceWrite),
            "bypass" => Ok(Self::Bypass),
            _ => Err(format!("Unsupported permission mode: {mode}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::Bypass => "bypass",
        }
    }
}

/// One of the person's accounts with a provider, as the variables that point
/// the agent at it. They replace the provider's account variables in the
/// launch's environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccount {
    pub id: String,
    pub label: String,
    pub env: BTreeMap<String, String>,
}

/// What a device learns of a recent chat: never the launcher behind it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentInfo {
    /// Names it in `ResumeChat`.
    pub id: String,
    pub provider: String,
    pub title: String,
    /// The app's project it ran in.
    pub project: String,
    pub cwd: PathBuf,
    /// Unix milliseconds.
    pub active_at: u64,
    /// The agent in the workspace that holds the chat, open or asleep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The person pinned it, so it shows above the rest however long ago it ran.
    #[serde(default)]
    pub pinned: bool,
}

/// Something read elsewhere and handed to the agent whole, such as an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatContext {
    pub uri: String,
    pub title: String,
    pub text: String,
}

/// The app's `acp_start` answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStart {
    pub session_id: String,
    pub capabilities: Value,
    pub setup: Value,
}

/// The last of a chat's changes a client heard. `feed` changes whenever the
/// chat's agent starts again, which starts `seq` over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMark {
    pub feed: String,
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatAttachment {
    /// The chat as it stands, a [`crate::chat::ChatView`] as JSON. Changes
    /// follow, numbered from `mark.seq + 1`.
    Live {
        view: Value,
        mark: ChatMark,
    },
    /// What changed since the mark the client attached with, as a list of
    /// [`crate::chat::ChatChange`] JSON. Changes follow `mark`.
    Resumed {
        changes: Value,
        mark: ChatMark,
    },
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatState {
    Starting,
    Ready,
    /// The app stopped its agent while idle; `AcpWake` starts it again.
    Asleep,
    /// The app has the chat open but its agent is not running, as it ended or failed to start.
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub agent_id: String,
    /// What the core calls the agent, such as Otter.
    #[serde(default)]
    pub name: Option<String>,
    pub provider: String,
    /// What the agent named the session, once it has.
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub state: ChatState,
    pub running: bool,
    pub pending_permissions: Vec<String>,
    /// Subagents still running for it.
    #[serde(default)]
    pub subagents: u32,
    /// Background shells and monitors still running for it.
    #[serde(default)]
    pub tasks: u32,
    /// The agent is retrying a request that got no answer over the network:
    /// the attempt it is on, or 0 when it did not say.
    #[serde(default)]
    pub reconnecting: Option<u32>,
    /// The paired device that started it. The app started the rest.
    pub started_by: Option<String>,
    /// The app's launcher a device started it with.
    pub launcher: Option<String>,
    pub permission_mode: PermissionMode,
    pub model: Option<String>,
    /// The provider its turns are paid with, in place of the agent's own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    /// It finished a turn or asked for something while the person was not
    /// looking at it in the app.
    pub unread: bool,
    /// When it last opened, started work, finished or asked for something,
    /// in Unix milliseconds. Unknown for chats from before the core started.
    pub active_at: Option<u64>,
    #[serde(default)]
    pub group: AgentGroup,
    #[serde(default)]
    pub status: AgentStatus,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_trigger: Option<u64>,
    /// When the agent was last named with `@` or sent a message by another agent, in Unix
    /// milliseconds, so a list can greet it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub called_at: Option<u64>,
    /// Its place among the open agents, chats and agent terminals together,
    /// in the order they want the person: 0 first.
    #[serde(default)]
    pub attention_rank: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionKind {
    /// Answer with `AcpPermissionReply`, naming `id` and one of the options
    /// in `request`.
    Permission,
}

/// Something an agent is waiting on a person for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub id: String,
    pub kind: AttentionKind,
    pub agent_id: String,
    pub provider: String,
    pub cwd: PathBuf,
    /// The agent's own request, with what it wants to do and the options.
    pub request: Value,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    #[serde(default)]
    pub choices: PermissionChoices,
}

/// What a permission request comes down to for a client that offers only
/// allow, always allow and deny: the option each answers with, and what the
/// agent wants to do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionChoices {
    pub allow_once: Option<String>,
    /// The allow that lasts longest, beyond the session when one does.
    pub allow_always: Option<String>,
    pub reject: Option<String>,
    /// The shell command it wants to run, when it is one.
    pub command: Option<String>,
    /// What its tool call is called.
    pub title: Option<String>,
}
