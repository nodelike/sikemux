// AWS surface — view-only dashboard over the user's profiles, and read-only
// aws_* tools for agents.
// Every API call shells out to the `aws` CLI so the heavy SDK crates stay out
// of the binary; the CLI reuses the user's existing SSO token cache and
// ~/.aws/config resolution rules (sso_session refs, source_profile chains,
// region precedence) for free.

mod agent;
mod agent_profile;
mod auth;
mod billing;
mod common;
mod ec2;
mod ecs;
mod elb;
mod error;
mod lambda;
mod logs;
mod metrics;
mod s3;
mod sqs;
mod views;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::common::Target;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Aws {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Aws {
    manifest: Manifest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileParams {
    profile: String,
}

#[derive(Deserialize)]
struct AgentProfilePicksParams {
    project: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct PaneProfile {
    profile: Option<String>,
}

#[derive(Deserialize)]
struct PickAgentProfileParams {
    project: Option<PathBuf>,
    profile: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityParams {
    profile: String,
    force: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClusterParams {
    #[serde(flatten)]
    target: Target,
    cluster: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceParams {
    #[serde(flatten)]
    target: Target,
    cluster: String,
    service: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskParams {
    #[serde(flatten)]
    target: Target,
    cluster: String,
    task_arn: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingParams {
    #[serde(flatten)]
    target: Target,
    months_back: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailParams {
    #[serde(flatten)]
    target: Target,
    log_group: String,
    log_stream: Option<String>,
    since: Option<String>,
}

impl Plugin for Aws {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn agent_environment(&self, ctx: &PluginContext, project: &Path) -> BTreeMap<String, String> {
        agent_profile::environment(ctx, project)
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            match method {
                "profiles" => reply(auth::profiles().await),
                "accountView" => reply(views::account(ctx).await),
                "agentProfileView" => {
                    let AgentProfilePicksParams { project } = params(input)?;
                    reply(views::agent_profiles(ctx, project.as_deref()))
                }
                "pickAgentProfile" => {
                    let PickAgentProfileParams { project, profile } = params(input)?;
                    reply(agent_profile::pick(ctx, project.as_deref(), profile)?)
                }
                "identity" => {
                    let IdentityParams { profile, force } = params(input)?;
                    reply(auth::identity(Target::named(&profile), force).await)
                }
                "status" => reply(auth::status(ctx).await),
                "profile" => reply(PaneProfile {
                    profile: agent_profile::pane_profile(ctx),
                }),
                "setDefaultAccount" => {
                    let PaneProfile { profile } = params(input)?;
                    reply(PaneProfile {
                        profile: agent_profile::pick_pane(ctx, profile)?,
                    })
                }
                "ecsTaskLogConfig" => {
                    let TaskParams {
                        target,
                        cluster,
                        task_arn,
                    } = params(input)?;
                    answer(ecs::task_log_config(&target, &cluster, &task_arn)).await
                }
                "ecsServiceLogConfig" => {
                    let ServiceParams {
                        target,
                        cluster,
                        service,
                    } = params(input)?;
                    answer(ecs::service_log_config(&target, &cluster, &service)).await
                }
                "ec2View" => answer(views::ec2(ctx, params(input)?)).await,
                "lambdaView" => answer(views::lambda(ctx, params(input)?)).await,
                "sqsView" => answer(views::sqs(ctx, params(input)?)).await,
                "s3View" => answer(views::s3(ctx, params(input)?)).await,
                "ecsClustersView" => answer(views::ecs_clusters(ctx, params(input)?)).await,
                "ecsServicesView" => {
                    let ClusterParams { target, cluster } = params(input)?;
                    answer(views::ecs_services(ctx, target, cluster)).await
                }
                "ecsTasksView" => {
                    let ServiceParams {
                        target,
                        cluster,
                        service,
                    } = params(input)?;
                    answer(views::ecs_tasks(ctx, target, cluster, service)).await
                }
                "billingView" => {
                    let BillingParams {
                        target,
                        months_back,
                    } = params(input)?;
                    answer(views::billing(&target, months_back)).await
                }
                "setup" => reply(auth::setup()),
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
                    let ProfileParams { profile } = params(input)?;
                    sink.send(reply(auth::sso_login(profile).await)?)
                }
                "tailLogs" => {
                    let TailParams {
                        target,
                        log_group,
                        log_stream,
                        since,
                    } = params(input)?;
                    logs::tail(target, log_group, log_stream, since, sink).await
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async {
            Ok(tokio::task::spawn_blocking(auth::aws_in_use)
                .await
                .unwrap_or(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_manifest_parses() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.aws");
    }
}
