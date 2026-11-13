//! A project's setup and its checks on fresh checkouts, as the Playbook's Setup page and
//! `setup_check` show them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sikemux_corekit::tasks::config::ProjectConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    /// A `worktree.onCreate` step.
    Create,
    /// A process with a port, started and answering.
    Process,
    /// The project's check command.
    Check,
    /// No heavy command still running once the check ended.
    Heavy,
    /// A `worktree.onRemove` step.
    Remove,
    /// Nothing left listening on the checkout's ports once it is gone.
    Ports,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Passed,
    Failed,
    NotRun,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStep {
    pub kind: StepKind,
    pub label: String,
    pub command: String,
    pub outcome: Outcome,
    /// The last line a failed step printed, or why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_line: Option<String>,
    /// Everything it printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<PathBuf>,
}

/// One check of a project's setup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRun {
    /// The `sikemux.json` it checked.
    pub fingerprint: String,
    pub at: u64,
    pub passed: bool,
    pub steps: Vec<SetupStep>,
}

/// What a project's setup is, as its Setup page shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    pub project: String,
    /// The project's `sikemux.json` as it reads now.
    pub config: Option<ProjectConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<SetupRun>,
    /// `sikemux.json` changed since the last check.
    pub stale: bool,
    /// A check is running now.
    pub checking: bool,
}

impl SetupState {
    /// Proved: the last check passed on the file as it is now.
    pub fn proved(&self) -> bool {
        !self.stale && self.last.as_ref().is_some_and(|last| last.passed)
    }
}

#[derive(Serialize, Deserialize)]
pub struct SetupParams {
    pub project: String,
}
