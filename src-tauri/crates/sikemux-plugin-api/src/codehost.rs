//! The shapes every code host hands the Git pane: pull requests, CI runs,
//! issues, releases and the rest. A host's plugin maps its own API onto these,
//! and src/codehost/types.generated.ts is written from them. Statuses and
//! conclusions use GitHub's words, which other hosts map onto. Ids a host may
//! keep as text are strings everywhere, so a client never tells hosts apart.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub repo: Option<Repo>,
    pub slug: Option<String>,
    /// The remote is on a server this host's plugin is signed in to or serves.
    pub same_host: bool,
}

#[derive(Serialize)]
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

#[derive(Serialize)]
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

#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: u64,
    pub next_page: Option<u32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub runner: Option<String>,
    pub url: Option<String>,
    /// Where this job's annotations live; absent on a job its host never checked.
    pub check_run_id: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Billable {
    /// The runner it ran on: `UBUNTU`, `MACOS`, `WINDOWS`.
    pub runner: String,
    pub total_ms: u64,
    pub jobs: u64,
}

/// What a run took on the clock, and what it is billed for. A run on a
/// private repository costs minutes; a public one is free and reports none.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunTiming {
    pub run_duration_ms: Option<u64>,
    pub billable: Vec<Billable>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub number: u64,
    pub timestamp: Option<String>,
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobLog {
    pub lines: Vec<LogLine>,
    /// The log has aged out of the host's retention, so nothing is coming.
    pub expired: bool,
    /// The log was too long to read whole, so only its end is here.
    pub truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogExcerpt {
    pub lines: Vec<LogLine>,
    pub truncated: bool,
}

#[derive(Serialize, Clone)]
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
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub title: String,
    /// Markdown, and usually the whole of it.
    pub body: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub path: String,
    pub text: String,
}

#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedArtifact {
    pub path: String,
    pub bytes: u64,
}

#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    /// Six hex digits, with no leading hash.
    pub color: String,
}

#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
pub struct PullCounts {
    pub open: u64,
    pub closed: u64,
}

/// The issue or pull request that mentioned another, for a cross-reference.
#[derive(Serialize, Clone)]
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
#[derive(Serialize, Clone)]
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
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommitAuthor {
    pub email: String,
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PullCommit {
    pub sha: String,
    pub message: String,
    /// The host's login, or the name in the commit when no account matches it.
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub date: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
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

#[derive(Serialize, Clone)]
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
#[derive(Serialize)]
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

#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    pub downloads: u64,
}

#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone)]
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
