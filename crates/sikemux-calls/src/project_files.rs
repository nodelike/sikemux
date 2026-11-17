use serde::{Deserialize, Serialize};

/// The answer to `projectFiles`: the whole list, only what changed since the
/// client's scan, or neither when nothing did.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFiles {
    pub scan_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilesParams {
    pub project: String,
    #[serde(default)]
    pub since: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntriesParams {
    pub project: String,
    #[serde(default)]
    pub query: String,
    #[serde(default = "entries_shown")]
    pub limit: usize,
}

pub fn entries_shown() -> usize {
    8
}
