use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FindDirection {
    #[default]
    Older,
    Newer,
}

/// Where a find goes on from: a place in a row's text, and how many it had counted to there.
/// An empty `part` stands before or after every part of its row, as the direction goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindAt {
    pub event: u64,
    #[serde(default)]
    pub part: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub counted: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindParams {
    pub agent_id: String,
    pub feed: String,
    pub query: String,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub from: Option<FindAt>,
    #[serde(default)]
    pub direction: FindDirection,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub count_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindHit {
    pub row: String,
    pub from_event: u64,
    pub part: String,
    /// UTF-16 offsets in the part's searchable text.
    pub range: [usize; 2],
    pub preview: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindScanned {
    pub from_event: Option<u64>,
    pub to_event: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindCounted {
    pub matches: u64,
    pub complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindAnswer {
    pub hits: Vec<FindHit>,
    /// Where to go on from; none once the chat's start, or end, is reached.
    pub next: Option<FindAt>,
    pub scanned: FindScanned,
    pub counted: FindCounted,
}
