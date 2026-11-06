// Rundeck integration: projects, jobs, running jobs and following them, as agent
// tools. Rundeck speaks bearer-token REST, so we call the API directly with reqwest.

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
mod shape;
mod watch;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

use crate::client::Session;
use crate::config::Saved;
use crate::error::RundeckResult;

sikemux_plugin_api::plugin!(Rundeck);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionParams {
    #[serde(deserialize_with = "executions::id_from_string_or_number")]
    execution_id: u64,
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

async fn signed_in(ctx: &PluginContext) -> RundeckResult<(Session, Saved)> {
    let saved = config::load(ctx.data_dir()).await?;
    Ok((Session::of(&saved)?, saved))
}

impl Plugin for Rundeck {
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
                "signInWithToken" => {
                    return answer(auth::login_with_token(data_dir, params(input)?)).await
                }
                "signOut" => return answer(auth::logout(data_dir)).await,
                "lists" => return answer(auth::lists(data_dir)).await,
                "setLists" => return answer(auth::set_lists(data_dir, params(input)?)).await,
                _ => {}
            }
            let (session, saved) = signed_in(ctx).await?;
            let session = &session;
            let branch_options = &saved.settings.branch_options;
            match method {
                "projects" => answer(projects::projects(session)).await,
                "abort" => {
                    let ExecutionParams { execution_id } = params(input)?;
                    answer(executions::abort(session, execution_id)).await
                }
                "agentJobs" => {
                    let FindJobsParams { project, filter } = params(input)?;
                    answer(agent::find_jobs(session, project, filter)).await
                }
                "agentJobPlace" => {
                    answer(agent::place(
                        session,
                        params(input)?,
                        &saved.settings.prod_envs,
                    ))
                    .await
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
        Box::pin(async move {
            Ok(config::load(ctx.data_dir())
                .await
                .is_ok_and(|saved| saved.is_signed_in()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_ids_may_be_strings() {
        let p: ExecutionParams = params(serde_json::json!({"executionId": "42"})).unwrap();
        assert_eq!(p.execution_id, 42);
    }
}
