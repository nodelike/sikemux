use serde::{Deserialize, Serialize};

/// Whether a new chat in the project at the folder `project` starts in a worktree of its own.
#[derive(Serialize, Deserialize)]
pub struct WorktreeDefaultParams {
    pub project: String,
    pub on: bool,
}
