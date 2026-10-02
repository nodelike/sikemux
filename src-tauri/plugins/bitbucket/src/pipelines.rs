// Pipelines, in the words the Git pane uses for every host's CI: a pipeline is
// a run, its steps are jobs, and each pipeline the repository's
// bitbucket-pipelines.yml defines is a workflow.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};
use crate::repo::{self, avatar_of, login_of, uuid_segment, RepoRef, User};

pub const PIPELINES_FILE: &str = "bitbucket-pipelines.yml";
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize, Clone)]
struct Named {
    name: String,
}

#[derive(Deserialize, Clone)]
struct State {
    name: String,
    result: Option<Named>,
    stage: Option<Named>,
}

/// The GitHub words every host maps onto: a status, and once completed, a conclusion.
fn status_of(state: Option<&State>) -> (String, Option<String>) {
    let Some(state) = state else {
        return ("queued".into(), None);
    };
    let paused = state
        .stage
        .as_ref()
        .is_some_and(|stage| matches!(stage.name.as_str(), "PAUSED" | "HALTED"));
    match state.name.as_str() {
        "COMPLETED" => {
            let conclusion = match state.result.as_ref().map(|result| result.name.as_str()) {
                Some("SUCCESSFUL") => "success",
                Some("FAILED" | "ERROR") => "failure",
                Some("STOPPED") => "cancelled",
                Some("EXPIRED") => "timed_out",
                Some("NOT_RUN" | "SKIPPED") => "skipped",
                _ => "neutral",
            };
            ("completed".into(), Some(conclusion.into()))
        }
        "PAUSED" | "HALTED" => ("waiting".into(), None),
        "IN_PROGRESS" | "RUNNING" if paused => ("waiting".into(), None),
        "IN_PROGRESS" | "RUNNING" => ("in_progress".into(), None),
        "NOT_RUN" => ("completed".into(), Some("skipped".into())),
        _ => ("queued".into(), None),
    }
}

#[derive(Deserialize)]
struct Trigger {
    name: Option<String>,
}

#[derive(Deserialize)]
struct PipelineRow {
    uuid: String,
    build_number: u64,
    creator: Option<User>,
    #[serde(default)]
    target: Value,
    trigger: Option<Trigger>,
    state: Option<State>,
    created_on: String,
    completed_on: Option<String>,
    run_number: Option<u64>,
    duration_in_seconds: Option<u64>,
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |value, key| value.get(key))?
        .as_str()
        .filter(|text| !text.is_empty())
}

/// Which pipeline in the file a run comes from, as `kind:pattern`.
fn workflow_of(target: &Value) -> (String, String) {
    let kind = text(target, &["selector", "type"]).unwrap_or("default");
    let pattern = text(target, &["selector", "pattern"]);
    let id = format!("{kind}:{}", pattern.unwrap_or_default());
    let name = match (kind, pattern) {
        ("default", _) | (_, None) => kind.to_string(),
        (_, Some(pattern)) => pattern.to_string(),
    };
    (id, name)
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
    fn from_row(repo: &RepoRef, row: &PipelineRow) -> Self {
        let target = &row.target;
        let (workflow_id, name) = workflow_of(target);
        let sha = text(target, &["commit", "hash"])
            .unwrap_or_default()
            .to_string();
        let pull = target
            .get("pullrequest")
            .and_then(|pull| pull.get("id"))
            .and_then(Value::as_u64);
        let branch = text(target, &["source"])
            .or_else(|| text(target, &["ref_name"]))
            .map(str::to_string);
        let event = match (
            pull,
            row.trigger
                .as_ref()
                .and_then(|trigger| trigger.name.as_deref()),
        ) {
            (Some(_), _) => "pull_request",
            (None, Some("MANUAL")) => "manual",
            (None, Some("SCHEDULE")) => "schedule",
            _ => "push",
        };
        let message = text(target, &["commit", "message"])
            .and_then(|message| message.lines().next())
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let title = match (message, pull, &branch) {
            (Some(line), _, _) => line.to_string(),
            (None, Some(number), _) => format!("Pull request #{number}"),
            (None, None, Some(branch)) => branch.clone(),
            (None, None, None) => sha.chars().take(7).collect(),
        };
        let (status, conclusion) = status_of(row.state.as_ref());
        Run {
            id: row.uuid.clone(),
            name,
            title,
            workflow_id,
            path: Some(PIPELINES_FILE.into()),
            run_number: row.build_number,
            attempt: row.run_number.unwrap_or(1),
            event: event.into(),
            status,
            conclusion,
            branch,
            short_sha: sha.chars().take(7).collect(),
            sha,
            actor: login_of(row.creator.as_ref()),
            avatar_url: avatar_of(row.creator.as_ref()),
            created_at: row.created_on.clone(),
            started_at: Some(row.created_on.clone()),
            updated_at: row
                .completed_on
                .clone()
                .unwrap_or_else(|| row.created_on.clone()),
            pull_requests: pull.into_iter().collect(),
            url: repo.web(&format!("/pipelines/results/{}", row.build_number)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub workflow_id: Option<String>,
    pub branch: Option<String>,
    pub status: Option<String>,
    pub event: Option<String>,
    pub actor: Option<String>,
    pub head_sha: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

impl RunQuery {
    /// Bitbucket filters by few of these itself, so every one is also
    /// applied here to the page that came back.
    fn keeps(&self, run: &Run) -> bool {
        let same = |wanted: &Option<String>, actual: Option<&str>| {
            wanted
                .as_deref()
                .is_none_or(|wanted| Some(wanted) == actual)
        };
        same(&self.workflow_id, Some(&run.workflow_id))
            && same(&self.branch, run.branch.as_deref())
            && same(&self.event, Some(&run.event))
            && same(&self.actor, run.actor.as_deref())
            && self.status.as_deref().is_none_or(|status| {
                run.status == status || run.conclusion.as_deref() == Some(status)
            })
            && self.head_sha.as_deref().is_none_or(|sha| {
                !run.sha.is_empty() && (run.sha.starts_with(sha) || sha.starts_with(&run.sha))
            })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: u64,
    pub next_page: Option<u32>,
}

pub async fn list(data_dir: &Path, input: RunQuery) -> BitbucketResult<RunPage> {
    let page = input.page.unwrap_or(1).max(1);
    let per_page = if input.head_sha.is_some() {
        100
    } else {
        input.per_page.unwrap_or(30).clamp(1, 100)
    };
    let mut query = vec![
        ("sort", "-created_on".to_string()),
        ("page", page.to_string()),
        ("pagelen", per_page.to_string()),
        // The list leaves out the commit's message unless asked, and it is what a run is titled by.
        ("fields", "+values.target.commit.message".to_string()),
    ];
    if let Some(branch) = &input.branch {
        query.push(("target.branch", branch.clone()));
    }
    let found: client::Page<PipelineRow> =
        client::get(data_dir, &input.repo.path("/pipelines/")?, &query).await?;
    let runs: Vec<Run> = found
        .values
        .iter()
        .map(|row| Run::from_row(&input.repo, row))
        .filter(|run| input.keeps(run))
        .collect();
    Ok(RunPage {
        total: found.size.unwrap_or(runs.len() as u64),
        next_page: found.next.is_some().then_some(page + 1),
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

fn pipeline_path(repo: &RepoRef, run_id: &str, rest: &str) -> BitbucketResult<String> {
    repo.path(&format!("/pipelines/{}{rest}", uuid_segment(run_id)?))
}

async fn pipeline(data_dir: &Path, repo: &RepoRef, run_id: &str) -> BitbucketResult<PipelineRow> {
    client::get(data_dir, &pipeline_path(repo, run_id, "")?, &[]).await
}

#[derive(Deserialize)]
struct StepRow {
    uuid: String,
    name: Option<String>,
    state: Option<State>,
    started_on: Option<String>,
    completed_on: Option<String>,
    image: Option<Named>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// The pipeline and the step, `pipeline:step`, since Bitbucket reaches a
    /// step's log through both.
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

fn job_id(run_id: &str, step_id: &str) -> String {
    format!("{run_id}:{step_id}")
}

fn split_job(job_id: &str) -> BitbucketResult<(&str, &str)> {
    job_id
        .split_once(':')
        .ok_or_else(|| BitbucketError::BadArg(format!("`{job_id}` is not a Bitbucket step")))
}

async fn steps(data_dir: &Path, repo: &RepoRef, run: &Run) -> BitbucketResult<Vec<Job>> {
    let rows: Vec<StepRow> = client::get_all(
        data_dir,
        &pipeline_path(repo, &run.id, "/steps/")?,
        &[("pagelen", "100".into())],
        3,
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let (status, conclusion) = status_of(row.state.as_ref());
            Job {
                id: job_id(&run.id, &row.uuid),
                name: row.name.unwrap_or_else(|| "Step".into()),
                status,
                conclusion,
                started_at: row.started_on,
                completed_at: row.completed_on,
                runner: row.image.map(|image| image.name),
                url: Some(format!("{}/steps/{}", run.url, row.uuid)),
                check_run_id: None,
                steps: Vec::new(),
            }
        })
        .collect())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
}

pub async fn detail(data_dir: &Path, input: RunRef) -> BitbucketResult<RunDetail> {
    let row = pipeline(data_dir, &input.repo, &input.run_id).await?;
    let run = Run::from_row(&input.repo, &row);
    let jobs = steps(data_dir, &input.repo, &run).await?;
    Ok(RunDetail { run, jobs })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub run_duration_ms: Option<u64>,
    pub billable: Vec<Value>,
}

pub async fn timing(data_dir: &Path, input: RunRef) -> BitbucketResult<Timing> {
    let row = pipeline(data_dir, &input.repo, &input.run_id).await?;
    Ok(Timing {
        run_duration_ms: row.duration_in_seconds.map(|secs| secs * 1000),
        billable: Vec::new(),
    })
}

pub async fn cancel(data_dir: &Path, input: RunRef) -> BitbucketResult<()> {
    let path = pipeline_path(&input.repo, &input.run_id, "/stopPipeline")?;
    client::post_empty(data_dir, &path, None).await
}

/// The parts of a pipeline's target that say what to build, without the
/// links and ids Bitbucket adds when it answers.
fn target_to_repeat(target: &Value) -> Value {
    let mut kept = Map::new();
    for key in [
        "type",
        "ref_type",
        "ref_name",
        "selector",
        "source",
        "destination",
        "destination_commit",
        "pullrequest",
    ] {
        if let Some(value) = target.get(key).filter(|value| !value.is_null()) {
            kept.insert(key.into(), value.clone());
        }
    }
    if let Some(hash) = text(target, &["commit", "hash"]) {
        kept.insert("commit".into(), json!({ "type": "commit", "hash": hash }));
    }
    if let Some(Value::Object(pull)) = kept.get_mut("pullrequest") {
        pull.retain(|key, _| key == "id");
    }
    Value::Object(kept)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rerun {
    #[serde(flatten)]
    pub run: RunRef,
    #[serde(default)]
    pub failed_only: bool,
}

/// Bitbucket runs a pipeline again as a new one on the same commit.
pub async fn rerun(data_dir: &Path, input: Rerun) -> BitbucketResult<()> {
    if input.failed_only {
        return Err(BitbucketError::Unsupported("re-run only the failed steps"));
    }
    let row = pipeline(data_dir, &input.run.repo, &input.run.run_id).await?;
    let body = json!({ "target": target_to_repeat(&row.target) });
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &input.run.repo.path("/pipelines/")?,
        &body,
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub job_id: String,
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

fn lines_of(text: &str, truncated: bool) -> Vec<LogLine> {
    let text = if truncated {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        text
    };
    text.lines()
        .enumerate()
        .map(|(index, line)| LogLine {
            number: index as u64 + 1,
            timestamp: None,
            text: line.trim_end_matches('\r').to_string(),
        })
        .collect()
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

/// The end of a step's log for an agent, optionally only the lines that mention something.
pub async fn excerpt(data_dir: &Path, input: Excerpt) -> BitbucketResult<LogExcerpt> {
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

/// A step that has not started yet has no log, which reads as an empty one.
pub async fn log(data_dir: &Path, input: JobRef) -> BitbucketResult<JobLog> {
    let (run_id, step_id) = split_job(&input.job_id)?;
    let path = pipeline_path(
        &input.repo,
        run_id,
        &format!("/steps/{}/log", uuid_segment(step_id)?),
    )?;
    let (status, bytes, truncated) =
        client::send_limited(data_dir, Method::GET, &path, &[], None, MAX_LOG_BYTES, true).await?;
    if status.as_u16() == 404 {
        return Ok(JobLog {
            lines: Vec::new(),
            expired: false,
            truncated: false,
        });
    }
    if !status.is_success() {
        return Err(client::classify(status, &bytes));
    }
    Ok(JobLog {
        lines: lines_of(&String::from_utf8_lossy(&bytes), truncated),
        expired: false,
        truncated,
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

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The key of a `key:` line, without quotes; nothing for a list item or a value.
fn key_of(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('-') || trimmed.starts_with('#') {
        return None;
    }
    let (key, rest) = if let Some(quoted) = trimmed.strip_prefix(['\'', '"']) {
        let quote = trimmed.chars().next()?;
        let (key, rest) = quoted.split_once(quote)?;
        (key, rest.trim_start().strip_prefix(':')?)
    } else {
        let (key, rest) = trimmed.split_once(':')?;
        (key.trim(), rest)
    };
    let rest = rest.trim();
    (rest.is_empty() || rest.starts_with('#') || rest.starts_with('&')).then(|| key.to_string())
}

/// Every pipeline under `pipelines:`, as `kind:pattern`. A light reading of
/// the file that relies only on its indentation, which is all this needs.
fn pipelines_in(file: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = file
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .collect();
    let Some(start) = lines
        .iter()
        .position(|line| indent_of(line) == 0 && key_of(line).as_deref() == Some("pipelines"))
    else {
        return Vec::new();
    };
    let body: Vec<&str> = lines
        .iter()
        .skip(start + 1)
        .take_while(|line| indent_of(line) > 0)
        .copied()
        .collect();
    let Some(section_indent) = body.first().map(|line| indent_of(line)) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut kind: Option<String> = None;
    let mut child_indent = None;
    for line in body {
        let indent = indent_of(line);
        if indent == section_indent {
            kind = key_of(line);
            child_indent = None;
            if kind.as_deref() == Some("default") {
                found.push(("default".into(), String::new()));
            }
            continue;
        }
        let Some(kind) = kind.as_deref().filter(|kind| *kind != "default") else {
            continue;
        };
        let expected = *child_indent.get_or_insert(indent);
        if indent == expected {
            if let Some(pattern) = key_of(line) {
                found.push((kind.to_string(), pattern));
            }
        }
    }
    found
}

pub async fn file(data_dir: &Path, repo: &RepoRef) -> BitbucketResult<String> {
    let branch = repo::default_branch(data_dir, repo).await?;
    client::get_text(
        data_dir,
        &repo.path(&format!("/src/{branch}/{PIPELINES_FILE}"))?,
        &[],
    )
    .await
}

pub async fn workflows(data_dir: &Path, repo: RepoRef) -> BitbucketResult<Vec<Workflow>> {
    let text = match file(data_dir, &repo).await {
        Ok(text) => text,
        Err(BitbucketError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let url = repo.web(&format!("/src/HEAD/{PIPELINES_FILE}"));
    Ok(pipelines_in(&text)
        .into_iter()
        .map(|(kind, pattern)| Workflow {
            id: format!("{kind}:{pattern}"),
            name: match kind.as_str() {
                "default" => "default".into(),
                "custom" => pattern,
                _ => format!("{kind}: {pattern}"),
            },
            path: PIPELINES_FILE.into(),
            state: "active".into(),
            active: true,
            url: url.clone(),
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    #[serde(flatten)]
    pub repo: RepoRef,
}

#[derive(Serialize)]
pub struct WorkflowFile {
    pub path: String,
    pub text: String,
}

pub async fn workflow_file(data_dir: &Path, input: FileRef) -> BitbucketResult<WorkflowFile> {
    Ok(WorkflowFile {
        path: PIPELINES_FILE.into(),
        text: file(data_dir, &input.repo).await?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub workflow_id: String,
    pub git_ref: String,
    #[serde(default)]
    pub inputs: Map<String, Value>,
}

fn dispatch_body(input: &Dispatch) -> BitbucketResult<Value> {
    let (kind, pattern) = input
        .workflow_id
        .split_once(':')
        .ok_or_else(|| BitbucketError::BadArg("that is not a pipeline in the file".into()))?;
    let git_ref = input.git_ref.trim();
    if git_ref.is_empty() {
        return Err(BitbucketError::BadArg(
            "a branch to run on is needed".into(),
        ));
    }
    let mut target = Map::new();
    target.insert("type".into(), json!("pipeline_ref_target"));
    target.insert("ref_type".into(), json!("branch"));
    target.insert("ref_name".into(), json!(git_ref));
    if kind != "default" {
        target.insert(
            "selector".into(),
            json!({ "type": kind, "pattern": pattern }),
        );
    }
    let variables: Vec<Value> = input
        .inputs
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_string);
            json!({ "key": key, "value": value })
        })
        .collect();
    Ok(json!({ "target": target, "variables": variables }))
}

pub async fn dispatch(data_dir: &Path, input: Dispatch) -> BitbucketResult<()> {
    let body = dispatch_body(&input)?;
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/pipelines/")?,
        &body,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef {
            owner: "swishx".into(),
            name: "api".into(),
        }
    }

    fn state(value: Value) -> State {
        serde_json::from_value(value).expect("a state")
    }

    #[test]
    fn states_use_the_words_every_host_shares() {
        let cases = [
            (json!({ "name": "PENDING" }), ("queued", None)),
            (
                json!({ "name": "IN_PROGRESS", "stage": { "name": "RUNNING" } }),
                ("in_progress", None),
            ),
            (
                json!({ "name": "IN_PROGRESS", "stage": { "name": "PAUSED" } }),
                ("waiting", None),
            ),
            (
                json!({ "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }),
                ("completed", Some("success")),
            ),
            (
                json!({ "name": "COMPLETED", "result": { "name": "FAILED" } }),
                ("completed", Some("failure")),
            ),
            (
                json!({ "name": "COMPLETED", "result": { "name": "ERROR" } }),
                ("completed", Some("failure")),
            ),
            (
                json!({ "name": "COMPLETED", "result": { "name": "STOPPED" } }),
                ("completed", Some("cancelled")),
            ),
            (
                json!({ "name": "COMPLETED", "result": { "name": "NOT_RUN" } }),
                ("completed", Some("skipped")),
            ),
        ];
        for (raw, (status, conclusion)) in cases {
            let (got_status, got_conclusion) = status_of(Some(&state(raw.clone())));
            assert_eq!(
                (got_status.as_str(), got_conclusion.as_deref()),
                (status, conclusion),
                "{raw}"
            );
        }
    }

    fn row(value: Value) -> PipelineRow {
        serde_json::from_value(value).expect("a pipeline row")
    }

    #[test]
    fn a_custom_pipeline_on_a_branch_reads_as_a_manual_run_of_that_workflow() {
        let run = Run::from_row(
            &repo(),
            &row(json!({
                "uuid": "{aaaa-1}", "build_number": 42, "created_on": "t0",
                "trigger": { "name": "MANUAL" },
                "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } },
                "creator": { "nickname": "ada" },
                "target": {
                    "type": "pipeline_ref_target", "ref_type": "branch", "ref_name": "main",
                    "commit": { "hash": "0123456789abcdef0123456789abcdef01234567" },
                    "selector": { "type": "custom", "pattern": "deploy-prod" }
                }
            })),
        );
        assert_eq!(run.id, "{aaaa-1}");
        assert_eq!(run.workflow_id, "custom:deploy-prod");
        assert_eq!(run.name, "deploy-prod");
        assert_eq!(run.event, "manual");
        assert_eq!(run.branch.as_deref(), Some("main"));
        assert_eq!(run.short_sha, "0123456");
        assert_eq!(run.conclusion.as_deref(), Some("success"));
        assert_eq!(
            run.url,
            "https://bitbucket.org/swishx/api/pipelines/results/42"
        );
    }

    #[test]
    fn a_pull_request_pipeline_names_its_pull_request_and_source_branch() {
        let run = Run::from_row(
            &repo(),
            &row(json!({
                "uuid": "{b}", "build_number": 7, "created_on": "t0",
                "target": {
                    "type": "pipeline_pullrequest_target", "source": "fix", "destination": "main",
                    "commit": { "hash": "abc" }, "pullrequest": { "id": 12 },
                    "selector": { "type": "pull-requests", "pattern": "**" }
                }
            })),
        );
        assert_eq!(run.event, "pull_request");
        assert_eq!(run.pull_requests, vec![12]);
        assert_eq!(run.branch.as_deref(), Some("fix"));
        assert_eq!(run.title, "Pull request #12");
        assert_eq!(run.workflow_id, "pull-requests:**");
    }

    #[test]
    fn a_pipeline_is_titled_by_its_commit_message_first_line() {
        let run = Run::from_row(
            &repo(),
            &row(json!({
                "uuid": "{d}", "build_number": 8, "created_on": "t0",
                "target": {
                    "type": "pipeline_pullrequest_target", "source": "fix", "destination": "main",
                    "commit": { "hash": "abc", "message": "build(dossier): gate the baseline\n\nWith a body." },
                    "pullrequest": { "id": 12 }
                }
            })),
        );
        assert_eq!(run.title, "build(dossier): gate the baseline");
    }

    #[test]
    fn filters_bitbucket_ignores_are_applied_to_the_page() {
        let run = Run::from_row(
            &repo(),
            &row(json!({
                "uuid": "{c}", "build_number": 1, "created_on": "t0",
                "state": { "name": "COMPLETED", "result": { "name": "FAILED" } },
                "target": { "ref_name": "main", "commit": { "hash": "abcdef1234" } }
            })),
        );
        let query = |value: Value| -> RunQuery {
            let mut value = value;
            value["owner"] = json!("swishx");
            value["name"] = json!("api");
            serde_json::from_value(value).expect("a query")
        };
        assert!(query(json!({ "status": "failure" })).keeps(&run));
        assert!(query(json!({ "status": "completed" })).keeps(&run));
        assert!(!query(json!({ "status": "success" })).keeps(&run));
        assert!(query(json!({ "headSha": "abcdef1234ffff" })).keeps(&run));
        assert!(!query(json!({ "headSha": "999" })).keeps(&run));
        assert!(query(json!({ "workflowId": "default:" })).keeps(&run));
        assert!(!query(json!({ "branch": "dev" })).keeps(&run));
    }

    #[test]
    fn a_rerun_asks_for_the_same_build_without_what_bitbucket_added() {
        let target = json!({
            "type": "pipeline_pullrequest_target", "source": "fix", "destination": "main",
            "destination_commit": { "hash": "d" },
            "commit": { "type": "commit", "hash": "abc", "links": { "self": { "href": "x" } } },
            "pullrequest": { "id": 12, "title": "t", "links": {} },
            "selector": { "type": "pull-requests", "pattern": "**" }
        });
        assert_eq!(
            target_to_repeat(&target),
            json!({
                "type": "pipeline_pullrequest_target", "source": "fix", "destination": "main",
                "destination_commit": { "hash": "d" },
                "commit": { "type": "commit", "hash": "abc" },
                "pullrequest": { "id": 12 },
                "selector": { "type": "pull-requests", "pattern": "**" }
            })
        );
    }

    #[test]
    fn a_step_is_named_by_its_pipeline_and_itself() -> BitbucketResult<()> {
        let id = job_id("{p}", "{s}");
        assert_eq!(split_job(&id)?, ("{p}", "{s}"));
        assert!(split_job("{p}").is_err());
        Ok(())
    }

    #[test]
    fn a_log_cut_to_its_end_drops_the_partial_first_line() {
        let whole = lines_of("one\r\ntwo\n", false);
        assert_eq!(
            whole
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        let cut = lines_of("ne\ntwo\nthree", true);
        assert_eq!(
            cut.iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(cut[0].number, 1);
    }

    #[test]
    fn reads_each_pipeline_the_file_defines() {
        let file = r#"
image: node:20
definitions:
  steps:
    - step: &build
        name: Build
pipelines:
  default:
    - step: *build
  branches:
    main:
      - step: *build
    'release/*':
      - step:
          name: Release
  pull-requests:
    "**":
      - step: *build
  custom:
    deploy-prod: # by hand only
      - variables:
          - name: ENV
      - step:
          name: Deploy
    nightly:
      - step: *build
"#;
        assert_eq!(
            pipelines_in(file),
            vec![
                ("default".to_string(), String::new()),
                ("branches".to_string(), "main".to_string()),
                ("branches".to_string(), "release/*".to_string()),
                ("pull-requests".to_string(), "**".to_string()),
                ("custom".to_string(), "deploy-prod".to_string()),
                ("custom".to_string(), "nightly".to_string()),
            ]
        );
        assert!(pipelines_in("image: node\n").is_empty());
    }

    #[test]
    fn starting_a_custom_pipeline_passes_its_variables() -> BitbucketResult<()> {
        let input: Dispatch = serde_json::from_value(json!({
            "owner": "swishx", "name": "api", "workflowId": "custom:deploy-prod",
            "gitRef": "main", "inputs": { "ENV": "prod" }
        }))?;
        assert_eq!(
            dispatch_body(&input)?,
            json!({
                "target": {
                    "type": "pipeline_ref_target", "ref_type": "branch", "ref_name": "main",
                    "selector": { "type": "custom", "pattern": "deploy-prod" }
                },
                "variables": [{ "key": "ENV", "value": "prod" }]
            })
        );
        let default: Dispatch = serde_json::from_value(json!({
            "owner": "swishx", "name": "api", "workflowId": "default:", "gitRef": "main"
        }))?;
        assert!(dispatch_body(&default)?["target"].get("selector").is_none());
        Ok(())
    }
}
