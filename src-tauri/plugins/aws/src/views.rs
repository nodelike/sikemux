//! The views the AWS pane draws, worked out here from what the CLI answers so every client
//! draws the same thing.

use std::collections::HashMap;
use std::path::Path;

use sikemux_plugin_api::aws::{
    Account, AccountProfile, AgentProfiles, Billing, BillingChange, BillingCharge, BillingMonth,
    Ec2, Ec2Instance, EcsClusterRow, EcsClusters, EcsEnv, EcsServiceRow, EcsServices, EcsTaskRow,
    EcsTasks, Health, Lambda, LambdaFunction, RuntimeFamily, S3Bucket, Sqs, SqsQueue, S3,
};
use sikemux_plugin_api::PluginContext;

use crate::agent_profile::AgentProfileSource;
use crate::common::Target;
use crate::error::AwsResult;
use crate::{agent_profile, auth, billing, ec2, ecs, lambda, s3, sqs};

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Amounts under half a cent are nothing.
const NOTHING: f64 = 0.005;

/// Where the console opens when a profile names no region.
const FALLBACK_REGION: &str = "us-east-1";

const DEPRECATED_RUNTIMES: [&str; 11] = [
    "python3.7",
    "python3.8",
    "python3.9",
    "nodejs12.x",
    "nodejs14.x",
    "nodejs16.x",
    "nodejs18.x",
    "go1.x",
    "java8",
    "dotnet6",
    "ruby2.7",
];

const RUNTIME_FAMILIES: [(&str, &str); 7] = [
    ("nodejs", "Node"),
    ("python", "Python"),
    ("java", "Java"),
    ("go", "Go"),
    ("dotnet", ".NET"),
    ("ruby", "Ruby"),
    ("provided", "Custom"),
];

/// The environments a cluster's name may end in, earliest stage first.
const ENV_ORDER: [&str; 12] = [
    "dev",
    "development",
    "test",
    "qa",
    "uat",
    "sandbox",
    "demo",
    "staging",
    "stage",
    "preprod",
    "prod",
    "production",
];

/// The smallest share of a bar a queue holding anything gets, so it still shows.
const SLIVER: f64 = 0.03;

/// The profile's own region, which is the one its calls run in.
async fn targeted(ctx: &PluginContext, asked: Target) -> AwsResult<(Target, String)> {
    let target = auth::resolve(ctx, asked).await?;
    let region = target
        .region
        .clone()
        .unwrap_or_else(|| FALLBACK_REGION.to_owned());
    Ok((target, region))
}

pub(crate) async fn ec2(ctx: &PluginContext, asked: Target) -> AwsResult<Ec2> {
    let (target, region) = targeted(ctx, asked).await?;
    let instances: Vec<Ec2Instance> = ec2::instances(&target)
        .await?
        .into_iter()
        .map(|instance| instance_of(instance, &region))
        .collect();
    Ok(Ec2 {
        running: instances
            .iter()
            .filter(|instance| instance.state.as_deref() == Some("running"))
            .count() as u32,
        instances,
        region,
    })
}

fn instance_of(instance: ec2::Ec2Instance, region: &str) -> Ec2Instance {
    let (health, label) = match instance.state.as_deref() {
        Some("running") => (Health::Ok, "Running"),
        Some("pending") => (Health::Warn, "Starting"),
        Some("stopping") => (Health::Warn, "Stopping"),
        Some("shutting-down") => (Health::Warn, "Shutting down"),
        Some("stopped") => (Health::Off, "Stopped"),
        Some("terminated") => (Health::Off, "Terminated"),
        Some(other) => (Health::Off, other),
        None => (Health::Off, "Unknown"),
    };
    Ec2Instance {
        state_label: label.to_owned(),
        health,
        idle: matches!(instance.state.as_deref(), Some("stopped" | "terminated")),
        console_url: format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#InstanceDetails:instanceId={}",
            instance.instance_id
        ),
        id: instance.instance_id,
        name: instance.name,
        state: instance.state,
        instance_type: instance.instance_type,
        private_ip: instance.private_ip,
        public_ip: instance.public_ip,
        launched_at: instance.launch_time,
    }
}

pub(crate) async fn lambda(ctx: &PluginContext, asked: Target) -> AwsResult<Lambda> {
    let (target, region) = targeted(ctx, asked).await?;
    let raw = lambda::functions(&target).await?;
    let most = raw
        .iter()
        .filter_map(|function| function.memory_size)
        .fold(1024, i64::max);
    let functions: Vec<LambdaFunction> = raw
        .into_iter()
        .map(|function| function_of(function, most, &region))
        .collect();
    let mut families: Vec<RuntimeFamily> = Vec::new();
    for function in &functions {
        match families
            .iter_mut()
            .find(|family| family.key == function.family)
        {
            Some(family) => family.count += 1,
            None => families.push(RuntimeFamily {
                key: function.family.clone(),
                label: family_label(&function.family).to_owned(),
                count: 1,
            }),
        }
    }
    families.sort_by_key(|family| std::cmp::Reverse(family.count));
    Ok(Lambda {
        region,
        families,
        functions,
    })
}

fn family_label(key: &str) -> &'static str {
    RUNTIME_FAMILIES
        .iter()
        .find(|(family, _)| *family == key)
        .map_or("Other", |(_, label)| *label)
}

fn function_of(function: lambda::LambdaFn, most_memory: i64, region: &str) -> LambdaFunction {
    let family = function
        .runtime
        .as_deref()
        .and_then(|runtime| {
            RUNTIME_FAMILIES
                .iter()
                .find(|(family, _)| runtime.starts_with(family))
        })
        .map_or("other", |(family, _)| *family);
    let memory_mb = function
        .memory_size
        .and_then(|mb| u32::try_from(mb).ok())
        .filter(|mb| *mb > 0);
    LambdaFunction {
        family: family.to_owned(),
        deprecated: function
            .runtime
            .as_deref()
            .is_some_and(|runtime| DEPRECATED_RUNTIMES.contains(&runtime)),
        memory_label: memory_mb.map_or_else(|| "—".to_owned(), memory_label),
        memory_ratio: memory_mb.map_or(0., |mb| f64::from(mb) / most_memory as f64),
        memory_mb,
        timeout_secs: function.timeout.and_then(|secs| u32::try_from(secs).ok()),
        handler: function.handler,
        modified_at: function.last_modified,
        log_group: format!("/aws/lambda/{}", function.name),
        console_url: format!(
            "https://{region}.console.aws.amazon.com/lambda/home?region={region}#/functions/{}",
            encoded(&function.name)
        ),
        runtime: function.runtime,
        name: function.name,
    }
}

/// `512 MB`, or `1.5 GB` from a gigabyte up.
fn memory_label(mb: u32) -> String {
    if mb < 1024 {
        return format!("{mb} MB");
    }
    let gb = format!("{:.1}", f64::from(mb) / 1024.);
    format!("{} GB", gb.strip_suffix(".0").unwrap_or(&gb))
}

pub(crate) async fn sqs(ctx: &PluginContext, asked: Target) -> AwsResult<Sqs> {
    let (target, region) = targeted(ctx, asked).await?;
    let raw = sqs::queues(&target).await?;
    let count = |value: &Option<String>| value.as_deref().and_then(|n| n.parse::<u64>().ok());
    let most = raw
        .iter()
        .filter_map(|queue| count(&queue.messages))
        .fold(1, u64::max);
    let queues: Vec<SqsQueue> = raw
        .into_iter()
        .map(|queue| {
            let available = count(&queue.messages);
            let dead_letter = is_dead_letter(&queue.name);
            SqsQueue {
                fifo: queue.name.ends_with(".fifo"),
                dead_letter,
                ratio: available
                    .filter(|n| *n > 0)
                    .map_or(0., |n| (n as f64 / most as f64).sqrt().max(SLIVER)),
                needs_look: dead_letter && available.is_some_and(|n| n > 0),
                in_flight: count(&queue.in_flight),
                delayed: count(&queue.delayed),
                available,
                console_url: format!(
                    "https://{region}.console.aws.amazon.com/sqs/v3/home?region={region}#/queues/{}",
                    encoded(&queue.url)
                ),
                name: queue.name,
                url: queue.url,
            }
        })
        .collect();
    Ok(Sqs {
        region,
        dead_letters: queues.iter().filter(|queue| queue.dead_letter).count() as u32,
        queues,
    })
}

/// Named as a dead-letter queue: `-dlq`, `_dlq`, `-dead-letter` or `-deadletter` at its end.
fn is_dead_letter(name: &str) -> bool {
    let name = name.to_lowercase();
    let name = name.strip_suffix(".fifo").unwrap_or(&name);
    ["dlq", "dead-letter", "deadletter"].iter().any(|ending| {
        name.strip_suffix(ending)
            .is_some_and(|rest| rest.ends_with('-') || rest.ends_with('_'))
    })
}

pub(crate) async fn s3(ctx: &PluginContext, asked: Target) -> AwsResult<S3> {
    let (target, region) = targeted(ctx, asked).await?;
    let buckets = s3::buckets(&target)
        .await?
        .into_iter()
        .map(|bucket| S3Bucket {
            uri: format!("s3://{}", bucket.name),
            arn: format!("arn:aws:s3:::{}", bucket.name),
            console_url: format!(
                "https://s3.console.aws.amazon.com/s3/buckets/{}",
                encoded(&bucket.name)
            ),
            name: bucket.name,
            created_at: bucket.created_at,
        })
        .collect();
    Ok(S3 { region, buckets })
}

pub(crate) async fn ecs_clusters(ctx: &PluginContext, asked: Target) -> AwsResult<EcsClusters> {
    let (target, region) = targeted(ctx, asked).await?;
    let clusters: Vec<EcsClusterRow> = ecs::clusters(&target)
        .await?
        .into_iter()
        .map(|cluster| cluster_of(cluster, &region))
        .collect();
    let mut envs: Vec<EcsEnv> = Vec::new();
    for cluster in &clusters {
        match envs.iter_mut().find(|env| env.key == cluster.env) {
            Some(env) => env.count += 1,
            None => envs.push(EcsEnv {
                label: capitalised(&cluster.env),
                key: cluster.env.clone(),
                count: 1,
            }),
        }
    }
    envs.sort_by_key(|env| {
        ENV_ORDER
            .iter()
            .position(|known| *known == env.key)
            .unwrap_or(ENV_ORDER.len())
    });
    Ok(EcsClusters {
        region,
        envs,
        clusters,
    })
}

fn cluster_of(cluster: ecs::EcsCluster, region: &str) -> EcsClusterRow {
    let short_name = cluster
        .name
        .strip_suffix("-cluster")
        .unwrap_or(&cluster.name)
        .to_owned();
    let (product, env, env_label) = read_cluster_name(&short_name);
    let count = |n: Option<i64>| n.and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
    let pending = count(cluster.tasks_pending);
    let health = match cluster.status.as_deref() {
        Some("INACTIVE") => Health::Off,
        Some(status) if status != "ACTIVE" => Health::Fail,
        _ if pending > 0 => Health::Warn,
        _ => Health::Ok,
    };
    EcsClusterRow {
        console_url: format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/clusters/{}/services?region={region}",
            encoded(&cluster.name)
        ),
        production: matches!(env.as_str(), "prod" | "production"),
        services: count(cluster.services_count),
        running: count(cluster.tasks_running),
        pending,
        health,
        product,
        env,
        env_label,
        short_name,
        name: cluster.name,
        arn: cluster.arn,
    }
}

/// `<product>-<env>` as its product, environment and the environment's chip; a name ending in
/// no known environment is `other`, its chip what follows its first word.
fn read_cluster_name(short_name: &str) -> (String, String, String) {
    let words: Vec<&str> = short_name.split('-').collect();
    match words.split_last() {
        Some((last, rest)) if ENV_ORDER.contains(last) => {
            let product = match rest.is_empty() {
                true => (*last).to_owned(),
                false => rest.join("-"),
            };
            (product, (*last).to_owned(), (*last).to_owned())
        }
        _ => {
            let (first, rest) = words.split_first().map_or(("", &[][..]), |(f, r)| (*f, r));
            let label = match rest.is_empty() {
                true => "other".to_owned(),
                false => rest.join("-"),
            };
            (first.to_owned(), "other".to_owned(), label)
        }
    }
}

pub(crate) async fn ecs_services(
    ctx: &PluginContext,
    asked: Target,
    cluster: String,
) -> AwsResult<EcsServices> {
    let (target, region) = targeted(ctx, asked).await?;
    let services = ecs::services(&target, &cluster)
        .await?
        .into_iter()
        .map(|service| service_of(service, &cluster, &region))
        .collect();
    Ok(EcsServices {
        region,
        cluster,
        services,
    })
}

fn service_of(service: ecs::EcsService, cluster: &str, region: &str) -> EcsServiceRow {
    let count = |n: Option<i64>| n.and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
    let (desired, running, pending) = (
        count(service.desired),
        count(service.running),
        count(service.pending),
    );
    let (health, label) = if pending > 0 {
        (Health::Warn, "Deploying")
    } else if desired == 0 && running == 0 {
        (Health::Off, "Scaled to 0")
    } else if running == desired {
        (Health::Ok, "Steady")
    } else {
        (Health::Fail, "Degraded")
    };
    EcsServiceRow {
        console_url: format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/clusters/{}/services/{}/health?region={region}",
            encoded(cluster),
            encoded(&service.name)
        ),
        deployed_at: service.primary_updated_at.or(service.primary_created_at),
        state_label: label.to_owned(),
        health,
        desired,
        running,
        pending,
        name: service.name,
        arn: service.arn,
    }
}

pub(crate) async fn ecs_tasks(
    ctx: &PluginContext,
    asked: Target,
    cluster: String,
    service: String,
) -> AwsResult<EcsTasks> {
    let (target, _) = targeted(ctx, asked).await?;
    let tasks = ecs::tasks(&target, &cluster, Some(&service), false)
        .await?
        .into_iter()
        .map(task_of)
        .collect();
    Ok(EcsTasks { tasks })
}

fn task_of(task: ecs::EcsTask) -> EcsTaskRow {
    let health = match task.status.as_deref() {
        Some("RUNNING") => Health::Ok,
        Some("STOPPED") => Health::Off,
        _ => Health::Warn,
    };
    let check = match task.health_status.as_deref() {
        Some("HEALTHY") => Some(Health::Ok),
        Some("UNHEALTHY") => Some(Health::Fail),
        _ => None,
    };
    let number = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|n| n.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n > 0.)
    };
    let size = number(&task.cpu)
        .zip(number(&task.memory))
        .map(|(cpu, memory)| {
            let memory = match memory >= 1024. {
                true => format!("{} GB", memory / 1024.),
                false => format!("{memory} MB"),
            };
            format!("{} vCPU · {memory}", cpu / 1024.)
        });
    EcsTaskRow {
        short_id: task.task_id.chars().take(12).collect(),
        id_rest: task.task_id.chars().skip(12).take(8).collect(),
        status_label: titled(task.status.as_deref()),
        check_label: titled(task.health_status.as_deref()),
        health,
        check,
        size,
        private_ip: task.private_ip,
        zone: task.availability_zone,
        started_at: task.started_at,
        id: task.task_id,
        arn: task.arn,
    }
}

/// `RUNNING` as `Running`.
fn titled(value: Option<&str>) -> String {
    value.map_or_else(
        || "—".to_owned(),
        |value| capitalised(&value.to_lowercase()),
    )
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The profile the pane shows and every profile to switch to, each with how it signs in
/// (`sso`, `role`, `credential_process` or `credentials`) under `kind`.
pub(crate) async fn account(ctx: &PluginContext) -> serde_json::Value {
    let profiles = auth::profiles().await;
    let kinds: Vec<_> = profiles
        .iter()
        .map(|profile| serde_json::to_value(profile.kind).unwrap_or_default())
        .collect();
    let account = Account {
        profile: agent_profile::pane_profile(ctx),
        profiles: profiles
            .into_iter()
            .map(|profile| {
                let (title, named) = split_account(&profile.name);
                AccountProfile {
                    account: profile
                        .sso_account_id
                        .as_deref()
                        .or(named)
                        .map(spaced_account),
                    title: title.to_owned(),
                    region: profile.region,
                    name: profile.name,
                }
            })
            .collect(),
    };
    let mut json = serde_json::to_value(account).unwrap_or_default();
    let listed = json
        .get_mut("profiles")
        .and_then(serde_json::Value::as_array_mut);
    for (profile, kind) in listed.into_iter().flatten().zip(kinds) {
        if let Some(profile) = profile.as_object_mut() {
            profile.insert("kind".into(), kind);
        }
    }
    json
}

/// `admin-123456789012` as `admin` and its account id.
fn split_account(name: &str) -> (&str, Option<&str>) {
    name.rsplit_once(['-', '_'])
        .filter(|(title, id)| {
            !title.is_empty() && id.len() == 12 && id.chars().all(|c| c.is_ascii_digit())
        })
        .map_or((name, None), |(title, id)| (title, Some(id)))
}

/// `123456789012` as `1234-5678-9012`.
fn spaced_account(id: &str) -> String {
    let digits: Vec<char> = id.chars().collect();
    match digits.len() == 12 && digits.iter().all(char::is_ascii_digit) {
        true => digits
            .chunks(4)
            .map(|four| four.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("-"),
        false => id.to_owned(),
    }
}

pub(crate) fn agent_profiles(ctx: &PluginContext, project: Option<&Path>) -> AgentProfiles {
    let picks = agent_profile::picks(ctx, project);
    let effective = picks.effective.as_ref();
    let from_shell = effective
        .filter(|effective| effective.source == AgentProfileSource::Environment)
        .map(|effective| match &effective.profile {
            Some(profile) => format!("{profile}, from your shell"),
            None => "The AWS keys your shell sets".to_owned(),
        });
    let automatic = effective
        .filter(|effective| effective.source == AgentProfileSource::Automatic)
        .and_then(|effective| effective.profile.as_deref());
    AgentProfiles {
        project_unset: picks.default.as_deref().map_or_else(
            || "Default".to_owned(),
            |default| format!("Default ({default})"),
        ),
        default_unset: automatic.map_or_else(
            || "None".to_owned(),
            |profile| format!("Automatic ({profile})"),
        ),
        effective: effective.and_then(|effective| effective.profile.clone()),
        from_shell,
        project: picks.project,
        default: picks.default,
        profiles: auth::profile_names(),
    }
}

/// `text` made safe to put in a URL, as a browser's `encodeURIComponent` makes it.
fn encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) async fn billing(target: &Target, months_back: u32) -> AwsResult<Billing> {
    let mut months = billing::months(target, months_back).await?;
    months.sort_by(|a, b| a.period_start.cmp(&b.period_start));
    Ok(billing_of(&months, time::OffsetDateTime::now_utc().day()))
}

/// The months Cost Explorer answered, oldest first, on day `today` (UTC) of the month in progress.
fn billing_of(raw: &[billing::BillingMonth], today: u8) -> Billing {
    let current = raw
        .iter()
        .position(|month| month.is_current)
        .unwrap_or(raw.len().saturating_sub(1));
    let befores = std::iter::once(None).chain(raw.iter().map(Some));
    let mut months: Vec<BillingMonth> = raw
        .iter()
        .zip(befores)
        .map(|(month, before)| month_of(month, before, today))
        .collect();
    let tallest = months.iter().map(|month| month.gross).fold(1., f64::max);
    for month in &mut months {
        month.ratio = month.gross / tallest;
    }
    let closed: Vec<f64> = months
        .iter()
        .filter(|month| !month.in_progress)
        .map(|month| month.gross)
        .collect();
    let last_closed = months.iter().rposition(|month| !month.in_progress);
    let average = (!closed.is_empty()).then(|| closed.iter().sum::<f64>() / closed.len() as f64);
    let forecast = months.get(current).map_or(0., |month| match month.day {
        Some(day) => month.gross / f64::from(day) * f64::from(month.days),
        None => month.gross,
    });
    let change_pct = closed
        .last()
        .copied()
        .filter(|before| *before > 0.)
        .map(|before| (forecast - before) / before * 100.);
    Billing {
        unit: raw
            .get(current)
            .map_or_else(|| "USD".to_owned(), |month| month.unit.clone()),
        current,
        forecast,
        last_closed,
        change_pct,
        average,
        average_ratio: average.map(|average| average / tallest),
        months,
    }
}

fn month_of(
    month: &billing::BillingMonth,
    before: Option<&billing::BillingMonth>,
    today: u8,
) -> BillingMonth {
    let amounts: Vec<(&str, f64)> = month
        .by_service
        .iter()
        .filter_map(|line| Some((line.service.as_str(), line.amount.parse::<f64>().ok()?)))
        .filter(|(_, amount)| amount.is_finite())
        .collect();
    let gross: f64 = amounts.iter().map(|(_, a)| a).filter(|a| **a >= 0.).sum();
    let credits: f64 = amounts.iter().map(|(_, a)| a).filter(|a| **a < 0.).sum();
    let (year, number) = year_month(&month.period_start);
    let days = number.map_or(30, |number| {
        time::Month::try_from(number).map_or(30, |name| u32::from(name.length(year)))
    });
    let day = month.is_current.then(|| u32::from(today).clamp(1, days));
    let pace = day.map_or(1., |day| f64::from(days) / f64::from(day));
    let earlier: Option<HashMap<&str, f64>> = before.map(|before| {
        let mut first = HashMap::new();
        for line in &before.by_service {
            let amount = line.amount.parse::<f64>().unwrap_or(f64::NAN);
            first.entry(line.service.as_str()).or_insert(amount);
        }
        first
    });
    let mut charges: Vec<(&str, f64)> = amounts
        .iter()
        .copied()
        .filter(|(_, a)| *a > NOTHING)
        .collect();
    charges.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut credit_lines: Vec<(&str, f64)> = amounts
        .iter()
        .copied()
        .filter(|(_, a)| *a < -NOTHING)
        .collect();
    credit_lines.sort_by(|a, b| a.1.total_cmp(&b.1));
    let top = charges.first().map_or(1., |(_, amount)| amount.max(1.));
    let line = |(service, amount): (&str, f64), change: Option<BillingChange>| BillingCharge {
        service: service.to_owned(),
        short_name: service
            .strip_prefix("Amazon ")
            .or_else(|| service.strip_prefix("AWS "))
            .unwrap_or(service)
            .to_owned(),
        amount,
        ratio: amount / top,
        change,
    };
    let (name, label, long_label) = names(&month.period_start);
    BillingMonth {
        period_start: month.period_start.clone(),
        label,
        name,
        long_label,
        in_progress: month.is_current,
        day,
        days,
        gross,
        credits,
        net: gross + credits,
        ratio: 0.,
        compared_to: before.map(|before| names(&before.period_start).1),
        charges: charges
            .into_iter()
            .map(|charge| {
                let change = earlier
                    .as_ref()
                    .map(|earlier| change_of(earlier.get(charge.0).copied(), charge.1 * pace));
                line(charge, change)
            })
            .collect(),
        credit_lines: credit_lines
            .into_iter()
            .map(|credit| line(credit, None))
            .collect(),
    }
}

/// How `now` moved from `before`, a percent either way counting as no move.
fn change_of(before: Option<f64>, now: f64) -> BillingChange {
    let Some(before) = before.filter(|before| before.is_finite() && *before > NOTHING) else {
        return BillingChange::New;
    };
    let pct = (now - before) / before * 100.;
    match pct {
        pct if pct.abs() < 1. => BillingChange::Flat,
        pct if pct > 0. => BillingChange::Up { pct },
        pct => BillingChange::Down { pct },
    }
}

fn year_month(period_start: &str) -> (i32, Option<u8>) {
    let mut parts = period_start.split('-');
    let year = parts
        .next()
        .and_then(|year| year.parse().ok())
        .unwrap_or(1970);
    let month = parts.next().and_then(|month| month.parse().ok());
    (year, month)
}

/// A month's name, as `Oct 26` and `October 2026`, from its first day.
fn names(period_start: &str) -> (String, String, String) {
    let (year, month) = year_month(period_start);
    let Some(name) = month
        .and_then(|month| usize::from(month).checked_sub(1))
        .and_then(|at| MONTH_NAMES.get(at))
    else {
        return (
            period_start.to_owned(),
            period_start.to_owned(),
            period_start.to_owned(),
        );
    };
    (
        (*name).to_owned(),
        format!(
            "{} {:02}",
            name.chars().take(3).collect::<String>(),
            year.rem_euclid(100)
        ),
        format!("{name} {year}"),
    )
}
