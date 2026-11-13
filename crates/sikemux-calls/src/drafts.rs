use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sikemux_view::chat::DraftExtra;

crate::calls! {
    "drafts" Drafts (Option<DraftsParams>) -> BTreeMap<String, Draft>;
    "setDraft" SetDraft (SetDraftParams) -> ();
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<DraftExtra>,
    /// Moves on with every change, so an app saving the draft says which one it last read.
    #[serde(default)]
    pub rev: u64,
}

/// The drafts to read: those under `keys`, else every one.
#[derive(Serialize, Deserialize, Default)]
pub struct DraftsParams {
    pub keys: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct SetDraftParams {
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<DraftExtra>,
    /// The draft's revision the app last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}
