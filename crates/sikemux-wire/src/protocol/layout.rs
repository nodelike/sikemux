//! How the app lays out tabs and panes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The tabs every desktop client draws, kept by the core.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    /// The project in front, by id.
    #[serde(default)]
    pub active_project: Option<String>,
    /// Each project's tabs, by project id.
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectLayout>,
    /// The SSH host in front instead of a project, by its alias.
    #[serde(default)]
    pub active_host: Option<String>,
    /// The terminal tabs open to each SSH host, by its alias. A host with
    /// none left has no strip.
    #[serde(default)]
    pub hosts: BTreeMap<String, ProjectLayout>,
    /// The tabs of terminals that belong to no project.
    #[serde(default)]
    pub terminals: ProjectLayout,
    /// Whether the terminals' strip is in front instead of a project or host.
    #[serde(default)]
    pub terminals_in_front: bool,
    /// The account's other host the person is on, by its key, whose own projects and tabs
    /// are shown instead of this host's; none while they are on this host or an SSH host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_host: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLayout {
    /// The tab in front, by id.
    #[serde(default)]
    pub active: Option<String>,
    /// In the order the tab strip shows them.
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    /// What the person named it; clients name an unnamed tab from its pane.
    #[serde(default)]
    pub title: Option<String>,
    pub pane: Pane,
    /// Opened to look at, by a single click: the next such tab takes its
    /// place, until the person keeps it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
}

/// What a tab shows: one view, or several side by side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Pane {
    View {
        id: String,
        kind: PaneKind,
        /// What it shows: an agent's id, a session's id, a path, a URL or a
        /// plugin's id, by `kind`.
        target: String,
    },
    Split {
        id: String,
        dir: SplitDir,
        panes: Vec<Pane>,
        /// Each pane's share of the split, in thousandths.
        sizes: Vec<u16>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaneKind {
    Agent,
    Terminal,
    Files,
    Git,
    Search,
    Board,
    Browser,
    Plugin,
    /// An agent's pages, files, task terminals and simulator; `target` is the agent.
    Desk,
    /// A project's own settings; `target` is its folder.
    ProjectSettings,
    /// What setting Sikemux up adds on an SSH host, and the one action that does it; `target`
    /// is the host's alias.
    InstallSikemux,
    /// What the account keeps in its Stash; `target` is `stash`.
    Stash,
    /// A kind a newer core shows, which this build does not know and leaves out.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitDir {
    Row,
    Column,
    /// In the same place, one in front and the rest reached by a strip of tabs over them.
    Stack,
}
