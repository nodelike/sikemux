use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_agents::harness::runs::Run;
use sikemux_corekit::tasks::check::Check;
use sikemux_corekit::tasks::config::ConfigLoad;
use sikemux_corekit::tasks::config::TrustCommand;
use sikemux_corekit::tasks::results::TestResults;
use std::path::PathBuf;

/// `sikemux.json` as read now, and whether the person trusts it on this host.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigRead {
    #[serde(flatten)]
    pub load: ConfigLoad,
    pub trusted: bool,
    /// What the person is asked before it runs, while it is not trusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_ask: Option<Value>,
}

/// A trust question waiting on the person, as the `trustAsks` call answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustQuestion {
    pub id: String,
    pub project: String,
    /// The run that waits on the answer, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub title: String,
    pub body: String,
    pub commands: Vec<TrustCommand>,
    pub confirm_label: String,
}

/// A run as clients show it, with the agent that started it and the test results it left,
/// as the `taskRuns` call answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    #[serde(flatten)]
    pub run: Run,
    pub agent_id: Option<String>,
    pub terminal_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestResults>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunParams {
    pub execution_id: String,
}

/// The task made to run only the failed tests, and its run.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunStarted {
    pub task_id: String,
    pub execution_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct ProjectParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
pub struct TrustProjectParams {
    pub project: String,
    pub fingerprint: String,
}

#[derive(Serialize, Deserialize)]
pub struct TrustAnswerParams {
    pub id: String,
    pub trust: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddParams {
    pub project: String,
    pub label: String,
    pub command: String,
    pub cwd: Option<String>,
    /// For the team in `sikemux.json`, or for this person only.
    pub team: bool,
    /// The task this one is an edit of, which goes when its id or place changes.
    pub replaces: Option<String>,
}

/// The task saved, by its id.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAdded {
    pub task_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCheckParams {
    pub project: String,
    pub command: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveTaskParams {
    pub project: String,
    pub task_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStartParams {
    pub project: String,
    pub task_id: String,
    /// Start whatever the checks found.
    #[serde(default)]
    pub force: bool,
    /// Install what the checks found missing and move the app off a taken
    /// port first.
    #[serde(default)]
    pub fix: bool,
}

/// The person starts a task: it replaces the one their project is running.
/// Answers the checks that stopped it, or the run it started.
/// What asking for a task to start did.
#[derive(Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum TaskStarted {
    /// Its checks hold it back, and the person decides.
    Held {
        checks: Vec<Check>,
    },
    Running {
        execution_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// From `sikemux.json`.
    Project,
    /// From the person's `.sikemux/tasks.json`.
    Personal,
    /// Suggested by the build files.
    Detected,
    /// A failed test run narrowed to its failures.
    Rerun,
    /// A shell command an agent gave.
    Command,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaView {
    pub required: u32,
    pub runs_on: Option<u32>,
    pub current: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub label: String,
    pub command: String,
    /// Relative to the project, `.` for its root.
    pub cwd: String,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java: Option<JavaView>,
    /// A run of it the person started is going.
    pub running: bool,
    /// The terminal that run shows in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
}

/// Whether the project's `sikemux.json` is there, and whether it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigStatus {
    Absent,
    Invalid,
    Valid,
}

/// Everything a project can run, as the `tasks` call answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub project: PathBuf,
    pub config_status: ConfigStatus,
    /// What is wrong with an invalid `sikemux.json`.
    pub config_errors: Option<Vec<sikemux_corekit::tasks::config::ConfigError>>,
    pub trusted: bool,
    pub tasks: Vec<TaskView>,
    /// The task to offer first: the one the person has running, else the first.
    pub default: Option<String>,
}
