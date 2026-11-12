//! What a client shows of a host, whole.

use std::path::PathBuf;

use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::{Attention, BuildChannel, ChatInfo, DeviceAccess, SessionInfo};

use crate::layout::Layout;
use crate::updates::UpdateStatus;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub workspace: Workspace,
    pub sessions: Vec<SessionInfo>,
    pub chats: Vec<ChatInfo>,
    pub attentions: Vec<Attention>,
    /// The newest saved chats in this host's projects, none of them open.
    pub recent: Vec<RecentInfo>,
    /// What waits on the person, ranked.
    pub inbox: crate::inbox::Inbox,
    /// The calls of every project still open, then those answered lately with what came of
    /// them, newest first. Clients show only the projects they scope to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<crate::calls::PersonCall>,
    /// Which project is in front, and each project's tabs.
    pub layout: Box<Layout>,
    /// Who this host is signed in to Sikemux as; none while nobody is.
    pub account: Option<Box<AccountInfo>>,
    pub face_style: Box<FaceStyle>,
    /// How busy this host is, while a client that shows it is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<HostLoad>,
    /// What about this host, or a linked host out of reach, needs the person now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<HostAlerted>,
    /// How this host keeps its core current, on a host that updates itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<UpdateStatus>,
}

/// How busy a host is, for the rows clients show it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLoad {
    /// How busy all its cores were lately, in percent.
    pub cpu: u32,
    /// Memory in use, in bytes, where the host says.
    pub memory_used: Option<u64>,
    /// More threads want to run than its cores take, so agents' simulators and builds wait.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overloaded: bool,
    /// The one-minute load average, rounded, while it is overloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average: Option<u32>,
    #[serde(default)]
    pub cores: u32,
    /// The builds agents run or wait to run, oldest first, while it is overloaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<AgentBuild>,
}

/// A build an agent started, such as `xcodebuild` or `cargo build`, running or waiting its turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBuild {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub command: String,
    /// When it started, or began to wait, in Unix milliseconds.
    pub since: u64,
    /// How many builds wait ahead of it, while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
}

/// An alert about this host, or about the linked host `host` names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAlerted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub alert: crate::host_stats::HostAlert,
}

/// The Sikemux account this host is signed in to, as clients show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub name: Option<String>,
    pub email: Option<String>,
    /// Names the picture `accountPicture` answers, which changes with it.
    pub picture_hash: Option<String>,
}

/// How agents' faces look, as the person picked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceStyle {
    /// Every agent wears a face; otherwise only one with a name does.
    pub on: bool,
    /// A pack's id, or `all` for a pack per agent.
    pub pack: String,
    /// A shape's id, or `mixed` for a shape per agent.
    pub shape: String,
    /// Faces move with what their agents are doing.
    pub motion: bool,
}

impl Default for FaceStyle {
    fn default() -> Self {
        Self {
            on: false,
            pack: "all".into(),
            shape: "mixed".into(),
            motion: true,
        }
    }
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
    /// Its triggers, paused since it closed, which go on when it is taken up again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<crate::chat::ChatTrigger>,
}

/// The computer a core runs on, as a person would recognise it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// The name set in System Settings, such as "Kishore's MacBook Pro".
    pub name: String,
    /// The kind of computer, such as "MacBook Pro" or "Mac mini".
    pub model: String,
    /// The Sikemux release running there, such as "0.4.3-nightly.5".
    pub version: String,
    pub channel: BuildChannel,
    /// The home folder of the person the core runs as, so a client can shorten its paths.
    pub home: PathBuf,
    /// What the paired device that asked may do here; none for a program on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<DeviceAccess>,
}
