//! Where jobs live, for picking one: the jobs a project on this host deploys
//! with, and every job to search.

use sikemux_plugin_rundeck_views::{DeployTarget, JobCatalog, JobPlace, LastRun, TargetRequest};

use crate::client::Session;
use crate::config::Settings;
use crate::error::RundeckResult;
use crate::projects::{self, RundeckJob};
use crate::shape::{
    display_status, group_segments, is_live, is_production, status_kind, target_tone,
};

const MAX_MATCHES: usize = 12;
const LIVE_TARGET_SECS: u64 = 10;

fn place_of(job: &RundeckJob, prod_envs: &[String]) -> JobPlace {
    JobPlace {
        project: job.project.clone(),
        job_id: job.id.clone(),
        name: job.name.clone(),
        group: job.group.clone().filter(|group| !group.is_empty()),
        place: std::iter::once(job.project.as_str())
            .chain(group_segments(job.group.as_deref()))
            .chain([job.name.as_str()])
            .collect::<Vec<_>>()
            .join(" / "),
        tone: target_tone(&job.project, job.group.as_deref(), prod_envs),
    }
}

pub async fn deploy_target_view(
    session: &Session,
    settings: &Settings,
    request: TargetRequest,
) -> RundeckResult<DeployTarget> {
    let index = projects::recent_job_index(session).await?;
    let all: Vec<&RundeckJob> = index.iter().flat_map(|entry| &entry.jobs).collect();
    let exact: Vec<&RundeckJob> = all
        .iter()
        .copied()
        .filter(|job| job.name == request.folder)
        .collect();
    let mut matches = match exact.is_empty() {
        true => all
            .into_iter()
            .filter(|job| job.name.to_lowercase() == request.folder.to_lowercase())
            .collect(),
        false => exact,
    };
    let prod_envs = &settings.prod_envs;
    matches.sort_by_cached_key(|job| place_of(job, prod_envs).place);
    matches.truncate(MAX_MATCHES);
    let active = matches
        .iter()
        .find(|job| {
            request
                .picked
                .as_ref()
                .is_some_and(|picked| picked.project == job.project && picked.job_id == job.id)
        })
        .or_else(|| {
            matches
                .iter()
                .find(|job| !is_production(&job.project, job.group.as_deref(), prod_envs))
        })
        .or(matches.first())
        .copied();
    let Some(active) = active else {
        return Ok(DeployTarget {
            jobs: Vec::new(),
            active: None,
            deployed_branch: None,
            runs_branch: false,
            last_run: None,
            summary: String::new(),
            read_again_secs: None,
        });
    };
    let cells = projects::job_cells(
        session,
        matches.iter().map(|job| (*job).clone()).collect(),
        &settings.branch_options,
    )
    .await?;
    let deploying = cells.iter().any(|cell| {
        cell.latest
            .as_ref()
            .is_some_and(|run| is_live(run.status.as_deref()))
    });
    let cell = cells.iter().find(|cell| cell.job_id == active.id);
    let deployed_branch = cell
        .and_then(|cell| cell.deployed.as_ref()?.branch.clone())
        .filter(|branch| !branch.is_empty());
    let latest = cell.and_then(|cell| cell.latest.as_ref());
    let last_run = latest.map(|run| LastRun {
        status_kind: status_kind(run.status.as_deref()),
        text: format!(
            "Last run {}{}",
            display_status(run.status.as_deref(), run.custom_status.as_deref()).to_lowercase(),
            run.branch
                .as_ref()
                .filter(|branch| !branch.is_empty())
                .map(|branch| format!(" · {branch}"))
                .unwrap_or_default()
        ),
    });
    let active = place_of(active, prod_envs);
    let summary = [
        Some(match &deployed_branch {
            Some(branch) => format!("{} runs {branch}", active.place),
            None => active.place.clone(),
        }),
        last_run.as_ref().map(|run| run.text.clone()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Ok(DeployTarget {
        jobs: matches.iter().map(|job| place_of(job, prod_envs)).collect(),
        runs_branch: cell.is_none()
            || deployed_branch.is_some()
            || latest.is_some_and(|run| run.branch.is_some()),
        active: Some(active),
        deployed_branch,
        last_run,
        summary,
        read_again_secs: deploying.then_some(LIVE_TARGET_SECS),
    })
}

pub async fn catalog_view(session: &Session, settings: &Settings) -> RundeckResult<JobCatalog> {
    let index = projects::recent_job_index(session).await?;
    let mut jobs: Vec<&RundeckJob> = index.iter().flat_map(|entry| &entry.jobs).collect();
    jobs.sort_by(|a, b| {
        (&a.project, a.group.as_deref().unwrap_or_default(), &a.name).cmp(&(
            &b.project,
            b.group.as_deref().unwrap_or_default(),
            &b.name,
        ))
    });
    Ok(JobCatalog {
        jobs: jobs
            .into_iter()
            .map(|job| place_of(job, &settings.prod_envs))
            .collect(),
        failed_projects: index.iter().filter(|entry| entry.error.is_some()).count() as u32,
    })
}
