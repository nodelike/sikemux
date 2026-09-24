// AWS surface — view-only dashboard over the user's SSO-configured profiles.
// Every API call shells out to the `aws` CLI so the heavy SDK crates stay out
// of the binary; the CLI reuses the user's existing SSO token cache and
// ~/.aws/config resolution rules (sso_session refs, source_profile chains,
// region precedence) for free.

mod auth;
mod billing;
mod common;
mod ec2;
mod ecs;
mod error;
mod lambda;
mod logs;
mod s3;
mod sqs;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::AwsResult;

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
#[serde(rename_all = "camelCase")]
struct IdentityParams {
    profile: String,
    force: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClusterParams {
    profile: String,
    cluster: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceParams {
    profile: String,
    cluster: String,
    service: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskParams {
    profile: String,
    cluster: String,
    task_arn: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingParams {
    profile: String,
    months_back: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailParams {
    profile: String,
    log_group: String,
    log_stream: Option<String>,
    since: Option<String>,
}

async fn answer<T: serde::Serialize>(
    result: impl std::future::Future<Output = AwsResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Aws {
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
                "profiles" => reply(auth::profiles().await),
                "identity" => {
                    let IdentityParams { profile, force } = params(input)?;
                    reply(auth::identity(profile, force).await)
                }
                "ecsClusters" => {
                    let ProfileParams { profile } = params(input)?;
                    answer(ecs::clusters(profile)).await
                }
                "ecsServices" => {
                    let ClusterParams { profile, cluster } = params(input)?;
                    answer(ecs::services(profile, cluster)).await
                }
                "ecsTasks" => {
                    let ServiceParams {
                        profile,
                        cluster,
                        service,
                    } = params(input)?;
                    answer(ecs::tasks(profile, cluster, service)).await
                }
                "ecsTaskLogConfig" => {
                    let TaskParams {
                        profile,
                        cluster,
                        task_arn,
                    } = params(input)?;
                    answer(ecs::task_log_config(profile, cluster, task_arn)).await
                }
                "ecsServiceLogConfig" => {
                    let ServiceParams {
                        profile,
                        cluster,
                        service,
                    } = params(input)?;
                    answer(ecs::service_log_config(profile, cluster, service)).await
                }
                "ec2Instances" => {
                    let ProfileParams { profile } = params(input)?;
                    answer(ec2::instances(profile)).await
                }
                "lambdaFunctions" => {
                    let ProfileParams { profile } = params(input)?;
                    answer(lambda::functions(profile)).await
                }
                "sqsQueues" => {
                    let ProfileParams { profile } = params(input)?;
                    answer(sqs::queues(profile)).await
                }
                "billingMonths" => {
                    let BillingParams {
                        profile,
                        months_back,
                    } = params(input)?;
                    answer(billing::months(profile, months_back)).await
                }
                "s3Buckets" => {
                    let ProfileParams { profile } = params(input)?;
                    answer(s3::buckets(profile)).await
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
                "ssoLogin" => {
                    let ProfileParams { profile } = params(input)?;
                    sink.send(reply(auth::sso_login(profile).await)?)
                }
                "tailLogs" => {
                    let TailParams {
                        profile,
                        log_group,
                        log_stream,
                        since,
                    } = params(input)?;
                    logs::tail(profile, log_group, log_stream, since, sink).await
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
        assert_eq!(plugin.manifest().id, "sikemux.aws");
    }
}
