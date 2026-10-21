// Google Cloud: a console over the projects the person's gcloud accounts can
// see, and gcp_* tools for agents.
//
//   agent       — the gcp_* tools agents call
//   agent_setup — the account and project agents' shells start on
//   gcloud   — the gcloud CLI, which holds the sign-in; nothing here stores a credential
//   auth     — accounts, configurations, and the access tokens gcloud hands out
//   client   — Google's REST APIs, pages and errors
//   projects — the projects an account can see
//   run      — Cloud Run services, revisions, traffic, jobs and executions
//   logging  — searching and following a project's logs
//   compute  — Compute Engine VMs, their power, serial output and SSH
//   gke      — GKE clusters, node pools, workloads, pods, services and pod logs
//   storage  — Cloud Storage buckets, folders, previews and downloads
//   sql      — Cloud SQL instances, databases, users, backups and power
//   pubsub   — Pub/Sub topics, subscriptions, publishing and peeking
//   build    — Cloud Build builds, logs, triggers, retries and cancels
//   secrets  — Secret Manager secrets and versions, by name only
//   functions — Cloud Functions of both generations
//   metrics  — Cloud Monitoring charts for services, VMs, databases and queues
//   errors   — Error Reporting groups, occurrences and statuses
//   registry — Artifact Registry repositories, images and packages
//   scheduler — Cloud Scheduler jobs, run now, paused or resumed
//   iam      — who holds which role, and service accounts
//   bigquery — datasets, tables, previews and capped queries
//   billing  — whether the project bills, to which account, and its budgets
//   network  — VPC networks, subnets and firewall rules
//   dns      — Cloud DNS zones and records
//   alerting — alert policies, uptime checks and notification channels

mod agent;
mod agent_setup;
mod alerting;
mod auth;
mod bigquery;
mod billing;
mod build;
mod client;
mod compute;
mod dns;
mod error;
mod errors;
mod functions;
mod gcloud;
mod gke;
mod iam;
mod logging;
mod metrics;
mod network;
mod projects;
mod pubsub;
mod registry;
mod run;
mod scheduler;
mod secrets;
mod services;
mod sql;
mod storage;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::GcpResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Gcp {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Gcp {
    manifest: Manifest,
}

#[derive(Deserialize)]
struct PickedParams {
    folder: Option<PathBuf>,
}

#[derive(Deserialize)]
struct PickParams {
    folder: Option<PathBuf>,
    #[serde(flatten)]
    pick: agent_setup::Pick,
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = GcpResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Gcp {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn agent_environment(&self, ctx: &PluginContext, project: &Path) -> BTreeMap<String, String> {
        agent_setup::environment(ctx, project)
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            match method {
                "status" => reply(auth::status().await),
                "agentPicks" => {
                    let PickedParams { folder } = params(input)?;
                    reply(agent_setup::picked(ctx, folder.as_deref()))
                }
                "pickForAgents" => {
                    let PickParams { folder, pick } = params(input)?;
                    reply(agent_setup::pick(ctx, folder.as_deref(), pick)?)
                }
                "projects" => answer(projects::projects(params(input)?)).await,
                "runServices" => answer(run::services(params(input)?)).await,
                "runService" => answer(run::service_detail(params(input)?)).await,
                "runSetTraffic" => answer(run::set_traffic(params(input)?)).await,
                "runDeploy" => answer(run::deploy(params(input)?)).await,
                "runJobs" => answer(run::jobs(params(input)?)).await,
                "runJob" => answer(run::job_detail(params(input)?)).await,
                "runJobExecute" => answer(run::run_job(params(input)?)).await,
                "logs" => answer(logging::search(params(input)?)).await,
                "instances" => answer(compute::instances(params(input)?)).await,
                "instance" => answer(compute::instance_detail(params(input)?)).await,
                "instancePower" => answer(compute::power(params(input)?)).await,
                "instanceSerial" => answer(compute::serial(params(input)?)).await,
                "snapshots" => answer(compute::snapshots(params(input)?)).await,
                "snapshotDisk" => {
                    answer(compute::take_snapshot(
                        params(input)?,
                        &logging::rfc3339_before(now_secs(), 0),
                    ))
                    .await
                }
                "clusters" => answer(gke::clusters(params(input)?)).await,
                "cluster" => answer(gke::cluster_detail(params(input)?)).await,
                "nodePoolResize" => answer(gke::resize_pool(params(input)?)).await,
                "workloads" => answer(gke::workloads(params(input)?)).await,
                "pods" => answer(gke::pods(params(input)?)).await,
                "kubeServices" => answer(gke::services(params(input)?)).await,
                "podLogs" => answer(gke::pod_logs(params(input)?)).await,
                "nodes" => answer(gke::nodes(params(input)?)).await,
                "podEvents" => answer(gke::pod_events(params(input)?)).await,
                "deploymentRestart" => {
                    answer(gke::restart(
                        params(input)?,
                        &logging::rfc3339_before(now_secs(), 0),
                    ))
                    .await
                }
                "deploymentScale" => answer(gke::scale(params(input)?)).await,
                "buckets" => answer(storage::buckets(params(input)?)).await,
                "objects" => answer(storage::objects(params(input)?)).await,
                "objectPreview" => answer(storage::preview(params(input)?)).await,
                "objectDownload" => answer(storage::download(params(input)?)).await,
                "objectUpload" => answer(storage::upload(params(input)?)).await,
                "sqlInstances" => answer(sql::instances(params(input)?)).await,
                "sqlInstance" => answer(sql::instance_detail(params(input)?)).await,
                "sqlChange" => answer(sql::change(params(input)?)).await,
                "topics" => answer(pubsub::topics(params(input)?)).await,
                "publish" => answer(pubsub::publish(params(input)?)).await,
                "peek" => answer(pubsub::peek(params(input)?)).await,
                "builds" => answer(build::builds(params(input)?)).await,
                "build" => answer(build::build_detail(params(input)?)).await,
                "buildChange" => answer(build::change(params(input)?)).await,
                "triggers" => answer(build::triggers(params(input)?)).await,
                "triggerRun" => answer(build::run_trigger(params(input)?)).await,
                "secrets" => answer(secrets::secrets(params(input)?)).await,
                "secret" => answer(secrets::secret_detail(params(input)?)).await,
                "secretVersion" => answer(secrets::set_version(params(input)?)).await,
                "functions" => answer(functions::functions(params(input)?)).await,
                "function" => answer(functions::function_detail(params(input)?)).await,
                "metrics" => answer(metrics::chart(params(input)?)).await,
                "errorGroups" => answer(errors::groups(params(input)?)).await,
                "errorEvents" => answer(errors::events(params(input)?)).await,
                "errorStatus" => answer(errors::set_status(params(input)?)).await,
                "repositories" => answer(registry::repositories(params(input)?)).await,
                "repository" => answer(registry::contents(params(input)?)).await,
                "schedulerJobs" => answer(scheduler::jobs(params(input)?)).await,
                "schedulerChange" => answer(scheduler::change(params(input)?)).await,
                "iamMembers" => answer(iam::members(params(input)?)).await,
                "serviceAccounts" => answer(iam::service_accounts(params(input)?)).await,
                "billing" => answer(billing::billing(params(input)?)).await,
                "network" => answer(network::overview(params(input)?)).await,
                "firewallToggle" => answer(network::set_firewall(params(input)?)).await,
                "dnsZones" => answer(dns::zones(params(input)?)).await,
                "dnsRecords" => answer(dns::records(params(input)?)).await,
                "alerting" => answer(alerting::alerting(params(input)?)).await,
                "alertPolicyToggle" => answer(alerting::set_policy(params(input)?)).await,
                "datasets" => answer(bigquery::datasets(params(input)?)).await,
                "tables" => answer(bigquery::tables(params(input)?)).await,
                "table" => answer(bigquery::table(params(input)?)).await,
                "query" => answer(bigquery::query(params(input)?)).await,
                "enableService" => answer(services::enable(params(input)?)).await,
                _ => match agent::call(ctx, method, input).await {
                    Some(answer) => answer,
                    None => Err(PluginError::unknown_method(method)),
                },
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
                "signIn" => {
                    auth::sign_in().await?;
                    sink.send(reply(auth::status().await)?)
                }
                "tailLogs" => Ok(logging::tail(params(input)?, sink).await?),
                "signInApplicationDefault" => {
                    auth::sign_in_application_default().await?;
                    sink.send(reply(auth::status().await)?)
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Google Cloud is not tied to a repository, so its tools are offered wherever gcloud has signed someone in.
    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async { Ok(auth::signed_in_here()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_manifest_parses() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.gcp");
        let names: Vec<&str> = plugin
            .manifest()
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "gcp_projects",
                "gcp_list",
                "gcp_get",
                "gcp_logs",
                "gcp_metrics",
                "gcp_kube",
                "gcp_act"
            ]
        );
    }
}
