// Load balancers (ELBv2): what a balancer routes where, and whether each
// target group's targets pass their health checks.

use futures::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};

use crate::common::{aws_json, cli_value, Target};
use crate::error::{AwsError, AwsResult};

const CONCURRENCY: usize = 4;
const MAX_TARGET_GROUPS: usize = 50;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LoadBalancer {
    load_balancer_arn: String,
    load_balancer_name: String,
    #[serde(rename = "Type")]
    kind: Option<String>,
    scheme: Option<String>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    state: Option<State>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct State {
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TargetGroup {
    target_group_arn: String,
    target_group_name: String,
    protocol: Option<String>,
    port: Option<i64>,
    target_type: Option<String>,
    health_check_protocol: Option<String>,
    health_check_path: Option<String>,
}

#[derive(Serialize)]
pub struct LoadBalancerSummary {
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    scheme: Option<String>,
    state: Option<String>,
    dns: Option<String>,
}

#[derive(Serialize)]
pub struct LoadBalancerHealth {
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    scheme: Option<String>,
    dns: Option<String>,
    listeners: Vec<ListenerSummary>,
    target_groups: Vec<TargetGroupHealth>,
}

#[derive(Serialize)]
pub struct ListenerSummary {
    port: Option<i64>,
    protocol: Option<String>,
    rules: Vec<String>,
}

#[derive(Serialize)]
pub struct TargetGroupHealth {
    name: String,
    protocol: Option<String>,
    port: Option<i64>,
    target_type: Option<String>,
    health_check: Option<String>,
    targets: Vec<TargetHealth>,
}

#[derive(Serialize)]
pub struct TargetHealth {
    id: String,
    port: Option<i64>,
    state: Option<String>,
    reason: Option<String>,
    description: Option<String>,
}

/// The name in an ELB ARN: `.../loadbalancer/app/<name>/<id>` or `.../targetgroup/<name>/<id>`.
fn name_in_arn(arn: &str) -> String {
    let mut parts = arn.rsplit('/');
    parts.next();
    parts.next().unwrap_or(arn).to_string()
}

fn by_name_or_arn<'a>(value: &'a str, arn_flag: &'static str) -> AwsResult<[&'a str; 2]> {
    let flag = if value.starts_with("arn:") {
        arn_flag
    } else {
        "--names"
    };
    Ok([flag, cli_value(value)?])
}

pub(crate) async fn load_balancers(target: &Target) -> AwsResult<Vec<LoadBalancerSummary>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "LoadBalancers")]
        load_balancers: Vec<LoadBalancer>,
    }
    let resp: Resp = aws_json(target, &["elbv2", "describe-load-balancers"]).await?;
    let mut out: Vec<LoadBalancerSummary> = resp
        .load_balancers
        .into_iter()
        .map(|lb| LoadBalancerSummary {
            name: lb.load_balancer_name,
            kind: lb.kind,
            scheme: lb.scheme,
            state: lb.state.and_then(|s| s.code),
            dns: lb.dns_name,
        })
        .collect();
    out.sort_by_key(|lb| lb.name.to_lowercase());
    Ok(out)
}

/// A load balancer's listeners and rules, and the health of every target group behind it.
pub(crate) async fn load_balancer(target: &Target, name: &str) -> AwsResult<LoadBalancerHealth> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "LoadBalancers")]
        load_balancers: Vec<LoadBalancer>,
    }
    let mut args = vec!["elbv2", "describe-load-balancers"];
    args.extend(by_name_or_arn(name, "--load-balancer-arns")?);
    let resp: Resp = aws_json(target, &args)
        .await
        .map_err(|e| missing(e, "load balancer", name))?;
    let lb = resp
        .load_balancers
        .into_iter()
        .next()
        .ok_or_else(|| AwsError::NotFound(format!("No load balancer {name}.")))?;

    let lookup = ["--load-balancer-arn", lb.load_balancer_arn.as_str()];
    let (listeners, groups) = futures::try_join!(
        listeners(target, &lb.load_balancer_arn),
        target_groups(target, &lookup),
    )?;
    Ok(LoadBalancerHealth {
        name: lb.load_balancer_name,
        kind: lb.kind,
        scheme: lb.scheme,
        dns: lb.dns_name,
        listeners,
        target_groups: health_of(target, groups).await?,
    })
}

pub(crate) async fn target_group(target: &Target, name: &str) -> AwsResult<TargetGroupHealth> {
    let lookup = by_name_or_arn(name, "--target-group-arns")?;
    let groups = target_groups(target, &lookup)
        .await
        .map_err(|e| missing(e, "target group", name))?;
    health_of(target, groups)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AwsError::NotFound(format!("No target group {name}.")))
}

fn missing(error: AwsError, what: &str, name: &str) -> AwsError {
    match error {
        AwsError::Aws(message) if message.contains("NotFound") => {
            AwsError::NotFound(format!("No {what} {name}."))
        }
        other => other,
    }
}

async fn target_groups(target: &Target, lookup: &[&str]) -> AwsResult<Vec<TargetGroup>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "TargetGroups")]
        target_groups: Vec<TargetGroup>,
    }
    let mut args = vec!["elbv2", "describe-target-groups"];
    args.extend(lookup);
    let resp: Resp = aws_json(target, &args).await?;
    Ok(resp.target_groups)
}

async fn health_of(target: &Target, groups: Vec<TargetGroup>) -> AwsResult<Vec<TargetGroupHealth>> {
    stream::iter(groups.into_iter().take(MAX_TARGET_GROUPS))
        .map(|group| async move {
            let targets = targets(target, &group.target_group_arn).await?;
            Ok::<_, AwsError>(TargetGroupHealth {
                health_check: group.health_check_protocol.map(|protocol| {
                    match &group.health_check_path {
                        Some(path) => format!("{protocol} {path}"),
                        None => protocol,
                    }
                }),
                name: group.target_group_name,
                protocol: group.protocol,
                port: group.port,
                target_type: group.target_type,
                targets,
            })
        })
        .buffered(CONCURRENCY)
        .try_collect()
        .await
}

async fn targets(target: &Target, group_arn: &str) -> AwsResult<Vec<TargetHealth>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "TargetHealthDescriptions")]
        descriptions: Vec<Description>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Description {
        target: Target0,
        target_health: Option<Health>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Target0 {
        id: String,
        port: Option<i64>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Health {
        state: Option<String>,
        reason: Option<String>,
        description: Option<String>,
    }
    let resp: Resp = aws_json(
        target,
        &[
            "elbv2",
            "describe-target-health",
            "--target-group-arn",
            group_arn,
        ],
    )
    .await?;
    Ok(resp
        .descriptions
        .into_iter()
        .map(|d| {
            let health = d.target_health;
            TargetHealth {
                id: d.target.id,
                port: d.target.port,
                state: health.as_ref().and_then(|h| h.state.clone()),
                reason: health.as_ref().and_then(|h| h.reason.clone()),
                description: health.and_then(|h| h.description),
            }
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Action {
    #[serde(rename = "Type")]
    kind: String,
    target_group_arn: Option<String>,
    forward_config: Option<ForwardConfig>,
    redirect_config: Option<serde_json::Value>,
    fixed_response_config: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ForwardConfig {
    #[serde(default)]
    target_groups: Vec<WeightedGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WeightedGroup {
    target_group_arn: String,
    weight: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Condition {
    field: Option<String>,
    #[serde(default)]
    values: Vec<String>,
}

async fn listeners(target: &Target, lb_arn: &str) -> AwsResult<Vec<ListenerSummary>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Listeners")]
        listeners: Vec<Listener>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Listener {
        listener_arn: String,
        port: Option<i64>,
        protocol: Option<String>,
    }
    let resp: Resp = aws_json(
        target,
        &["elbv2", "describe-listeners", "--load-balancer-arn", lb_arn],
    )
    .await?;
    stream::iter(resp.listeners)
        .map(|listener| async move {
            Ok::<_, AwsError>(ListenerSummary {
                rules: rules(target, &listener.listener_arn).await?,
                port: listener.port,
                protocol: listener.protocol,
            })
        })
        .buffered(CONCURRENCY)
        .try_collect()
        .await
}

async fn rules(target: &Target, listener_arn: &str) -> AwsResult<Vec<String>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Rules")]
        rules: Vec<Rule>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Rule {
        priority: Option<String>,
        #[serde(default)]
        conditions: Vec<Condition>,
        #[serde(default)]
        actions: Vec<Action>,
    }
    let resp: Resp = aws_json(
        target,
        &["elbv2", "describe-rules", "--listener-arn", listener_arn],
    )
    .await?;
    Ok(resp
        .rules
        .into_iter()
        .map(|rule| {
            let when = rule
                .conditions
                .iter()
                .map(|c| {
                    format!(
                        "{}={}",
                        c.field.as_deref().unwrap_or("?"),
                        c.values.join(",")
                    )
                })
                .collect::<Vec<_>>()
                .join(" & ");
            let then = rule
                .actions
                .iter()
                .map(action_text)
                .collect::<Vec<_>>()
                .join(", ");
            let priority = rule.priority.as_deref().unwrap_or("?");
            if when.is_empty() {
                format!("{priority}: {then}")
            } else {
                format!("{priority}: {when} -> {then}")
            }
        })
        .collect())
}

fn action_text(action: &Action) -> String {
    let config = |value: &Option<serde_json::Value>| {
        value
            .as_ref()
            .and_then(|v| v.as_object())
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };
    match action.kind.as_str() {
        "forward" => match action.forward_config.as_ref().map(|f| &f.target_groups) {
            Some(groups) if groups.len() > 1 => format!(
                "forward {}",
                groups
                    .iter()
                    .map(|g| format!(
                        "{}@{}",
                        name_in_arn(&g.target_group_arn),
                        g.weight.unwrap_or(0)
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            _ => format!(
                "forward {}",
                action
                    .target_group_arn
                    .as_deref()
                    .map(name_in_arn)
                    .unwrap_or_default()
            ),
        },
        "redirect" => format!("redirect {}", config(&action.redirect_config)),
        "fixed-response" => format!("fixed {}", config(&action.fixed_response_config)),
        other => other.to_string(),
    }
}
