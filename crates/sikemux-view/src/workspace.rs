//! The app's projects, spaces and agents, and what the core knows of each.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use sikemux_wire::protocol::{PermissionMode, SessionId};

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
    /// What the project is known by on every host (ADR 0023): its remote,
    /// normalised, else its repository's first commit, then its folder within
    /// the repository when it is not the top. A folder outside git has none.
    /// The core keeps it; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The copies the account's other hosts hold of the same repository, as
    /// each last said. The core fills it in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<ProjectCopy>,
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

/// A project's copy on another host of the account (ADR 0023), as that host
/// last said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCopy {
    /// The host's key.
    pub host: String,
    /// What the person named the host.
    pub host_name: String,
    /// The project's id on that host.
    pub project: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<ProjectGit>,
    /// This host cannot reach that one now, so the copy is as it last said.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub away: bool,
}

/// A project only the account's other hosts hold, each with its copy there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectElsewhere {
    pub name: String,
    /// Its identity, as [`ProjectInfo::identity`]; none for a folder outside
    /// git, which has one copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    pub copies: Vec<ProjectCopy>,
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
    pub permission_mode: PermissionMode,
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
    /// The projects only the account's other hosts hold, after this host's
    /// own. The core fills it in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elsewhere: Vec<ProjectElsewhere>,
    #[serde(default)]
    pub spaces: Vec<SpaceInfo>,
    #[serde(default)]
    pub agents: Vec<WorkspaceAgent>,
    #[serde(default)]
    pub pinned: Vec<PinnedChat>,
    /// The mode agents that can bypass permissions start in.
    #[serde(default)]
    pub permission_mode: PermissionMode,
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

/// The checkout an agent was given, as its workspace record keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorktree {
    /// The main checkout, which git commands about the worktree run against.
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The branch the project was on when the worktree was cut, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub start_sha: String,
}

/// The character an agent wears: a pack and shape when the person picked one,
/// and a slot in the pack's palette, which the core gives out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBuddy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

/// An agent in the workspace, kept whether or not it runs now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgent {
    pub id: String,
    /// The CLI it runs, such as `claude`.
    pub provider: String,
    /// What the core calls it, such as Otter, open or asleep. The core fills
    /// it in; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its project, by id.
    pub project: String,
    pub cwd: PathBuf,
    pub title: String,
    /// The person named it, so the agent's own titles no longer replace it.
    #[serde(default)]
    pub renamed: bool,
    /// Its title is its own: the person named it, or it is neither empty, its kind nor its
    /// account's name. The core fills it in; clients only read it.
    #[serde(default)]
    pub titled: bool,
    /// The agent that started it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// The provider's session, which resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_id: Option<String>,
    pub permission_mode: PermissionMode,
    /// The person's account it runs on, by profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model provider it runs on in place of its maker's account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Never put to sleep while idle.
    #[serde(default)]
    pub keep_alive: bool,
    /// Stopped, and woken again on its saved conversation when opened.
    #[serde(default)]
    pub asleep: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktree>,
    /// The character drawn for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buddy: Option<AgentBuddy>,
    /// The face it wears in place of its CLI's logo, from `buddy` and the
    /// person's face style; none while it shows the logo. The core fills it
    /// in; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<AgentFace>,
    /// The terminal session running it, for an agent in a terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_id: Option<SessionId>,
    /// It runs in its CLI in a terminal rather than as a chat, whether or not that terminal
    /// runs now. One whose terminal ended starts it again once a client shows it.
    #[serde(default)]
    pub terminal: bool,
    /// Unix milliseconds.
    #[serde(default)]
    pub created_at: u64,
}

/// The face an agent wears, drawn by each client from the same art.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFace {
    pub pack: String,
    pub shape: String,
    /// Which of the pack's colours, in the order its palette lists them.
    pub color: u32,
    /// The part of the shape's 24-unit square its drawing fills, as an SVG
    /// viewBox.
    #[serde(rename = "box")]
    pub view_box: String,
}
