// The gcp_* tools agents call. Each runs as the account and project the call
// names, else the ones picked for agents, else gcloud's active configuration,
// and answers with the same trimmed shapes the pane reads.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::{PluginContext, PluginResult};

use crate::agent_setup;
use crate::auth::{self, Target};
use crate::error::{GcpError, GcpResult};
use crate::{
    alerting, bigquery, build, compute, dns, errors, functions, gke, iam, logging, metrics,
    network, projects, pubsub, registry, run, scheduler, secrets, services, sql, storage,
};

/// A tool's input as the shape a module reads, saying which field was wrong when one was.
fn read<T: DeserializeOwned>(value: Value) -> GcpResult<T> {
    serde_json::from_value(value).map_err(|error| GcpError::BadArg(error.to_string()))
}

/// The account and project a tool call runs as.
async fn resolve(ctx: &PluginContext, asked: Target) -> GcpResult<Target> {
    let picked = agent_setup::picked(ctx, None);
    let pick = picked_pick(&picked);
    let account = asked
        .account()
        .map(str::to_string)
        .or_else(|| pick.as_ref().and_then(|pick| pick.account.clone()));
    let mut project = asked
        .project
        .clone()
        .filter(|project| !project.trim().is_empty())
        .or_else(|| pick.and_then(|pick| pick.project));
    if project.is_none() {
        project = auth::status()
            .await
            .configurations
            .into_iter()
            .find(|configuration| configuration.active)
            .and_then(|configuration| configuration.project);
    }
    Ok(Target { account, project })
}

fn picked_pick(picked: &agent_setup::Picked) -> Option<agent_setup::Pick> {
    serde_json::to_value(picked)
        .ok()
        .and_then(|value| value.pointer("/effective/pick").cloned())
        .and_then(|pick| serde_json::from_value(pick).ok())
}

fn needs_project(target: &Target) -> GcpResult<()> {
    target.project().map(|_| ()).map_err(|_| GcpError::BadArg("no project was named and none is picked for agents or set in gcloud; list them with gcp_projects".into()))
}

fn field(input: &Value, key: &str) -> Value {
    input.get(key).cloned().unwrap_or(Value::Null)
}

/// The module call's parameters: the resolved account and project with the tool's own fields.
fn with_target(target: &Target, fields: Value) -> Value {
    let mut object = fields.as_object().cloned().unwrap_or_default();
    object.insert("account".into(), json!(target.account));
    object.insert("project".into(), json!(target.project));
    Value::Object(object)
}

fn need(input: &Value, key: &str) -> GcpResult<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| GcpError::BadArg(format!("name the {key} too")))
}

/// Answers an agent tool's method, or `None` when `method` is not one.
pub async fn call(ctx: &PluginContext, method: &str, input: Value) -> Option<PluginResult<Value>> {
    let result = match method {
        "agentProjects" => projects_tool(ctx, input).await,
        "agentList" => list(ctx, input).await,
        "agentGet" => get(ctx, input).await,
        "agentLogs" => logs(ctx, input).await,
        "agentMetrics" => chart(ctx, input).await,
        "agentKube" => kube(ctx, input).await,
        "agentAct" => act(ctx, input).await,
        _ => return None,
    };
    Some(result.map_err(Into::into))
}

async fn projects_tool(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, Target::default()).await?;
    let listed = projects::projects(projects::Listing {
        target: Target {
            account: target.account.clone(),
            project: None,
        },
        query: input
            .get("query")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
    .await?;
    Ok(json!({ "projects": listed, "default": target.project }))
}

async fn list(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    let kind = need(&input, "kind")?;
    let fields = with_target(
        &target,
        json!({ "query": field(&input, "query"), "region": field(&input, "region") }),
    );
    let answer = |value: Value| Ok(value);
    match kind.as_str() {
        "run_services" => answer(json!(run::services(read(fields)?).await?)),
        "run_jobs" => answer(json!(run::jobs(read(fields)?).await?)),
        "functions" => answer(json!(functions::functions(read(fields)?).await?)),
        "instances" => answer(json!(compute::instances(read(fields)?).await?)),
        "clusters" => answer(json!(gke::clusters(read(fields)?).await?)),
        "buckets" => answer(json!(storage::buckets(read(fields)?).await?)),
        "sql" => answer(json!(sql::instances(read(fields)?).await?)),
        "pubsub" => answer(pubsub::topics(read(fields)?).await?),
        "builds" => answer(json!(build::builds(read(fields)?).await?)),
        "triggers" => answer(json!(build::triggers(read(fields)?).await?)),
        "secrets" => answer(json!(secrets::secrets(read(fields)?).await?)),
        "errors" => answer(json!(errors::groups(read(fields)?).await?)),
        "repositories" => answer(json!(registry::repositories(read(fields)?).await?)),
        "scheduler" => answer(json!(scheduler::jobs(read(fields)?).await?)),
        "iam" => answer(json!(iam::members(read(fields)?).await?)),
        "service_accounts" => answer(json!(iam::service_accounts(read(fields)?).await?)),
        "datasets" => answer(json!(bigquery::datasets(read(fields)?).await?)),
        "network" => answer(network::overview(read(fields)?).await?),
        "dns" => answer(json!(dns::zones(read(fields)?).await?)),
        "alerting" => answer(alerting::alerting(read(fields)?).await?),
        "snapshots" => answer(json!(compute::snapshots(read(fields)?).await?)),
        other => Err(GcpError::BadArg(format!(
            "`{other}` is not a kind gcp_list knows"
        ))),
    }
}

async fn get(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    let kind = need(&input, "kind")?;
    let name = need(&input, "name")?;
    let location = field(&input, "location");
    let path = field(&input, "path");
    let located = |key: &str| with_target(&target, json!({ "location": location, key: name }));
    match kind.as_str() {
        "run_service" => Ok(run::service_detail(read(located("service"))?).await?),
        "run_job" => Ok(run::job_detail(read(located("job"))?).await?),
        "function" => Ok(functions::function_detail(read(located("function"))?).await?),
        "instance" => Ok(compute::instance_detail(read(with_target(
            &target,
            json!({ "zone": location, "instance": name }),
        ))?)
        .await?),
        "serial" => Ok(compute::serial(read(with_target(
            &target,
            json!({ "zone": location, "instance": name }),
        ))?)
        .await?),
        "cluster" => Ok(gke::cluster_detail(read(located("cluster"))?).await?),
        "objects" => Ok(storage::objects(read(with_target(
            &target,
            json!({ "bucket": name, "prefix": path }),
        ))?)
        .await?),
        "object" => Ok(storage::preview(read(with_target(
            &target,
            json!({ "bucket": name, "object": need(&input, "path")? }),
        ))?)
        .await?),
        "sql" => Ok(
            sql::instance_detail(read(with_target(&target, json!({ "instance": name })))?).await?,
        ),
        "build" => Ok(build::build_detail(read(with_target(
            &target,
            json!({ "id": name, "region": location }),
        ))?)
        .await?),
        "secret" => Ok(secrets::secret_detail(read(with_target(
            &target,
            json!({ "secret": name }),
        ))?)
        .await?),
        "messages" => Ok(json!(
            pubsub::peek(read(with_target(&target, json!({ "subscription": name })))?).await?
        )),
        other => Err(GcpError::BadArg(format!(
            "`{other}` is not a kind gcp_get knows"
        ))),
    }
}

async fn logs(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    let mut query: logging::Query = read(with_target(&target, input))?;
    query.limit = Some(query.limit.unwrap_or(100).min(300));
    Ok(json!(logging::search(query).await?))
}

async fn chart(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    metrics::chart(read(with_target(&target, input))?).await
}

#[derive(Deserialize)]
struct Kube {
    what: String,
}

async fn kube(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    let Kube { what } = read(input.clone())?;
    let fields = with_target(&target, input);
    match what.as_str() {
        "workloads" => Ok(json!(gke::workloads(read(fields)?).await?)),
        "pods" => Ok(json!(gke::pods(read(fields)?).await?)),
        "services" => Ok(json!(gke::services(read(fields)?).await?)),
        "nodes" => Ok(json!(gke::nodes(read(fields)?).await?)),
        "events" => Ok(json!(gke::pod_events(read(fields)?).await?)),
        "logs" => {
            let mut logs: gke::PodLogs = read(fields)?;
            logs.tail = Some(logs.tail.unwrap_or(200).min(1000));
            gke::pod_logs(logs).await
        }
        other => Err(GcpError::BadArg(format!(
            "`{other}` is not something gcp_kube reads"
        ))),
    }
}

async fn act(ctx: &PluginContext, input: Value) -> GcpResult<Value> {
    let target = resolve(ctx, read(input.clone())?).await?;
    needs_project(&target)?;
    let action = need(&input, "action")?;
    if action == "bq_estimate" || action == "bq_query" {
        let sql = need(&input, "data")?;
        return bigquery::query(read(with_target(
            &target,
            json!({ "sql": sql, "dryRun": action == "bq_estimate" }),
        ))?)
        .await;
    }
    let name = need(&input, "name")?;
    let location = field(&input, "location");
    let revision = field(&input, "revision");
    let count = field(&input, "count");
    let with = |fields: Value| with_target(&target, fields);
    let vm = |action: &str| with(json!({ "zone": location, "instance": name, "action": action }));
    let database = |action: &str| with(json!({ "instance": name, "action": action }));
    match action.as_str() {
        "run_traffic" => run::set_traffic(read(with(json!({ "location": location, "service": name, "revision": revision })))?).await,
        "run_deploy" => run::deploy(read(with(json!({ "location": location, "service": name, "image": field(&input, "data") })))?).await,
        "run_scale" => run::deploy(read(with(json!({ "location": location, "service": name, "minInstances": count })))?).await,
        "run_job" => run::run_job(read(with(json!({ "location": location, "job": name })))?).await,
        "vm_start" => compute::power(read(vm("start"))?).await,
        "vm_stop" => compute::power(read(vm("stop"))?).await,
        "vm_reset" => compute::power(read(vm("reset"))?).await,
        "vm_suspend" => compute::power(read(vm("suspend"))?).await,
        "vm_resume" => compute::power(read(vm("resume"))?).await,
        "sql_start" => sql::change(read(database("start"))?).await,
        "sql_stop" => sql::change(read(database("stop"))?).await,
        "sql_restart" => sql::change(read(database("restart"))?).await,
        "sql_backup" => sql::change(read(database("backup"))?).await,
        "build_retry" => build::change(read(with(json!({ "id": name, "region": location, "action": "retry" })))?).await,
        "build_cancel" => build::change(read(with(json!({ "id": name, "region": location, "action": "cancel" })))?).await,
        "trigger_run" => build::run_trigger(read(with(json!({ "trigger": name, "region": location, "branch": field(&input, "branch") })))?).await,
        "deployment_restart" => {
            let fields = with(json!({ "location": location, "cluster": need(&input, "cluster")?, "namespace": need(&input, "namespace")?, "deployment": name }));
            gke::restart(read(fields)?, &logging::rfc3339_before(crate::now_secs(), 0)).await
        }
        "deployment_scale" => {
            let fields = with(json!({ "location": location, "cluster": need(&input, "cluster")?, "namespace": need(&input, "namespace")?, "deployment": name, "replicas": count }));
            gke::scale(read(fields)?).await
        }
        "node_pool_resize" => gke::resize_pool(read(with(json!({ "location": location, "cluster": need(&input, "cluster")?, "nodePool": name, "nodes": count })))?).await,
        "publish" => pubsub::publish(read(with(json!({ "topic": name, "data": need(&input, "data")? })))?).await,
        "secret_enable" | "secret_disable" => {
            secrets::set_version(read(with(json!({ "secret": name, "version": need(&input, "revision")?, "enabled": action == "secret_enable" })))?).await
        }
        "error_resolve" | "error_ack" | "error_mute" | "error_open" => {
            let status = match action.as_str() {
                "error_resolve" => "RESOLVED",
                "error_ack" => "ACKNOWLEDGED",
                "error_mute" => "MUTED",
                _ => "OPEN",
            };
            errors::set_status(read(with(json!({ "group": name, "status": status })))?).await
        }
        "vm_snapshot" => compute::take_snapshot(read(with(json!({ "zone": location, "disk": name })))?, &logging::rfc3339_before(crate::now_secs(), 0)).await,
        "firewall_disable" | "firewall_enable" => network::set_firewall(read(with(json!({ "rule": name, "disabled": action == "firewall_disable" })))?).await,
        "alert_disable" | "alert_enable" => alerting::set_policy(read(with(json!({ "policy": name, "enabled": action == "alert_enable" })))?).await,
        "schedule_run" | "schedule_pause" | "schedule_resume" => {
            let change = action.trim_start_matches("schedule_");
            scheduler::change(read(with(json!({ "location": location, "job": name, "action": change })))?).await
        }
        "api_enable" => services::enable(read(with(json!({ "service": name })))?).await,
        other => Err(GcpError::BadArg(format!("`{other}` is not an action gcp_act knows"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tools_fields_ride_beside_the_resolved_target() {
        let target = Target {
            account: Some("ankit@example.com".into()),
            project: Some("acme".into()),
        };
        let fields = with_target(&target, json!({ "service": "api", "project": "ignored" }));
        assert_eq!(fields["project"], "acme");
        assert_eq!(fields["account"], "ankit@example.com");
        assert_eq!(fields["service"], "api");
        assert!(need(&json!({ "name": "  " }), "name").is_err());
    }

    #[test]
    fn a_call_without_a_project_says_how_to_find_one() {
        let error = needs_project(&Target::default()).expect_err("no project");
        assert!(error.to_string().contains("gcp_projects"));
    }
}
