use serde::{Deserialize, Serialize};
use sikemux_view::memory::MemoryCheck;

/// What changes about an entry. A rule is only corrected or retired;
/// knowledge may also be said not to hold, and have what it is about and how
/// it is tested changed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUpdateParams {
    pub project: String,
    pub id: u64,
    /// What is so instead, correcting it.
    pub text: Option<String>,
    /// Why knowledge no longer holds, when what does is not known.
    pub wrong: Option<String>,
    #[serde(default)]
    pub retire: bool,
    pub about: Option<Vec<String>>,
    /// How the core tests knowledge from now on.
    pub check: Option<MemoryCheck>,
    /// The core stops testing knowledge.
    #[serde(default)]
    pub drop_check: bool,
    /// The goal card whose own rule `id` is.
    pub goal: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntryParams {
    pub project: String,
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepSeededParams {
    pub project: String,
    /// The offered rules to keep; the rest are dropped.
    pub keep: Vec<u64>,
}

/// A rule or knowledge to keep, as the person writes it.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NewMemory {
    Rule {
        text: String,
        /// The goal card it is kept on instead, for a rule only that goal's
        /// agents follow.
        goal: Option<u64>,
        sensitive: bool,
        /// A folder above the project, to keep it for every project under it.
        folder: Option<String>,
    },
    Knowledge {
        text: String,
        about: Vec<String>,
        sensitive: bool,
        check: Option<MemoryCheck>,
        /// A folder above the project, to keep it for every project under it.
        folder: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAddParams {
    pub project: String,
    pub entry: NewMemory,
}
