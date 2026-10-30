use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OutputQuery {
    pub cursor: u64,
    pub limit: usize,
    pub tail: Option<usize>,
    pub search: Option<String>,
    pub context: usize,
    pub plain: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub bytes: Vec<u8>,
    pub cursor: u64,
    pub end: u64,
    pub truncated: bool,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<usize>,
}
