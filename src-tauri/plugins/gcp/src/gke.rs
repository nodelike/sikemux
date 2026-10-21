// Google Kubernetes Engine: clusters with their node pools, and inside each
// cluster its workloads, pods, services and pod logs, read from the cluster's
// own Kubernetes API with the account's Google token. Restarting and scaling a
// Deployment and resizing a node pool are the changes it makes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use reqwest::{Client, Method};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, Target};
use crate::client::{self, at, segment, short, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://container.googleapis.com/v1";
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_TAIL: u32 = 500;
const MAX_TAIL: u32 = 5000;
const MAX_REPLICAS: u32 = 1000;

static CLUSTER_CLIENTS: Mutex<Option<HashMap<String, Client>>> = Mutex::new(None);

fn node_pool(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "status": at(value, "/status"),
        "version": at(value, "/version"),
        "machineType": at(value, "/config/machineType"),
        "diskSizeGb": at(value, "/config/diskSizeGb"),
        "spot": at(value, "/config/spot").as_bool() == Some(true) || at(value, "/config/preemptible").as_bool() == Some(true),
        "nodes": at(value, "/initialNodeCount"),
        "autoscaling": at(value, "/autoscaling/enabled").as_bool() == Some(true),
        "minNodes": at(value, "/autoscaling/minNodeCount"),
        "maxNodes": at(value, "/autoscaling/maxNodeCount"),
    })
}

/// A cluster as the pane and agents read it, without its certificate or endpoint secrets.
pub fn cluster(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "location": at(value, "/location"),
        "status": at(value, "/status"),
        "message": at(value, "/statusMessage"),
        "version": at(value, "/currentMasterVersion"),
        "nodeVersion": at(value, "/currentNodeVersion"),
        "nodes": at(value, "/currentNodeCount"),
        "autopilot": at(value, "/autopilot/enabled").as_bool() == Some(true),
        "channel": at(value, "/releaseChannel/channel"),
        "network": short(&text(value, "/network")),
        "private": at(value, "/privateClusterConfig/enablePrivateEndpoint").as_bool() == Some(true),
        "nodePools": at(value, "/nodePools").as_array().into_iter().flatten().map(node_pool).collect::<Vec<_>>(),
        "createdAt": at(value, "/createTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Every GKE cluster in the project, in every location.
pub async fn clusters(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let page = client::get(
        &input.target,
        &format!("{API}/projects/{project}/locations/-/clusters"),
        &[],
    )
    .await?;
    let mut listed: Vec<Value> = at(&page, "/clusters")
        .as_array()
        .into_iter()
        .flatten()
        .map(cluster)
        .collect();
    listed.sort_by_key(|each| text(each, "/name"));
    Ok(listed)
}

#[derive(Deserialize, Clone)]
pub struct ClusterRef {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub cluster: String,
}

fn cluster_url(input: &ClusterRef) -> GcpResult<String> {
    let project = input.target.project()?;
    let location = segment("location", &input.location)?;
    let name = segment("cluster", &input.cluster)?;
    Ok(format!(
        "{API}/projects/{project}/locations/{location}/clusters/{name}"
    ))
}

pub async fn cluster_detail(input: ClusterRef) -> GcpResult<Value> {
    let raw = client::get(&input.target, &cluster_url(&input)?, &[]).await?;
    Ok(cluster(&raw))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolSize {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    pub node_pool: String,
    pub nodes: u32,
}

/// Sets how many nodes a node pool runs, per zone.
pub async fn resize_pool(input: PoolSize) -> GcpResult<Value> {
    if input.nodes > MAX_REPLICAS {
        return Err(GcpError::BadArg(format!(
            "{} nodes is more than this will set",
            input.nodes
        )));
    }
    let url = format!(
        "{}/nodePools/{}:setSize",
        cluster_url(&input.cluster)?,
        segment("node pool", &input.node_pool)?
    );
    let operation = client::post(
        &input.cluster.target,
        &url,
        &json!({ "nodeCount": input.nodes }),
    )
    .await?;
    Ok(json!({ "operation": at(&operation, "/name"), "status": at(&operation, "/status") }))
}

/// Where a cluster's Kubernetes API answers and a client that trusts only its certificate.
async fn kubernetes(input: &ClusterRef) -> GcpResult<(String, Client)> {
    let raw = client::get(&input.target, &cluster_url(input)?, &[]).await?;
    let endpoint = text(&raw, "/endpoint");
    if endpoint.is_empty() {
        return Err(GcpError::Response("the cluster has no endpoint yet".into()));
    }
    let base = format!("https://{endpoint}");
    if let Ok(held) = CLUSTER_CLIENTS.lock() {
        if let Some(client) = held.as_ref().and_then(|clients| clients.get(&base)) {
            return Ok((base, client.clone()));
        }
    }
    let pem = base64::engine::general_purpose::STANDARD
        .decode(text(&raw, "/masterAuth/clusterCaCertificate"))
        .map_err(|_| GcpError::Response("the cluster's certificate could not be read".into()))?;
    let certificate = reqwest::Certificate::from_pem(&pem)
        .map_err(|error| GcpError::Response(format!("the cluster's certificate: {error}")))?;
    let client = Client::builder()
        .tls_certs_only([certificate])
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(60))
        .user_agent("sikemux-gcp/0.1")
        .build()
        .map_err(|error| GcpError::Transport(error.to_string()))?;
    if let Ok(mut held) = CLUSTER_CLIENTS.lock() {
        held.get_or_insert_with(HashMap::new)
            .insert(base.clone(), client.clone());
    }
    Ok((base, client))
}

/// Kubernetes says a call failed with a Status object holding a message and a reason.
fn kubernetes_checked(status: u16, body: &[u8]) -> GcpResult<Value> {
    let value: Value = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or_else(|_| json!({ "message": String::from_utf8_lossy(body).chars().take(300).collect::<String>() }))
    };
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Kubernetes refused the request")
        .to_string();
    Err(match status {
        401 => GcpError::Auth(message),
        403 => GcpError::Permission(message),
        404 => GcpError::NotFound(message),
        429 => GcpError::RateLimited,
        _ => GcpError::Api { status, message },
    })
}

async fn kube(
    input: &ClusterRef,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    patch: Option<(&str, &Value)>,
) -> GcpResult<(u16, Vec<u8>)> {
    let (base, http) = kubernetes(input).await?;
    let token = auth::token(input.target.account()).await?;
    let mut request = http
        .request(method, format!("{base}{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some((content_type, body)) = patch {
        request = request
            .header("Content-Type", content_type)
            .body(body.to_string());
    }
    let response = request.send().await.map_err(|error| {
        if error.is_connect() || error.is_timeout() {
            GcpError::Transport(format!("the cluster's API could not be reached; a private cluster answers only inside its network ({error})"))
        } else {
            GcpError::from(error)
        }
    })?;
    let status = response.status().as_u16();
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_LOG_BYTES {
            break;
        }
    }
    Ok((status, bytes))
}

async fn kube_json(input: &ClusterRef, path: &str, query: &[(&str, String)]) -> GcpResult<Value> {
    let (status, bytes) = kube(input, Method::GET, path, query, None).await?;
    kubernetes_checked(status, &bytes)
}

fn namespaced(kind: &str, namespace: Option<&str>) -> GcpResult<String> {
    match namespace
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
    {
        Some(namespace) => Ok(format!(
            "/namespaces/{}/{kind}",
            segment("namespace", namespace)?
        )),
        None => Ok(format!("/{kind}")),
    }
}

fn containers_of(spec: &Value) -> Vec<Value> {
    at(spec, "/containers")
        .as_array()
        .into_iter()
        .flatten()
        .map(
            |container| json!({ "name": at(container, "/name"), "image": at(container, "/image") }),
        )
        .collect()
}

fn selector_of(value: &Value) -> String {
    at(value, "/spec/selector/matchLabels")
        .as_object()
        .map(|labels| {
            labels
                .iter()
                .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

/// A Deployment, StatefulSet or DaemonSet as the pane and agents read it.
pub fn workload(kind: &str, value: &Value) -> Value {
    let (desired, ready) = if kind == "DaemonSet" {
        (
            at(value, "/status/desiredNumberScheduled")
                .as_u64()
                .unwrap_or(0),
            at(value, "/status/numberReady").as_u64().unwrap_or(0),
        )
    } else {
        (
            at(value, "/spec/replicas").as_u64().unwrap_or(0),
            at(value, "/status/readyReplicas").as_u64().unwrap_or(0),
        )
    };
    json!({
        "kind": kind,
        "name": at(value, "/metadata/name"),
        "namespace": at(value, "/metadata/namespace"),
        "desired": desired,
        "ready": ready,
        "updated": at(value, "/status/updatedReplicas"),
        "containers": containers_of(at(value, "/spec/template/spec")),
        "selector": selector_of(value),
        "createdAt": at(value, "/metadata/creationTimestamp"),
    })
}

#[derive(Deserialize)]
pub struct InCluster {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Every Deployment, StatefulSet and DaemonSet, in one namespace or all of them.
pub async fn workloads(input: InCluster) -> GcpResult<Vec<Value>> {
    let namespace = input.namespace.as_deref();
    let (deployments, statefulsets, daemonsets) = futures::future::try_join3(
        kube_json(
            &input.cluster,
            &format!("/apis/apps/v1{}", namespaced("deployments", namespace)?),
            &[],
        ),
        kube_json(
            &input.cluster,
            &format!("/apis/apps/v1{}", namespaced("statefulsets", namespace)?),
            &[],
        ),
        kube_json(
            &input.cluster,
            &format!("/apis/apps/v1{}", namespaced("daemonsets", namespace)?),
            &[],
        ),
    )
    .await?;
    let mut listed = Vec::new();
    for (kind, list) in [
        ("Deployment", deployments),
        ("StatefulSet", statefulsets),
        ("DaemonSet", daemonsets),
    ] {
        listed.extend(
            at(&list, "/items")
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| workload(kind, item)),
        );
    }
    listed.retain(|each| {
        !text(each, "/namespace").starts_with("kube-")
            && text(each, "/namespace") != "gke-managed-system"
            && text(each, "/namespace") != "gmp-system"
    });
    listed.sort_by_key(|each| (text(each, "/namespace"), text(each, "/name")));
    Ok(listed)
}

/// A pod as the pane and agents read it, with why a container is not running when it is not.
pub fn pod(value: &Value) -> Value {
    let statuses = at(value, "/status/containerStatuses")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let restarts: u64 = statuses
        .iter()
        .filter_map(|status| at(status, "/restartCount").as_u64())
        .sum();
    let ready = statuses
        .iter()
        .filter(|status| at(status, "/ready").as_bool() == Some(true))
        .count();
    let reason = statuses
        .iter()
        .find_map(|status| {
            at(status, "/state/waiting/reason")
                .as_str()
                .or_else(|| at(status, "/state/terminated/reason").as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    json!({
        "name": at(value, "/metadata/name"),
        "namespace": at(value, "/metadata/namespace"),
        "phase": at(value, "/status/phase"),
        "reason": if reason.is_empty() { Value::Null } else { json!(reason) },
        "ready": format!("{ready}/{}", statuses.len()),
        "restarts": restarts,
        "node": at(value, "/spec/nodeName"),
        "ip": at(value, "/status/podIP"),
        "containers": containers_of(at(value, "/spec")),
        "createdAt": at(value, "/metadata/creationTimestamp"),
    })
}

#[derive(Deserialize)]
pub struct Pods {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    #[serde(default)]
    pub namespace: Option<String>,
    /// A label selector such as `app=api`, as a workload's `selector` gives.
    #[serde(default)]
    pub selector: Option<String>,
}

pub async fn pods(input: Pods) -> GcpResult<Vec<Value>> {
    let mut query = Vec::new();
    if let Some(selector) = input
        .selector
        .as_deref()
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
    {
        query.push(("labelSelector", selector.to_string()));
    }
    let list = kube_json(
        &input.cluster,
        &format!("/api/v1{}", namespaced("pods", input.namespace.as_deref())?),
        &query,
    )
    .await?;
    let mut listed: Vec<Value> = at(&list, "/items")
        .as_array()
        .into_iter()
        .flatten()
        .map(pod)
        .collect();
    listed.retain(|each| !text(each, "/namespace").starts_with("kube-"));
    listed.sort_by_key(|each| (text(each, "/namespace"), text(each, "/name")));
    Ok(listed)
}

pub fn service(value: &Value) -> Value {
    let ports: Vec<String> = at(value, "/spec/ports")
        .as_array()
        .into_iter()
        .flatten()
        .map(|port| format!("{}/{}", at(port, "/port"), text(port, "/protocol")))
        .collect();
    let external: Vec<String> = at(value, "/status/loadBalancer/ingress")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ingress| {
            ingress
                .get("ip")
                .or_else(|| ingress.get("hostname"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    json!({
        "name": at(value, "/metadata/name"),
        "namespace": at(value, "/metadata/namespace"),
        "type": at(value, "/spec/type"),
        "clusterIp": at(value, "/spec/clusterIP"),
        "externalIps": external,
        "ports": ports,
    })
}

pub async fn services(input: InCluster) -> GcpResult<Vec<Value>> {
    let list = kube_json(
        &input.cluster,
        &format!(
            "/api/v1{}",
            namespaced("services", input.namespace.as_deref())?
        ),
        &[],
    )
    .await?;
    let mut listed: Vec<Value> = at(&list, "/items")
        .as_array()
        .into_iter()
        .flatten()
        .map(service)
        .collect();
    listed.retain(|each| {
        !text(each, "/namespace").starts_with("kube-")
            && !(text(each, "/namespace") == "default" && text(each, "/name") == "kubernetes")
    });
    Ok(listed)
}

/// A node as the pane and agents read it: whether it is ready, why not, and what it can hold.
pub fn node(value: &Value) -> Value {
    let conditions = at(value, "/status/conditions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ready = conditions.iter().any(|condition| {
        text(condition, "/type") == "Ready" && text(condition, "/status") == "True"
    });
    let pressure: Vec<String> = conditions
        .iter()
        .filter(|condition| {
            text(condition, "/type") != "Ready" && text(condition, "/status") == "True"
        })
        .map(|condition| text(condition, "/type"))
        .collect();
    let labels = at(value, "/metadata/labels");
    json!({
        "name": at(value, "/metadata/name"),
        "ready": ready,
        "pressure": pressure,
        "unschedulable": at(value, "/spec/unschedulable").as_bool() == Some(true),
        "pool": at(labels, "/cloud.google.com~1gke-nodepool"),
        "machineType": at(labels, "/node.kubernetes.io~1instance-type"),
        "zone": at(labels, "/topology.kubernetes.io~1zone"),
        "spot": at(labels, "/cloud.google.com~1gke-spot").as_str() == Some("true") || at(labels, "/cloud.google.com~1gke-preemptible").as_str() == Some("true"),
        "cpu": at(value, "/status/allocatable/cpu"),
        "memory": at(value, "/status/allocatable/memory"),
        "pods": at(value, "/status/allocatable/pods"),
        "version": at(value, "/status/nodeInfo/kubeletVersion"),
        "createdAt": at(value, "/metadata/creationTimestamp"),
    })
}

/// The cluster's nodes, ready or not.
pub async fn nodes(input: ClusterRef) -> GcpResult<Vec<Value>> {
    let list = kube_json(&input, "/api/v1/nodes", &[]).await?;
    let mut listed: Vec<Value> = at(&list, "/items")
        .as_array()
        .into_iter()
        .flatten()
        .map(node)
        .collect();
    listed.sort_by_key(|each| text(each, "/name"));
    Ok(listed)
}

/// An event as `kubectl describe` lists it: what happened, why, how often and when last.
pub fn event(value: &Value) -> Value {
    json!({
        "type": at(value, "/type"),
        "reason": at(value, "/reason"),
        "message": at(value, "/message"),
        "count": value.get("count").cloned().unwrap_or(json!(1)),
        "lastAt": value.get("lastTimestamp").filter(|v| !v.is_null()).or_else(|| value.get("eventTime")).cloned().unwrap_or(Value::Null),
        "source": at(value, "/source/component"),
    })
}

#[derive(Deserialize)]
pub struct PodRef {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    pub namespace: String,
    pub pod: String,
}

/// What Kubernetes recorded about a pod, newest first: scheduling, pulls, starts, probes and kills.
pub async fn pod_events(input: PodRef) -> GcpResult<Vec<Value>> {
    let path = format!(
        "/api/v1/namespaces/{}/events",
        segment("namespace", &input.namespace)?
    );
    let selector = format!("involvedObject.name={}", segment("pod", &input.pod)?);
    let list = kube_json(&input.cluster, &path, &[("fieldSelector", selector)]).await?;
    let mut listed: Vec<Value> = at(&list, "/items")
        .as_array()
        .into_iter()
        .flatten()
        .map(event)
        .collect();
    listed.sort_by_key(|each| std::cmp::Reverse(text(each, "/lastAt")));
    Ok(listed)
}

#[derive(Deserialize)]
pub struct PodLogs {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    pub namespace: String,
    pub pod: String,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub tail: Option<u32>,
    /// The logs of the container's previous run, after it crashed.
    #[serde(default)]
    pub previous: bool,
}

/// The last lines a pod's container printed.
pub async fn pod_logs(input: PodLogs) -> GcpResult<Value> {
    let path = format!(
        "/api/v1/namespaces/{}/pods/{}/log",
        segment("namespace", &input.namespace)?,
        segment("pod", &input.pod)?
    );
    let mut query = vec![
        (
            "tailLines",
            input
                .tail
                .unwrap_or(DEFAULT_TAIL)
                .clamp(1, MAX_TAIL)
                .to_string(),
        ),
        ("timestamps", "true".to_string()),
    ];
    if let Some(container) = input
        .container
        .as_deref()
        .map(str::trim)
        .filter(|container| !container.is_empty())
    {
        query.push(("container", segment("container", container)?));
    }
    if input.previous {
        query.push(("previous", "true".to_string()));
    }
    let (status, bytes) = kube(&input.cluster, Method::GET, &path, &query, None).await?;
    if !(200..300).contains(&status) {
        kubernetes_checked(status, &bytes)?;
    }
    let lines: Vec<Value> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(|line| match line.split_once(' ') {
            Some((at, text)) if at.contains('T') && at.ends_with('Z') => {
                json!({ "at": at, "text": text })
            }
            _ => json!({ "at": Value::Null, "text": line }),
        })
        .collect();
    Ok(json!({ "lines": lines }))
}

#[derive(Deserialize)]
pub struct DeploymentRef {
    #[serde(flatten)]
    pub cluster: ClusterRef,
    pub namespace: String,
    pub deployment: String,
}

fn deployment_path(input: &DeploymentRef) -> GcpResult<String> {
    Ok(format!(
        "/apis/apps/v1/namespaces/{}/deployments/{}",
        segment("namespace", &input.namespace)?,
        segment("deployment", &input.deployment)?
    ))
}

/// Replaces a Deployment's pods one by one, as `kubectl rollout restart` does.
pub async fn restart(input: DeploymentRef, now: &str) -> GcpResult<Value> {
    let body = json!({ "spec": { "template": { "metadata": { "annotations": { "kubectl.kubernetes.io/restartedAt": now } } } } });
    let (status, bytes) = kube(
        &input.cluster,
        Method::PATCH,
        &deployment_path(&input)?,
        &[],
        Some(("application/strategic-merge-patch+json", &body)),
    )
    .await?;
    let updated = kubernetes_checked(status, &bytes)?;
    Ok(workload("Deployment", &updated))
}

#[derive(Deserialize)]
pub struct Scale {
    #[serde(flatten)]
    pub deployment: DeploymentRef,
    pub replicas: u32,
}

/// Sets how many pods a Deployment runs.
pub async fn scale(input: Scale) -> GcpResult<Value> {
    if input.replicas > MAX_REPLICAS {
        return Err(GcpError::BadArg(format!(
            "{} replicas is more than this will set",
            input.replicas
        )));
    }
    let path = format!("{}/scale", deployment_path(&input.deployment)?);
    let body = json!({ "spec": { "replicas": input.replicas } });
    let (status, bytes) = kube(
        &input.deployment.cluster,
        Method::PATCH,
        &path,
        &[],
        Some(("application/merge-patch+json", &body)),
    )
    .await?;
    let scaled = kubernetes_checked(status, &bytes)?;
    Ok(json!({ "replicas": at(&scaled, "/spec/replicas") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_reads_without_its_certificate() {
        let read = cluster(&json!({
            "name": "prod", "location": "us-central1", "status": "RUNNING", "currentMasterVersion": "1.30.4-gke.1", "currentNodeCount": 6,
            "autopilot": { "enabled": false }, "network": "projects/acme/global/networks/default",
            "masterAuth": { "clusterCaCertificate": "LS0tLS1CRUdJTi1TRUNSRVQ=" }, "endpoint": "34.1.1.1",
            "nodePools": [{ "name": "default-pool", "status": "RUNNING", "config": { "machineType": "e2-standard-4", "spot": true }, "autoscaling": { "enabled": true, "minNodeCount": 1, "maxNodeCount": 5 } }]
        }));
        assert_eq!(read["nodes"], 6);
        assert_eq!(read["network"], "default");
        assert_eq!(read["nodePools"][0]["spot"], true);
        assert_eq!(read["nodePools"][0]["maxNodes"], 5);
        assert!(!read.to_string().contains("LS0t"));
        assert!(!read.to_string().contains("34.1.1.1"));
    }

    #[test]
    fn workloads_say_how_many_pods_are_ready() {
        let deployment = workload(
            "Deployment",
            &json!({ "metadata": { "name": "api", "namespace": "shop" }, "spec": { "replicas": 3, "selector": { "matchLabels": { "app": "api" } }, "template": { "spec": { "containers": [{ "name": "api", "image": "img:1" }] } } }, "status": { "readyReplicas": 2 } }),
        );
        assert_eq!(
            (deployment["desired"].as_u64(), deployment["ready"].as_u64()),
            (Some(3), Some(2))
        );
        assert_eq!(deployment["selector"], "app=api");
        let daemon = workload(
            "DaemonSet",
            &json!({ "metadata": { "name": "agent" }, "status": { "desiredNumberScheduled": 6, "numberReady": 6 } }),
        );
        assert_eq!(daemon["ready"], 6);
    }

    #[test]
    fn a_pod_says_why_it_is_not_running() {
        let read = pod(
            &json!({ "metadata": { "name": "api-1", "namespace": "shop" }, "status": { "phase": "Running", "containerStatuses": [
            { "name": "api", "ready": false, "restartCount": 7, "state": { "waiting": { "reason": "CrashLoopBackOff" } } },
            { "name": "proxy", "ready": true, "restartCount": 0, "state": { "running": {} } }
        ] } }),
        );
        assert_eq!(read["reason"], "CrashLoopBackOff");
        assert_eq!(read["ready"], "1/2");
        assert_eq!(read["restarts"], 7);
    }

    #[test]
    fn a_service_lists_its_ports_and_outside_addresses() {
        let read = service(
            &json!({ "metadata": { "name": "web", "namespace": "shop" }, "spec": { "type": "LoadBalancer", "clusterIP": "10.0.0.9", "ports": [{ "port": 80, "protocol": "TCP" }] }, "status": { "loadBalancer": { "ingress": [{ "ip": "35.2.2.2" }] } } }),
        );
        assert_eq!(read["ports"], json!(["80/TCP"]));
        assert_eq!(read["externalIps"], json!(["35.2.2.2"]));
    }

    #[test]
    fn a_node_says_whether_it_is_ready_and_what_presses_on_it() {
        let read = node(&json!({
            "metadata": { "name": "gke-prod-default-pool-a1", "labels": { "cloud.google.com/gke-nodepool": "default-pool", "node.kubernetes.io/instance-type": "e2-standard-4", "cloud.google.com/gke-spot": "true" } },
            "status": { "conditions": [{ "type": "MemoryPressure", "status": "True" }, { "type": "Ready", "status": "True" }], "allocatable": { "cpu": "3920m", "memory": "13Gi", "pods": "110" } }
        }));
        assert_eq!(read["ready"], true);
        assert_eq!(read["pressure"], json!(["MemoryPressure"]));
        assert_eq!(read["pool"], "default-pool");
        assert_eq!(read["machineType"], "e2-standard-4");
        assert_eq!(read["spot"], true);
        assert_eq!(read["cpu"], "3920m");
        assert_eq!(
            node(&json!({ "status": { "conditions": [{ "type": "Ready", "status": "False" }] } }))
                ["ready"],
            false
        );
    }

    #[test]
    fn an_event_reads_as_kubectl_describe_lists_it() {
        let read = event(
            &json!({ "type": "Warning", "reason": "BackOff", "message": "Back-off restarting failed container", "count": 31, "lastTimestamp": "t2", "source": { "component": "kubelet" } }),
        );
        assert_eq!(
            (read["reason"].as_str(), read["count"].as_u64()),
            (Some("BackOff"), Some(31))
        );
        assert_eq!(
            event(&json!({ "reason": "Scheduled", "lastTimestamp": null, "eventTime": "t1" }))
                ["lastAt"],
            "t1"
        );
    }

    #[test]
    fn kubernetes_errors_read_in_words() {
        assert!(
            matches!(kubernetes_checked(403, br#"{"kind":"Status","message":"pods is forbidden"}"#), Err(GcpError::Permission(m)) if m == "pods is forbidden")
        );
        assert!(matches!(
            kubernetes_checked(404, b"{}"),
            Err(GcpError::NotFound(_))
        ));
        assert_eq!(
            namespaced("pods", Some("shop")).ok().as_deref(),
            Some("/namespaces/shop/pods")
        );
        assert_eq!(namespaced("pods", None).ok().as_deref(), Some("/pods"));
        assert!(namespaced("pods", Some("../secrets")).is_err());
    }
}
