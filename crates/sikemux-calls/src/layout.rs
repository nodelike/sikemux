use serde::{Deserialize, Serialize};
use sikemux_corekit::cli::protocol::CliOpenTarget;
use sikemux_view::layout::At;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabParams {
    #[serde(flatten)]
    pub at: At,
    pub tab: String,
    /// Where `moveTab` puts it, counted in the strip as it is now.
    pub to: Option<usize>,
    /// The name `renameTab` gives it; an empty one gives back its own name.
    pub title: Option<String>,
}

impl TabParams {
    pub fn new(at: At, tab: impl Into<String>) -> Self {
        Self {
            at,
            tab: tab.into(),
            to: None,
            title: None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabsParams {
    #[serde(flatten)]
    pub at: At,
    pub tabs: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ActivateProjectParams {
    pub project: String,
}

#[derive(Deserialize, Serialize)]
pub struct HostParams {
    /// "this", "host:<key>" for another host on the account, or "ssh:<alias>".
    pub host: String,
}

#[derive(Deserialize, Serialize)]
pub struct OpenPaths {
    pub targets: Vec<CliOpenTarget>,
    /// Opens the files without bringing their projects forward.
    #[serde(default)]
    pub behind: bool,
}

/// How one target of [`OpenPaths`] went: its project and tab, or why it did not open.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathOpened {
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneParams {
    #[serde(flatten)]
    pub at: At,
    pub tab: String,
    pub pane: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeSplit {
    #[serde(flatten)]
    pub at: At,
    pub tab: String,
    pub split: String,
    /// Each pane's share, in any unit; the core keeps them in thousandths.
    pub sizes: Vec<u16>,
}
