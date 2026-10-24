//! What the Rundeck plugin's views hand a pane: the projects' folders, a
//! folder's jobs with how each last ran, and a job's or a project's runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The projects the person can see, each with its folders of jobs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tree {
    pub projects: Vec<TreeProject>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TreeProject {
    pub name: String,
    /// Why its jobs could not be listed.
    pub error: Option<String>,
    /// Its jobs, in every folder.
    pub total: u32,
    /// Its jobs in no folder.
    pub ungrouped: u32,
    pub groups: Vec<GroupNode>,
}

/// A folder of jobs, by name, with the folders below it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupNode {
    pub name: String,
    /// Slash-joined from the project's root, in the folders' own case.
    pub path: String,
    /// Jobs in it and every folder below it.
    pub count: u32,
    pub children: Vec<GroupNode>,
}

/// How a run went, as a pane colours it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StatusKind {
    Succeeded,
    Failed,
    Running,
    Aborted,
    Unknown,
}

/// One run of a job, as lists show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub execution_id: u64,
    pub status: String,
    pub status_kind: StatusKind,
    /// Still running, scheduled or queued.
    pub live: bool,
    pub user: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub branch: Option<String>,
}

/// A job in a folder, with its newest run and the newest that succeeded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub job_id: String,
    pub name: String,
    pub group: Option<String>,
    pub enabled: bool,
    pub scheduled: bool,
    /// Why its runs could not be read; `timed out` when Rundeck was too slow.
    pub error: Option<String>,
    pub latest: Option<RunSummary>,
    pub deployed: Option<RunSummary>,
    /// The branch it runs: the newest run's, else the one last deployed.
    pub branch: Option<String>,
    /// What starting it is called: `Deploy` when it runs a branch, else `Run`.
    pub run_word: String,
    /// While a run of another branch goes on, the branch that is live.
    pub live_branch: Option<String>,
}

/// A run of jobs under one heading: a folder below the one shown, or the
/// folder itself.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// The folder below the one shown, or none for the jobs in it directly.
    pub segment: Option<String>,
    /// The folder's whole path from the project's root.
    pub path: String,
    /// What its heading says.
    pub label: String,
    /// The folder or project names production.
    pub prod: bool,
    /// Its jobs' ids, by name.
    pub jobs: Vec<String>,
}

/// A folder's jobs, with how each last ran.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Jobs {
    pub project: String,
    pub group: Option<String>,
    /// The folder's own name, or the project's at its root.
    pub title: String,
    /// Why the jobs could not be listed.
    pub error: Option<String>,
    /// Rundeck was too slow for some jobs, which show no runs.
    pub partial: bool,
    pub timed_out: u32,
    pub running: u32,
    pub failed: u32,
    /// By name.
    pub jobs: Vec<JobRow>,
    /// Each job under the path below the folder it sits in.
    pub list: Vec<Section>,
    /// Each job under the folder right below this one, to compare the same job across
    /// folders; empty unless more than one folder holds a job of the same name.
    pub compare: Vec<Section>,
    /// Job names found in more than one of those folders.
    pub shared: u32,
    /// Jobs that sit in this folder itself, which comparing leaves out.
    pub loose: u32,
    /// Every folder in the project, and the folders above it, holding a running job; the
    /// project itself is the empty path.
    pub live_paths: Vec<String>,
}

/// One run in a job's or a project's history, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub execution_id: u64,
    pub status: String,
    pub status_kind: StatusKind,
    pub live: bool,
    pub user: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// How long it took, once it ended.
    pub secs: Option<u64>,
    pub branch: Option<String>,
    pub job_id: Option<String>,
    pub job_name: Option<String>,
    pub job_group: Option<String>,
    /// How long its job's runs that succeeded usually take, of those read.
    pub usual_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    Pending,
    Running,
    Ok,
    Fail,
    Skip,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveStep {
    pub name: String,
    pub state: StepState,
}

/// The run of a job going on now, step by step.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun {
    pub execution_id: u64,
    pub user: Option<String>,
    pub started_at: Option<String>,
    pub branch: Option<String>,
    pub steps: Vec<LiveStep>,
    pub completed: u32,
    pub total: u32,
}

/// A job with its recent runs and how they went.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobPage {
    pub job_id: String,
    pub name: String,
    pub group: Option<String>,
    pub project: String,
    pub description: Option<String>,
    pub permalink: Option<String>,
    pub execution_enabled: bool,
    pub node_filter: Option<String>,
    pub prod: bool,
    /// It takes a branch, so starting it is a deploy.
    pub runs_branch: bool,
    pub run_word: String,
    pub runs: Vec<Run>,
    pub live: Option<LiveRun>,
    /// The newest run that succeeded, with a branch when the job takes one.
    pub last_good: Option<Run>,
    /// That run's options, but the secure ones, to start it again with.
    pub repeat_options: BTreeMap<String, String>,
    pub finished: u32,
    pub succeeded: u32,
    pub usual_secs: Option<u64>,
    pub fastest_secs: Option<u64>,
    pub slowest_secs: Option<u64>,
    /// While a run goes on, how soon the core reads it again.
    pub read_again_secs: Option<u64>,
}

/// A project's recent runs of every job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    pub project: String,
    /// Who the sign-in is, to pick out their runs.
    pub me: Option<String>,
    pub runs: Vec<Run>,
    /// Every run Rundeck holds for the project.
    pub total: Option<u64>,
    pub running: u32,
    pub failed: u32,
    pub mine: u32,
    /// While a run goes on, how soon the core reads it again.
    pub read_again_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OptionKind {
    Text,
    /// Only Rundeck's own form can upload a file.
    File,
}

/// A value a job takes when it starts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobOption {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub secure: bool,
    pub default: Option<String>,
    /// The values Rundeck lists for it.
    pub values: Option<Vec<String>>,
    /// Where its values are read from instead, before `${option.NAME.value}` is filled in.
    pub values_url: Option<String>,
    /// Only listed values are allowed.
    pub enforced: bool,
    pub multivalued: bool,
    /// What separates its values when it takes several.
    pub delimiter: String,
    pub is_date: bool,
    pub date_format: Option<String>,
    pub kind: OptionKind,
}

/// One of the values an option may take, read from its values URL.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OptionChoice {
    pub name: String,
    pub value: String,
}

/// The form that starts a job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeployForm {
    pub job_id: String,
    pub name: String,
    pub group: Option<String>,
    pub project: String,
    pub description: Option<String>,
    pub permalink: Option<String>,
    pub node_filter: Option<String>,
    pub execution_enabled: bool,
    pub prod: bool,
    /// The environment it starts in: its top folder, or its project.
    pub env: String,
    /// The job's name below its folders, as a deploy plan names it.
    pub service: String,
    pub runs_branch: bool,
    /// The option that holds the branch.
    pub branch_key: Option<String>,
    pub run_word: String,
    pub options: Vec<JobOption>,
    /// Why the job's own options could not be read; the form then offers the branch alone.
    pub options_error: Option<String>,
    /// What each option starts filled in with.
    pub values: BTreeMap<String, String>,
    pub usual_secs: Option<u64>,
    /// Branches it ran lately, newest first.
    pub recent_branches: Vec<String>,
}

/// What a filled-in form would start, and what stops it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunCheck {
    /// By option name.
    pub errors: BTreeMap<String, String>,
    /// The options Rundeck is sent.
    pub send: BTreeMap<String, String>,
    /// Each option's values URL with the other options filled in.
    pub values_urls: BTreeMap<String, String>,
    /// Whether a run of the job goes on now, when asked.
    pub running: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Ok,
    Info,
    Warn,
    Danger,
}

/// A line of a deploy plan.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCheck {
    pub tone: Tone,
    pub text: String,
    pub value: Option<String>,
    /// It says the checkout is pushed first.
    pub pushes: bool,
}

/// What deploying a branch would do.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub target_branch: String,
    pub deployed_branch: Option<String>,
    pub current_branch: Option<String>,
    /// The checkout is pushed before the run starts.
    pub pushes: bool,
    pub checks: Vec<PlanCheck>,
    pub detail: Option<String>,
}

/// A step of a run, as its page lists it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunStep {
    /// What its log lines name it by.
    pub key: String,
    pub name: String,
    pub state: StepState,
    /// Rundeck's word for its state, such as `not started`.
    pub state_name: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

/// One run of a job, step by step.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPage {
    pub execution_id: u64,
    pub status: String,
    pub status_kind: StatusKind,
    pub live: bool,
    pub user: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub permalink: Option<String>,
    /// The job's folder or project names production.
    pub prod: bool,
    pub branch: Option<String>,
    pub run_word: String,
    pub steps: Vec<RunStep>,
    pub completed: u32,
    pub total: u32,
    /// How far through its steps it is, once it has any.
    pub percent: Option<u32>,
    /// How long the job's runs that succeeded usually take.
    pub usual_secs: Option<u64>,
    /// Rundeck says which job it ran, so it can be started again from here.
    pub can_run_again: bool,
    /// Its options, but the secure ones, to start it again with.
    pub repeat_options: BTreeMap<String, String>,
    /// While it runs, how soon the core reads it again.
    pub read_again_secs: Option<u64>,
}

/// What to tell the person when a run they started ends.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunNotice {
    pub ok: bool,
    pub title: String,
    pub body: String,
}

/// How a place a job deploys to is coloured.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EnvTone {
    Prod,
    Staging,
    Preprod,
    Dev,
    Other,
}

/// A job somewhere in Rundeck, as a list of places to pick from shows it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobPlace {
    pub project: String,
    pub job_id: String,
    pub name: String,
    pub group: Option<String>,
    /// The project, folders and name, slash-separated with spaces.
    pub place: String,
    pub tone: EnvTone,
}

/// Where a project on this host deploys: the jobs named like its folder, and
/// how the one chosen last ran.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeployTarget {
    /// By place.
    pub jobs: Vec<JobPlace>,
    /// The one picked, else the first outside production, else the first.
    pub active: Option<JobPlace>,
    pub deployed_branch: Option<String>,
    pub runs_branch: bool,
    pub last_run: Option<LastRun>,
    /// Where it deploys and what runs there, in a line.
    pub summary: String,
    /// While a run goes on, how soon the core reads it again.
    pub read_again_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub status_kind: StatusKind,
    /// Such as `Last run succeeded · main`.
    pub text: String,
}

/// Every job in every project, to search.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobCatalog {
    /// By project, folder and name.
    pub jobs: Vec<JobPlace>,
    /// Projects whose jobs could not be listed.
    pub failed_projects: u32,
}

/// A colour output asks for: one of the terminal's 256, the first sixteen its named ones, or exact.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TermColor {
    Palette { index: u8 },
    Rgb { r: u8, g: u8, b: u8 },
}

/// A stretch of output in one style.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StyledText {
    pub text: String,
    pub fg: Option<TermColor>,
    pub bg: Option<TermColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A line of a run's output, as Rundeck sends it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OutputLine {
    pub time: Option<String>,
    pub level: Option<String>,
    pub log: Option<String>,
    pub user: Option<String>,
    /// The step it came from, as `2` or `2/1@node`.
    #[serde(rename = "stepctx")]
    pub step_ctx: Option<String>,
    pub node: Option<String>,
    /// The line in its styles, when its terminal codes set any; `log` keeps the codes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<StyledText>,
}

/// The run's output since the last tick.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OutputTick {
    pub entries: Vec<OutputLine>,
    /// Where the next tick reads from.
    pub offset: String,
    pub completed: bool,
    /// The stream gave up after too many errors.
    pub failed: bool,
    pub error: Option<String>,
}
