use serde::{Deserialize, Serialize};
use serde_json::Map;
use serde_json::Value;
use sikemux_agents::launch::PermissionMode;
use sikemux_view::workspace::SpaceInfo;
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
