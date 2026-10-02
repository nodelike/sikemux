// Workflow runs, the jobs inside one, and the buttons that re-run or stop
// them. Times are passed through as GitHub wrote them, so whoever shows them
// decides how to phrase "3 minutes ago".

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client;
use crate::common::LIST_PAGES;
use crate::error::{GithubError, GithubResult};
use crate::workflows::RepoRef;

const DEFAULT_PER_PAGE: u32 = 30;
const MAX_PER_PAGE: u32 = 100;

#[derive(Deserialize)]
struct Actor {
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct PullRequestRef {
    number: u64,
}

#[derive(Deserialize)]
struct RunRow {
    id: u64,
    name: Option<String>,
    display_title: Option<String>,
    workflow_id: u64,
    path: Option<String>,
    run_number: u64,
    run_attempt: Option<u64>,
    event: String,
    status: Option<String>,
    conclusion: Option<String>,
    head_branch: Option<String>,
    head_sha: String,
    html_url: String,
    created_at: String,
    updated_at: String,
    run_started_at: Option<String>,
    actor: Option<Actor>,
    triggering_actor: Option<Actor>,
    #[serde(default)]
    pull_requests: Vec<PullRequestRef>,
}

#[derive(Deserialize)]
struct RunList {
    total_count: u64,
    workflow_runs: Vec<RunRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: u64,
    pub name: String,
    pub title: String,
    pub workflow_id: u64,
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

/// A run is over once GitHub says it is completed; everything else is still moving.
pub fn is_finished(status: &str) -> bool {
    status == "completed"
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

impl From<RunRow> for Run {
    fn from(row: RunRow) -> Self {
        let actor = row.triggering_actor.or(row.actor);
        Self {
            title: row
                .display_title
                .clone()
                .or_else(|| row.name.clone())
                .unwrap_or_default(),
            name: row.name.unwrap_or_default(),
            short_sha: short(&row.head_sha),
            id: row.id,
            workflow_id: row.workflow_id,
            path: row
                .path
                .map(|path| path.split('@').next().unwrap_or_default().to_string()),
            run_number: row.run_number,
            attempt: row.run_attempt.unwrap_or(1),
            event: row.event,
            status: row.status.unwrap_or_else(|| "queued".into()),
            conclusion: row.conclusion,
            branch: row.head_branch,
            sha: row.head_sha,
            actor: actor.as_ref().map(|actor| actor.login.clone()),
            avatar_url: actor.and_then(|actor| actor.avatar_url),
            created_at: row.created_at,
            started_at: row.run_started_at,
            updated_at: row.updated_at,
            pull_requests: row
                .pull_requests
                .into_iter()
                .map(|pull| pull.number)
                .collect(),
            url: row.html_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub workflow_id: Option<u64>,
    pub branch: Option<String>,
    /// One of GitHub's run statuses or conclusions, which it filters by interchangeably.
    pub status: Option<String>,
    pub event: Option<String>,
    pub actor: Option<String>,
    /// Only the runs for one commit, which is how a pull request's checks are found.
    pub head_sha: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: u64,
    pub next_page: Option<u32>,
}

/// Filters only ever reach GitHub as query values it recognises, so a typed
/// one that is not a status is dropped rather than sent.
const STATUSES: [&str; 13] = [
    "queued",
    "in_progress",
    "completed",
    "requested",
    "waiting",
    "pending",
    "success",
    "failure",
    "neutral",
    "cancelled",
    "skipped",
    "timed_out",
    "action_required",
];

fn known_status(value: &str) -> bool {
    STATUSES.contains(&value)
}

pub async fn list(data_dir: &Path, input: RunQuery) -> GithubResult<RunPage> {
    let per_page = input
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = input.page.unwrap_or(1).max(1);
    let mut query = vec![
        ("per_page", per_page.to_string()),
        ("page", page.to_string()),
    ];
    if let Some(branch) = input
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        query.push(("branch", branch.to_string()));
    }
    if let Some(status) = input
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !known_status(status) {
            return Err(GithubError::BadArg(format!(
                "`{status}` is not a run status"
            )));
        }
        query.push(("status", status.to_string()));
    }
    if let Some(event) = input
        .event
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        query.push(("event", event.to_string()));
    }
    if let Some(actor) = input
        .actor
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        query.push(("actor", actor.to_string()));
    }
    if let Some(sha) = input
        .head_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GithubError::BadArg(format!("`{sha}` is not a commit")));
        }
        query.push(("head_sha", sha.to_string()));
    }
    let path = match input.workflow_id {
        Some(id) => input.repo.path(&format!("/actions/workflows/{id}/runs"))?,
        None => input.repo.path("/actions/runs")?,
    };
    let list: RunList = client::get(data_dir, &path, &query).await?;
    let runs: Vec<Run> = list.workflow_runs.into_iter().map(Run::from).collect();
    let seen = u64::from(page.saturating_sub(1)) * u64::from(per_page) + runs.len() as u64;
    Ok(RunPage {
        next_page: (seen < list.total_count && !runs.is_empty()).then(|| page + 1),
        total: list.total_count,
        runs,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub run_id: u64,
}

pub async fn get(data_dir: &Path, input: RunRef) -> GithubResult<Run> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}", input.run_id))?;
    let row: RunRow = client::get(data_dir, &path, &[]).await?;
    Ok(Run::from(row))
}

#[derive(Deserialize)]
struct StepRow {
    name: String,
    status: Option<String>,
    conclusion: Option<String>,
    number: u64,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Deserialize)]
struct JobRow {
    id: u64,
    name: String,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    html_url: Option<String>,
    runner_name: Option<String>,
    check_run_url: Option<String>,
    #[serde(default)]
    steps: Vec<StepRow>,
}

#[derive(Deserialize)]
struct JobList {
    #[serde(default)]
    total_count: u64,
    jobs: Vec<JobRow>,
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
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub runner: Option<String>,
    pub url: Option<String>,
    /// The check run this job is also recorded as, which is where its
    /// annotations live.
    pub check_run_id: Option<u64>,
    pub steps: Vec<Step>,
}

impl From<JobRow> for Job {
    fn from(row: JobRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            status: row.status.unwrap_or_else(|| "queued".into()),
            conclusion: row.conclusion,
            started_at: row.started_at,
            completed_at: row.completed_at,
            runner: row.runner_name,
            url: row.html_url,
            check_run_id: row
                .check_run_url
                .as_deref()
                .and_then(crate::annotations::check_run_id),
            steps: row
                .steps
                .into_iter()
                .map(|step| Step {
                    number: step.number,
                    name: step.name,
                    status: step.status.unwrap_or_else(|| "queued".into()),
                    conclusion: step.conclusion,
                    started_at: step.started_at,
                    completed_at: step.completed_at,
                })
                .collect(),
        }
    }
}

async fn job_rows(data_dir: &Path, path: &str, query: &[(&str, String)]) -> GithubResult<Vec<Job>> {
    let rows =
        client::get_all(data_dir, path, query, LIST_PAGES, |list: JobList| list.jobs).await?;
    Ok(rows.into_iter().map(Job::from).collect())
}

fn latest() -> [(&'static str, String); 1] {
    [("filter", "latest".to_string())]
}

pub async fn jobs(data_dir: &Path, input: RunRef) -> GithubResult<Vec<Job>> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}/jobs", input.run_id))?;
    job_rows(data_dir, &path, &latest()).await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
}

pub async fn detail(data_dir: &Path, input: RunRef) -> GithubResult<RunDetail> {
    let repo = RepoRef {
        owner: input.repo.owner.clone(),
        name: input.repo.name.clone(),
    };
    let run_id = input.run_id;
    let (run, jobs) = futures::future::join(
        get(data_dir, input),
        jobs(data_dir, RunRef { repo, run_id }),
    )
    .await;
    Ok(RunDetail {
        run: run?,
        jobs: jobs?,
    })
}

/// What a watch remembers between reads, so an unchanged run costs nothing.
#[derive(Default)]
pub struct Followed {
    run: Option<(Run, Option<String>)>,
    jobs: Option<(Vec<Job>, Option<String>)>,
}

impl Followed {
    pub fn last(&self) -> (Option<Run>, Vec<Job>) {
        (
            self.run.as_ref().map(|(run, _)| run.clone()),
            self.jobs
                .as_ref()
                .map(|(jobs, _)| jobs.clone())
                .unwrap_or_default(),
        )
    }
}

/// The run and its jobs as they are now, and how many requests GitHub will
/// still take this hour.
pub async fn follow(
    data_dir: &Path,
    input: &RunRef,
    held: &mut Followed,
) -> GithubResult<(RunDetail, Option<u64>)> {
    let base = format!("/actions/runs/{}", input.run_id);
    let run_etag = held.run.as_ref().and_then(|(_, etag)| etag.clone());
    let jobs_etag = held.jobs.as_ref().and_then(|(_, etag)| etag.clone());
    let jobs_query = [
        ("per_page", MAX_PER_PAGE.to_string()),
        ("filter", "latest".to_string()),
    ];
    let paged = held
        .jobs
        .as_ref()
        .is_some_and(|(jobs, _)| jobs.len() >= MAX_PER_PAGE as usize);
    let run_path = input.repo.path(&base)?;
    let jobs_path = input.repo.path(&format!("{base}/jobs"))?;
    let (run, jobs) = futures::future::join(
        client::get_if_changed::<RunRow>(data_dir, &run_path, &[], run_etag.as_deref()),
        client::get_if_changed::<JobList>(data_dir, &jobs_path, &jobs_query, jobs_etag.as_deref()),
    )
    .await;
    let (run, jobs) = (run?, jobs?);
    if let Some(row) = run.value {
        held.run = Some((Run::from(row), run.etag));
    }
    // Past a hundred jobs the first page no longer shows every change, so
    // the whole list is read again.
    match jobs.value {
        Some(list) if list.total_count <= list.jobs.len() as u64 => {
            held.jobs = Some((list.jobs.into_iter().map(Job::from).collect(), jobs.etag));
        }
        Some(_) => held.jobs = Some((job_rows(data_dir, &jobs_path, &latest()).await?, jobs.etag)),
        None if paged => {
            let all = job_rows(data_dir, &jobs_path, &latest()).await?;
            if let Some(held) = held.jobs.as_mut() {
                held.0 = all;
            }
        }
        None => {}
    }
    let remaining = match (run.remaining, jobs.remaining) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let (run, jobs) = held.last();
    let run = run.ok_or_else(|| GithubError::Response("GitHub sent no run".into()))?;
    Ok((RunDetail { run, jobs }, remaining))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rerun {
    #[serde(flatten)]
    pub run: RunRef,
    /// Re-run only the jobs that did not pass, rather than the whole run.
    #[serde(default)]
    pub failed_only: bool,
    /// Turn on the runner's own debug logging for the new attempt.
    #[serde(default)]
    pub debug: bool,
}

pub async fn rerun(data_dir: &Path, input: Rerun) -> GithubResult<()> {
    let tail = if input.failed_only {
        "rerun-failed-jobs"
    } else {
        "rerun"
    };
    let path = input
        .run
        .repo
        .path(&format!("/actions/runs/{}/{tail}", input.run.run_id))?;
    let body = json!({ "enable_debug_logging": input.debug });
    client::post_empty(data_dir, &path, Some(&body)).await
}

pub async fn cancel(data_dir: &Path, input: RunRef) -> GithubResult<()> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}/cancel", input.run_id))?;
    client::post_empty(data_dir, &path, None).await
}

pub async fn delete_logs(data_dir: &Path, input: RunRef) -> GithubResult<()> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}/logs", input.run_id))?;
    client::act(data_dir, Method::DELETE, &path, None).await
}

pub async fn delete(data_dir: &Path, input: RunRef) -> GithubResult<()> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}", input.run_id))?;
    client::act(data_dir, Method::DELETE, &path, None).await
}

#[derive(Deserialize)]
struct BillableRow {
    total_ms: Option<u64>,
    jobs: Option<u64>,
}

#[derive(Deserialize)]
struct TimingRow {
    run_duration_ms: Option<u64>,
    #[serde(default)]
    billable: std::collections::BTreeMap<String, BillableRow>,
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
pub struct Timing {
    pub run_duration_ms: Option<u64>,
    pub billable: Vec<Billable>,
}

pub async fn timing(data_dir: &Path, input: RunRef) -> GithubResult<Timing> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}/timing", input.run_id))?;
    let row: TimingRow = client::get(data_dir, &path, &[]).await?;
    Ok(Timing {
        run_duration_ms: row.run_duration_ms,
        billable: row
            .billable
            .into_iter()
            .map(|(runner, spent)| Billable {
                runner,
                total_ms: spent.total_ms.unwrap_or(0),
                jobs: spent.jobs.unwrap_or(0),
            })
            .collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub job_id: u64,
}

pub async fn job_status(data_dir: &Path, input: JobRef) -> GithubResult<String> {
    let path = input
        .repo
        .path(&format!("/actions/jobs/{}", input.job_id))?;
    let row: JobRow = client::get(data_dir, &path, &[]).await?;
    Ok(row.status.unwrap_or_else(|| "queued".into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunJob {
    #[serde(flatten)]
    pub job: JobRef,
    #[serde(default)]
    pub debug: bool,
}

pub async fn rerun_job(data_dir: &Path, input: RerunJob) -> GithubResult<()> {
    let path = input
        .job
        .repo
        .path(&format!("/actions/jobs/{}/rerun", input.job.job_id))?;
    let body = json!({ "enable_debug_logging": input.debug });
    client::post_empty(data_dir, &path, Some(&body)).await
}

/// An earlier try of the same run, which GitHub keeps whole.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub run_id: u64,
    pub attempt: u64,
}

pub async fn attempt(data_dir: &Path, input: AttemptRef) -> GithubResult<RunDetail> {
    let base = format!("/actions/runs/{}/attempts/{}", input.run_id, input.attempt);
    // Neither answer needs the other, so they are asked for together the way
    // the current attempt's are.
    let jobs_path = input.repo.path(&format!("{base}/jobs"))?;
    let (run, jobs): (GithubResult<RunRow>, _) = futures::future::join(
        client::get(data_dir, &input.repo.path(&base)?, &[]),
        job_rows(data_dir, &jobs_path, &[]),
    )
    .await;
    Ok(RunDetail {
        run: Run::from(run?),
        jobs: jobs?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_a_sha_the_way_git_does() {
        assert_eq!(short("b8feb3812345"), "b8feb38");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }

    #[test]
    fn only_a_completed_run_is_finished() {
        assert!(is_finished("completed"));
        for moving in ["queued", "in_progress", "waiting", "requested", "pending"] {
            assert!(!is_finished(moving), "{moving}");
        }
    }

    #[test]
    fn knows_the_statuses_github_filters_by() {
        assert!(known_status("in_progress"));
        assert!(known_status("timed_out"));
        assert!(!known_status("exploded"));
        assert!(!known_status(""));
    }

    #[test]
    fn a_run_falls_back_to_its_workflow_name_for_a_title() {
        let row: RunRow = serde_json::from_value(json!({
            "id": 1, "name": "CI", "display_title": null, "workflow_id": 9,
            "run_number": 4, "event": "push", "status": "completed", "conclusion": "success",
            "head_sha": "deadbeefcafe", "html_url": "https://example.com",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:01:00Z",
        }))
        .expect("a run parses");
        let run = Run::from(row);
        assert_eq!(run.title, "CI");
        assert_eq!(run.short_sha, "deadbee");
        assert_eq!(run.attempt, 1);
        assert!(run.pull_requests.is_empty());
    }

    #[test]
    fn the_person_who_set_a_run_going_wins_over_its_owner() {
        let row: RunRow = serde_json::from_value(json!({
            "id": 1, "workflow_id": 9, "run_number": 4, "event": "push",
            "status": "queued", "head_sha": "abc", "html_url": "https://example.com",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "actor": { "login": "owner", "avatar_url": null },
            "triggering_actor": { "login": "rerunner", "avatar_url": "https://avatar" },
        }))
        .expect("a run parses");
        let run = Run::from(row);
        assert_eq!(run.actor.as_deref(), Some("rerunner"));
        assert_eq!(run.avatar_url.as_deref(), Some("https://avatar"));
    }
}
