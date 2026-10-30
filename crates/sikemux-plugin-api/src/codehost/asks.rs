//! What the core and clients ask a code host, one shape per question, so the
//! three hosts read the same fields. A host that has no use for a field
//! leaves it alone. Ids are text, which a host whose API takes numbers parses.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::PluginError;

/// An id given as a number or as text, kept as text.
fn id_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    Ok(match Id::deserialize(deserializer)? {
        Id::Number(number) => number.to_string(),
        Id::Text(text) => text,
    })
}

fn id_text_if_any<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Given(#[serde(deserialize_with = "id_text")] String);
    Ok(Option::<Given>::deserialize(deserializer)?.map(|given| given.0))
}

/// `id` as the number a host's API takes.
pub fn numeric(id: &str) -> Result<u64, PluginError> {
    id.parse()
        .map_err(|_| PluginError::bad_params(format!("`{id}` is not an id")))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

/// Which repository an account is asked for, on which server of the host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteQuery {
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MineQuery {
    #[serde(default = "MineQuery::default_limit")]
    pub limit: u32,
}

impl MineQuery {
    fn default_limit() -> u32 {
        50
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageRef {
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default, deserialize_with = "id_text_if_any")]
    pub workflow_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAt {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub path: String,
    pub git_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default, deserialize_with = "id_text_if_any")]
    pub workflow_id: Option<String>,
    pub git_ref: String,
    #[serde(default)]
    pub inputs: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default, deserialize_with = "id_text_if_any")]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(deserialize_with = "id_text")]
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRef {
    #[serde(flatten)]
    pub run: RunRef,
    pub attempt: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rerun {
    #[serde(flatten)]
    pub run: RunRef,
    #[serde(default)]
    pub failed_only: bool,
    #[serde(default)]
    pub debug: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(deserialize_with = "id_text")]
    pub job_id: String,
    /// The run the job is in, for a host that names a job only within it.
    #[serde(default, deserialize_with = "id_text_if_any")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunJob {
    #[serde(flatten)]
    pub job: JobRef,
    #[serde(default)]
    pub debug: bool,
}

/// The lines of a job's log to hand back: the last `tail`, or those matching `grep`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcerptQuery {
    #[serde(flatten)]
    pub job: JobRef,
    #[serde(default)]
    pub tail: Option<usize>,
    #[serde(default)]
    pub grep: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRunRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(deserialize_with = "id_text")]
    pub check_run_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub sha: String,
    #[serde(default)]
    pub base: Option<String>,
}

/// Approving or rejecting a run held at its environments.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentReview {
    #[serde(flatten)]
    pub run: RunRef,
    pub environment_ids: Vec<u64>,
    pub state: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    /// `open`, `closed` or `all`; open when not given.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

/// Whether a thread of comments is a pull request's or an issue's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadOf {
    #[default]
    Pull,
    Issue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
    #[serde(default)]
    pub of: ThreadOf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub sha: String,
}

/// Who wrote the commits on `git_ref`, the default branch when not given.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitsQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default)]
    pub git_ref: Option<String>,
}

/// Which of a pull request's patches to hand back, and how much of each.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchQuery {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub lines: Option<usize>,
    #[serde(default)]
    pub full_patches: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesQuery {
    #[serde(flatten)]
    pub pull: PullRef,
    #[serde(flatten)]
    pub patches: PatchQuery,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Merge {
    #[serde(flatten)]
    pub pull: PullRef,
    /// The repository's own default when not given.
    #[serde(default)]
    pub method: Option<String>,
    /// The head the checks passed on, so a pull request that moved since is not merged.
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPull {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    pub head: String,
    pub base: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    /// Deletes the head branch once it is merged, where the host can.
    #[serde(default)]
    pub close_branch: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullEdit {
    #[serde(flatten)]
    pub pull: PullRef,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetPullState {
    #[serde(flatten)]
    pub pull: PullRef,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetIssueState {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewReview {
    #[serde(flatten)]
    pub pull: PullRef,
    /// `APPROVE`, `REQUEST_CHANGES` or `COMMENT`.
    pub event: String,
    #[serde(default)]
    pub body: String,
}

/// What reviewers said on a pull request at or after `since`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedbackQuery {
    #[serde(flatten)]
    pub pull: PullRef,
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    /// The comment answered.
    pub to: String,
    pub body: String,
    /// What the reply answers, quoted where it cannot sit in that thread.
    #[serde(default)]
    pub quote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answers {
    #[serde(flatten)]
    pub pull: PullRef,
    pub replies: Vec<Reply>,
    /// Asked to look again, where the host can.
    #[serde(default)]
    pub reviewers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewIssue {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewComment {
    #[serde(flatten)]
    pub thread: ThreadRef,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxQuery {
    /// Read ones too.
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationRef {
    pub id: String,
}
