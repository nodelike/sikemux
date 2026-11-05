//! What a client shows of a host, whole.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_wire::protocol::{Attention, ChatInfo, SessionInfo, Workspace};

use crate::layout::Layout;

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
    /// Which project is in front, and each project's tabs.
    pub layout: Box<Layout>,
    /// Who this host is signed in to Sikemux as; none while nobody is.
    pub account: Option<Box<AccountInfo>>,
    pub face_style: Box<FaceStyle>,
    /// How busy this host is, while a client that shows it is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<HostLoad>,
    /// How this host keeps its core current, as the core's update status, on
    /// a host that updates itself. Kept as JSON so a change to its shape
    /// reaches phones over the air.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<Value>,
}

/// How busy a host is, for the rows clients show it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLoad {
    /// The last minute's load as a share of the host's cores, in percent.
    pub cpu: u32,
    /// Memory in use, in bytes, where the host says.
    pub memory_used: Option<u64>,
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
}
