use crate::params::given;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use serde_json::Value;
use sikemux_agents::launch::PermissionMode;
use sikemux_agents::sessions::said::FoundLine;
use sikemux_view::device::RecentInfo;
use sikemux_view::workspace::AgentBuddy;
use sikemux_view::workspace::PinnedChat;
use sikemux_view::workspace::SpaceInfo;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
pub struct RemoveProjectParams {
    pub path: PathBuf,
}

#[derive(Deserialize, Serialize)]
pub struct UpdateProjectParams {
    pub id: String,
    pub patch: Map<String, Value>,
}

impl UpdateProjectParams {
    /// Moves the project into `space`, or out of any.
    pub fn space(id: impl Into<String>, space: Option<String>) -> Self {
        let mut patch = Map::new();
        patch.insert("space".to_owned(), Value::from(space));
        Self {
            id: id.into(),
            patch,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MoveProjectParams {
    pub id: String,
    pub index: usize,
}

#[derive(Deserialize, Serialize)]
pub struct SpacesParams {
    pub spaces: Vec<SpaceInfo>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsParams {
    pub permission_mode: PermissionMode,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseAgentsParams {
    pub agent_ids: Vec<String>,
}

/// An agent's session a client takes up again, named by its agent and the project it ran in.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChoice {
    pub provider: String,
    pub session_id: String,
    pub project: PathBuf,
    /// The worktree of `project` it ran in, when it ran in one.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct PinChatParams {
    pub recent: String,
    pub pinned: bool,
}

#[derive(Deserialize, Serialize)]
pub struct AddProjectParams {
    pub path: PathBuf,
    pub name: Option<String>,
    pub id: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentParams {
    pub agent_id: String,
    pub patch: AgentPatch,
}

/// The fields of an agent a client may change; a null one is cleared. The
/// rest, such as its terminal or worktree, only the core sets.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub lead: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub resume_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub profile_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_provider: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub effort: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub buddy: Option<Option<AgentBuddy>>,
}

impl UpdateAgentParams {
    pub fn new(agent_id: impl Into<String>, patch: AgentPatch) -> Self {
        Self {
            agent_id: agent_id.into(),
            patch,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct PinnedParams {
    pub chats: Vec<PinnedChat>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentChatsParams {
    /// Every project's when absent.
    pub project: Option<String>,
    pub query: Option<String>,
    pub cursor: Option<sikemux_agents::sessions::recent::RecentCursor>,
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub struct RecentChats {
    pub chats: Vec<RecentInfo>,
    pub next: Option<sikemux_agents::sessions::recent::RecentCursor>,
    /// Where each chat found by its words said them, by the chat's id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub said: HashMap<String, FoundLine>,
}
