// What agents reach through their rundeck_* tools: jobs named by their
// group/name path, runs that wait for their end, and bounded log excerpts.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::client::{get_json, seg, Session, HTTP};
use crate::error::{RundeckError, RundeckResult};
use crate::executions::{self, branch_from_options, Execution, ExecutionList, RunRequest};
use crate::jobs::{self, JobDetail, JobOption};
use crate::logs;
use crate::plan;
use crate::projects;
use crate::watch;

/// Agent tool calls reach the plugin through a relay that gives up after 70s, so a call
/// waits at most this long from its start and still has time to answer.
const WAIT_LIMIT: Duration = Duration::from_secs(50);
const OUTCOME_LOG_LINES: usize = 30;
const MOST_JOBS_WITH_OPTIONS: usize = 40;
const CLOSEST_NAMES: usize = 5;

fn path_of(detail: &JobDetail) -> String {
    match detail.group.as_deref() {
        Some(group) if !group.is_empty() => format!("{group}/{}", detail.name),
        _ => detail.name.clone(),
    }
}

// ---- jobs -----------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionEntry {
    name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    only_these: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    secure: bool,
}

impl From<&JobOption> for OptionEntry {
    fn from(option: &JobOption) -> Self {
        Self {
            name: option.name.clone(),
            required: option.required,
            default: option.default.clone(),
            values: option.values.clone().filter(|values| !values.is_empty()),
            only_these: option.enforced,
            secure: option.secure,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEntry {
    project: String,
    path: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<OptionEntry>>,
}

fn entry(detail: &JobDetail, with_options: bool) -> JobEntry {
    JobEntry {
        project: detail.project.clone(),
        path: path_of(detail),
        id: detail.id.clone(),
        description: detail.description.clone(),
        disabled: !detail.execution_enabled,
        options: with_options.then(|| detail.options.iter().map(OptionEntry::from).collect()),
    }
}

#[derive(Serialize)]
pub struct JobList {
    jobs: Vec<JobEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    closest: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// Every job definition in the project, or in every project when none is named.
async fn definitions(session: &Session, project: Option<&str>) -> RundeckResult<Vec<JobDetail>> {
    if let Some(project) = project {
        return jobs::definitions(session, project).await;
    }
    let projects = projects::projects(session).await?;
    let fetched = join_all(
        projects
            .iter()
            .map(|project| HTTP.limited(jobs::definitions(session, &project.name))),
    )
    .await;
    let mut found = Vec::new();
    let mut first_error = None;
    for result in fetched {
        match result {
            Ok(mut details) => found.append(&mut details),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) if found.is_empty() => Err(error),
        _ => Ok(found),
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn distance(from: &str, to: &str) -> usize {
    let to: Vec<char> = to.chars().collect();
    let mut previous: Vec<usize> = (0..=to.len()).collect();
    for (row, left) in from.chars().enumerate() {
        let mut current = Vec::with_capacity(previous.len());
        current.push(row + 1);
        for ((right, diagonal), above) in to.iter().zip(&previous).zip(previous.iter().skip(1)) {
            let beside = current.last().copied().unwrap_or(row + 1);
            current.push(
                (diagonal + usize::from(left != *right))
                    .min(above + 1)
                    .min(beside + 1),
            );
        }
        previous = current;
    }
    previous.last().copied().unwrap_or_default()
}

/// The paths most like `wanted`: those sharing the most of its words, then the nearest in spelling.
fn closest(paths: &[String], wanted: &str) -> Vec<String> {
    let wanted_words = words(wanted);
    let wanted = wanted.to_lowercase();
    let mut scored: Vec<(usize, usize, &String)> = paths
        .iter()
        .map(|path| {
            let lower = path.to_lowercase();
            let shared = wanted_words
                .iter()
                .filter(|word| lower.contains(word.as_str()))
                .count();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            (
                shared,
                distance(&wanted, name).min(distance(&wanted, &lower)),
                path,
            )
        })
        .filter(|(shared, apart, _)| *shared > 0 || *apart <= wanted.len() / 2)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    scored
        .into_iter()
        .take(CLOSEST_NAMES)
        .map(|(_, _, path)| path.clone())
        .collect()
}

fn qualified(detail: &JobDetail) -> String {
    format!("{}: {}", detail.project, path_of(detail))
}

pub async fn find_jobs(
    session: &Session,
    project: Option<String>,
    filter: Option<String>,
) -> RundeckResult<JobList> {
    let all = definitions(session, project.as_deref()).await?;
    let filter = filter
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty());
    let mut matched: Vec<&JobDetail> = all
        .iter()
        .filter(|detail| {
            filter
                .as_deref()
                .is_none_or(|filter| path_of(detail).to_lowercase().contains(filter))
        })
        .collect();
    matched.sort_by_key(|detail| (detail.project.clone(), path_of(detail)));
    let with_options = matched.len() <= MOST_JOBS_WITH_OPTIONS;
    let closest = match (&filter, matched.is_empty()) {
        (Some(filter), true) => {
            let paths: Vec<String> = all.iter().map(qualified).collect();
            closest(&paths, filter)
        }
        _ => Vec::new(),
    };
    Ok(JobList {
        jobs: matched.iter().map(|detail| entry(detail, with_options)).collect(),
        closest,
        note: (!with_options).then(|| {
            format!(
                "{} jobs; options are shown for {MOST_JOBS_WITH_OPTIONS} or fewer, so narrow the filter",
                matched.len()
            )
        }),
    })
}

/// A job id is a UUID unless someone chose their own, so anything shaped like one is tried as an id first.
fn looks_like_id(text: &str) -> bool {
    text.len() >= 8 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The job named by its id, its full group/name path, or a path's tail when only one job ends that way.
pub async fn resolve(
    session: &Session,
    job: &str,
    project: Option<&str>,
) -> RundeckResult<JobDetail> {
    let wanted = job.trim().trim_matches('/');
    if wanted.is_empty() {
        return Err(RundeckError::BadArg(
            "name a job by its id or group/name path".into(),
        ));
    }
    if looks_like_id(wanted) {
        match jobs::job_detail(session, wanted.to_string()).await {
            Ok(detail) => return Ok(detail),
            Err(RundeckError::Http { status: 404, .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let all = definitions(session, project).await?;
    let lower = wanted.to_lowercase();
    let exact: Vec<&JobDetail> = all
        .iter()
        .filter(|detail| path_of(detail).to_lowercase() == lower)
        .collect();
    let candidates = if exact.is_empty() {
        let tail = format!("/{lower}");
        all.iter()
            .filter(|detail| path_of(detail).to_lowercase().ends_with(&tail))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [detail] => Ok((*detail).clone()),
        [] => {
            let paths: Vec<String> = all.iter().map(qualified).collect();
            let near = closest(&paths, wanted);
            let place =
                project.map_or_else(|| "any project".to_string(), |p| format!("project {p}"));
            Err(RundeckError::BadArg(if near.is_empty() {
                format!("no job in {place} is named {wanted}; rundeck_jobs lists them")
            } else {
                format!(
                    "no job in {place} is named {wanted}; the closest are {}",
                    near.join(", ")
                )
            }))
        }
        many => Err(RundeckError::BadArg(format!(
            "{wanted} names {} jobs: {}; give the full path, or the project",
            many.len(),
            many.iter()
                .map(|detail| qualified(detail))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

// ---- runs -----------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_secs: Option<i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    options: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    log: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

fn status_of(execution: &Execution) -> String {
    execution
        .custom_status
        .clone()
        .or_else(|| execution.status.clone())
        .unwrap_or_else(|| "unknown".into())
}

fn job_path(execution: &Execution) -> Option<String> {
    let job = execution.job.as_ref()?;
    let name = job.name.clone()?;
    Some(match job.group.as_deref() {
        Some(group) if !group.is_empty() => format!("{group}/{name}"),
        _ => name,
    })
}

fn duration_secs(execution: &Execution) -> Option<i64> {
    let started = execution.date_started.as_ref()?.unixtime? / 1000;
    let ended = match execution.date_ended.as_ref().and_then(|date| date.unixtime) {
        Some(ended) => ended / 1000,
        None => sikemux_process::clock::epoch_secs() as i64,
    };
    Some((ended - started).max(0))
}

fn outcome(execution: Execution) -> Outcome {
    Outcome {
        id: execution.id,
        job: job_path(&execution),
        project: execution.project.clone(),
        status: status_of(&execution),
        user: execution.user.clone(),
        started: execution
            .date_started
            .as_ref()
            .and_then(|date| date.date.clone()),
        ended: execution
            .date_ended
            .as_ref()
            .and_then(|date| date.date.clone()),
        duration_secs: duration_secs(&execution),
        url: execution.permalink.clone(),
        options: execution
            .job
            .and_then(|job| job.options)
            .unwrap_or_default(),
        log: Vec::new(),
        note: None,
    }
}

/// Follows the run until it ends or the wait runs out, then adds the end of its log.
async fn awaited(
    session: &Session,
    execution_id: u64,
    deadline: Instant,
) -> RundeckResult<Outcome> {
    let mut last: Option<Execution> = None;
    let followed = tokio::time::timeout_at(
        deadline,
        watch::follow(session, execution_id, |update| {
            if let Some(execution) = update.execution {
                last = Some(execution);
            }
            Ok(())
        }),
    )
    .await;
    let ended = matches!(followed, Ok(Ok(())));
    let execution = match last {
        Some(execution) => execution,
        None => executions::execution(session, execution_id).await?,
    };
    let mut outcome = outcome(execution);
    if ended {
        outcome.log = logs::excerpt(session, execution_id, OUTCOME_LOG_LINES, None)
            .await
            .map(|excerpt| excerpt.lines)
            .unwrap_or_default();
    } else {
        outcome.note = Some(format!(
            "still {} after {}s; rundeck_execution with wait keeps waiting",
            outcome.status,
            WAIT_LIMIT.as_secs()
        ));
    }
    Ok(outcome)
}

fn is_running(execution: &Execution) -> bool {
    execution.status.as_deref().is_some_and(|status| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "running" | "scheduled" | "queued"
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAsk {
    job: String,
    project: Option<String>,
    #[serde(default)]
    options: HashMap<String, String>,
    wait: Option<bool>,
    #[serde(default)]
    production: bool,
}

fn check_options(detail: &JobDetail, given: &HashMap<String, String>) -> RundeckResult<()> {
    let known: Vec<&str> = detail
        .options
        .iter()
        .map(|option| option.name.as_str())
        .collect();
    let mut unknown: Vec<&str> = given
        .keys()
        .map(String::as_str)
        .filter(|name| !known.contains(name))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(RundeckError::BadArg(format!(
            "{} has no option {}; its options are {}",
            path_of(detail),
            unknown.join(", "),
            if known.is_empty() {
                "none".into()
            } else {
                known.join(", ")
            }
        )));
    }
    let missing: Vec<&str> = detail
        .options
        .iter()
        .filter(|option| option.required && option.default.is_none())
        .filter(|option| {
            given
                .get(&option.name)
                .is_none_or(|value| value.trim().is_empty())
        })
        .map(|option| option.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(RundeckError::BadArg(format!(
            "{} needs {}",
            path_of(detail),
            missing.join(", ")
        )));
    }
    Ok(())
}

pub async fn run(session: &Session, ask: RunAsk, prod_envs: &[String]) -> RundeckResult<Outcome> {
    let deadline = Instant::now() + WAIT_LIMIT;
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    if crate::shape::is_production(&detail.project, detail.group.as_deref(), prod_envs)
        && !ask.production
    {
        return Err(RundeckError::BadArg(format!(
            "{} runs in production. Ask the person first, and call again with production: true once they agree.",
            path_of(&detail)
        )));
    }
    check_options(&detail, &ask.options)?;
    let started = executions::run(
        session,
        detail.id.clone(),
        RunRequest {
            options: ask.options,
            ..RunRequest::default()
        },
    )
    .await?;
    if ask.wait.unwrap_or(true) {
        return awaited(session, started.id, deadline).await;
    }
    Ok(Outcome {
        job: Some(path_of(&detail)),
        project: Some(detail.project),
        url: started.permalink,
        ..outcome(executions::execution(session, started.id).await?)
    })
}

pub async fn execution(session: &Session, execution_id: u64, wait: bool) -> RundeckResult<Outcome> {
    let deadline = Instant::now() + WAIT_LIMIT;
    let execution = executions::execution(session, execution_id).await?;
    if wait && is_running(&execution) {
        return awaited(session, execution_id, deadline).await;
    }
    Ok(outcome(execution))
}

// ---- history --------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_secs: Option<i64>,
}

fn row(execution: &Execution, branch_options: &[String]) -> Row {
    Row {
        id: execution.id,
        job: job_path(execution),
        project: execution.project.clone(),
        status: status_of(execution),
        branch: execution
            .job
            .as_ref()
            .and_then(|job| job.options.as_ref())
            .and_then(|options| branch_from_options(options, branch_options)),
        user: execution.user.clone(),
        started: execution
            .date_started
            .as_ref()
            .and_then(|date| date.date.clone()),
        duration_secs: duration_secs(execution),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAsk {
    job: Option<String>,
    project: Option<String>,
    #[serde(default)]
    running: bool,
    max: Option<u32>,
}

pub async fn history(
    session: &Session,
    ask: HistoryAsk,
    branch_options: &[String],
) -> RundeckResult<Vec<Row>> {
    let max = ask.max.unwrap_or(10).clamp(1, 50);
    let mut found = match (&ask.job, &ask.project) {
        (Some(job), project) => {
            let detail = resolve(session, job, project.as_deref()).await?;
            executions::executions(session, detail.id, detail.project, Some(max), None).await?
        }
        (None, Some(project)) if !ask.running => {
            let path = format!("/project/{}/executions", seg(project)?);
            get_json::<ExecutionList>(session, &path, &[("max", max.to_string())])
                .await?
                .executions
        }
        (None, project) => {
            let place = project.as_deref().map_or(Ok("*".to_string()), seg)?;
            get_json::<ExecutionList>(
                session,
                &format!("/project/{place}/executions/running"),
                &[],
            )
            .await?
            .executions
        }
    };
    if ask.running {
        found.retain(is_running);
    }
    found.truncate(max as usize);
    Ok(found
        .iter()
        .map(|execution| row(execution, branch_options))
        .collect())
}

// ---- plan -----------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAsk {
    job: String,
    project: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
}

#[derive(Serialize)]
pub struct AgentPlan {
    #[serde(flatten)]
    plan: plan::PlanResult,
    branch_option: Option<String>,
}

pub async fn plan(
    session: &Session,
    ask: PlanAsk,
    branch_options: &[String],
) -> RundeckResult<AgentPlan> {
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let repo_path = ask.repo.unwrap_or_default();
    let target_branch = match ask.branch.filter(|branch| !branch.trim().is_empty()) {
        Some(branch) => branch,
        None if !repo_path.trim().is_empty() => {
            let repo = PathBuf::from(&repo_path);
            tokio::task::spawn_blocking(move || plan::current_branch(&repo))
                .await
                .map_err(|error| RundeckError::Api(error.to_string()))??
        }
        None => {
            return Err(RundeckError::BadArg(
                "name a branch or a local checkout".into(),
            ))
        }
    };
    let names: Vec<String> = detail
        .options
        .iter()
        .map(|option| option.name.clone())
        .collect();
    let branch_option = branch_options.iter().find_map(|wanted| {
        names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(wanted))
            .cloned()
    });
    let plan = plan::plan(
        session,
        plan::PlanRequest {
            service: path_of(&detail),
            job_id: detail.id,
            project: detail.project,
            target_branch,
            repo_path,
            branch_options: branch_options.to_vec(),
        },
    )
    .await?;
    Ok(AgentPlan {
        plan,
        branch_option,
    })
}
