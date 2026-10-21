//! Pods: what each one runs, how it is doing in the words `kubectl get pods`
//! uses, and the workload that owns it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KubeResult;
use crate::kubectl::{self, Scope};

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    pub name: String,
    pub namespace: String,
    /// What `kubectl get pods` shows as STATUS: Running, CrashLoopBackOff, Completed, Init:0/1 and so on.
    pub status: String,
    /// Running, Pending, Succeeded, Failed or Unknown.
    pub phase: String,
    pub ready: u32,
    pub containers: u32,
    pub restarts: u32,
    /// RFC 3339.
    pub created: Option<String>,
    pub node: Option<String>,
    pub ip: Option<String>,
    /// The workload that made it, such as Deployment/web.
    pub owner: Option<Owner>,
}

#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct Owner {
    pub kind: String,
    pub name: String,
}

pub fn text(value: &Value, path: &str) -> Option<String> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn count(value: &Value, path: &str) -> u32 {
    value
        .pointer(path)
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(0)
}

fn statuses<'a>(pod: &'a Value, path: &str) -> Vec<&'a Value> {
    pod.pointer(path)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

/// A container's trouble, when it has one: why it waits, or why it ended.
fn trouble(container: &Value) -> Option<String> {
    text(container, "/state/waiting/reason").or_else(|| {
        let ended = text(container, "/state/terminated/reason")?;
        Some(if ended == "Error" {
            match container
                .pointer("/state/terminated/exitCode")
                .and_then(Value::as_i64)
            {
                Some(code) => format!("Error ({code})"),
                None => ended,
            }
        } else {
            ended
        })
    })
}

/// STATUS as kubectl prints it, worked out the same way.
pub fn status_of(pod: &Value) -> String {
    if pod
        .pointer("/metadata/deletionTimestamp")
        .is_some_and(|stamp| !stamp.is_null())
    {
        return "Terminating".into();
    }
    let inits = statuses(pod, "/status/initContainerStatuses");
    let init_total = pod
        .pointer("/spec/initContainers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    for (index, init) in inits.iter().enumerate() {
        let finished_ok = init
            .pointer("/state/terminated/exitCode")
            .and_then(Value::as_i64)
            == Some(0);
        if finished_ok {
            continue;
        }
        return match trouble(init) {
            Some(reason) if reason != "PodInitializing" => format!("Init:{reason}"),
            _ => format!("Init:{index}/{init_total}"),
        };
    }
    let containers = statuses(pod, "/status/containerStatuses");
    if let Some(reason) = containers
        .iter()
        .filter(|container| container.pointer("/ready").and_then(Value::as_bool) != Some(true))
        .find_map(|container| trouble(container))
    {
        return reason;
    }
    text(pod, "/status/reason")
        .or_else(|| text(pod, "/status/phase"))
        .unwrap_or_else(|| "Unknown".into())
}

/// The workload behind the pod: a ReplicaSet's Deployment, or the owner itself.
fn owner_of(pod: &Value) -> Option<Owner> {
    let owner = pod
        .pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)?
        .iter()
        .find(|owner| owner.pointer("/controller").and_then(Value::as_bool) == Some(true))?;
    let kind = text(owner, "/kind")?;
    let name = text(owner, "/name")?;
    let hash = text(pod, "/metadata/labels/pod-template-hash");
    match (kind.as_str(), hash) {
        ("ReplicaSet", Some(hash)) => match name.strip_suffix(&format!("-{hash}")) {
            Some(deployment) => Some(Owner {
                kind: "Deployment".into(),
                name: deployment.to_string(),
            }),
            None => Some(Owner { kind, name }),
        },
        _ => Some(Owner { kind, name }),
    }
}

pub fn pod_of(pod: &Value) -> Option<Pod> {
    let containers = statuses(pod, "/status/containerStatuses");
    let declared = pod
        .pointer("/spec/containers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Some(Pod {
        name: text(pod, "/metadata/name")?,
        namespace: text(pod, "/metadata/namespace").unwrap_or_default(),
        status: status_of(pod),
        phase: text(pod, "/status/phase").unwrap_or_else(|| "Unknown".into()),
        ready: u32::try_from(
            containers
                .iter()
                .filter(|container| {
                    container.pointer("/ready").and_then(Value::as_bool) == Some(true)
                })
                .count(),
        )
        .unwrap_or(0),
        containers: u32::try_from(declared.max(containers.len())).unwrap_or(0),
        restarts: containers
            .iter()
            .map(|container| count(container, "/restartCount"))
            .sum(),
        created: text(pod, "/metadata/creationTimestamp"),
        node: text(pod, "/spec/nodeName"),
        ip: text(pod, "/status/podIP"),
        owner: owner_of(pod),
    })
}

/// Pods with trouble first, then by name.
pub fn pods_of(list: &Value) -> Vec<Pod> {
    let mut pods: Vec<Pod> = list
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(pod_of)
        .collect();
    let calm = |pod: &Pod| {
        matches!(pod.status.as_str(), "Running" | "Completed" | "Succeeded")
            && pod.ready == pod.containers
            || pod.status == "Completed"
    };
    pods.sort_by(|a, b| (calm(a), &a.namespace, &a.name).cmp(&(calm(b), &b.namespace, &b.name)));
    pods
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListPods {
    #[serde(flatten)]
    pub scope: Scope,
    /// Every namespace's, not only the one in scope.
    pub all_namespaces: bool,
    /// A label selector, such as app=web.
    pub selector: Option<String>,
    /// Words in the pod's name.
    pub query: Option<String>,
}

pub async fn list(input: ListPods) -> KubeResult<Vec<Pod>> {
    let mut args = vec!["get", "pods"];
    if input.all_namespaces {
        args.push("--all-namespaces");
    }
    let selector;
    if let Some(chosen) = input
        .selector
        .as_deref()
        .filter(|chosen| !chosen.trim().is_empty())
    {
        selector = format!("--selector={}", kubectl::plain("label selector", chosen)?);
        args.push(&selector);
    }
    let list: Value = kubectl::json(&input.scope, &args).await?;
    let mut pods = pods_of(&list);
    if let Some(words) = input
        .query
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty())
    {
        pods.retain(|pod| pod.name.contains(&words));
    }
    Ok(pods)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;

    pub fn crashing() -> Value {
        json!({
            "metadata": { "name": "worker-546f6f965-9swqn", "namespace": "shop", "creationTimestamp": "2026-10-19T08:00:00Z",
                          "labels": { "pod-template-hash": "546f6f965", "app": "worker" },
                          "ownerReferences": [{ "kind": "ReplicaSet", "name": "worker-546f6f965", "controller": true }] },
            "spec": { "nodeName": "sikemux-control-plane", "containers": [{ "name": "worker" }] },
            "status": { "phase": "Running", "podIP": "10.244.0.7", "containerStatuses": [
                { "name": "worker", "ready": false, "restartCount": 4,
                  "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                  "lastState": { "terminated": { "reason": "Error", "exitCode": 1 } } }
            ] }
        })
    }

    #[test]
    fn a_pod_reads_as_kubectl_get_pods_shows_it() {
        let pod = pod_of(&crashing()).unwrap();
        assert_eq!(pod.status, "CrashLoopBackOff");
        assert_eq!((pod.ready, pod.containers, pod.restarts), (0, 1, 4));
        assert_eq!(
            pod.owner,
            Some(Owner {
                kind: "Deployment".into(),
                name: "worker".into()
            })
        );
        assert_eq!(pod.node.as_deref(), Some("sikemux-control-plane"));
    }

    #[test]
    fn statuses_follow_kubectls_own_rules() {
        let done = json!({ "metadata": { "name": "report-now-5gwc2" }, "status": { "phase": "Succeeded", "containerStatuses": [
            { "ready": false, "state": { "terminated": { "reason": "Completed", "exitCode": 0 } } }] } });
        assert_eq!(status_of(&done), "Completed");
        let failed = json!({ "status": { "phase": "Failed", "containerStatuses": [
            { "ready": false, "state": { "terminated": { "reason": "Error", "exitCode": 137 } } }] } });
        assert_eq!(status_of(&failed), "Error (137)");
        let pulling = json!({ "status": { "phase": "Pending", "containerStatuses": [
            { "ready": false, "state": { "waiting": { "reason": "ImagePullBackOff" } } }] } });
        assert_eq!(status_of(&pulling), "ImagePullBackOff");
        let init = json!({ "spec": { "initContainers": [{}, {}] }, "status": { "phase": "Pending", "initContainerStatuses": [
            { "state": { "terminated": { "exitCode": 0 } } }, { "state": { "running": {} } }] } });
        assert_eq!(status_of(&init), "Init:1/2");
        let leaving = json!({ "metadata": { "deletionTimestamp": "2026-10-19T08:00:00Z" }, "status": { "phase": "Running" } });
        assert_eq!(status_of(&leaving), "Terminating");
        let evicted = json!({ "status": { "phase": "Failed", "reason": "Evicted" } });
        assert_eq!(status_of(&evicted), "Evicted");
    }

    #[test]
    fn pods_in_trouble_come_first() {
        let running = json!({ "metadata": { "name": "a-web", "namespace": "shop" }, "spec": { "containers": [{}] },
            "status": { "phase": "Running", "containerStatuses": [{ "ready": true, "state": { "running": {} } }] } });
        let list = json!({ "items": [running, crashing()] });
        let names: Vec<String> = pods_of(&list).into_iter().map(|pod| pod.name).collect();
        assert_eq!(names, ["worker-546f6f965-9swqn", "a-web"]);
    }
}
