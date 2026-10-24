//! What the Rundeck pane shows: the projects' folders, a folder's jobs with
//! how each last ran, and a job's or a project's runs. The core keeps each and
//! reads it again on the pace the manifest gives.

use std::collections::{BTreeMap, BTreeSet};

use futures::future::try_join;
use serde::Deserialize;
use sikemux_plugin_api::rundeck::{
    ActivityPage, ExecutionPage, GroupNode, JobPage, JobRow, Jobs, LiveRun, LiveStep, Run, RunStep,
    Section, StatusKind, StepState, Tree, TreeProject,
};

use crate::client::Session;
use crate::config::Settings;
use crate::error::RundeckResult;
use crate::executions::{self, deployed_branch, Execution, WorkflowState};
use crate::jobs;
use crate::projects::{self, MatrixCell};
use crate::shape::{
    display_status, group_segments, is_live, is_production, run_summary, status_kind,
};

pub async fn tree_view(session: &Session) -> RundeckResult<Tree> {
    let projects = projects::job_index(session)
        .await?
        .into_iter()
        .map(|entry| {
            let groups: Vec<Option<&str>> =
                entry.jobs.iter().map(|job| job.group.as_deref()).collect();
            let (groups, ungrouped) = folders(&groups);
            TreeProject {
                name: entry.project,
                error: entry.error,
                total: entry.jobs.len() as u32,
                ungrouped,
                groups,
            }
        })
        .collect();
    Ok(Tree { projects })
}

/// The folders the jobs in `groups` sit in, nested and counted, and how many sit in none.
fn folders(groups: &[Option<&str>]) -> (Vec<GroupNode>, u32) {
    let mut root = GroupNode {
        name: String::new(),
        path: String::new(),
        count: 0,
        children: Vec::new(),
    };
    let mut ungrouped = 0;
    for group in groups {
        let segments = group_segments(*group);
        if segments.is_empty() {
            ungrouped += 1;
            continue;
        }
        let mut node = &mut root;
        for segment in segments {
            if !node.children.iter().any(|child| child.name == segment) {
                let path = match node.path.is_empty() {
                    true => segment.to_owned(),
                    false => format!("{}/{segment}", node.path),
                };
                node.children.push(GroupNode {
                    name: segment.to_owned(),
                    path,
                    count: 0,
                    children: Vec::new(),
                });
            }
            let Some(child) = node.children.iter_mut().find(|child| child.name == segment) else {
                break;
            };
            node = child;
            node.count += 1;
        }
    }
    sort(&mut root.children);
    (root.children, ungrouped)
}

fn sort(nodes: &mut [GroupNode]) {
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    for node in nodes {
        sort(&mut node.children);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsRequest {
    project: String,
    #[serde(default)]
    group: Option<String>,
}

pub async fn jobs_view(
    session: &Session,
    settings: &Settings,
    request: JobsRequest,
) -> RundeckResult<Jobs> {
    let matrix =
        projects::branches_matrix(session, request.project.clone(), &settings.branch_options)
            .await?;
    Ok(jobs_of(
        &request.project,
        request.group.as_deref(),
        matrix.cells,
        matrix.error,
        matrix.partial,
        &settings.prod_envs,
    ))
}

fn jobs_of(
    project: &str,
    group: Option<&str>,
    cells: Vec<MatrixCell>,
    error: Option<String>,
    partial: bool,
    prod_envs: &[String],
) -> Jobs {
    let here = group_segments(group);
    let mut live_paths = BTreeSet::new();
    for cell in cells.iter().filter(|cell| {
        cell.latest
            .as_ref()
            .is_some_and(|run| crate::shape::is_live(run.status.as_deref()))
    }) {
        let mut path = String::new();
        live_paths.insert(path.clone());
        for segment in group_segments(cell.group.as_deref()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            live_paths.insert(path.clone());
        }
    }
    let mut cells: Vec<MatrixCell> = cells
        .into_iter()
        .filter(|cell| group_segments(cell.group.as_deref()).starts_with(&here))
        .collect();
    cells.sort_by(|a, b| a.name.cmp(&b.name));
    let path_of = |below: &[&str]| -> String { [here.as_slice(), below].concat().join("/") };
    let rows: Vec<JobRow> = cells.iter().map(row).collect();

    let below = |cell: &MatrixCell| -> Vec<String> {
        group_segments(cell.group.as_deref())
            .into_iter()
            .skip(here.len())
            .map(|segment| (*segment).to_owned())
            .collect()
    };
    let sections = |key: &dyn Fn(&MatrixCell) -> Option<String>| -> Vec<Section> {
        let mut by: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
        for cell in &cells {
            by.entry(key(cell)).or_default().push(cell.job_id.clone());
        }
        by.into_iter()
            .map(|(segment, jobs)| {
                let path = match &segment {
                    Some(segment) => path_of(&segment.split('/').collect::<Vec<_>>()),
                    None => here.join("/"),
                };
                Section {
                    label: match (&segment, path.is_empty()) {
                        (None, true) => project.to_owned(),
                        _ => path.clone(),
                    },
                    prod: is_production(
                        project,
                        Some(&path)
                            .filter(|path| !path.is_empty())
                            .map(String::as_str),
                        prod_envs,
                    ),
                    segment,
                    path,
                    jobs,
                }
            })
            .collect()
    };
    let list = sections(&|cell| {
        let below = below(cell);
        (!below.is_empty()).then(|| below.join("/"))
    });
    let mut compare = sections(&|cell| below(cell).first().cloned());
    let loose = compare
        .iter()
        .find(|section| section.segment.is_none())
        .map_or(0, |section| section.jobs.len() as u32);
    compare.retain(|section| section.segment.is_some());
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for section in &compare {
        let names: BTreeSet<&str> = section
            .jobs
            .iter()
            .filter_map(|id| cells.iter().find(|cell| &cell.job_id == id))
            .map(|cell| cell.name.as_str())
            .collect();
        for name in names {
            *seen.entry(name).or_default() += 1;
        }
    }
    let shared = seen.values().filter(|count| **count > 1).count() as u32;
    if compare.len() < 2 || shared == 0 {
        compare.clear();
    }

    Jobs {
        project: project.to_owned(),
        group: group.map(str::to_owned),
        title: here
            .last()
            .map_or_else(|| project.to_owned(), |name| (*name).to_owned()),
        error,
        partial,
        timed_out: cells
            .iter()
            .filter(|cell| cell.error.as_deref() == Some("timed out"))
            .count() as u32,
        running: rows
            .iter()
            .filter(|row| row.latest.as_ref().is_some_and(|run| run.live))
            .count() as u32,
        failed: rows
            .iter()
            .filter(|row| {
                row.latest
                    .as_ref()
                    .is_some_and(|run| run.status_kind == StatusKind::Failed)
            })
            .count() as u32,
        jobs: rows,
        list,
        compare,
        shared,
        loose,
        live_paths: live_paths.into_iter().collect(),
    }
}

fn row(cell: &MatrixCell) -> JobRow {
    let latest = cell.latest.as_ref().map(run_summary);
    let deployed = cell.deployed.as_ref().map(run_summary);
    let branch = cell
        .latest
        .as_ref()
        .and_then(|run| run.branch.clone())
        .or_else(|| cell.deployed.as_ref().and_then(|run| run.branch.clone()));
    let live_branch = match (&latest, &deployed) {
        (Some(latest), Some(deployed))
            if latest.live && latest.branch.is_some() && deployed.branch != latest.branch =>
        {
            deployed.branch.clone()
        }
        _ => None,
    };
    JobRow {
        job_id: cell.job_id.clone(),
        name: cell.name.clone(),
        group: cell.group.clone(),
        enabled: cell.enabled != Some(false),
        scheduled: cell.scheduled == Some(true),
        error: cell.error.clone(),
        run_word: if branch.is_some() { "Deploy" } else { "Run" }.to_owned(),
        branch,
        latest,
        deployed,
        live_branch,
    }
}

/// How many of a job's runs its page shows.
const JOB_RUNS: u32 = 30;
/// How soon a page with a run going on is read again.
const LIVE_JOB_SECS: u64 = 3;
const LIVE_ACTIVITY_SECS: u64 = 5;
const LIVE_EXECUTION_SECS: u64 = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    job_id: String,
    project: String,
}

pub async fn job_view(
    session: &Session,
    settings: &Settings,
    request: JobRequest,
) -> RundeckResult<JobPage> {
    let (detail, executions) = try_join(
        jobs::job_detail(session, request.job_id.clone()),
        executions::executions(
            session,
            request.job_id.clone(),
            request.project.clone(),
            Some(JOB_RUNS),
            None,
        ),
    )
    .await?;
    let branch_options = &settings.branch_options;
    let runs = runs_of(&executions, branch_options);
    let runs_branch = detail.options.iter().any(|option| {
        branch_options
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(&option.name))
    });
    let usual = usual_secs(&runs);
    let runs: Vec<Run> = runs
        .into_iter()
        .map(|run| Run {
            usual_secs: usual,
            ..run
        })
        .collect();
    let succeeded_secs: Vec<u64> = runs
        .iter()
        .filter(|run| run.status_kind == StatusKind::Succeeded)
        .filter_map(|run| run.secs)
        .collect();
    let last_good = runs
        .iter()
        .find(|run| {
            run.status_kind == StatusKind::Succeeded && (!runs_branch || run.branch.is_some())
        })
        .cloned();
    let repeat_options = reusable_options(
        last_good
            .as_ref()
            .and_then(|good| executions.iter().find(|ex| ex.id == good.execution_id))
            .and_then(|ex| ex.job.as_ref()?.options.clone())
            .unwrap_or_default(),
        &detail.options,
    );
    let live = executions
        .iter()
        .zip(&runs)
        .find(|(_, run)| run.live)
        .map(|(ex, run)| live_run(ex, run, &detail.steps));
    let finished = runs.iter().filter(|run| !run.live).count() as u32;
    Ok(JobPage {
        permalink: Some(job_permalink(session, &detail.project, &detail.id)),
        prod: is_production(
            &detail.project,
            detail.group.as_deref(),
            &settings.prod_envs,
        ),
        run_word: if runs_branch { "Deploy" } else { "Run" }.to_owned(),
        runs_branch,
        execution_enabled: detail.execution_enabled,
        node_filter: detail.node_filter,
        description: detail.description,
        job_id: detail.id,
        name: detail.name,
        group: detail.group,
        project: detail.project,
        read_again_secs: live.is_some().then_some(LIVE_JOB_SECS),
        live,
        last_good,
        repeat_options,
        finished,
        succeeded: succeeded_secs.len() as u32,
        usual_secs: usual,
        fastest_secs: succeeded_secs.iter().min().copied(),
        slowest_secs: succeeded_secs.iter().max().copied(),
        runs,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRequest {
    project: String,
    max: u32,
}

pub async fn activity_view(
    session: &Session,
    settings: &Settings,
    request: ActivityRequest,
) -> RundeckResult<ActivityPage> {
    let page =
        executions::project_executions(session, request.project.clone(), Some(request.max), None)
            .await?;
    let mut runs = runs_of(&page.executions, &settings.branch_options);
    let mut by_job: BTreeMap<String, Vec<Run>> = BTreeMap::new();
    for run in &runs {
        if let Some(job) = &run.job_id {
            by_job.entry(job.clone()).or_default().push(run.clone());
        }
    }
    let usual: BTreeMap<String, Option<u64>> = by_job
        .into_iter()
        .map(|(job, runs)| (job, usual_secs(&runs)))
        .collect();
    for run in &mut runs {
        run.usual_secs = run
            .job_id
            .as_ref()
            .and_then(|job| usual.get(job).copied().flatten());
    }
    let me = Some(session.user.clone()).filter(|user| !user.is_empty());
    let running = runs.iter().filter(|run| run.live).count() as u32;
    Ok(ActivityPage {
        project: request.project,
        failed: runs
            .iter()
            .filter(|run| run.status_kind == StatusKind::Failed)
            .count() as u32,
        mine: me.as_ref().map_or(0, |me| {
            runs.iter()
                .filter(|run| run.user.as_ref() == Some(me))
                .count() as u32
        }),
        me,
        running,
        total: page.total,
        read_again_secs: (running > 0).then_some(LIVE_ACTIVITY_SECS),
        runs,
    })
}

pub(crate) fn runs_of(executions: &[Execution], branch_options: &[String]) -> Vec<Run> {
    let mut runs: Vec<(i64, Run)> = executions
        .iter()
        .map(|ex| {
            let started = ex.date_started.as_ref().and_then(|date| date.unixtime);
            let ended = ex.date_ended.as_ref().and_then(|date| date.unixtime);
            let job = ex.job.as_ref();
            let run = Run {
                execution_id: ex.id,
                status: display_status(ex.status.as_deref(), ex.custom_status.as_deref()),
                status_kind: status_kind(ex.status.as_deref()),
                live: is_live(ex.status.as_deref()),
                user: ex.user.clone(),
                started_at: ex.date_started.as_ref().and_then(|date| date.date.clone()),
                ended_at: ex.date_ended.as_ref().and_then(|date| date.date.clone()),
                secs: started
                    .zip(ended)
                    .map(|(started, ended)| (ended - started).max(0) as u64 / 1000),
                branch: deployed_branch(ex, branch_options).filter(|branch| !branch.is_empty()),
                job_id: job.and_then(|job| job.id.clone()),
                job_name: job.and_then(|job| job.name.clone()),
                job_group: job
                    .and_then(|job| job.group.clone())
                    .filter(|group| !group.is_empty()),
                usual_secs: None,
            };
            (started.unwrap_or(0), run)
        })
        .collect();
    runs.sort_by(|(a, left), (b, right)| {
        b.cmp(a)
            .then_with(|| right.execution_id.cmp(&left.execution_id))
    });
    runs.into_iter().map(|(_, run)| run).collect()
}

/// The middle time of the runs that succeeded, so one stuck or instant run does not skew it.
pub(crate) fn usual_secs(runs: &[Run]) -> Option<u64> {
    let mut times: Vec<u64> = runs
        .iter()
        .filter(|run| run.status_kind == StatusKind::Succeeded)
        .filter_map(|run| run.secs)
        .collect();
    times.sort_unstable();
    times.get(times.len() / 2).copied()
}

fn live_run(execution: &Execution, run: &Run, step_names: &[String]) -> LiveRun {
    let state = execution.workflow_state.as_ref();
    let steps: Vec<LiveStep> = state
        .map(|state| state.steps.as_slice())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(at, step)| LiveStep {
            name: step_name(step_names, at),
            state: step_state(step.execution_state.as_deref()),
        })
        .collect();
    let (completed, total) = progress(state);
    LiveRun {
        execution_id: run.execution_id,
        user: run.user.clone(),
        started_at: run.started_at.clone(),
        branch: run.branch.clone(),
        completed,
        total,
        steps,
    }
}

fn step_state(state: Option<&str>) -> StepState {
    match state.unwrap_or_default().to_ascii_uppercase().as_str() {
        "RUNNING" | "RUNNING_HANDLER" => StepState::Running,
        "SUCCEEDED" => StepState::Ok,
        "FAILED" | "ABORTED" => StepState::Fail,
        "NOT_ELIGIBLE" | "SKIPPED" => StepState::Skip,
        _ => StepState::Pending,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    execution_id: u64,
    job_id: String,
    project: String,
}

pub async fn execution_view(
    session: &Session,
    settings: &Settings,
    request: ExecutionRequest,
) -> RundeckResult<ExecutionPage> {
    let (execution, state, detail, history) = futures::join!(
        executions::execution(session, request.execution_id),
        executions::execution_state(session, request.execution_id),
        jobs::job_detail(session, request.job_id.clone()),
        executions::executions(
            session,
            request.job_id,
            request.project.clone(),
            Some(JOB_RUNS),
            None,
        )
    );
    let execution = execution?;
    let request_project = request.project;
    let job = execution.job.as_ref();
    let state = state.ok();
    let detail = detail.ok();
    let raw = execution.status.clone().or_else(|| {
        state
            .as_ref()
            .and_then(|state| state.execution_state.clone())
    });
    let live = is_live(raw.as_deref());
    let step_names = detail.as_ref().map_or(&[][..], |detail| &detail.steps[..]);
    let steps: Vec<RunStep> = state
        .as_ref()
        .map_or(&[][..], |state| &state.steps[..])
        .iter()
        .enumerate()
        .map(|(at, step)| {
            let state_name = step.execution_state.as_deref().unwrap_or("NOT_STARTED");
            RunStep {
                key: step.stepctx.clone().unwrap_or_else(|| (at + 1).to_string()),
                name: step_name(step_names, at),
                state: step_state(Some(state_name)),
                state_name: state_name.to_lowercase().replace('_', " "),
                started_at: step.start_time.clone(),
                ended_at: step.end_time.clone(),
            }
        })
        .collect();
    let (completed, total) = progress(state.as_ref());
    let options = execution
        .job
        .as_ref()
        .and_then(|job| job.options.clone())
        .unwrap_or_default();
    let branch =
        deployed_branch(&execution, &settings.branch_options).filter(|branch| !branch.is_empty());
    let usual = usual_secs(&runs_of(
        &history.unwrap_or_default(),
        &settings.branch_options,
    ));
    Ok(ExecutionPage {
        execution_id: execution.id,
        status: display_status(raw.as_deref(), execution.custom_status.as_deref()),
        status_kind: status_kind(raw.as_deref()),
        live,
        user: execution.user.clone(),
        started_at: execution
            .date_started
            .as_ref()
            .and_then(|date| date.date.clone()),
        ended_at: execution
            .date_ended
            .as_ref()
            .and_then(|date| date.date.clone()),
        permalink: execution.permalink.clone(),
        prod: is_production(
            job.and_then(|job| job.project.as_deref())
                .unwrap_or(&request_project),
            job.and_then(|job| job.group.as_deref()),
            &settings.prod_envs,
        ),
        run_word: if branch.is_some() { "Deploy" } else { "Run" }.to_owned(),
        branch,
        steps,
        completed,
        total,
        percent: (total > 0).then(|| (completed * 100 + total / 2) / total),
        usual_secs: usual,
        can_run_again: execution.job.is_some(),
        repeat_options: reusable_options(
            options,
            detail
                .as_ref()
                .map_or(&[][..], |detail| &detail.options[..]),
        ),
        read_again_secs: live.then_some(LIVE_EXECUTION_SECS),
    })
}

/// How many of a run's steps are done, of how many; Rundeck may count steps
/// whose state has not arrived yet.
fn progress(state: Option<&WorkflowState>) -> (u32, u32) {
    let steps = state.map_or(&[][..], |state| &state.steps[..]);
    let total = state
        .and_then(|state| state.step_count)
        .unwrap_or(0)
        .max(steps.len() as u32);
    let done = steps
        .iter()
        .filter(|step| {
            !matches!(
                step_state(step.execution_state.as_deref()),
                StepState::Pending | StepState::Running
            )
        })
        .count() as u32;
    (done.min(total), total)
}

/// A past run's options that may be shown and reused: never the secure ones.
fn reusable_options(
    options: BTreeMap<String, String>,
    defined: &[jobs::JobOption],
) -> BTreeMap<String, String> {
    let secure: BTreeSet<&str> = defined
        .iter()
        .filter(|option| option.secure)
        .map(|option| option.name.as_str())
        .collect();
    options
        .into_iter()
        .filter(|(name, _)| !secure.contains(name.as_str()))
        .collect()
}

fn step_name(names: &[String], at: usize) -> String {
    names
        .get(at)
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("step {}", at + 1))
}

/// Where Rundeck shows the job.
pub(crate) fn job_permalink(session: &Session, project: &str, job_id: &str) -> String {
    format!(
        "{}/project/{project}/job/show/{job_id}",
        session.url.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executions::Step;

    fn step(state: &str) -> Step {
        Step {
            execution_state: Some(state.to_owned()),
            ..Step::default()
        }
    }

    fn state(steps: Vec<Step>, step_count: u32) -> WorkflowState {
        WorkflowState {
            execution_state: Some("RUNNING".to_owned()),
            steps,
            step_count: Some(step_count),
            completed: Some(false),
        }
    }

    #[test]
    fn counts_finished_workflow_steps() {
        let steps = ["SUCCEEDED", "RUNNING", "NOT_STARTED", "NOT_ELIGIBLE"].map(step);
        assert_eq!(progress(Some(&state(steps.to_vec(), 4))), (2, 4));
    }

    #[test]
    fn uses_the_reported_step_count_when_steps_have_not_all_arrived() {
        assert_eq!(progress(Some(&state(vec![step("SUCCEEDED")], 5))), (1, 5));
    }

    #[test]
    fn drops_secure_options_from_a_past_run() {
        let option = |name: &str, secure: bool| jobs::JobOption {
            name: name.to_owned(),
            label: None,
            description: None,
            required: false,
            secure,
            value_exposed: false,
            default: None,
            values: None,
            values_url: None,
            enforced: false,
            multivalued: false,
            delimiter: None,
            is_date: false,
            date_format: None,
            kind: "text",
        };
        let past = BTreeMap::from([
            ("A".to_owned(), "1".to_owned()),
            ("SECRET".to_owned(), "s".to_owned()),
        ]);
        assert_eq!(
            reusable_options(past, &[option("A", false), option("SECRET", true)]),
            BTreeMap::from([("A".to_owned(), "1".to_owned())])
        );
    }
}
