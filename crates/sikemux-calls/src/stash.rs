use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub struct StashItemParams {
    pub id: String,
}

#[derive(Deserialize, Serialize)]
pub struct SeeParams {
    pub at: String,
}

#[derive(Serialize, Deserialize)]
pub struct StashKept {
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachStashParams {
    pub id: String,
    pub agent_id: String,
    /// False when the app keeps its own draft: the item's file is only kept here, and the app
    /// adds it to its message and marks the item used once it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachedStash {
    pub agent_id: String,
    /// Where the item's file was kept on this host, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Where each of the item's files was kept: its own, or a prompt's.
    pub paths: Vec<PathBuf>,
}

impl AttachedStash {
    pub fn new(agent_id: String, paths: Vec<PathBuf>) -> Self {
        Self {
            agent_id,
            path: paths.first().cloned(),
            paths,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashPromptParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct StashedPrompt {
    pub id: String,
}
