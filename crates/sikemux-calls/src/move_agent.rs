use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveParams {
    pub agent_id: String,
    /// The project, by id.
    pub project: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveToFolder {
    pub agent_id: String,
    /// The folder, which becomes a project first when it is not one.
    pub path: PathBuf,
}
