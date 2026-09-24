// Rundeck integration: projects, jobs, the deploy matrix, running jobs and
// following them live. Rundeck speaks bearer-token REST, so we call the API
// directly with reqwest rather than shelling out to the `rnd` CLI, and share
// `~/.rd-config` with that CLI so a login in either place works in both.
//
// Module split:
//   config      — read/write ~/.rd-config and check the URL is safe to call
//   client      — shared HTTP client, error classification, request limiter
//   auth        — password login, token login, logout, status
//   projects    — projects, jobs, job index, the matrix
//   jobs        — job definition and remote option values
//   executions  — history, run, abort, latest/deployed summaries
//   watch       — stream of execution state until it finishes
//   logs        — stream of log output with an offset cursor
//   plan        — read-only git inspection (dirty / ahead-behind / relation)

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
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

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
struct MatrixParams {
    project: String,
    #[serde(default)]
    branch_options: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobCellsParams {
    jobs: Vec<projects::RundeckJob>,
    #[serde(default)]
    branch_options: Vec<String>,
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
    #[serde(default)]
    branch_options: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
    offset: Option<String>,
    backlog: Option<u32>,
}

async fn answer<T: serde::Serialize>(
    result: impl std::future::Future<Output = RundeckResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Rundeck {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            match method {
                "status" => reply(auth::status().await),
                "login" => answer(auth::login(params(input)?)).await,
                "loginWithToken" => answer(auth::login_with_token(params(input)?)).await,
                "logout" => answer(auth::logout()).await,
                "projects" => answer(projects::projects()).await,
                "jobs" => {
                    let ProjectParams { project } = params(input)?;
                    answer(projects::jobs(project)).await
                }
                "jobIndex" => answer(projects::job_index()).await,
                "jobDetail" => {
                    let JobParams { job_id } = params(input)?;
                    answer(jobs::job_detail(job_id)).await
                }
                "optionValues" => {
                    let UrlParams { url } = params(input)?;
                    answer(jobs::option_values(url)).await
                }
                "branchesMatrix" => {
                    let MatrixParams {
                        project,
                        branch_options,
                    } = params(input)?;
                    answer(projects::branches_matrix(project, branch_options)).await
                }
                "jobCells" => {
                    let JobCellsParams {
                        jobs,
                        branch_options,
                    } = params(input)?;
                    answer(projects::job_cells(jobs, branch_options)).await
                }
                "executions" => {
                    let p: ExecutionsParams = params(input)?;
                    answer(executions::executions(
                        p.job_id,
                        p.project,
                        p.max,
                        p.only_succeeded,
                    ))
                    .await
                }
                "execution" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::execution(execution_id)).await
                }
                "executionState" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::execution_state(execution_id)).await
                }
                "run" => {
                    let RunParams { job_id, request } = params(input)?;
                    answer(executions::run(job_id, request)).await
                }
                "abort" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::abort(execution_id)).await
                }
                "plan" => {
                    let p: PlanParams = params(input)?;
                    answer(plan::plan(plan::PlanRequest {
                        job_id: p.job_id,
                        project: p.project,
                        service: p.service,
                        target_branch: p.target_branch,
                        repo_path: p.repo_path,
                        branch_options: p.branch_options,
                    }))
                    .await
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            match method {
                "watch" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    watch::watch(execution_id, sink).await
                }
                "logs" => {
                    let LogsParams {
                        execution_id,
                        offset,
                        backlog,
                    } = params(input)?;
                    logs::logs(execution_id, offset, backlog, sink).await
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
