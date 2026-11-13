use serde::{Deserialize, Serialize};
use sikemux_corekit::accounts::protocol::StashItem;
use sikemux_corekit::accounts::protocol::StashLink;
use sikemux_corekit::accounts::protocol::StashTaken;
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

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    #[serde(default)]
    pub taken: Option<StashTaken>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The stash as the apps draw it: none while this host is signed out.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownStash {
    pub signed_in: bool,
    pub items: Vec<ShownStashItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub seen_at: Option<String>,
}

/// An item as the server keeps it, with what apps draw a text or prompt by: its `words`, its
/// text with the links to pages taken out, and the state of each pull request or issue it
/// links to.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownStashItem {
    #[serde(flatten)]
    pub item: StashItem,
    /// What it is called: its page's title, its text's first line, or its file's name.
    pub title: String,
    /// It was stashed since the person last looked, on any device.
    pub fresh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<ShownStashLink>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ShownStashLink {
    #[serde(flatten)]
    pub link: StashLink,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}
