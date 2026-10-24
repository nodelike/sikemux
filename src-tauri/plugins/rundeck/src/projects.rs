// Projects and jobs, plus the matrix: every job of one project with its
// newest execution and its newest successful one, fetched in one call.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::time::timeout_at;

use crate::error::RundeckResult;

use crate::client::{get_json, seg, Session, HTTP};
use crate::executions::{latest_and_deployed, ExecSummary};

const MATRIX_DEADLINE: Duration = Duration::from_secs(40);

// ---- projects ------------------------------------------------------------

#[derive(Serialize, Clone, Deserialize)]
pub struct RundeckProject {
    pub name: String,
    pub description: Option<String>,
}

pub async fn projects(session: &Session) -> RundeckResult<Vec<RundeckProject>> {
    let mut out: Vec<RundeckProject> = get_json(session, "/projects", &[]).await?;
    out.sort_by_key(|project| project.name.to_lowercase());
    Ok(out)
}

// ---- jobs ---------------------------------------------------------------

#[derive(Serialize, Clone, Deserialize)]
pub struct RundeckJob {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub project: String,
    pub description: Option<String>,
    pub href: Option<String>,
    pub permalink: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub scheduled: Option<bool>,
    #[serde(rename = "scheduleEnabled", default)]
    pub schedule_enabled: Option<bool>,
}

impl RundeckJob {
    /// "group/name" or "name" when no group — same display the CLI uses.
    pub fn qualified_name(&self) -> String {
        match self.group.as_deref() {
            Some(g) if !g.is_empty() => format!("{g}/{}", self.name),
            _ => self.name.clone(),
        }
    }
}

pub async fn jobs(session: &Session, project: String) -> RundeckResult<Vec<RundeckJob>> {
    let path = format!("/project/{}/jobs", seg(&project)?);
    let mut out: Vec<RundeckJob> = get_json(session, &path, &[]).await?;
    out.sort_by_key(|job| job.qualified_name());
    Ok(out)
}

#[derive(Serialize)]
pub struct JobIndexEntry {
    pub project: String,
    pub jobs: Vec<RundeckJob>,
    pub error: Option<String>,
}

pub async fn job_index(session: &Session) -> RundeckResult<Vec<JobIndexEntry>> {
    let all = projects(session).await?;
    Ok(join_all(all.into_iter().map(|project| async move {
        match HTTP.limited(jobs(session, project.name.clone())).await {
            Ok(jobs) => JobIndexEntry {
                project: project.name,
                jobs,
                error: None,
            },
            Err(error) => JobIndexEntry {
                project: project.name,
                jobs: Vec::new(),
                error: Some(error.to_string()),
            },
        }
    }))
    .await)
}

/// How long the list of every job is reused by the top bar and the job search,
/// which would otherwise ask every project for its jobs each time they look.
const INDEX_KEPT: Duration = Duration::from_secs(300);

struct KeptIndex {
    account: String,
    read: Instant,
    index: Arc<Vec<JobIndexEntry>>,
}

static INDEX: Mutex<Option<KeptIndex>> = Mutex::new(None);

/// Every project's jobs, read again once the copy kept for this account is a few minutes old.
pub async fn recent_job_index(session: &Session) -> RundeckResult<Arc<Vec<JobIndexEntry>>> {
    let account = format!("{} {}", session.url, session.user);
    let kept = INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|kept| kept.account == account && kept.read.elapsed() < INDEX_KEPT)
        .map(|kept| kept.index.clone());
    if let Some(index) = kept {
        return Ok(index);
    }
    let index = Arc::new(job_index(session).await?);
    *INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(KeptIndex {
        account,
        read: Instant::now(),
        index: index.clone(),
    });
    Ok(index)
}

// ---- matrix ---------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct MatrixCell {
    pub service: String,
    pub name: String,
    pub job_id: String,
    pub group: Option<String>,
    pub enabled: Option<bool>,
    pub scheduled: Option<bool>,
    pub latest: Option<ExecSummary>,
    pub deployed: Option<ExecSummary>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct MatrixResult {
    pub project: String,
    pub cells: Vec<MatrixCell>,
    pub error: Option<String>,
    pub partial: bool,
    pub elapsed_ms: u64,
}

fn empty_cell(job: &RundeckJob) -> MatrixCell {
    MatrixCell {
        service: job.qualified_name(),
        name: job.name.clone(),
        job_id: job.id.clone(),
        group: job.group.clone(),
        enabled: job.enabled,
        scheduled: job.scheduled,
        latest: None,
        deployed: None,
        error: None,
    }
}

async fn cell_for(session: &Session, job: &RundeckJob, branch_options: &[String]) -> MatrixCell {
    let mut cell = empty_cell(job);
    match latest_and_deployed(session, &job.id, branch_options).await {
        Ok((latest, deployed)) => {
            cell.latest = latest;
            cell.deployed = deployed;
        }
        Err(error) => cell.error = Some(error.to_string()),
    }
    cell
}

/// One cell per job, in input order. Cells still loading at `deadline` come
/// back with a "timed out" error; the flag says whether any did.
async fn cells_until(
    session: &Session,
    jobs: &[RundeckJob],
    branch_options: &[String],
    deadline: tokio::time::Instant,
) -> (Vec<MatrixCell>, bool) {
    let cells = join_all(jobs.iter().map(|job| async move {
        match timeout_at(deadline, cell_for(session, job, branch_options)).await {
            Ok(cell) => (cell, false),
            Err(_) => {
                let mut cell = empty_cell(job);
                cell.error = Some("timed out".into());
                (cell, true)
            }
        }
    }))
    .await;
    let partial = cells.iter().any(|(_, timed_out)| *timed_out);
    (cells.into_iter().map(|(cell, _)| cell).collect(), partial)
}

pub async fn branches_matrix(
    session: &Session,
    project: String,
    branch_options: &[String],
) -> RundeckResult<MatrixResult> {
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + MATRIX_DEADLINE;
    let elapsed_ms = |started: Instant| started.elapsed().as_millis() as u64;

    let listed = match timeout_at(deadline, HTTP.limited(jobs(session, project.clone()))).await {
        Ok(listed) => listed,
        Err(_) => {
            return Ok(MatrixResult {
                project,
                cells: Vec::new(),
                error: Some("timed out".into()),
                partial: true,
                elapsed_ms: elapsed_ms(started),
            })
        }
    };
    let jobs = match listed {
        Ok(jobs) => jobs,
        Err(error) => {
            return Ok(MatrixResult {
                project,
                cells: Vec::new(),
                error: Some(error.to_string()),
                partial: false,
                elapsed_ms: elapsed_ms(started),
            })
        }
    };
    let (cells, partial) = cells_until(session, &jobs, branch_options, deadline).await;
    Ok(MatrixResult {
        project,
        cells,
        error: None,
        partial,
        elapsed_ms: elapsed_ms(started),
    })
}

pub async fn job_cells(
    session: &Session,
    jobs: Vec<RundeckJob>,
    branch_options: &[String],
) -> RundeckResult<Vec<MatrixCell>> {
    let deadline = tokio::time::Instant::now() + MATRIX_DEADLINE;
    Ok(cells_until(session, &jobs, branch_options, deadline)
        .await
        .0)
}
