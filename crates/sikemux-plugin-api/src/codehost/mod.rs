//! The shapes every code host hands the Git pane: pull requests, CI runs,
//! issues, releases and the rest. A host's plugin maps its own API onto these,
//! and the core reads them back to work out what clients show. Statuses and
//! conclusions use GitHub's words, which other hosts map onto. Ids a host may
//! keep as text are strings everywhere, so a client never tells hosts apart.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

pub mod asks;
mod host;

pub use host::{dispatch, work, CodeHost};

/// An id the shapes below carry as text, for a host whose API takes it as a number.
pub fn numeric_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse()
        .map_err(|_| serde::de::Error::custom(format!("`{text}` is not an id")))
}

pub fn numeric_id_if_any<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|text| {
            text.parse()
                .map_err(|_| serde::de::Error::custom(format!("`{text}` is not an id")))
        })
        .transpose()
}

/// Where the token a code host's plugin uses came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenSource {
    Keychain,
    Environment,
    GhCli,
}

/// How an account signed in, on a host that offers more than one way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignedInWith {
    /// Through the browser; the Keychain holds a refresh token.
    Oauth,
    /// A pasted API token or access token; the Keychain holds the token.
    Token,
}

/// Who is signed in to a code host, as every code host's sign-in reads it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Signed {
    /// Which account this is about; absent while a token the shell or `gh`
    /// holds is used before anybody has signed in.
    pub account: Option<String>,
    /// The server, on a host a company can run its own of.
    pub host: Option<String>,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub method: Option<SignedInWith>,
    pub token_source: Option<TokenSource>,
    /// The shell variable a borrowed token was read from.
    pub token_variable: Option<String>,
    pub scopes: Vec<String>,
    /// The sign-in may start, re-run and stop CI runs, not only read them.
    pub can_write_ci: bool,
    /// It can be signed in to through the browser.
    pub browser_sign_in: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub repo: Option<Repo>,
    pub slug: Option<String>,
    /// The remote is on a server this host's plugin is signed in to or serves.
    pub same_host: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoListing {
    pub owner: String,
    pub name: String,
    pub slug: String,
    pub private: bool,
    pub archived: bool,
    pub default_branch: Option<String>,
    pub pushed_at: Option<String>,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub path: String,
    /// `active`, or one of the ways a host says a workflow is switched off.
    pub state: String,
    pub active: bool,
    pub url: String,
}

/// How a run, job or check stands, whichever host it ran on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Running,
    Queued,
    Success,
    Failure,
    Cancelled,
    Skipped,
    /// Waiting on someone's approval.
    Blocked,
    #[default]
    Unknown,
}

impl Outcome {
    /// What a host's `status`, and its `conclusion` once `completed`, mean.
    pub fn of(status: &str, conclusion: Option<&str>) -> Self {
        if status == "completed" {
            return match conclusion.unwrap_or_default() {
                "success" => Self::Success,
                "failure" | "timed_out" | "startup_failure" => Self::Failure,
                "cancelled" | "stale" => Self::Cancelled,
                "skipped" | "neutral" => Self::Skipped,
                "action_required" => Self::Blocked,
                _ => Self::Unknown,
            };
        }
        match status {
            "in_progress" => Self::Running,
            "queued" | "requested" | "pending" => Self::Queued,
            "waiting" | "action_required" => Self::Blocked,
            _ => Self::Unknown,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub name: String,
    pub title: String,
    pub workflow_id: String,
    /// The workflow file, e.g. `.github/workflows/release.yml`.
    pub path: Option<String>,
    pub run_number: u64,
    pub attempt: u64,
    pub event: String,
    /// `queued`, `in_progress`, `completed`, and the waiting-for-approval ones.
    pub status: String,
    /// Only set once the run is over: `success`, `failure`, `cancelled`, and the rest.
    pub conclusion: Option<String>,
    /// What `status` and `conclusion` come to, the one thing clients show.
    pub outcome: Outcome,
    pub branch: Option<String>,
    pub sha: String,
    pub short_sha: String,
    pub actor: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub updated_at: String,
    pub pull_requests: Vec<u64>,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: u64,
    pub next_page: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub outcome: Outcome,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub outcome: Outcome,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub runner: Option<String>,
    pub url: Option<String>,
    /// Where this job's annotations live; absent on a job its host never checked.
    pub check_run_id: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Billable {
    /// The runner it ran on: `UBUNTU`, `MACOS`, `WINDOWS`.
    pub runner: String,
    pub total_ms: u64,
    pub jobs: u64,
}

/// What a run took on the clock, and what it is billed for. A run on a
/// private repository costs minutes; a public one is free and reports none.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunTiming {
    pub run_duration_ms: Option<u64>,
    pub billable: Vec<Billable>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub number: u64,
    pub timestamp: Option<String>,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobLog {
    pub lines: Vec<LogLine>,
    /// The log has aged out of the host's retention, so nothing is coming.
    pub expired: bool,
    /// The log was too long to read whole, so only its end is here.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogExcerpt {
    pub lines: Vec<LogLine>,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    /// The file it points at, relative to the repository root. `.github` when
    /// the failure belongs to the workflow itself rather than to code.
    pub path: Option<String>,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    /// `failure`, `warning` or `notice`.
    pub level: String,
    pub title: Option<String>,
    pub message: String,
    pub details: Option<String>,
}

/// What a job wrote to be read rather than dug out of the log: the test
/// report, the coverage table.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub title: String,
    /// Markdown, and usually the whole of it.
    pub body: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub path: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    /// The host has stopped keeping it, which makes it undownloadable.
    pub expired: bool,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedArtifact {
    pub path: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub environment_id: u64,
    pub environment: String,
    pub wait_minutes: u64,
    /// False when somebody else has to sign this off, which is worth saying
    /// rather than offering a button that will be refused.
    pub can_approve: bool,
    pub reviewers: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    /// Six hex digits, with no leading hash.
    pub color: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    /// `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR` and the rest.
    pub author_association: Option<String>,
    pub body: String,
    pub created_at: String,
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `open`, `merged`, or `closed`.
    pub state: String,
    pub draft: bool,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    /// `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR` and the rest.
    pub author_association: Option<String>,
    pub head: Option<String>,
    /// `owner:branch`, which names the fork a branch lives on.
    pub head_label: Option<String>,
    pub base: Option<String>,
    /// The commit at the tip of the branch, which is what its checks ran on.
    pub head_sha: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Absent when the host did not say, which a list of pull requests often does not.
    pub comments: Option<u64>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
    /// Whether it can merge cleanly; unknown until the host has looked.
    pub mergeable: Option<bool>,
    /// `clean`, `blocked`, `dirty`, `behind` and the rest of GitHub's words.
    pub merge_state: Option<String>,
    pub labels: Vec<Label>,
    pub reviewers: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub commits: Option<u64>,
    pub merged_at: Option<String>,
    pub merged_by: Option<String>,
    pub merge_commit_sha: Option<String>,
    /// The picture of each person named above by login alone.
    pub avatars: BTreeMap<String, String>,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PullCounts {
    pub open: u64,
    pub closed: u64,
}

/// The issue or pull request that mentioned another, for a cross-reference.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    pub number: u64,
    pub pull: bool,
    /// `open` or `closed`; a merged pull request also says when.
    pub state: Option<String>,
    pub merged: bool,
    pub repo: Option<String>,
}

/// One thing in a pull request's or issue's history: a comment, a review, a
/// commit, a merge or anything else the host records, in one shape.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    /// GitHub's event name, such as `commented`, `reviewed` or `committed`.
    pub kind: String,
    pub id: Option<u64>,
    pub actor: Option<String>,
    pub avatar_url: Option<String>,
    pub association: Option<String>,
    pub at: Option<String>,
    pub body: Option<String>,
    /// A review's verdict, in lower case, or why an issue was closed.
    pub state: Option<String>,
    pub sha: Option<String>,
    pub message: Option<String>,
    /// Who or what it was about: a requested reviewer, a label, a new title.
    pub subject: Option<String>,
    /// Where the host says which issue or pull request mentioned this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Mention>,
}

/// Who wrote a commit, by the email in it, and the account the host matched that email to.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommitAuthor {
    pub email: String,
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PullCommit {
    pub sha: String,
    pub message: String,
    /// The host's login, or the name in the commit when no account matches it.
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub date: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    /// `added`, `modified`, `removed`, `renamed`, `copied` or `changed`.
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// The unified diff, which a host leaves out for a file too big to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Says which part of the patch this is and how to read the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED` or `DISMISSED`.
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

/// One thing a reviewer asked of a pull request, general or on a line, for an
/// agent to answer in its own thread.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFeedback {
    /// `comment/`, `inline/` or `review/` and what names it on the host, which
    /// answering it hands back.
    pub id: String,
    pub author: Option<String>,
    pub bot: bool,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    /// `CHANGES_REQUESTED` for a reviewer who asked for changes.
    pub state: Option<String>,
    pub at: String,
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    /// Why it was closed: `completed`, `not_planned` or `reopened`.
    pub state_reason: Option<String>,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub comments: u64,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    pub downloads: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub id: u64,
    pub tag: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
    /// A draft has never been published, so it has only a creation time.
    pub published_at: Option<String>,
    pub author: Option<String>,
    /// The author's picture, from hosts that send one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub assets: Vec<ReleaseAsset>,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// What marking it read needs.
    pub id: String,
    pub title: String,
    /// `PullRequest`, `Issue`, `Release`, `CheckSuite`, `Discussion` or `Commit`.
    pub kind: String,
    /// Why it arrived: `review_requested`, `mention`, `assign`, `author` and so on.
    pub reason: String,
    pub repo: String,
    /// The number of the pull request or issue, when it has one.
    pub number: Option<u64>,
    pub unread: bool,
    pub updated_at: String,
    /// Where a person would go to read it, or nothing when the host gave no address.
    pub url: Option<String>,
}

/// A pull request as a list shows it, with what the core works out from it.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListedPull {
    pub pull: Pull,
    pub state: PullState,
    /// The issues its description says it closes, such as "Fixes #12".
    pub closes: Vec<u64>,
    /// Its branch is in this repository rather than a fork's, so a branch of
    /// the same name here is the one it was opened from.
    pub own_branch: bool,
    /// How the runs on its head commit stand, once any have run.
    pub checks: Option<CheckTally>,
}

/// A pull request's state, with a draft and a merge told apart.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PullState {
    Open,
    Draft,
    Closed,
    Merged,
}

/// How many of a commit's runs stand each way, and what they come to.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckTally {
    /// Any failure fails them; then any still going keeps them running, or
    /// queued while none has started; then any waiting on approval.
    pub outcome: Outcome,
    pub total: u32,
    /// Passed or skipped, which count as passing.
    pub passed: u32,
    pub failing: u32,
    pub running: u32,
    pub queued: u32,
    /// Waiting on someone's approval.
    pub waiting: u32,
    pub cancelled: u32,
    pub unknown: u32,
    /// Of those passing, how many were skipped.
    #[serde(default)]
    pub skipped: u32,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PullList {
    pub pulls: Vec<ListedPull>,
    /// How many are open and closed, from hosts that count them cheaply.
    pub counts: Option<PullCounts>,
}

/// One check on a commit: a job of one of its runs, or a status an app outside
/// the host's CI posted.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub outcome: Outcome,
    /// The run it is a job of, and its id there; none for an app's status.
    pub run_id: Option<String>,
    pub job_id: Option<String>,
    /// The app that posted it, when that is not the host's CI.
    pub app: Option<String>,
    /// The base branch's rules require it to pass.
    pub required: bool,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub url: Option<String>,
}

/// A run as a list shows it.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListedRun {
    pub run: Run,
    /// Still going, including held for approval: it can be cancelled but not
    /// run again.
    pub unfinished: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunList {
    pub runs: Vec<ListedRun>,
    pub total: u64,
    pub next_page: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IssueList {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
    /// How many are open and closed, from hosts that count them cheaply.
    pub counts: Option<PullCounts>,
}

/// One pull request as its page shows it, with what the core works out from
/// it and its reviews.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenPull {
    pub pull: Pull,
    pub state: PullState,
    pub closes: Vec<u64>,
    pub own_branch: bool,
    /// What its reviewers' latest reviews come to, once one approved or asked
    /// for changes.
    pub verdict: Option<ReviewVerdict>,
    /// Somebody reviewed it at all.
    pub reviewed: bool,
    /// Whether it can merge, from what the host said of its merge state.
    pub merge: MergeReadiness,
    /// How the runs on its head commit stand, once any have run.
    pub checks: Option<CheckTally>,
    /// The runs on its head commit.
    pub runs: Vec<ListedRun>,
    /// Every check on its head commit, each job of its runs and each status an
    /// app outside the host's CI posted, from hosts that list them.
    #[serde(default)]
    pub jobs: Vec<Check>,
    /// The files its head and its base both changed in ways git cannot merge,
    /// as the project's clone works them out while it conflicts.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// The local branch checking it out makes: its own branch when it lives on
    /// the repository, else `pr-<number>` from the host's ref for it. None when
    /// it comes from a fork the host keeps no ref for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeReadiness {
    Ready,
    /// It conflicts with its base.
    Conflicts,
    /// Waiting on required reviews or checks.
    Blocked,
    /// Its branch is behind its base.
    Behind,
    /// The host has not worked it out yet.
    Checking,
}

/// An open issue or pull request a message can be about.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrackedItem {
    pub kind: TrackedKind,
    pub number: u64,
    pub title: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrackedKind {
    Issue,
    Pull,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseList {
    pub releases: Vec<Release>,
    /// The newest published release that is not a pre-release.
    pub latest: Option<u64>,
}

/// One attempt of a run as its page shows it, with what the core works out
/// from it.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenRun {
    pub run: Run,
    pub jobs: Vec<ListedJob>,
    /// How many attempts the run has had.
    pub attempts: u64,
    pub unfinished: bool,
    /// How many jobs are over.
    pub done: u32,
    pub failed: u32,
    /// The first job that failed.
    pub failed_job: Option<String>,
    pub graph: JobGraph,
    /// What it took and is billed for, once it is over.
    pub timing: Option<RunTiming>,
    /// The jobs that may have written a summary, once the run is over.
    pub summaries: Vec<String>,
    /// The deployments it waits on someone to approve, while it waits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<PendingApproval>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListedJob {
    pub job: Job,
    /// The step it failed at.
    pub stopped_at: Option<String>,
}

/// A run's jobs in columns as the host draws them: each column after every
/// column it waits for.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobGraph {
    pub columns: Vec<Vec<GraphBox>>,
    pub edges: Vec<GraphEdge>,
}

/// A lone job, a matrix job's runs, or jobs that wait for the same jobs and
/// are waited on by the same jobs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphBox {
    pub id: String,
    /// A matrix job's or a called workflow's name.
    pub title: Option<String>,
    pub jobs: Vec<GraphJob>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphJob {
    /// None for a job the workflow file declares that the run has not made yet.
    pub job: Option<String>,
    pub label: String,
    /// The job that called its workflow was skipped, so it never will run.
    pub skipped: bool,
}

/// From a box to a box that waits for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

/// A job's log as its page shows it, split into its steps: all of it, or the
/// lines a client asked for.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenJobLog {
    pub lines: Vec<JobLogLine>,
    /// Where in the log the first of `lines` is.
    pub from: usize,
    /// How many lines the whole log has.
    pub total: usize,
    /// Over the whole log.
    pub sections: Vec<LogSection>,
    /// The log has aged out of the host's retention, so nothing is coming.
    pub expired: bool,
    /// The log was too long to read whole, so only its end is here.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobLogLine {
    pub number: u64,
    /// Without its colour codes or the runner's `##[...]` markup, which `mark`
    /// says instead.
    pub text: String,
    /// Its text in the colours the job printed it in; none for a plain line.
    pub spans: Vec<LogSpan>,
    pub mark: Option<LogMark>,
    /// Part of the script a step runs, as its log repeats it before running it.
    pub script: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LogMark {
    Group,
    Endgroup,
    Section,
    Command,
    Error,
    Warning,
    Notice,
    Debug,
}

/// A stretch of a line in one style.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogSpan {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<LogColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<LogColor>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LogColor {
    /// One of the terminal's 256: the first sixteen are its theme's own.
    Palette { index: u8 },
    /// Written out as `#rrggbb`.
    Rgb { hex: String },
}

/// One step's stretch of the log: lines `from` up to `to`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogSection {
    /// The step's number, or none for a job without steps.
    pub step: Option<u64>,
    pub from: usize,
    pub to: usize,
}

#[cfg(test)]
mod tests {
    use super::Outcome;

    #[test]
    fn reads_a_finished_run_by_its_conclusion() {
        let done = |conclusion| Outcome::of("completed", Some(conclusion));
        assert_eq!(done("success"), Outcome::Success);
        assert_eq!(done("failure"), Outcome::Failure);
        assert_eq!(done("timed_out"), Outcome::Failure);
        assert_eq!(done("startup_failure"), Outcome::Failure);
        assert_eq!(done("cancelled"), Outcome::Cancelled);
        assert_eq!(done("skipped"), Outcome::Skipped);
    }

    #[test]
    fn reads_a_run_still_going_by_its_status() {
        assert_eq!(Outcome::of("in_progress", None), Outcome::Running);
        assert_eq!(Outcome::of("queued", None), Outcome::Queued);
        assert_eq!(Outcome::of("waiting", None), Outcome::Blocked);
    }

    #[test]
    fn does_not_guess_at_a_state_it_has_not_seen() {
        assert_eq!(Outcome::of("completed", None), Outcome::Unknown);
        assert_eq!(Outcome::of("something_new", None), Outcome::Unknown);
    }
}
