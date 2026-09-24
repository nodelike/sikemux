// ECS — clusters, services, tasks, and the per-task / per-service log
// configuration the UI uses to jump straight to CloudWatch streams.

use serde::{Deserialize, Serialize};

use crate::error::{AwsError, AwsResult};

use crate::common::{aws_json, describe_in_chunks};

// AWS describe-services / describe-tasks accept at most this many ARNs.
const ECS_DESCRIBE_CHUNK: usize = 10;

// ---- clusters ------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct EcsCluster {
    name: String,
    arn: String,
    services_count: Option<i64>,
    tasks_running: Option<i64>,
    tasks_pending: Option<i64>,
    status: Option<String>,
}

pub(crate) async fn clusters(profile: String) -> AwsResult<Vec<EcsCluster>> {
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "clusterArns")]
        cluster_arns: Vec<String>,
    }
    let list: ArnList = aws_json(&profile, &["ecs", "list-clusters", "--output", "json"]).await?;
    if list.cluster_arns.is_empty() {
        return Ok(Vec::new());
    }

    #[derive(Deserialize)]
    struct Describe {
        clusters: Vec<DescribeCluster>,
    }
    #[derive(Deserialize)]
    struct DescribeCluster {
        #[serde(rename = "clusterName")]
        cluster_name: String,
        #[serde(rename = "clusterArn")]
        cluster_arn: String,
        status: Option<String>,
        #[serde(rename = "activeServicesCount")]
        active_services_count: Option<i64>,
        #[serde(rename = "runningTasksCount")]
        running_tasks_count: Option<i64>,
        #[serde(rename = "pendingTasksCount")]
        pending_tasks_count: Option<i64>,
    }

    let chunks: Vec<Describe> = describe_in_chunks(
        profile,
        vec!["ecs".into(), "describe-clusters".into()],
        "--clusters",
        list.cluster_arns,
        100, // describe-clusters has a generous cap
        vec!["--output".into(), "json".into()],
    )
    .await?;

    Ok(chunks
        .into_iter()
        .flat_map(|d| d.clusters)
        .map(|c| EcsCluster {
            name: c.cluster_name,
            arn: c.cluster_arn,
            services_count: c.active_services_count,
            tasks_running: c.running_tasks_count,
            tasks_pending: c.pending_tasks_count,
            status: c.status,
        })
        .collect())
}

// ---- services ------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct EcsService {
    name: String,
    arn: String,
    desired: Option<i64>,
    running: Option<i64>,
    pending: Option<i64>,
    status: Option<String>,
    primary_created_at: Option<String>,
    primary_updated_at: Option<String>,
}

pub(crate) async fn services(profile: String, cluster: String) -> AwsResult<Vec<EcsService>> {
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "serviceArns")]
        arns: Vec<String>,
    }
    let list: ArnList = aws_json(
        &profile,
        &[
            "ecs",
            "list-services",
            "--cluster",
            &cluster,
            "--output",
            "json",
        ],
    )
    .await?;
    if list.arns.is_empty() {
        return Ok(Vec::new());
    }

    #[derive(Deserialize)]
    struct Resp {
        services: Vec<Svc>,
    }
    #[derive(Deserialize)]
    struct Svc {
        #[serde(rename = "serviceName")]
        service_name: String,
        #[serde(rename = "serviceArn")]
        service_arn: String,
        #[serde(rename = "desiredCount")]
        desired_count: Option<i64>,
        #[serde(rename = "runningCount")]
        running_count: Option<i64>,
        #[serde(rename = "pendingCount")]
        pending_count: Option<i64>,
        status: Option<String>,
        deployments: Option<Vec<Deploy>>,
    }
    #[derive(Deserialize)]
    struct Deploy {
        status: Option<String>,
        #[serde(rename = "createdAt")]
        created_at: Option<String>,
        #[serde(rename = "updatedAt")]
        updated_at: Option<String>,
    }

    let chunks: Vec<Resp> = describe_in_chunks(
        profile,
        vec![
            "ecs".into(),
            "describe-services".into(),
            "--cluster".into(),
            cluster,
        ],
        "--services",
        list.arns,
        ECS_DESCRIBE_CHUNK,
        vec!["--output".into(), "json".into()],
    )
    .await?;

    let mut out: Vec<EcsService> = Vec::new();
    for resp in chunks {
        for s in resp.services {
            let primary = s
                .deployments
                .as_ref()
                .and_then(|d| d.iter().find(|x| x.status.as_deref() == Some("PRIMARY")));
            out.push(EcsService {
                primary_created_at: primary.and_then(|p| p.created_at.clone()),
                primary_updated_at: primary.and_then(|p| p.updated_at.clone()),
                name: s.service_name,
                arn: s.service_arn,
                desired: s.desired_count,
                running: s.running_count,
                pending: s.pending_count,
                status: s.status,
            });
        }
    }
    out.sort_by_key(|service| service.name.to_lowercase());
    Ok(out)
}

// ---- tasks ---------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct EcsTask {
    arn: String,
    task_id: String,
    status: Option<String>,
    desired_status: Option<String>,
    health_status: Option<String>,
    cpu: Option<String>,
    memory: Option<String>,
    started_at: Option<String>,
    last_status_change: Option<String>,
}

pub(crate) async fn tasks(
    profile: String,
    cluster: String,
    service: String,
) -> AwsResult<Vec<EcsTask>> {
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "taskArns")]
        arns: Vec<String>,
    }
    let list: ArnList = aws_json(
        &profile,
        &[
            "ecs",
            "list-tasks",
            "--cluster",
            &cluster,
            "--service-name",
            &service,
            "--output",
            "json",
        ],
    )
    .await?;
    if list.arns.is_empty() {
        return Ok(Vec::new());
    }

    #[derive(Deserialize)]
    struct Resp {
        tasks: Vec<Task>,
    }
    #[derive(Deserialize)]
    struct Task {
        #[serde(rename = "taskArn")]
        task_arn: String,
        #[serde(rename = "lastStatus")]
        last_status: Option<String>,
        #[serde(rename = "desiredStatus")]
        desired_status: Option<String>,
        #[serde(rename = "healthStatus")]
        health_status: Option<String>,
        cpu: Option<String>,
        memory: Option<String>,
        #[serde(rename = "startedAt")]
        started_at: Option<String>,
        #[serde(rename = "executionStoppedAt")]
        execution_stopped_at: Option<String>,
    }

    let chunks: Vec<Resp> = describe_in_chunks(
        profile,
        vec![
            "ecs".into(),
            "describe-tasks".into(),
            "--cluster".into(),
            cluster,
        ],
        "--tasks",
        list.arns,
        ECS_DESCRIBE_CHUNK,
        vec!["--output".into(), "json".into()],
    )
    .await?;

    Ok(chunks
        .into_iter()
        .flat_map(|r| r.tasks)
        .map(|t| EcsTask {
            task_id: t
                .task_arn
                .rsplit('/')
                .next()
                .unwrap_or(&t.task_arn)
                .to_string(),
            arn: t.task_arn,
            status: t.last_status,
            desired_status: t.desired_status,
            health_status: t.health_status,
            cpu: t.cpu,
            memory: t.memory,
            started_at: t.started_at,
            last_status_change: t.execution_stopped_at,
        })
        .collect())
}

// ---- log configuration --------------------------------------------------
//
// ECS surfaces logs via the task definition's containerDefinitions[].
// logConfiguration. For the `awslogs` driver that pins (logGroup, region,
// streamPrefix); a specific task's stream is `<prefix>/<container>/<taskId>`.
// We expose service-level (group only) and task-level (group + stream)
// resolutions so the UI can hop straight to the right CloudWatch view.

#[derive(Serialize, Clone)]
pub struct EcsTaskLog {
    log_group: String,
    log_stream: String,
    container_name: String,
    region: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct EcsServiceLog {
    log_group: String,
    container_name: String,
    region: Option<String>,
}

pub(crate) async fn service_log_config(
    profile: String,
    cluster: String,
    service: String,
) -> AwsResult<EcsServiceLog> {
    let svc_v: serde_json::Value = aws_json(
        &profile,
        &[
            "ecs",
            "describe-services",
            "--cluster",
            &cluster,
            "--services",
            &service,
            "--output",
            "json",
        ],
    )
    .await?;
    let td_arn = svc_v
        .get("services")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.get("taskDefinition"))
        .and_then(|v| v.as_str())
        .ok_or(AwsError::BadArg("service has no taskDefinition"))?
        .to_string();

    let td_v: serde_json::Value = aws_json(
        &profile,
        &[
            "ecs",
            "describe-task-definition",
            "--task-definition",
            &td_arn,
            "--output",
            "json",
        ],
    )
    .await?;
    let containers = td_v
        .get("taskDefinition")
        .and_then(|t| t.get("containerDefinitions"))
        .and_then(|c| c.as_array())
        .ok_or(AwsError::BadArg("no containerDefinitions"))?;

    containers
        .iter()
        .find_map(|c| {
            let lc = c.get("logConfiguration")?;
            if lc.get("logDriver").and_then(|d| d.as_str()) != Some("awslogs") {
                return None;
            }
            let opts = lc.get("options")?;
            let group = opts
                .get("awslogs-group")
                .and_then(|v| v.as_str())?
                .to_string();
            let region = opts
                .get("awslogs-region")
                .and_then(|v| v.as_str())
                .map(String::from);
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(EcsServiceLog {
                log_group: group,
                container_name: name,
                region,
            })
        })
        .ok_or(AwsError::BadArg("no container with awslogs driver"))
}

pub(crate) async fn task_log_config(
    profile: String,
    cluster: String,
    task_arn: String,
) -> AwsResult<EcsTaskLog> {
    let task_v: serde_json::Value = aws_json(
        &profile,
        &[
            "ecs",
            "describe-tasks",
            "--cluster",
            &cluster,
            "--tasks",
            &task_arn,
            "--output",
            "json",
        ],
    )
    .await?;

    let task = task_v
        .get("tasks")
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .ok_or(AwsError::BadArg("task not found"))?;
    let td_arn = task
        .get("taskDefinitionArn")
        .and_then(|v| v.as_str())
        .ok_or(AwsError::BadArg("no taskDefinitionArn"))?;
    let task_id = task_arn.rsplit('/').next().unwrap_or(&task_arn).to_string();

    let td_v: serde_json::Value = aws_json(
        &profile,
        &[
            "ecs",
            "describe-task-definition",
            "--task-definition",
            td_arn,
            "--output",
            "json",
        ],
    )
    .await?;
    let containers = td_v
        .get("taskDefinition")
        .and_then(|t| t.get("containerDefinitions"))
        .and_then(|c| c.as_array())
        .ok_or(AwsError::BadArg("no containerDefinitions"))?;

    containers
        .iter()
        .find_map(|c| {
            let lc = c.get("logConfiguration")?;
            if lc.get("logDriver").and_then(|d| d.as_str()) != Some("awslogs") {
                return None;
            }
            let opts = lc.get("options")?;
            let group = opts
                .get("awslogs-group")
                .and_then(|v| v.as_str())?
                .to_string();
            let prefix = opts
                .get("awslogs-stream-prefix")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let region = opts
                .get("awslogs-region")
                .and_then(|v| v.as_str())
                .map(String::from);
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let stream = if prefix.is_empty() {
                format!("{}/{}", name, task_id)
            } else {
                format!("{}/{}/{}", prefix, name, task_id)
            };
            Some(EcsTaskLog {
                log_group: group,
                log_stream: stream,
                container_name: name,
                region,
            })
        })
        .ok_or(AwsError::BadArg("no container with awslogs driver"))
}
