//! The app's projects and spaces, and what the core knows of each.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::WorkspaceAgent;

/// A project on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub pinned: bool,
    /// The space it is shown in, by id; none shows it in every space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// What the person picked by hand for it on each code host, by the
    /// host's plugin id, over what its remote and the host's accounts say.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub code_hosts: BTreeMap<String, CodeHostChoice>,
    /// What the project is known by on every host (ADR 0017): its remote,
    /// normalised, else its repository's first commit. A folder outside git
    /// has none. The core keeps it; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Where its repository stands, while the folder is one. The core fills
    /// it in; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<ProjectGit>,
    /// The launcher a new agent here starts with: that of the agent last
    /// started here, else one of the kind of agent last started anywhere, else
    /// the first offered. The core fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    /// A new chat here starts in a worktree of its own, as the person chose for
    /// the project. The core fills it in from its preferences.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree_default: bool,
    /// Where its repository is on a code host, while its origin is on one.
    /// The core fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_host: Option<ProjectCodeHost>,
}

/// A project's repository on a code host, and what Sikemux shows of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCodeHost {
    /// The code host, such as `github`.
    pub host: String,
    /// The plugin that speaks for the host, such as `sikemux.github`.
    pub plugin: String,
    /// What people call the host, such as GitHub.
    pub label: String,
    /// `owner/name`.
    pub repo: String,
    /// What the host calls its CI, such as Actions or Pipelines.
    pub ci_name: String,
    /// The sections clients list for it: `pulls`, `actions`, `issues` and
    /// `releases`, in order.
    pub sections: Vec<String>,
    /// What the host lets a pull request do, which clients offer accordingly.
    #[serde(default)]
    pub pulls: PullAbilities,
    /// What the host's CI shows and lets a person do.
    #[serde(default)]
    pub ci: CiAbilities,
}

/// What a code host's CI shows of a run and lets a person do to it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiAbilities {
    /// A run keeps its earlier attempts.
    pub attempts: bool,
    /// A run can wait on someone approving a deployment.
    pub approvals: bool,
    /// A workflow can be started by hand.
    pub dispatch: bool,
    pub annotations: bool,
    /// Jobs write summaries of their own.
    pub summaries: bool,
    pub artifacts: bool,
    /// It says how many minutes a run was billed.
    pub billing: bool,
    pub workflow_file: bool,
    pub rerun_failed: bool,
    pub rerun_job: bool,
    /// A job can run again with its runner's debug logging on.
    pub debug_logs: bool,
    pub delete_runs: bool,
    /// A run's logs can go while the run stays.
    pub delete_logs: bool,
}

/// What a code host lets a pull request do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullAbilities {
    /// The ways it merges, `squash`, `merge` or `rebase`, in the order offered.
    pub merge_methods: Vec<String>,
    /// A review can ask for changes.
    pub request_changes: bool,
    /// A closed one can be opened again.
    pub reopen: bool,
    /// It says whether one can merge cleanly.
    pub mergeability: bool,
}

/// The open pull request a project's branch is the head of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPull {
    pub number: u64,
    pub title: String,
    pub draft: bool,
    pub url: String,
    /// How the checks on its latest commit stand; none before any ran.
    pub checks: Option<PullChecks>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullChecks {
    Pending,
    Passing,
    Failing,
}

/// A project's branch as its chip shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGit {
    pub branch: String,
    /// Commits it has that its upstream lacks, and the other way round.
    pub ahead: u32,
    pub behind: u32,
    /// Something in it changed since its last commit.
    pub dirty: bool,
    /// The open pull request the branch is the head of, as its code host
    /// last said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<ProjectPull>,
    /// The branch has no open pull request and is not one others merge into,
    /// so clients offer to open one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_pull: bool,
}

/// A repository or account picked for a project on one code host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeHostChoice {
    /// `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// A set of projects the person shows on their own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    /// Empty when the person removed it.
    #[serde(default)]
    pub icon: String,
}

/// A saved chat kept at the top of the list, open or closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedChat {
    pub provider: String,
    /// The provider's own id for the conversation, which resumes it.
    pub session_id: String,
    pub title: String,
    /// The folder it ran in.
    pub project: PathBuf,
}

/// What a device learns about a launcher: never its program or environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherInfo {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub permission_mode: String,
    /// The `configOptions` the provider's last session offered, such as its
    /// models and effort levels, or null before one has started.
    pub config_options: Value,
    /// Whether the agent is ready, signed out, missing or broken; absent when the app has not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The models in `config_options`, as a picker shows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ConfigPicker>,
    /// How hard the model thinks, as a picker shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ConfigPicker>,
}

/// One of a session's select options, read for a picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPicker {
    /// The config id to set it with.
    pub id: String,
    pub current: String,
    pub choices: Vec<ConfigChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChoice {
    pub value: String,
    pub label: String,
    /// What the agent says about it, shown under its label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The choice the agent takes when none is made, which a picker marks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub projects: Vec<ProjectInfo>,
    #[serde(default)]
    pub spaces: Vec<SpaceInfo>,
    #[serde(default)]
    pub agents: Vec<WorkspaceAgent>,
    #[serde(default)]
    pub pinned: Vec<PinnedChat>,
    /// The mode agents that can bypass permissions start in.
    #[serde(default)]
    pub permission_mode: String,
    /// The plugins the person switched off, by id.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    pub launchers: Vec<LauncherInfo>,
    /// Agents that run only in their own terminal, such as Pi, which the app on a host starts
    /// there; phones start only chats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminal_launchers: Vec<LauncherInfo>,
    /// DESIGN's colour tokens for the person's theme, by name.
    pub palette: BTreeMap<String, String>,
    pub backdrop: Backdrop,
}

/// What the app draws behind its panes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backdrop {
    /// The dithered grain that moves behind each pane.
    pub texture: bool,
    /// Names the picture shown in place of the grain; `BackdropImage` fetches it.
    pub image: Option<String>,
}

/// A picture shown behind the panes, shrunk to suit a phone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackdropImage {
    /// Changes whenever the picture does, so a device fetches it only then.
    pub id: String,
    pub data_url: String,
}
