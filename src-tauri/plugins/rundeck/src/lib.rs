// Rundeck integration: projects, jobs, the deploy matrix, running jobs and
// following them live. Rundeck speaks bearer-token REST, so we call the API
// directly with reqwest.
//
// Module split:
//   agent       — what the rundeck_* agent tools answer
//   config      — the saved sign-in and settings, and whether the URL is safe to call
//   client      — shared HTTP client, error classification, request limiter
//   auth        — password login, token login, logout, status
//   projects    — projects, jobs, job index, the matrix
//   jobs        — job definition and remote option values
//   executions  — history, a project's runs, run, abort, latest/deployed summaries
//   watch       — stream of execution state until it finishes
//   logs        — stream of log output with an offset cursor
//   plan        — read-only git inspection (dirty / ahead-behind / relation)

mod agent;
mod auth;
mod client;
mod config;
mod error;
mod executions;
mod jobs;
mod logs;
mod plan;
mod projects;
mod watch;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::client::Session;
use crate::config::Saved;
use crate::error::RundeckResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Rundeck {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Rundeck {
    manifest: Manifest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectParams {
    project: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobParams {
    job_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UrlParams {
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobCellsParams {
    jobs: Vec<projects::RundeckJob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionsParams {
    job_id: String,
    project: String,
    max: Option<u32>,
    only_succeeded: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectExecutionsParams {
    project: String,
    max: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunParams {
    job_id: String,
    #[serde(flatten)]
    request: executions::RunRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanParams {
    job_id: String,
    project: String,
    service: String,
    target_branch: String,
    #[serde(default)]
    repo_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindJobsParams {
    project: Option<String>,
    filter: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutcomeParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
    #[serde(default)]
    wait: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExcerptParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
    tail: Option<usize>,
    grep: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
    offset: Option<String>,
    backlog: Option<u32>,
}

async fn signed_in(ctx: &PluginContext) -> RundeckResult<(Session, Saved)> {
    let saved = config::load(ctx.data_dir()).await?;
    Ok((Session::of(&saved)?, saved))
}

impl Plugin for Rundeck {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "status" => return reply(auth::status(data_dir).await),
                "login" => return answer(auth::login(data_dir, params(input)?)).await,
                "loginWithToken" => {
                    return answer(auth::login_with_token(data_dir, params(input)?)).await
                }
                "logout" => return answer(auth::logout(data_dir)).await,
                "lists" => return answer(auth::lists(data_dir)).await,
                "setLists" => return answer(auth::set_lists(data_dir, params(input)?)).await,
                _ => {}
            }
            let (session, saved) = signed_in(ctx).await?;
            let session = &session;
            let branch_options = &saved.settings.branch_options;
            match method {
                "projects" => answer(projects::projects(session)).await,
                "jobs" => {
                    let ProjectParams { project } = params(input)?;
                    answer(projects::jobs(session, project)).await
                }
                "jobIndex" => answer(projects::job_index(session)).await,
                "jobDetail" => {
                    let JobParams { job_id } = params(input)?;
                    answer(jobs::job_detail(session, job_id)).await
                }
                "optionValues" => {
                    let UrlParams { url } = params(input)?;
                    answer(jobs::option_values(url)).await
                }
                "branchesMatrix" => {
                    let ProjectParams { project } = params(input)?;
                    answer(projects::branches_matrix(session, project, branch_options)).await
                }
                "jobCells" => {
                    let JobCellsParams { jobs } = params(input)?;
                    answer(projects::job_cells(session, jobs, branch_options)).await
                }
                "executions" => {
                    let p: ExecutionsParams = params(input)?;
                    answer(executions::executions(
                        session,
                        p.job_id,
                        p.project,
                        p.max,
                        p.only_succeeded,
                    ))
                    .await
                }
                "projectExecutions" => {
                    let p: ProjectExecutionsParams = params(input)?;
                    answer(executions::project_executions(
                        session, p.project, p.max, p.offset,
                    ))
                    .await
                }
                "execution" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::execution(session, execution_id)).await
                }
                "executionState" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::execution_state(session, execution_id)).await
                }
                "run" => {
                    let RunParams { job_id, request } = params(input)?;
                    answer(executions::run(session, job_id, request)).await
                }
                "abort" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::abort(session, execution_id)).await
                }
                "plan" => {
                    let p: PlanParams = params(input)?;
                    answer(plan::plan(
                        session,
                        plan::PlanRequest {
                            job_id: p.job_id,
                            project: p.project,
                            service: p.service,
                            target_branch: p.target_branch,
                            repo_path: p.repo_path,
                            branch_options: branch_options.clone(),
                        },
                    ))
                    .await
                }
                "agentJobs" => {
                    let FindJobsParams { project, filter } = params(input)?;
                    answer(agent::find_jobs(session, project, filter)).await
                }
                "agentRun" => {
                    answer(agent::run(
                        session,
                        params(input)?,
                        &saved.settings.prod_envs,
                    ))
                    .await
                }
                "agentExecutions" => {
                    answer(agent::history(session, params(input)?, branch_options)).await
                }
                "agentExecution" => {
                    let OutcomeParams { execution_id, wait } = params(input)?;
                    answer(agent::execution(session, execution_id, wait)).await
                }
                "agentLogs" => {
                    let ExcerptParams {
                        execution_id,
                        tail,
                        grep,
                    } = params(input)?;
                    answer(logs::excerpt(
                        session,
                        execution_id,
                        tail.unwrap_or(100),
                        grep.as_deref(),
                    ))
                    .await
                }
                "agentPlan" => answer(agent::plan(session, params(input)?, branch_options)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(config::load(ctx.data_dir()).await?.is_signed_in()) })
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            match method {
                "watch" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    let (session, _) = signed_in(ctx).await?;
                    watch::watch(session, execution_id, sink).await
                }
                "logs" => {
                    let LogsParams {
                        execution_id,
                        offset,
                        backlog,
                    } = params(input)?;
                    let (session, _) = signed_in(ctx).await?;
                    logs::logs(session, execution_id, offset, backlog, sink).await
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_manifest_parses() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.rundeck");
    }

    #[test]
    fn run_params_take_the_job_id_and_skip_missing_fields() {
        let p: RunParams = params(serde_json::json!({
            "jobId": "abc",
            "options": {"BRANCH": "main"},
            "loglevel": "DEBUG",
            "filter": null,
            "runAtTime": null,
            "asUser": null
        }))
        .unwrap();
        assert_eq!(p.job_id, "abc");
        assert_eq!(
            serde_json::to_value(&p.request).unwrap(),
            serde_json::json!({"options": {"BRANCH": "main"}, "loglevel": "DEBUG"})
        );
    }

    #[test]
    fn execution_ids_may_be_strings() {
        let p: ExecutionParams = params(serde_json::json!({"executionId": "42"})).unwrap();
        assert_eq!(p.execution_id, 42);
    }
}
