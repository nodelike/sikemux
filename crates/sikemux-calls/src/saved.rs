use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchParams {
    pub query: String,
    #[serde(default)]
    pub projects: Option<Vec<PathBuf>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub budget_ms: Option<u64>,
}
