use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A recent chat, named as the workspace lists it.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentChat {
    pub recent: String,
    /// The folder it ran in, for a chat the core no longer lists.
    pub cwd: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRenamed {
    pub recent: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
}
