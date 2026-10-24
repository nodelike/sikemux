// ECS — clusters, services, tasks, task definitions, and the per-task /
// per-service log configuration the UI uses to jump straight to CloudWatch.

use serde::{Deserialize, Serialize};

use crate::error::{AwsError, AwsResult};

use crate::common::{aws_json, cli_value, describe_in_chunks, last_segment, Target};

// AWS describe-services / describe-tasks accept at most this many ARNs.
const ECS_DESCRIBE_CHUNK: usize = 10;
const MAX_TASKS: &str = "100";
const SERVICE_EVENTS: usize = 10;

// ---- clusters ------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct EcsCluster {
    pub(crate) name: String,
    pub(crate) arn: String,
    pub(crate) services_count: Option<i64>,
    pub(crate) tasks_running: Option<i64>,
    pub(crate) tasks_pending: Option<i64>,
    pub(crate) status: Option<String>,
}

pub(crate) async fn clusters(target: &Target) -> AwsResult<Vec<EcsCluster>> {
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "clusterArns")]
        cluster_arns: Vec<String>,
    }
    let list: ArnList = aws_json(target, &["ecs", "list-clusters"]).await?;

    #[derive(Deserialize)]
    struct Describe {
        clusters: Vec<DescribeCluster>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DescribeCluster {
        cluster_name: String,
        cluster_arn: String,
        status: Option<String>,
        active_services_count: Option<i64>,
        running_tasks_count: Option<i64>,
        pending_tasks_count: Option<i64>,
    }

    let chunks: Vec<Describe> = describe_in_chunks(
        target,
        vec!["ecs".into(), "describe-clusters".into()],
        "--clusters",
        list.cluster_arns,
        100, // describe-clusters has a generous cap
        Vec::new(),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescribedService {
    service_name: String,
    service_arn: String,
    desired_count: Option<i64>,
    running_count: Option<i64>,
    pending_count: Option<i64>,
    status: Option<String>,
    launch_type: Option<String>,
    task_definition: Option<String>,
    #[serde(default)]
    deployments: Vec<DescribedDeployment>,
    #[serde(default)]
    load_balancers: Vec<DescribedLoadBalancer>,
    #[serde(default)]
    events: Vec<DescribedEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescribedDeployment {
    status: Option<String>,
    task_definition: Option<String>,
    desired_count: Option<i64>,
    running_count: Option<i64>,
    pending_count: Option<i64>,
    failed_tasks: Option<i64>,
    created_at: Option<String>,
    updated_at: Option<String>,
    rollout_state: Option<String>,
    rollout_state_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescribedLoadBalancer {
    target_group_arn: Option<String>,
    load_balancer_name: Option<String>,
    container_name: Option<String>,
    container_port: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescribedEvent {
    created_at: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct DescribedServices {
    services: Vec<DescribedService>,
}

impl DescribedService {
    fn primary(&self) -> Option<&DescribedDeployment> {
        self.deployments
            .iter()
            .find(|deployment| deployment.status.as_deref() == Some("PRIMARY"))
    }
}

#[derive(Serialize, Clone)]
pub struct EcsService {
    pub(crate) name: String,
    pub(crate) arn: String,
    pub(crate) desired: Option<i64>,
    pub(crate) running: Option<i64>,
    pub(crate) pending: Option<i64>,
    pub(crate) status: Option<String>,
    pub(crate) task_definition: Option<String>,
    pub(crate) rollout: Option<String>,
    pub(crate) primary_created_at: Option<String>,
    pub(crate) primary_updated_at: Option<String>,
}

pub(crate) async fn services(target: &Target, cluster: &str) -> AwsResult<Vec<EcsService>> {
    cli_value(cluster)?;
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "serviceArns")]
        arns: Vec<String>,
    }
    let list: ArnList = aws_json(target, &["ecs", "list-services", "--cluster", cluster]).await?;

    let chunks: Vec<DescribedServices> = describe_in_chunks(
        target,
        vec![
            "ecs".into(),
            "describe-services".into(),
            "--cluster".into(),
            cluster.into(),
        ],
        "--services",
        list.arns,
        ECS_DESCRIBE_CHUNK,
        Vec::new(),
    )
    .await?;

    let mut out: Vec<EcsService> = chunks
        .into_iter()
        .flat_map(|resp| resp.services)
        .map(|s| {
            let primary = s.primary();
            EcsService {
                primary_created_at: primary.and_then(|p| p.created_at.clone()),
                primary_updated_at: primary.and_then(|p| p.updated_at.clone()),
                rollout: primary.and_then(|p| p.rollout_state.clone()),
                task_definition: s.task_definition.as_deref().map(last_segment),
                name: s.service_name,
                arn: s.service_arn,
                desired: s.desired_count,
                running: s.running_count,
                pending: s.pending_count,
                status: s.status,
            }
        })
        .collect();
    out.sort_by_key(|service| service.name.to_lowercase());
    Ok(out)
}

#[derive(Serialize)]
pub struct EcsServiceDetail {
    name: String,
    status: Option<String>,
    launch_type: Option<String>,
    desired: Option<i64>,
    running: Option<i64>,
    pending: Option<i64>,
    task_definition: Option<String>,
    deployments: Vec<EcsDeployment>,
    load_balancers: Vec<EcsServiceTarget>,
    events: Vec<EcsEvent>,
}

#[derive(Serialize)]
pub struct EcsDeployment {
    status: Option<String>,
    rollout: Option<String>,
    rollout_reason: Option<String>,
    task_definition: Option<String>,
    desired: Option<i64>,
    running: Option<i64>,
    pending: Option<i64>,
    failed: Option<i64>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Serialize)]
pub struct EcsServiceTarget {
    target_group: Option<String>,
    load_balancer: Option<String>,
    container: Option<String>,
    port: Option<i64>,
}

#[derive(Serialize)]
pub struct EcsEvent {
    at: Option<String>,
    message: Option<String>,
}

/// One service with its deployments and its latest events, newest first.
pub(crate) async fn service(
    target: &Target,
    cluster: &str,
    service: &str,
) -> AwsResult<EcsServiceDetail> {
    let s = describe_service(target, cluster, service).await?;
    Ok(EcsServiceDetail {
        task_definition: s.task_definition.as_deref().map(last_segment),
        deployments: s
            .deployments
            .into_iter()
            .map(|d| EcsDeployment {
                status: d.status,
                rollout: d.rollout_state,
                rollout_reason: d.rollout_state_reason,
                task_definition: d.task_definition.as_deref().map(last_segment),
                desired: d.desired_count,
                running: d.running_count,
                pending: d.pending_count,
                failed: d.failed_tasks,
                created_at: d.created_at,
                updated_at: d.updated_at,
            })
            .collect(),
        load_balancers: s
            .load_balancers
            .into_iter()
            .map(|lb| EcsServiceTarget {
                target_group: lb.target_group_arn.as_deref().map(target_group_name),
                load_balancer: lb.load_balancer_name,
                container: lb.container_name,
                port: lb.container_port,
            })
            .collect(),
        events: s
            .events
            .into_iter()
            .take(SERVICE_EVENTS)
            .map(|e| EcsEvent {
                at: e.created_at,
                message: e.message,
            })
            .collect(),
        name: s.service_name,
        status: s.status,
        launch_type: s.launch_type,
        desired: s.desired_count,
        running: s.running_count,
        pending: s.pending_count,
    })
}

/// A target group's ARN ends in `targetgroup/<name>/<id>`.
fn target_group_name(arn: &str) -> String {
    arn.split('/').nth(1).unwrap_or(arn).to_string()
}

async fn describe_service(
    target: &Target,
    cluster: &str,
    service: &str,
) -> AwsResult<DescribedService> {
    cli_value(cluster)?;
    cli_value(service)?;
    let resp: DescribedServices = aws_json(
        target,
        &[
            "ecs",
            "describe-services",
            "--cluster",
            cluster,
            "--services",
            service,
        ],
    )
    .await?;
    resp.services
        .into_iter()
        .next()
        .ok_or_else(|| AwsError::NotFound(format!("No service {service} in cluster {cluster}.")))
}

// ---- tasks ---------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct EcsTask {
    pub(crate) arn: String,
    pub(crate) task_id: String,
    pub(crate) status: Option<String>,
    pub(crate) desired_status: Option<String>,
    pub(crate) health_status: Option<String>,
    pub(crate) task_definition: Option<String>,
    pub(crate) cpu: Option<String>,
    pub(crate) memory: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) last_status_change: Option<String>,
    pub(crate) stopped_reason: Option<String>,
    pub(crate) availability_zone: Option<String>,
    pub(crate) private_ip: Option<String>,
    pub(crate) containers: Vec<EcsContainer>,
}

#[derive(Serialize, Clone)]
pub struct EcsContainer {
    name: String,
    image: Option<String>,
    status: Option<String>,
    health: Option<String>,
    exit_code: Option<i64>,
    reason: Option<String>,
}

/// The service's tasks, or the whole cluster's, that are running or recently stopped.
pub(crate) async fn tasks(
    target: &Target,
    cluster: &str,
    service: Option<&str>,
    stopped: bool,
) -> AwsResult<Vec<EcsTask>> {
    cli_value(cluster)?;
    #[derive(Deserialize)]
    struct ArnList {
        #[serde(rename = "taskArns")]
        arns: Vec<String>,
    }
    let mut args = vec![
        "ecs",
        "list-tasks",
        "--cluster",
        cluster,
        "--max-items",
        MAX_TASKS,
    ];
    if let Some(service) = service {
        args.extend(["--service-name", cli_value(service)?]);
    }
    if stopped {
        args.extend(["--desired-status", "STOPPED"]);
    }
    let list: ArnList = aws_json(target, &args).await?;

    #[derive(Deserialize)]
    struct Resp {
        tasks: Vec<Task>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Task {
        task_arn: String,
        last_status: Option<String>,
        desired_status: Option<String>,
        health_status: Option<String>,
        task_definition_arn: Option<String>,
        cpu: Option<String>,
        memory: Option<String>,
        started_at: Option<String>,
        execution_stopped_at: Option<String>,
        stopped_reason: Option<String>,
        availability_zone: Option<String>,
        #[serde(default)]
        attachments: Vec<Attachment>,
        #[serde(default)]
        containers: Vec<Container>,
    }
    #[derive(Deserialize)]
    struct Attachment {
        #[serde(default)]
        details: Vec<AttachmentDetail>,
    }
    #[derive(Deserialize)]
    struct AttachmentDetail {
        name: String,
        value: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Container {
        name: String,
        image: Option<String>,
        last_status: Option<String>,
        health_status: Option<String>,
        exit_code: Option<i64>,
        reason: Option<String>,
    }

    let chunks: Vec<Resp> = describe_in_chunks(
        target,
        vec![
            "ecs".into(),
            "describe-tasks".into(),
            "--cluster".into(),
            cluster.into(),
        ],
        "--tasks",
        list.arns,
        ECS_DESCRIBE_CHUNK,
        Vec::new(),
    )
    .await?;

    Ok(chunks
        .into_iter()
        .flat_map(|r| r.tasks)
        .map(|t| EcsTask {
            private_ip: t
                .attachments
                .iter()
                .flat_map(|a| &a.details)
                .find(|d| d.name == "privateIPv4Address")
                .and_then(|d| d.value.clone()),
            availability_zone: t.availability_zone,
            task_id: last_segment(&t.task_arn),
            arn: t.task_arn,
            status: t.last_status,
            desired_status: t.desired_status,
            health_status: t.health_status,
            task_definition: t.task_definition_arn.as_deref().map(last_segment),
            cpu: t.cpu,
            memory: t.memory,
            started_at: t.started_at,
            last_status_change: t.execution_stopped_at,
            stopped_reason: t.stopped_reason,
            containers: t
                .containers
                .into_iter()
                .map(|c| EcsContainer {
                    name: c.name,
                    image: c.image,
                    status: c.last_status,
                    health: c.health_status,
                    exit_code: c.exit_code,
                    reason: c.reason,
                })
                .collect(),
        })
        .collect())
}

// ---- task definitions ---------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskDefinition {
    family: Option<String>,
    revision: Option<i64>,
    status: Option<String>,
    cpu: Option<String>,
    memory: Option<String>,
    network_mode: Option<String>,
    task_role_arn: Option<String>,
    execution_role_arn: Option<String>,
    registered_at: Option<String>,
    #[serde(default)]
    container_definitions: Vec<ContainerDefinition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContainerDefinition {
    #[serde(default)]
    name: String,
    image: Option<String>,
    cpu: Option<i64>,
    memory: Option<i64>,
    memory_reservation: Option<i64>,
    essential: Option<bool>,
    #[serde(default)]
    port_mappings: Vec<PortMapping>,
    #[serde(default)]
    environment: Vec<Named>,
    #[serde(default)]
    secrets: Vec<Named>,
    log_configuration: Option<LogConfiguration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortMapping {
    container_port: Option<i64>,
    protocol: Option<String>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogConfiguration {
    log_driver: Option<String>,
    #[serde(default)]
    options: std::collections::HashMap<String, String>,
}

impl ContainerDefinition {
    /// Where the `awslogs` driver sends this container's output: group, region, stream prefix.
    fn awslogs(&self) -> Option<(String, Option<String>, Option<String>)> {
        let config = self.log_configuration.as_ref()?;
        if config.log_driver.as_deref() != Some("awslogs") {
            return None;
        }
        Some((
            config.options.get("awslogs-group")?.clone(),
            config.options.get("awslogs-region").cloned(),
            config.options.get("awslogs-stream-prefix").cloned(),
        ))
    }
}

async fn describe_task_definition(target: &Target, name: &str) -> AwsResult<TaskDefinition> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resp {
        task_definition: TaskDefinition,
    }
    let resp: Resp = aws_json(
        target,
        &[
            "ecs",
            "describe-task-definition",
            "--task-definition",
            cli_value(name)?,
        ],
    )
    .await?;
    Ok(resp.task_definition)
}

#[derive(Serialize)]
pub struct EcsTaskDefinition {
    name: String,
    status: Option<String>,
    cpu: Option<String>,
    memory: Option<String>,
    network_mode: Option<String>,
    task_role: Option<String>,
    execution_role: Option<String>,
    registered_at: Option<String>,
    containers: Vec<EcsContainerDefinition>,
}

#[derive(Serialize)]
pub struct EcsContainerDefinition {
    name: String,
    image: Option<String>,
    cpu: Option<i64>,
    memory: Option<i64>,
    memory_reservation: Option<i64>,
    essential: Option<bool>,
    ports: Vec<String>,
    env: Vec<String>,
    secrets: Vec<String>,
    log_driver: Option<String>,
    log_group: Option<String>,
    log_stream_prefix: Option<String>,
}

/// A task definition with its containers. Environment variables and secrets
/// are named only, never valued.
pub(crate) async fn task_definition(target: &Target, name: &str) -> AwsResult<EcsTaskDefinition> {
    let td = describe_task_definition(target, name).await?;
    let names = |list: Vec<Named>| list.into_iter().map(|n| n.name).collect();
    Ok(EcsTaskDefinition {
        name: match (&td.family, td.revision) {
            (Some(family), Some(revision)) => format!("{family}:{revision}"),
            _ => name.to_string(),
        },
        status: td.status,
        cpu: td.cpu,
        memory: td.memory,
        network_mode: td.network_mode,
        task_role: td.task_role_arn.as_deref().map(last_segment),
        execution_role: td.execution_role_arn.as_deref().map(last_segment),
        registered_at: td.registered_at,
        containers: td
            .container_definitions
            .into_iter()
            .map(|c| {
                let logs = c.awslogs();
                EcsContainerDefinition {
                    ports: c
                        .port_mappings
                        .iter()
                        .filter_map(|p| {
                            let port = p.container_port?;
                            Some(format!("{port}/{}", p.protocol.as_deref().unwrap_or("tcp")))
                        })
                        .collect(),
                    log_driver: c
                        .log_configuration
                        .as_ref()
                        .and_then(|l| l.log_driver.clone()),
                    log_group: logs.as_ref().map(|l| l.0.clone()),
                    log_stream_prefix: logs.and_then(|l| l.2),
                    env: names(c.environment),
                    secrets: names(c.secrets),
                    name: c.name,
                    image: c.image,
                    cpu: c.cpu,
                    memory: c.memory,
                    memory_reservation: c.memory_reservation,
                    essential: c.essential,
                }
            })
            .collect(),
    })
}

// ---- log configuration --------------------------------------------------
//
// For the `awslogs` driver a specific task's stream is
// `<prefix>/<container>/<taskId>`. We expose service-level (group only) and
// task-level (group + stream) resolutions so the UI can hop straight to the
// right CloudWatch view.

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

fn no_awslogs() -> AwsError {
    AwsError::BadArg("no container with awslogs driver")
}

pub(crate) async fn service_log_config(
    target: &Target,
    cluster: &str,
    service: &str,
) -> AwsResult<EcsServiceLog> {
    let described = describe_service(target, cluster, service).await?;
    let td_arn = described
        .task_definition
        .ok_or(AwsError::BadArg("service has no taskDefinition"))?;
    let td = describe_task_definition(target, &td_arn).await?;
    td.container_definitions
        .iter()
        .find_map(|c| {
            let (log_group, region, _) = c.awslogs()?;
            Some(EcsServiceLog {
                log_group,
                container_name: c.name.clone(),
                region,
            })
        })
        .ok_or_else(no_awslogs)
}

pub(crate) async fn task_log_config(
    target: &Target,
    cluster: &str,
    task_arn: &str,
) -> AwsResult<EcsTaskLog> {
    #[derive(Deserialize)]
    struct Resp {
        tasks: Vec<Task>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Task {
        task_definition_arn: Option<String>,
    }
    let resp: Resp = aws_json(
        target,
        &[
            "ecs",
            "describe-tasks",
            "--cluster",
            cli_value(cluster)?,
            "--tasks",
            cli_value(task_arn)?,
        ],
    )
    .await?;
    let td_arn = resp
        .tasks
        .into_iter()
        .next()
        .ok_or(AwsError::BadArg("task not found"))?
        .task_definition_arn
        .ok_or(AwsError::BadArg("no taskDefinitionArn"))?;
    let task_id = last_segment(task_arn);

    let td = describe_task_definition(target, &td_arn).await?;
    td.container_definitions
        .iter()
        .find_map(|c| {
            let (log_group, region, prefix) = c.awslogs()?;
            let log_stream = match prefix.filter(|prefix| !prefix.is_empty()) {
                Some(prefix) => format!("{prefix}/{}/{task_id}", c.name),
                None => format!("{}/{task_id}", c.name),
            };
            Some(EcsTaskLog {
                log_group,
                log_stream,
                container_name: c.name.clone(),
                region,
            })
        })
        .ok_or_else(no_awslogs)
}
