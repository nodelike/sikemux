// Forgejo Actions and Gitea Actions: a repository's runs, their jobs and each
// job's log, and starting, stopping and removing runs. The two servers answer
// in different shapes and words, which are read here into GitHub's.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use sikemux_plugin_api::codehost::answers;
use sikemux_plugin_api::codehost::asks::{
    numeric, ExcerptQuery, JobRef, Rerun, RerunJob, RunQuery, RunRef,
};
use sikemux_plugin_api::codehost::{
    Job, JobLog, LogExcerpt, LogLine, Outcome, Run, RunDetail, RunPage, Step,
};
use sikemux_plugin_api::http::{Read, MAX_ANSWER};
use sikemux_plugin_api::terminal::plain;

use crate::client;
use crate::error::{ForgejoError, ForgejoResult};
use crate::repo::{avatar_of, login_of, ApiPath, User};

/// A status and, once it is over, a conclusion, in GitHub's words. Gitea
/// already answers in them; Forgejo says one word for both.
pub fn status_of(status: &str, conclusion: Option<&str>) -> (String, Option<String>) {
    let (status, conclusion) = match status {
        "completed" => ("completed", conclusion),
        "success" | "failure" | "cancelled" | "skipped" => ("completed", Some(status)),
        "running" => ("in_progress", None),
        "waiting" => ("queued", None),
        "blocked" => ("waiting", None),
        other => (other, None),
    };
    (status.to_string(), conclusion.map(str::to_string))
}

/// Forgejo writes a time that never came as the year 1.
fn when(time: Option<String>) -> Option<String> {
    time.filter(|time| !time.is_empty() && !time.starts_with("0001-"))
}

#[derive(Deserialize)]
struct PullNumber {
    number: u64,
}

#[derive(Deserialize)]
pub struct RunRow {
    id: u64,
    #[serde(alias = "index_in_repo")]
    run_number: Option<u64>,
    #[serde(alias = "title")]
    display_title: Option<String>,
    path: Option<String>,
    workflow_id: Option<String>,
    run_attempt: Option<u64>,
    event: Option<String>,
    #[serde(default)]
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    need_approval: bool,
    head_branch: Option<String>,
    prettyref: Option<String>,
    #[serde(alias = "commit_sha")]
    head_sha: Option<String>,
    #[serde(alias = "trigger_user")]
    actor: Option<User>,
    #[serde(alias = "created")]
    created_at: Option<String>,
    #[serde(alias = "started")]
    started_at: Option<String>,
    #[serde(alias = "updated")]
    updated_at: Option<String>,
    #[serde(default)]
    html_url: String,
    pull_requests: Option<Vec<PullNumber>>,
}

pub fn run_of(row: RunRow) -> Run {
    let (mut status, conclusion) = status_of(&row.status, row.conclusion.as_deref());
    if row.need_approval && status != "completed" {
        status = "waiting".into();
    }
    let path = row
        .path
        .as_deref()
        .and_then(|path| path.split('@').next())
        .filter(|path| !path.is_empty())
        .map(str::to_string);
    let workflow = row
        .workflow_id
        .clone()
        .or_else(|| {
            path.as_deref()
                .and_then(|path| path.rsplit('/').next())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let mut pulls: Vec<u64> = row
        .pull_requests
        .unwrap_or_default()
        .into_iter()
        .map(|pull| pull.number)
        .collect();
    let pretty_pull = row
        .prettyref
        .as_deref()
        .and_then(|pretty| pretty.strip_prefix('#'))
        .and_then(|number| number.parse::<u64>().ok());
    pulls.extend(pretty_pull);
    let branch = row
        .head_branch
        .filter(|branch| !branch.is_empty())
        .or_else(|| row.prettyref.filter(|_| pretty_pull.is_none()));
    let sha = row.head_sha.unwrap_or_default();
    let created_at = when(row.created_at).unwrap_or_default();
    Run {
        id: row.id.to_string(),
        name: workflow
            .rsplit_once('.')
            .map_or(workflow.as_str(), |(name, _)| name)
            .to_string(),
        title: row.display_title.unwrap_or_default(),
        workflow_id: workflow,
        path,
        run_number: row.run_number.unwrap_or(row.id),
        attempt: row.run_attempt.unwrap_or(1),
        event: row.event.unwrap_or_default(),
        outcome: Outcome::of(&status, conclusion.as_deref()),
        status,
        conclusion,
        branch,
        short_sha: sha.chars().take(7).collect(),
        sha,
        actor: login_of(row.actor.as_ref()),
        avatar_url: avatar_of(row.actor.as_ref()),
        started_at: when(row.started_at),
        updated_at: when(row.updated_at).unwrap_or_else(|| created_at.clone()),
        created_at,
        pull_requests: pulls,
        url: row.html_url,
    }
}

#[derive(Deserialize)]
struct RunList {
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    workflow_runs: Vec<RunRow>,
}

/// Whether a run is in the state an agent asked for, a status or a conclusion.
fn wanted(run: &Run, status: &str) -> bool {
    run.status == status || run.conclusion.as_deref() == Some(status)
}

pub async fn list(data_dir: &Path, input: RunQuery) -> ForgejoResult<RunPage> {
    let page = input.page.unwrap_or(1).max(1);
    let per_page = input.per_page.unwrap_or(30).clamp(1, client::PER_PAGE);
    let mut query = vec![("page", page.to_string()), ("limit", per_page.to_string())];
    if let Some(branch) = input.branch.filter(|branch| !branch.is_empty()) {
        query.push(("ref", format!("refs/heads/{branch}")));
        query.push(("branch", branch));
    }
    for (key, value) in [
        ("event", input.event),
        ("actor", input.actor),
        ("head_sha", input.head_sha),
        ("workflow_id", input.workflow_id),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            query.push((key, value));
        }
    }
    let found: RunList = client::API
        .get(data_dir, &input.repo.path("/actions/runs")?, &query)
        .await?;
    let got = found.workflow_runs.len();
    let runs: Vec<Run> = found
        .workflow_runs
        .into_iter()
        .map(run_of)
        .filter(|run| {
            input
                .status
                .as_deref()
                .is_none_or(|status| wanted(run, status))
        })
        .collect();
    Ok(RunPage {
        runs,
        total: found.total_count,
        next_page: client::next_page(page, per_page, got, Some(found.total_count)),
    })
}

fn run_path(input: &RunRef, rest: &str) -> ForgejoResult<String> {
    let id = numeric(&input.run_id)?;
    input.repo.path(&format!("/actions/runs/{id}{rest}"))
}

#[derive(Deserialize)]
struct StepRow {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    conclusion: Option<String>,
    #[serde(alias = "started")]
    started_at: Option<String>,
    #[serde(alias = "stopped")]
    completed_at: Option<String>,
}

#[derive(Deserialize)]
pub struct JobRow {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    runner_name: Option<String>,
    runs_on: Option<Vec<String>>,
    html_url: Option<String>,
    steps: Option<Vec<StepRow>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JobList {
    Wrapped { jobs: Vec<JobRow> },
    Bare(Vec<JobRow>),
}

pub fn job_of(row: JobRow) -> Job {
    let (status, conclusion) = status_of(&row.status, row.conclusion.as_deref());
    let steps: Vec<Step> = row
        .steps
        .unwrap_or_default()
        .into_iter()
        .map(|step| {
            let (status, conclusion) = status_of(&step.status, step.conclusion.as_deref());
            Step {
                number: step.number,
                name: step.name,
                outcome: Outcome::of(&status, conclusion.as_deref()),
                status,
                conclusion,
                started_at: when(step.started_at),
                completed_at: when(step.completed_at),
            }
        })
        .collect();
    Job {
        id: row.id.to_string(),
        name: row.name,
        outcome: Outcome::of(&status, conclusion.as_deref()),
        status,
        conclusion,
        started_at: when(row.started_at)
            .or_else(|| steps.first().and_then(|step| step.started_at.clone())),
        completed_at: when(row.completed_at)
            .or_else(|| steps.last().and_then(|step| step.completed_at.clone())),
        runner: row.runner_name.filter(|name| !name.is_empty()).or_else(|| {
            row.runs_on
                .filter(|labels| !labels.is_empty())
                .map(|labels| labels.join(", "))
        }),
        url: row.html_url.filter(|url| !url.is_empty()),
        check_run_id: None,
        steps,
    }
}

pub async fn detail(data_dir: &Path, input: RunRef) -> ForgejoResult<RunDetail> {
    let run_at = run_path(&input, "")?;
    let jobs_at = run_path(&input, "/jobs")?;
    let jobs_query = [("limit", client::PER_PAGE.to_string())];
    let (run, jobs) = futures::try_join!(
        client::API.get::<RunRow>(data_dir, &run_at, &[]),
        client::API.get::<JobList>(data_dir, &jobs_at, &jobs_query),
    )?;
    let jobs = match jobs {
        JobList::Wrapped { jobs } | JobList::Bare(jobs) => jobs,
    };
    Ok(RunDetail {
        run: run_of(run),
        jobs: jobs.into_iter().map(job_of).collect(),
    })
}

pub async fn cancel(data_dir: &Path, input: RunRef) -> ForgejoResult<()> {
    client::API
        .write(data_dir, Method::POST, &run_path(&input, "/cancel")?, None)
        .await
}

pub async fn delete(data_dir: &Path, input: RunRef) -> ForgejoResult<()> {
    client::API
        .write(data_dir, Method::DELETE, &run_path(&input, "")?, None)
        .await
}

/// Neither server runs only a run's failed jobs again by itself, so each failed job is run again.
pub async fn rerun(data_dir: &Path, input: Rerun) -> ForgejoResult<()> {
    if input.failed_only {
        let found = detail(data_dir, input.run.clone()).await?;
        let failed: Vec<Job> = found
            .jobs
            .into_iter()
            .filter(|job| job.outcome == Outcome::Failure)
            .collect();
        if failed.is_empty() {
            return Err(ForgejoError::BadArg("no job of this run failed".into()));
        }
        for job in failed {
            let job = JobRef {
                repo: input.run.repo.clone(),
                job_id: job.id,
                run_id: Some(input.run.run_id.clone()),
            };
            rerun_job(data_dir, RerunJob { job, debug: false }).await?;
        }
        return Ok(());
    }
    client::API
        .write(
            data_dir,
            Method::POST,
            &run_path(&input.run, "/rerun")?,
            None,
        )
        .await
}

/// Forgejo names a job on its own and Gitea within its run, so Gitea's way is
/// tried when Forgejo's is not there.
pub async fn rerun_job(data_dir: &Path, input: RerunJob) -> ForgejoResult<()> {
    let job = numeric(&input.job.job_id)?;
    let repo = &input.job.repo;
    let answer = client::API
        .send(
            data_dir,
            Method::POST,
            &repo.path(&format!("/actions/jobs/{job}/rerun"))?,
            &[],
            None,
            Read::ALL,
        )
        .await?;
    let run = input.job.run_id.as_deref().map(numeric).transpose()?;
    match (answer.status.as_u16(), run) {
        (404 | 405, Some(run)) => {
            client::API
                .write(
                    data_dir,
                    Method::POST,
                    &repo.path(&format!("/actions/runs/{run}/jobs/{job}/rerun"))?,
                    None,
                )
                .await
        }
        _ => answer.succeeded().map(|_| ()),
    }
}

/// A log line without its colour codes, with the time the runner wrote before it set apart.
pub fn line_of(number: u64, raw: &str) -> LogLine {
    let raw = raw.trim_end_matches('\r');
    let stamped = raw.split_once(' ').filter(|(time, _)| {
        let bytes = time.as_bytes();
        time.len() >= 20
            && bytes.get(4) == Some(&b'-')
            && bytes.get(10) == Some(&b'T')
            && time.ends_with('Z')
    });
    let (timestamp, text) = match stamped {
        Some((time, text)) => (Some(time.to_string()), text),
        None => (None, raw),
    };
    LogLine {
        number,
        timestamp,
        text: plain(text),
    }
}

/// A job that has not started yet has no log, which reads as an empty one.
pub async fn log(data_dir: &Path, input: JobRef) -> ForgejoResult<JobLog> {
    let job = numeric(&input.job_id)?;
    let answer = client::API
        .send(
            data_dir,
            Method::GET,
            &input.repo.path(&format!("/actions/jobs/{job}/logs"))?,
            &[],
            None,
            Read::Tail(MAX_ANSWER),
        )
        .await?;
    if answer.status.as_u16() == 404 {
        return Ok(JobLog {
            lines: Vec::new(),
            expired: false,
            truncated: false,
        });
    }
    let answer = answer.succeeded()?;
    let text = String::from_utf8_lossy(&answer.bytes);
    let text = if answer.cut {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    Ok(JobLog {
        lines: text
            .lines()
            .enumerate()
            .map(|(index, raw)| line_of(index as u64 + 1, raw))
            .collect(),
        expired: false,
        truncated: answer.cut,
    })
}

pub async fn excerpt(data_dir: &Path, input: ExcerptQuery) -> ForgejoResult<LogExcerpt> {
    let whole = log(data_dir, input.job).await?;
    Ok(answers::log_excerpt(whole, input.pick))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_forgejo_run_reads_in_githubs_words() {
        let run = run_of(
            serde_json::from_value(json!({
                "id": 9120, "index_in_repo": 57, "title": "fix: VAT", "workflow_id": "ci.yml",
                "event": "pull_request", "status": "failure", "prettyref": "#42",
                "commit_sha": "9c41e2a0ffee", "trigger_user": { "login": "ankit" },
                "created": "t1", "started": "t2", "stopped": "t3", "updated": "t4",
                "html_url": "https://codeberg.org/acme/api/actions/runs/57"
            }))
            .expect("run parses"),
        );
        assert_eq!(
            (run.id.as_str(), run.run_number, run.name.as_str()),
            ("9120", 57, "ci")
        );
        assert_eq!(
            (run.status.as_str(), run.conclusion.as_deref(), run.outcome),
            ("completed", Some("failure"), Outcome::Failure)
        );
        assert_eq!((run.branch, run.pull_requests), (None, vec![42]));
        assert_eq!(run.short_sha, "9c41e2a");
        assert_eq!(run.actor.as_deref(), Some("ankit"));
    }

    #[test]
    fn a_gitea_run_keeps_its_own_words() {
        let run = run_of(
            serde_json::from_value(json!({
                "id": 31, "run_number": 4, "display_title": "Release", "path": ".gitea/workflows/release.yml@refs/heads/main",
                "event": "push", "status": "in_progress", "head_branch": "main", "head_sha": "abc",
                "actor": { "login": "irwan" }, "created_at": "t1", "started_at": "0001-01-01T00:00:00Z",
                "html_url": "u", "pull_requests": [ { "number": 3 } ]
            }))
            .expect("run parses"),
        );
        assert_eq!(run.path.as_deref(), Some(".gitea/workflows/release.yml"));
        assert_eq!(run.workflow_id, "release.yml");
        assert_eq!(run.outcome, Outcome::Running);
        assert_eq!(
            (run.branch.as_deref(), run.started_at),
            (Some("main"), None)
        );
        assert_eq!(run.pull_requests, [3]);
    }

    #[test]
    fn jobs_come_bare_from_forgejo_and_wrapped_from_gitea() {
        let bare: JobList = serde_json::from_value(json!([
            { "id": 5, "name": "test", "status": "running", "runs_on": ["docker"],
              "steps": [ { "number": 0, "name": "Set up job", "status": "success", "started": "t1", "stopped": "t2" } ] }
        ]))
        .expect("jobs parse");
        let JobList::Bare(rows) = bare else {
            panic!("bare list");
        };
        let job = job_of(rows.into_iter().next().expect("a job"));
        assert_eq!(
            (job.outcome, job.runner.as_deref()),
            (Outcome::Running, Some("docker"))
        );
        assert_eq!(job.steps[0].outcome, Outcome::Success);
        assert_eq!(job.started_at.as_deref(), Some("t1"));
        let wrapped: JobList =
            serde_json::from_value(json!({ "total_count": 1, "jobs": [ { "id": 6, "status": "completed", "conclusion": "success" } ] }))
                .expect("jobs parse");
        assert!(matches!(wrapped, JobList::Wrapped { jobs } if jobs.len() == 1));
    }

    #[test]
    fn a_log_line_sets_its_time_apart() {
        let line = line_of(3, "2026-11-12T10:00:00.1234567Z \u{1b}[32mok\u{1b}[0m");
        assert_eq!(
            line.timestamp.as_deref(),
            Some("2026-11-12T10:00:00.1234567Z")
        );
        assert_eq!(line.text, "ok");
        assert_eq!(line_of(1, "plain line").timestamp, None);
    }
}
