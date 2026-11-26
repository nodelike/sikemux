use serde::{Deserialize, Serialize};
use sikemux_view::memory::{MemoryCheck, Store};

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
    /// Moves it into this store, keeping its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
    /// The person turned down the chief's offer to move it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_now: bool,
    /// `id` is the number a chat line kept: an entry renumbered since is found under its new one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
    /// The words the kept entry had when the line was written: nothing changes when the entry
    /// now under its number has others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    /// Pins or unpins the entry in Tier 1 briefs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Marks an earlier entry as superseded by this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u64>,
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
        #[serde(default)]
        store: Store,
        #[serde(default)]
        pinned: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<u64>,
    },
    Knowledge {
        text: String,
        about: Vec<String>,
        sensitive: bool,
        check: Option<MemoryCheck>,
        #[serde(default)]
        store: Store,
        #[serde(default)]
        pinned: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<u64>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAddParams {
    pub project: String,
    pub entry: NewMemory,
}
