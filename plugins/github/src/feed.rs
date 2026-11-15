// CI as GitHub's webhooks tell it. A repository whose workflow_run and
// workflow_job events reach the core keeps its recent runs and their jobs here,
// so `runs` and `run` answer from them rather than asking GitHub again.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use reqwest::Method;
use ring::hmac;
use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_api::codehost::asks::{CiEvent, CiHookAdd, RepoRef, RunQuery};
use sikemux_plugin_api::codehost::{CiMoved, Job, Run, RunDetail, RunPage};

use crate::client;
use crate::error::{GithubError, GithubResult};
use crate::repo::RepoPath;
use crate::runs::{is_finished, JobRow, RunRow};

/// A repository whose events stopped this long ago is asked again, in case its hook is gone.
const LIVE_FOR: Duration = Duration::from_secs(60 * 60);
/// A going run that heard nothing for this long is read from GitHub, in case an event was lost.
const QUIET_RUN: Duration = Duration::from_secs(10 * 60);
const RUNS_KEPT: usize = 150;

static FED: LazyLock<Mutex<HashMap<String, Fed>>> = LazyLock::new(Default::default);

struct Fed {
    heard: Instant,
    /// By run id, which GitHub hands out in order, so the first is the oldest.
    runs: BTreeMap<u64, Kept>,
}

#[derive(Default)]
struct Kept {
    /// None while only its jobs were heard of.
    run: Option<Run>,
    attempt: u64,
    jobs: BTreeMap<u64, Job>,
    /// Its jobs were read whole from GitHub once it ended, so none can be missing.
    whole: bool,
    touched: Option<Instant>,
}

fn key(owner: &str, name: &str) -> String {
    format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        name.to_ascii_lowercase()
    )
}

fn with_live<T>(repo: &RepoRef, read: impl FnOnce(&mut Fed) -> Option<T>) -> Option<T> {
    let mut fed = FED.lock().ok()?;
    let fed = fed.get_mut(&key(&repo.owner, &repo.name))?;
    (fed.heard.elapsed() < LIVE_FOR)
        .then(|| read(fed))
        .flatten()
}

#[derive(Deserialize)]
struct Owner {
    login: String,
}

#[derive(Deserialize)]
struct Repository {
    name: String,
    owner: Owner,
}

#[derive(Deserialize)]
struct RunEvent {
    workflow_run: RunRow,
    repository: Repository,
}

#[derive(Deserialize)]
struct JobEvent {
    workflow_job: JobInRun,
    repository: Repository,
}

#[derive(Deserialize)]
struct JobInRun {
    run_id: u64,
    run_attempt: Option<u64>,
    #[serde(flatten)]
    row: JobRow,
}

/// Takes in one webhook request, once its signature shows it came from the hook the core made.
pub fn take(event: CiEvent) -> GithubResult<Option<CiMoved>> {
    let signed = (event.signature.as_deref())
        .and_then(|signature| signature.strip_prefix("sha256="))
        .and_then(unhex)
        .is_some_and(|signature| {
            let key = hmac::Key::new(hmac::HMAC_SHA256, event.secret.as_bytes());
            hmac::verify(&key, event.body.as_bytes(), &signature).is_ok()
        });
    if !signed {
        return Err(GithubError::BadArg(
            "the request is not signed with this hook's secret".into(),
        ));
    }
    let parse = |error: serde_json::Error| GithubError::Response(error.to_string());
    let (repository, run_moved) = match event.event.as_str() {
        "workflow_run" => {
            let event: RunEvent = serde_json::from_str(&event.body).map_err(parse)?;
            let attempt = event.workflow_run.run_attempt.unwrap_or(1);
            let run = Run::from(event.workflow_run);
            change(
                &event.repository,
                run.id.parse().unwrap_or_default(),
                |kept| {
                    kept.attempt_is(attempt);
                    kept.whole &= is_finished(&run.status);
                    kept.run = Some(run);
                },
            );
            (event.repository, true)
        }
        "workflow_job" => {
            let event: JobEvent = serde_json::from_str(&event.body).map_err(parse)?;
            let attempt = event.workflow_job.run_attempt.unwrap_or(1);
            let job_id = event.workflow_job.row.id;
            let job = Job::from(event.workflow_job.row);
            change(&event.repository, event.workflow_job.run_id, |kept| {
                kept.attempt_is(attempt);
                kept.jobs.insert(job_id, job);
            });
            (event.repository, false)
        }
        _ => return Ok(None),
    };
    Ok(Some(CiMoved {
        owner: repository.owner.login,
        name: repository.name,
        run_moved,
    }))
}

impl Kept {
    /// It goes on, yet nothing was heard of it lately, so an event may have been lost.
    fn quiet(&self) -> bool {
        let going = (self.run.as_ref()).is_some_and(|run| !is_finished(&run.status));
        going && self.touched.is_none_or(|at| at.elapsed() >= QUIET_RUN)
    }

    /// A run started again is a new attempt, whose jobs are new.
    fn attempt_is(&mut self, attempt: u64) {
        if attempt > self.attempt {
            self.attempt = attempt;
            self.jobs.clear();
            self.whole = false;
        }
    }
}

fn change(repository: &Repository, run_id: u64, change: impl FnOnce(&mut Kept)) {
    let Ok(mut fed) = FED.lock() else { return };
    let fed = (fed.entry(key(&repository.owner.login, &repository.name))).or_insert_with(|| Fed {
        heard: Instant::now(),
        runs: BTreeMap::new(),
    });
    fed.heard = Instant::now();
    let kept = fed.runs.entry(run_id).or_default();
    change(kept);
    kept.touched = Some(Instant::now());
    while fed.runs.len() > RUNS_KEPT {
        fed.runs.pop_first();
    }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn given(field: &Option<String>) -> Option<&str> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn matches(run: &Run, query: &RunQuery) -> bool {
    given(&query.workflow_id).is_none_or(|id| run.workflow_id == id)
        && given(&query.branch).is_none_or(|branch| run.branch.as_deref() == Some(branch))
        && given(&query.status)
            .is_none_or(|status| run.status == status || run.conclusion.as_deref() == Some(status))
        && given(&query.event).is_none_or(|event| run.event == event)
        && given(&query.actor).is_none_or(|actor| run.actor.as_deref() == Some(actor))
        && given(&query.head_sha).is_none_or(|sha| run.sha == sha)
}

/// The first page of runs `query` asks for, when the kept runs hold all of it: every run of a
/// commit they know of, or as many runs as the page takes, none of them going yet quiet.
pub fn runs(query: &RunQuery, per_page: u32) -> Option<RunPage> {
    if query.page.is_some_and(|page| page > 1) {
        return None;
    }
    with_live(&query.repo, |fed| {
        let found: Vec<&Kept> = (fed.runs.values().rev())
            .filter(|kept| kept.run.as_ref().is_some_and(|run| matches(run, query)))
            .collect();
        let of_a_commit = query.head_sha.is_some() && !found.is_empty();
        if !of_a_commit && found.len() < per_page as usize {
            return None;
        }
        let page: Vec<&Kept> = found.iter().take(per_page as usize).copied().collect();
        if page.iter().any(|kept| kept.quiet()) {
            return None;
        }
        let runs: Vec<Run> = page.iter().filter_map(|kept| kept.run.clone()).collect();
        let total = found.len() as u64;
        Some(RunPage {
            next_page: (total > runs.len() as u64).then_some(2),
            total,
            runs,
        })
    })
}

/// Keeps the runs GitHub listed, where they are newer than what was heard.
pub fn listed(repo: &RepoRef, runs: &[Run]) {
    with_live(repo, |fed| {
        for run in runs {
            let Ok(id) = run.id.parse::<u64>() else {
                continue;
            };
            let kept = fed.runs.entry(id).or_default();
            let newer = (kept.run.as_ref()).is_none_or(|held| held.updated_at < run.updated_at);
            if newer {
                kept.attempt_is(run.attempt);
                kept.run = Some(run.clone());
            }
            kept.touched = Some(Instant::now());
        }
        while fed.runs.len() > RUNS_KEPT {
            fed.runs.pop_first();
        }
        Some(())
    });
}

/// A run and its jobs, when what was heard of it still holds: it ended and its jobs were read
/// whole, or it goes on and was heard of lately.
pub fn detail(repo: &RepoRef, run_id: u64) -> Option<RunDetail> {
    with_live(repo, |fed| {
        let kept = fed.runs.get(&run_id)?;
        let run = kept.run.clone()?;
        let holds = if is_finished(&run.status) {
            kept.whole
        } else {
            !kept.quiet()
        };
        holds.then(|| RunDetail {
            run,
            jobs: kept.jobs.values().cloned().collect(),
        })
    })
}

/// Keeps a run GitHub was asked for, whose jobs are then whole once it has ended.
pub fn read(repo: &RepoRef, detail: &RunDetail) {
    with_live(repo, |fed| {
        let id = detail.run.id.parse::<u64>().ok()?;
        let kept = fed.runs.entry(id).or_default();
        kept.attempt_is(detail.run.attempt);
        kept.run = Some(detail.run.clone());
        kept.jobs = (detail.jobs.iter())
            .filter_map(|job| Some((job.id.parse().ok()?, job.clone())))
            .collect();
        kept.whole = is_finished(&detail.run.status);
        kept.touched = Some(Instant::now());
        Some(())
    });
}

/// Whether a job has ended, as far as its events told.
pub fn job_ended(repo: &RepoRef, job_id: u64) -> Option<bool> {
    with_live(repo, |fed| {
        (fed.runs.values())
            .find_map(|kept| kept.jobs.get(&job_id))
            .map(|job| is_finished(&job.status))
    })
}

#[derive(Deserialize)]
struct Hook {
    id: u64,
}

pub async fn add_hook(data_dir: &Path, ask: CiHookAdd) -> GithubResult<String> {
    let path = ask.repo.path("/hooks")?;
    let body = json!({
        "name": "web",
        "active": true,
        "events": ["workflow_run", "workflow_job"],
        "config": {
            "url": ask.url,
            "content_type": "json",
            "secret": ask.secret,
            "insecure_ssl": "0",
        },
    });
    let hook: Hook = client::API
        .send_json(data_dir, Method::POST, &path, &body)
        .await?;
    Ok(hook.id.to_string())
}
