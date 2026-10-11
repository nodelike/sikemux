// Pipelines, in the words the Git pane uses for every host's CI: a pipeline is
// a run and its jobs are jobs, named `stage / job` so the graph groups them by
// stage. A project has one pipeline definition, .gitlab-ci.yml, so it has one
// workflow, which can also be started by hand with variables.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use futures::future::join_all;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::repo::{self, avatar_of, encoded, login_of, RepoRef, User};

pub const PIPELINE_FILE: &str = ".gitlab-ci.yml";
pub const WORKFLOW_ID: &str = "pipeline";
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;
/// Commit titles are looked up for at most this many runs on a page that does not know them yet.
const TITLES_PER_PAGE: usize = 20;
const TITLES_KEPT: usize = 2000;

/// The GitHub words every host maps onto: a status, and once completed, a conclusion.
pub fn status_of(status: &str, allow_failure: bool) -> (String, Option<String>) {
    let done = |conclusion: &str| ("completed".to_string(), Some(conclusion.to_string()));
    match status {
        "success" => done("success"),
        "failed" if allow_failure => done("neutral"),
        "failed" => done("failure"),
        "canceled" => done("cancelled"),
        "skipped" => done("skipped"),
        "running" | "canceling" => ("in_progress".into(), None),
        "manual" => ("waiting".into(), None),
        _ => ("queued".into(), None),
    }
}

fn event_of(source: Option<&str>) -> &'static str {
    match source {
        Some("merge_request_event") => "pull_request",
        Some("schedule") => "schedule",
        Some("web" | "api" | "trigger" | "pipeline" | "chat") => "manual",
        _ => "push",
    }
}

/// The merge request a pipeline's ref belongs to, from `refs/merge-requests/12/head`.
fn merge_request_of(git_ref: &str) -> Option<u64> {
    git_ref
        .strip_prefix("refs/merge-requests/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

#[derive(Deserialize, Clone)]
pub struct PipelineRow {
    pub id: u64,
    pub iid: Option<u64>,
    #[serde(default)]
    pub sha: String,
    #[serde(rename = "ref", default)]
    pub git_ref: String,
    #[serde(default)]
    pub status: String,
    pub source: Option<String>,
    pub name: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub started_at: Option<String>,
    pub duration: Option<f64>,
    pub web_url: String,
    pub user: Option<User>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub name: String,
    pub title: String,
    pub workflow_id: String,
    pub path: Option<String>,
    pub run_number: u64,
    pub attempt: u64,
    pub event: String,
    pub status: String,
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

impl Run {
    pub fn from_row(row: &PipelineRow, commit_title: Option<&str>) -> Self {
        let merge_request = merge_request_of(&row.git_ref);
        let (status, conclusion) = status_of(&row.status, false);
        let name = row
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Pipeline".into());
        let title = commit_title
            .map(str::to_string)
            .or_else(|| row.name.clone().filter(|name| !name.is_empty()))
            .unwrap_or_else(|| match merge_request {
                Some(number) => format!("Merge request !{number}"),
                None => row.git_ref.clone(),
            });
        Run {
            id: row.id.to_string(),
            name,
            title,
            workflow_id: WORKFLOW_ID.into(),
            path: Some(PIPELINE_FILE.into()),
            run_number: row.iid.unwrap_or(row.id),
            attempt: 1,
            event: event_of(row.source.as_deref()).into(),
            status,
            conclusion,
            branch: merge_request
                .is_none()
                .then(|| row.git_ref.clone())
                .filter(|branch| !branch.is_empty()),
            short_sha: row.sha.chars().take(8).collect(),
            sha: row.sha.clone(),
            actor: login_of(row.user.as_ref()),
            avatar_url: avatar_of(row.user.as_ref()),
            created_at: row.created_at.clone(),
            started_at: row
                .started_at
                .clone()
                .or_else(|| Some(row.created_at.clone())),
            updated_at: row
                .updated_at
                .clone()
                .unwrap_or_else(|| row.created_at.clone()),
            pull_requests: merge_request.into_iter().collect(),
            url: row.web_url.clone(),
        }
    }
}

static TITLES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn known_title(sha: &str) -> Option<String> {
    TITLES.lock().ok()?.as_ref()?.get(sha).cloned()
}

fn remember_title(sha: &str, title: &str) {
    if let Ok(mut titles) = TITLES.lock() {
        let titles = titles.get_or_insert_with(HashMap::new);
        if titles.len() >= TITLES_KEPT {
            titles.clear();
        }
        titles.insert(sha.to_string(), title.to_string());
    }
}

#[derive(Deserialize)]
struct CommitRow {
    title: Option<String>,
}

/// A pipeline is best named by its commit, which GitLab's lists leave out; titles are kept once read.
async fn commit_title(data_dir: &Path, repo: &RepoRef, sha: &str) -> Option<String> {
    if sha.is_empty() {
        return None;
    }
    if let Some(title) = known_title(sha) {
        return Some(title);
    }
    let path = repo.path(&format!("/repository/commits/{sha}")).ok()?;
    let commit: CommitRow = client::get(data_dir, &path, &[]).await.ok()?;
    let title = commit.title.filter(|title| !title.is_empty())?;
    remember_title(sha, &title);
    Some(title)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub branch: Option<String>,
    pub status: Option<String>,
    pub event: Option<String>,
    pub actor: Option<String>,
    pub head_sha: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

/// GitLab's own status names for one of the Git pane's, so the server filters.
fn gitlab_status(wanted: &str) -> Option<&'static str> {
    Some(match wanted {
        "success" => "success",
        "failure" => "failed",
        "cancelled" => "canceled",
        "skipped" => "skipped",
        "in_progress" => "running",
        "queued" | "pending" => "pending",
        "waiting" | "action_required" => "manual",
        _ => return None,
    })
}

fn source_of(event: &str) -> Option<&'static str> {
    Some(match event {
        "pull_request" => "merge_request_event",
        "schedule" => "schedule",
        "manual" => "web",
        "push" => "push",
        _ => return None,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: u64,
    pub next_page: Option<u32>,
}

pub async fn list(data_dir: &Path, input: RunQuery) -> GitlabResult<RunPage> {
    let page = input.page.unwrap_or(1).max(1);
    let per_page = input.per_page.unwrap_or(30).clamp(1, 100);
    let mut query = vec![("order_by", "id".to_string()), ("sort", "desc".to_string())];
    if let Some(branch) = &input.branch {
        query.push(("ref", branch.clone()));
    }
    if let Some(status) = input.status.as_deref().and_then(gitlab_status) {
        query.push(("status", status.into()));
    }
    if let Some(source) = input.event.as_deref().and_then(source_of) {
        query.push(("source", source.into()));
    }
    if let Some(actor) = &input.actor {
        query.push(("username", actor.clone()));
    }
    if let Some(sha) = &input.head_sha {
        query.push(("sha", sha.clone()));
    }
    let found: client::Page<PipelineRow> = client::get_page(
        data_dir,
        &input.repo.path("/pipelines")?,
        &query,
        page,
        per_page,
    )
    .await?;
    let mut unknown: Vec<&str> = Vec::new();
    for row in &found.items {
        if known_title(&row.sha).is_none()
            && !unknown.contains(&row.sha.as_str())
            && unknown.len() < TITLES_PER_PAGE
        {
            unknown.push(&row.sha);
        }
    }
    join_all(
        unknown
            .iter()
            .map(|sha| commit_title(data_dir, &input.repo, sha)),
    )
    .await;
    let runs: Vec<Run> = found
        .items
        .iter()
        .map(|row| Run::from_row(row, known_title(&row.sha).as_deref()))
        .collect();
    Ok(RunPage {
        total: found.total.unwrap_or(runs.len() as u64),
        next_page: found.next,
        runs,
    })
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub run_id: String,
}

fn number(raw: &str, what: &str) -> GitlabResult<u64> {
    raw.trim()
        .parse()
        .map_err(|_| GitlabError::BadArg(format!("`{raw}` is not a GitLab {what} id")))
}

fn pipeline_path(repo: &RepoRef, run_id: &str, rest: &str) -> GitlabResult<String> {
    repo.path(&format!("/pipelines/{}{rest}", number(run_id, "pipeline")?))
}

async fn pipeline(data_dir: &Path, repo: &RepoRef, run_id: &str) -> GitlabResult<PipelineRow> {
    client::get(data_dir, &pipeline_path(repo, run_id, "")?, &[]).await
}

#[derive(Deserialize)]
struct Runner {
    description: Option<String>,
}

#[derive(Deserialize)]
pub struct JobRow {
    id: u64,
    name: String,
    stage: Option<String>,
    status: String,
    #[serde(default)]
    allow_failure: bool,
    started_at: Option<String>,
    finished_at: Option<String>,
    runner: Option<Runner>,
    web_url: Option<String>,
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
    pub check_run_id: Option<String>,
    pub steps: Vec<Value>,
}

impl Job {
    pub fn from_row(row: JobRow) -> Self {
        let (status, conclusion) = status_of(&row.status, row.allow_failure);
        Job {
            id: row.id.to_string(),
            name: match row.stage.filter(|stage| !stage.is_empty()) {
                Some(stage) => format!("{stage} / {}", row.name),
                None => row.name,
            },
            status,
            conclusion,
            started_at: row.started_at,
            completed_at: row.finished_at,
            runner: row.runner.and_then(|runner| runner.description),
            url: row.web_url,
            check_run_id: None,
            steps: Vec::new(),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
}

pub async fn detail(data_dir: &Path, input: RunRef) -> GitlabResult<RunDetail> {
    let row = pipeline(data_dir, &input.repo, &input.run_id).await?;
    let title = commit_title(data_dir, &input.repo, &row.sha).await;
    let rows: Vec<JobRow> = client::get_all(
        data_dir,
        &pipeline_path(&input.repo, &input.run_id, "/jobs")?,
        &[],
        3,
    )
    .await?;
    let mut jobs: Vec<Job> = rows.into_iter().map(Job::from_row).collect();
    jobs.sort_by_key(|job| job.id.parse::<u64>().unwrap_or(0));
    Ok(RunDetail {
        run: Run::from_row(&row, title.as_deref()),
        jobs,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub run_duration_ms: Option<u64>,
    pub billable: Vec<Value>,
}

pub async fn timing(data_dir: &Path, input: RunRef) -> GitlabResult<Timing> {
    let row = pipeline(data_dir, &input.repo, &input.run_id).await?;
    Ok(Timing {
        run_duration_ms: row.duration.map(|secs| (secs * 1000.0) as u64),
        billable: Vec::new(),
    })
}

pub async fn cancel(data_dir: &Path, input: RunRef) -> GitlabResult<()> {
    client::write(
        data_dir,
        Method::POST,
        &pipeline_path(&input.repo, &input.run_id, "/cancel")?,
        None,
    )
    .await
}

pub async fn delete(data_dir: &Path, input: RunRef) -> GitlabResult<()> {
    client::write(
        data_dir,
        Method::DELETE,
        &pipeline_path(&input.repo, &input.run_id, "")?,
        None,
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rerun {
    #[serde(flatten)]
    pub run: RunRef,
    #[serde(default)]
    pub failed_only: bool,
}

/// Re-running the failed jobs is GitLab's Retry. Running everything again
/// starts a new pipeline on the same branch, or for a merge request, on it.
pub async fn rerun(data_dir: &Path, input: Rerun) -> GitlabResult<()> {
    let repo = &input.run.repo;
    if input.failed_only {
        return client::write(
            data_dir,
            Method::POST,
            &pipeline_path(repo, &input.run.run_id, "/retry")?,
            None,
        )
        .await;
    }
    let row = pipeline(data_dir, repo, &input.run.run_id).await?;
    match merge_request_of(&row.git_ref) {
        Some(number) => {
            client::write(
                data_dir,
                Method::POST,
                &repo.path(&format!("/merge_requests/{number}/pipelines"))?,
                None,
            )
            .await
        }
        None => {
            let _: Value = client::send_json(
                data_dir,
                Method::POST,
                &repo.path("/pipeline")?,
                &json!({ "ref": row.git_ref }),
            )
            .await?;
            Ok(())
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub job_id: String,
}

pub async fn rerun_job(data_dir: &Path, input: JobRef) -> GitlabResult<()> {
    let path = input
        .repo
        .path(&format!("/jobs/{}/retry", number(&input.job_id, "job")?))?;
    let _: Value = client::send_json(data_dir, Method::POST, &path, &json!({})).await?;
    Ok(())
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
    pub expired: bool,
    pub truncated: bool,
}

/// A log line as a person reads it: without colour codes, without GitLab's
/// folding markers, and with only what was last written over a `\r`.
pub fn readable(line: &str) -> String {
    let shown = line
        .rsplit('\r')
        .find(|part| !part.trim().is_empty())
        .unwrap_or_default();
    let mut out = String::with_capacity(shown.len());
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    let before = out.split("section_start:").next().unwrap_or_default();
    before
        .split("section_end:")
        .next()
        .unwrap_or_default()
        .to_string()
}

pub fn lines_of(text: &str, truncated: bool) -> Vec<LogLine> {
    let text = if truncated {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        text
    };
    text.lines()
        .map(readable)
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, text)| LogLine {
            number: index as u64 + 1,
            timestamp: None,
            text,
        })
        .collect()
}

/// A job that has not started yet has no log, which reads as an empty one.
pub async fn log(data_dir: &Path, input: JobRef) -> GitlabResult<JobLog> {
    let path = input
        .repo
        .path(&format!("/jobs/{}/trace", number(&input.job_id, "job")?))?;
    let answer =
        client::send_limited(data_dir, Method::GET, &path, &[], None, MAX_LOG_BYTES, true).await?;
    if answer.status.as_u16() == 404 {
        return Ok(JobLog {
            lines: Vec::new(),
            expired: false,
            truncated: false,
        });
    }
    if !answer.status.is_success() {
        return Err(client::classify(answer.status, &answer.bytes));
    }
    Ok(JobLog {
        lines: lines_of(&String::from_utf8_lossy(&answer.bytes), answer.cut),
        expired: false,
        truncated: answer.cut,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    #[serde(flatten)]
    pub job: JobRef,
    pub tail: Option<usize>,
    pub grep: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogExcerpt {
    pub lines: Vec<LogLine>,
    pub truncated: bool,
}

const DEFAULT_TAIL: usize = 200;
const MAX_TAIL: usize = 2000;

/// The end of a job's log for an agent, optionally only the lines that mention something.
pub async fn excerpt(data_dir: &Path, input: Excerpt) -> GitlabResult<LogExcerpt> {
    let whole = log(data_dir, input.job).await?;
    let needle = input
        .grep
        .map(|grep| grep.to_lowercase())
        .filter(|grep| !grep.is_empty());
    let mut lines: Vec<LogLine> = whole
        .lines
        .into_iter()
        .filter(|line| {
            needle
                .as_deref()
                .is_none_or(|needle| line.text.to_lowercase().contains(needle))
        })
        .collect();
    let tail = input.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    let cut = lines.len() > tail;
    if cut {
        lines.drain(..lines.len() - tail);
    }
    Ok(LogExcerpt {
        lines,
        truncated: cut || whole.truncated,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub state: String,
    pub active: bool,
    pub url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    #[serde(flatten)]
    pub repo: RepoRef,
}

async fn file(data_dir: &Path, repo: &RepoRef) -> GitlabResult<String> {
    let branch = repo::default_branch(data_dir, repo).await?;
    client::get_text(
        data_dir,
        &repo.path(&format!("/repository/files/{}/raw", encoded(PIPELINE_FILE)))?,
        &[("ref", branch)],
    )
    .await
}

/// Asks for the file's headers only, so a large one is not downloaded just to see it is there.
async fn has_file(data_dir: &Path, repo: &RepoRef) -> GitlabResult<bool> {
    let branch = repo::default_branch(data_dir, repo).await?;
    let answer = client::send_limited(
        data_dir,
        Method::HEAD,
        &repo.path(&format!("/repository/files/{}", encoded(PIPELINE_FILE)))?,
        &[("ref", branch)],
        None,
        64 * 1024,
        false,
    )
    .await?;
    match answer.status {
        status if status.is_success() => Ok(true),
        StatusCode::NOT_FOUND => Ok(false),
        status => Err(client::classify(status, &answer.bytes)),
    }
}

/// The one pipeline a project defines, when it has a .gitlab-ci.yml.
pub async fn workflows(data_dir: &Path, repo: RepoRef) -> GitlabResult<Vec<Workflow>> {
    match has_file(data_dir, &repo).await {
        Ok(true) => {}
        Ok(false) | Err(GitlabError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    Ok(vec![Workflow {
        id: WORKFLOW_ID.into(),
        name: "Pipeline".into(),
        path: PIPELINE_FILE.into(),
        state: "active".into(),
        active: true,
        url: String::new(),
    }])
}

#[derive(Serialize)]
pub struct WorkflowFile {
    pub path: String,
    pub text: String,
}

pub async fn workflow_file(data_dir: &Path, input: FileRef) -> GitlabResult<WorkflowFile> {
    Ok(WorkflowFile {
        path: PIPELINE_FILE.into(),
        text: file(data_dir, &input.repo).await?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub git_ref: String,
    #[serde(default)]
    pub inputs: Map<String, Value>,
}

/// A pipeline started by hand: the branch, and the inputs as CI variables.
pub fn dispatch_body(input: &Dispatch) -> GitlabResult<Value> {
    let git_ref = input.git_ref.trim();
    if git_ref.is_empty() {
        return Err(GitlabError::BadArg("a branch to run on is needed".into()));
    }
    let variables: Vec<Value> = input
        .inputs
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_string);
            json!({ "key": key, "value": value, "variable_type": "env_var" })
        })
        .collect();
    Ok(json!({ "ref": git_ref, "variables": variables }))
}

pub async fn dispatch(data_dir: &Path, input: Dispatch) -> GitlabResult<()> {
    let body = dispatch_body(&input)?;
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/pipeline")?,
        &body,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(value: Value) -> PipelineRow {
        serde_json::from_value(value).expect("pipeline parses")
    }

    #[test]
    fn statuses_read_in_the_words_every_host_uses() {
        let pair = |status: &str, allow: bool| {
            let (status, conclusion) = status_of(status, allow);
            format!("{status}/{}", conclusion.unwrap_or_default())
        };
        assert_eq!(pair("success", false), "completed/success");
        assert_eq!(pair("failed", false), "completed/failure");
        assert_eq!(pair("failed", true), "completed/neutral");
        assert_eq!(pair("canceled", false), "completed/cancelled");
        assert_eq!(pair("running", false), "in_progress/");
        assert_eq!(pair("manual", false), "waiting/");
        assert_eq!(pair("waiting_for_resource", false), "queued/");
    }

    #[test]
    fn a_pipeline_reads_as_a_run_titled_by_its_commit() {
        let pipeline = row(json!({
            "id": 1834, "iid": 212, "sha": "9c41e2a0b1c2", "ref": "main", "status": "running", "source": "push",
            "created_at": "2026-10-09T08:00:00Z", "updated_at": "2026-10-09T08:03:00Z",
            "web_url": "https://gitlab.com/acme/api/-/pipelines/1834",
            "user": { "id": 3, "username": "ankit", "avatar_url": "https://gitlab.com/a.png" }
        }));
        let run = Run::from_row(&pipeline, Some("fix(invoices): round VAT per line"));
        assert_eq!((run.id.as_str(), run.run_number), ("1834", 212));
        assert_eq!(run.title, "fix(invoices): round VAT per line");
        assert_eq!(
            (run.status.as_str(), run.event.as_str()),
            ("in_progress", "push")
        );
        assert_eq!(run.branch.as_deref(), Some("main"));
        assert_eq!(run.short_sha, "9c41e2a0");
        assert_eq!(run.actor.as_deref(), Some("ankit"));
    }

    #[test]
    fn a_merge_request_pipeline_names_its_merge_request() {
        let pipeline = row(json!({
            "id": 9, "sha": "", "ref": "refs/merge-requests/42/head", "status": "success", "source": "merge_request_event",
            "created_at": "2026-10-09T08:00:00Z", "web_url": "https://gitlab.com/acme/api/-/pipelines/9"
        }));
        let run = Run::from_row(&pipeline, None);
        assert_eq!(run.pull_requests, [42]);
        assert_eq!(run.branch, None);
        assert_eq!(run.title, "Merge request !42");
        assert_eq!(
            (run.event.as_str(), run.conclusion.as_deref()),
            ("pull_request", Some("success"))
        );
    }

    #[test]
    fn jobs_are_named_by_stage_so_the_graph_groups_them() {
        let job: JobRow = serde_json::from_value(json!({
            "id": 77, "name": "unit", "stage": "test", "status": "failed", "allow_failure": true,
            "started_at": "a", "finished_at": "b", "runner": { "description": "shared-runner-3" }, "web_url": "https://x"
        }))
        .expect("job parses");
        let job = Job::from_row(job);
        assert_eq!(job.name, "test / unit");
        assert_eq!(job.conclusion.as_deref(), Some("neutral"));
        assert_eq!(job.runner.as_deref(), Some("shared-runner-3"));
    }

    #[test]
    fn a_log_line_loses_colours_folding_markers_and_overwritten_text() {
        assert_eq!(readable("\u{1b}[0Ksection_start:1696800000:step_script\r\u{1b}[0K\u{1b}[36;1mExecuting \"step_script\"\u{1b}[0;m"), "Executing \"step_script\"");
        assert_eq!(
            readable("\u{1b}[0Ksection_end:1696800000:step_script\r\u{1b}[0K"),
            ""
        );
        assert_eq!(
            readable("Downloading 10%\rDownloading 100%"),
            "Downloading 100%"
        );
        assert_eq!(
            readable("\u{1b}[32;1mJob succeeded\u{1b}[0;m"),
            "Job succeeded"
        );
        let lines = lines_of("one\n\u{1b}[0Ksection_end:1:x\r\u{1b}[0K\ntwo\n", false);
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
    }
}
