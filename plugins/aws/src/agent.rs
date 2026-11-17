// The read-only aws tool agents call. Each picks its profile and region
// the way `auth::resolve` says, and answers with a trimmed shape rather than
// the CLI's raw JSON.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::{params, reply, PluginContext, PluginResult};

use crate::auth;
use crate::common::{aws_json, Target};
use crate::error::AwsError;
use crate::{cost, ec2, ecs, elb, logs, metrics};

const MAX_LISTED: usize = 100;

#[derive(Deserialize)]
struct Ecs {
    #[serde(flatten)]
    target: Target,
    cluster: Option<String>,
    service: Option<String>,
    tasks: Option<TaskState>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum TaskState {
    Running,
    Stopped,
}

#[derive(Deserialize)]
struct Named {
    #[serde(flatten)]
    target: Target,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Instances {
    #[serde(flatten)]
    target: Target,
    name: Option<String>,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    tag: Option<String>,
    vpc: Option<String>,
    #[serde(default)]
    volumes: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cost {
    #[serde(flatten)]
    target: Target,
    days: Option<i64>,
    #[serde(default)]
    daily: bool,
    group_by: Option<String>,
    service: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetHealth {
    #[serde(flatten)]
    target: Target,
    load_balancer: Option<String>,
    target_group: Option<String>,
}

#[derive(Deserialize)]
struct Logs {
    #[serde(flatten)]
    target: Target,
    group: String,
    stream: Option<String>,
    pattern: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct Metrics {
    #[serde(flatten)]
    target: Target,
    namespace: String,
    metric: String,
    #[serde(default)]
    dimensions: BTreeMap<String, String>,
    stat: Option<String>,
    since: Option<String>,
    period: Option<u64>,
}

/// Answers an agent tool's method, or `None` when `method` is not one.
pub(crate) async fn call(
    ctx: &PluginContext,
    method: &str,
    input: Value,
) -> Option<PluginResult<Value>> {
    Some(match method {
        "agentProfiles" => profiles(ctx).await,
        "agentIdentity" => identity(ctx, input).await,
        "agentEcs" => ecs_view(ctx, input).await,
        "agentTaskDefinition" => task_definition(ctx, input).await,
        "agentTargetHealth" => target_health(ctx, input).await,
        "agentLogs" => log_search(ctx, input).await,
        "agentMetrics" => metric_series(ctx, input).await,
        "agentEc2" => instances(ctx, input).await,
        "agentCost" => spending(ctx, input).await,
        _ => return None,
    })
}

async fn profiles(ctx: &PluginContext) -> PluginResult<Value> {
    let default = auth::resolve(ctx, Target::default()).await.ok();
    reply(json!({
        "profiles": auth::profiles().await,
        "default": default.and_then(|target| target.profile),
    }))
}

async fn identity(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Caller {
        account: Option<String>,
        arn: Option<String>,
    }
    let target = auth::resolve(ctx, params(input)?).await?;
    let caller: Caller = aws_json(&target, &["sts", "get-caller-identity"]).await?;
    let role = caller
        .arn
        .as_deref()
        .and_then(|arn| arn.split_once(":assumed-role/"))
        .and_then(|(_, rest)| rest.split('/').next())
        .map(String::from);
    reply(json!({
        "profile": target.label(),
        "region": target.region,
        "account": caller.account,
        "arn": caller.arn,
        "role": role,
    }))
}

async fn ecs_view(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Ecs = params(input)?;
    let target = auth::resolve(ctx, ask.target).await?;
    match (ask.cluster.as_deref(), ask.service.as_deref(), ask.tasks) {
        (None, None, None) => reply(ecs::clusters(&target).await?),
        (None, Some(service), None) => reply(ecs::find_service(&target, service).await?),
        (None, ..) => Err(AwsError::BadArg("name the cluster too").into()),
        (Some(cluster), service, Some(state)) => {
            reply(ecs::tasks(&target, cluster, service, state == TaskState::Stopped).await?)
        }
        (Some(cluster), Some(service), None) => {
            reply(ecs::service(&target, cluster, service).await?)
        }
        (Some(cluster), None, None) => reply(ecs::services(&target, cluster).await?),
    }
}

async fn task_definition(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Named = params(input)?;
    let name = ask
        .name
        .ok_or(AwsError::BadArg("name the task definition"))?;
    let target = auth::resolve(ctx, ask.target).await?;
    reply(ecs::task_definition(&target, &name).await?)
}

async fn target_health(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: TargetHealth = params(input)?;
    let target = auth::resolve(ctx, ask.target).await?;
    match (ask.load_balancer.as_deref(), ask.target_group.as_deref()) {
        (Some(lb), _) => reply(elb::load_balancer(&target, lb).await?),
        (None, Some(group)) => reply(elb::target_group(&target, group).await?),
        (None, None) => reply(elb::load_balancers(&target).await?),
    }
}

async fn log_search(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Logs = params(input)?;
    let target = auth::resolve(ctx, ask.target).await?;
    reply(
        logs::search(
            &target,
            &ask.group,
            ask.stream.as_deref(),
            ask.pattern.as_deref(),
            ask.since.as_deref(),
            ask.limit,
        )
        .await?,
    )
}

async fn metric_series(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Metrics = params(input)?;
    let target = auth::resolve(ctx, ask.target).await?;
    reply(
        metrics::statistics(
            &target,
            metrics::MetricQuery {
                namespace: &ask.namespace,
                metric: &ask.metric,
                dimensions: &ask.dimensions,
                stat: ask.stat.as_deref(),
                since: ask.since.as_deref(),
                period: ask.period,
            },
        )
        .await?,
    )
}

/// The first `MAX_LISTED` of `found`, with a note when there were more.
fn listed<T: serde::Serialize>(key: &str, mut found: Vec<T>) -> PluginResult<Value> {
    let total = found.len();
    found.truncate(MAX_LISTED);
    reply(json!({
        key: found,
        "note": (total > MAX_LISTED)
            .then(|| format!("{MAX_LISTED} of {total}; narrow them with the filters.")),
    }))
}

fn tag_filter(tag: &str) -> (String, String) {
    match tag.split_once('=') {
        Some((key, value)) => (format!("tag:{key}"), value.to_owned()),
        None => ("tag-key".to_owned(), tag.to_owned()),
    }
}

async fn instances(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Instances = params(input)?;
    if ask.volumes && ask.vpc.is_some() {
        return Err(AwsError::BadArg("volumes have no VPC; leave vpc out").into());
    }
    let target = auth::resolve(ctx, ask.target).await?;
    let (state, kind) = if ask.volumes {
        ("status", "volume-type")
    } else {
        ("instance-state-name", "instance-type")
    };
    let filters: ec2::Filters = [
        ask.state.map(|value| (state.to_owned(), value)),
        ask.kind.map(|value| (kind.to_owned(), value)),
        ask.vpc.map(|vpc| ("vpc-id".to_owned(), vpc)),
        ask.tag.as_deref().map(tag_filter),
    ]
    .into_iter()
    .flatten()
    .collect();
    let wanted = ask.name.map(|name| name.to_lowercase());
    let wanted = |text: &str| wanted.as_deref().is_none_or(|wanted| text.contains(wanted));
    if ask.volumes {
        let found = ec2::volumes(&target, &filters)
            .await?
            .into_iter()
            .filter(|volume| volume.matches(wanted))
            .collect();
        return listed("volumes", found);
    }
    let found = ec2::instances(&target, &filters)
        .await?
        .into_iter()
        .filter(|instance| instance.matches(wanted))
        .collect();
    listed("instances", found)
}

async fn spending(ctx: &PluginContext, input: Value) -> PluginResult<Value> {
    let ask: Cost = params(input)?;
    let target = auth::resolve(ctx, ask.target).await?;
    reply(
        cost::costs(
            &target,
            cost::CostQuery {
                days: ask.days,
                daily: ask.daily,
                group_by: ask.group_by.as_deref(),
                service: ask.service.as_deref(),
            },
        )
        .await?,
    )
}
