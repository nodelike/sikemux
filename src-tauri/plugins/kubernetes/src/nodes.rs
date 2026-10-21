//! A cluster's nodes: their roles and whether they're ready, the pressure the
//! kubelet reports, what they can hold, and cordoning one so nothing new lands on it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};
use crate::pods::text;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub name: String,
    /// Such as control-plane or worker, from the node-role labels.
    pub roles: Vec<String>,
    /// Ready, NotReady or Unknown.
    pub status: String,
    /// Cordoned: running pods stay, new ones aren't placed here.
    pub cordoned: bool,
    /// MemoryPressure, DiskPressure, PIDPressure or NetworkUnavailable, whichever are on.
    pub pressure: Vec<String>,
    /// What the Ready condition says when the node isn't ready.
    pub reason: Option<String>,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub runtime: String,
    pub internal_ip: Option<String>,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub max_pods: u64,
    /// Only when the cluster runs metrics-server.
    pub cpu_used_millis: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub taints: Vec<String>,
    pub created: Option<String>,
}

/// CPU as Kubernetes writes it, such as `4`, `3800m` or `0.5`, in thousandths of a core.
pub fn cpu_millis(quantity: &str) -> u64 {
    let quantity = quantity.trim();
    if let Some(millis) = quantity.strip_suffix('m') {
        return millis.parse().unwrap_or(0);
    }
    if let Some(nanos) = quantity.strip_suffix('n') {
        return nanos.parse::<u64>().map_or(0, |nanos| nanos / 1_000_000);
    }
    quantity
        .parse::<f64>()
        .map_or(0, |cores| (cores * 1000.0).round() as u64)
}

/// Memory as Kubernetes writes it, such as `6052828Ki`, `2Gi` or `512M`, in bytes.
pub fn bytes(quantity: &str) -> u64 {
    let quantity = quantity.trim();
    const UNITS: [(&str, f64); 12] = [
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("Ti", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("Pi", 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("Ei", 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("P", 1e15),
    ];
    for (suffix, scale) in UNITS {
        if let Some(number) = quantity.strip_suffix(suffix) {
            return number
                .parse::<f64>()
                .map_or(0, |value| (value * scale).round() as u64);
        }
    }
    quantity
        .parse::<f64>()
        .map_or(0, |value| value.round() as u64)
}

const PRESSURES: [&str; 4] = [
    "MemoryPressure",
    "DiskPressure",
    "PIDPressure",
    "NetworkUnavailable",
];

pub fn node_of(item: &Value) -> Option<Node> {
    let name = text(item, "/metadata/name")?;
    let mut roles: Vec<String> = item
        .pointer("/metadata/labels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, _)| key.strip_prefix("node-role.kubernetes.io/"))
        .filter(|role| !role.is_empty())
        .map(str::to_string)
        .collect();
    roles.sort();
    let conditions: Vec<&Value> = item
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let condition = |kind: &str| {
        conditions
            .iter()
            .find(|condition| text(condition, "/type").as_deref() == Some(kind))
    };
    let ready = condition("Ready");
    let status = match ready.and_then(|ready| text(ready, "/status")).as_deref() {
        Some("True") => "Ready",
        Some("False") => "NotReady",
        _ => "Unknown",
    };
    let pressure = PRESSURES
        .iter()
        .filter(|kind| {
            condition(kind)
                .and_then(|on| text(on, "/status"))
                .as_deref()
                == Some("True")
        })
        .map(|kind| (*kind).to_string())
        .collect();
    let quantity = |path: &str| text(item, path).unwrap_or_default();
    let allocatable_or_capacity = |field: &str| {
        let allocatable = quantity(&format!("/status/allocatable/{field}"));
        if allocatable.is_empty() {
            quantity(&format!("/status/capacity/{field}"))
        } else {
            allocatable
        }
    };
    Some(Node {
        name,
        roles,
        status: status.to_string(),
        cordoned: item
            .pointer("/spec/unschedulable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        pressure,
        reason: ready
            .filter(|_| status != "Ready")
            .and_then(|ready| text(ready, "/message").or_else(|| text(ready, "/reason"))),
        version: quantity("/status/nodeInfo/kubeletVersion"),
        os: quantity("/status/nodeInfo/osImage"),
        arch: quantity("/status/nodeInfo/architecture"),
        runtime: quantity("/status/nodeInfo/containerRuntimeVersion"),
        internal_ip: item
            .pointer("/status/addresses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|address| text(address, "/type").as_deref() == Some("InternalIP"))
            .and_then(|address| text(address, "/address")),
        cpu_millis: cpu_millis(&allocatable_or_capacity("cpu")),
        memory_bytes: bytes(&allocatable_or_capacity("memory")),
        max_pods: allocatable_or_capacity("pods").parse().unwrap_or(0),
        cpu_used_millis: None,
        memory_used_bytes: None,
        taints: item
            .pointer("/spec/taints")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|taint| {
                let key = text(taint, "/key")?;
                let value = text(taint, "/value")
                    .map(|value| format!("={value}"))
                    .unwrap_or_default();
                let effect = text(taint, "/effect").unwrap_or_default();
                Some(format!("{key}{value}:{effect}"))
            })
            .collect(),
        created: text(item, "/metadata/creationTimestamp"),
    })
}

/// What `kubectl top nodes --no-headers` prints: name, CPU, CPU%, memory, memory%.
pub fn usage_of(output: &str) -> Vec<(String, u64, u64)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?.to_string();
            let cpu = cpu_millis(fields.next()?);
            let _cpu_share = fields.next()?;
            let memory = bytes(fields.next()?);
            Some((name, cpu, memory))
        })
        .collect()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ListNodes {
    #[serde(flatten)]
    pub scope: Scope,
}

pub async fn list(input: ListNodes) -> KubeResult<Vec<Node>> {
    let scope = Scope {
        context: input.scope.context,
        namespace: None,
    };
    let (listed, usage) = tokio::join!(
        kubectl::json::<Value>(&scope, &["get", "nodes"]),
        kubectl::run(&scope, &["top", "nodes", "--no-headers"])
    );
    let mut nodes: Vec<Node> = listed?
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(node_of)
        .collect();
    if let Ok(usage) = usage {
        for (name, cpu, memory) in usage_of(&usage) {
            if let Some(node) = nodes.iter_mut().find(|node| node.name == name) {
                node.cpu_used_millis = Some(cpu);
                node.memory_used_bytes = Some(memory);
            }
        }
    }
    nodes.sort_by(|a, b| {
        (
            a.status != "NotReady",
            !a.roles.iter().any(|role| role == "control-plane"),
            &a.name,
        )
            .cmp(&(
                b.status != "NotReady",
                !b.roles.iter().any(|role| role == "control-plane"),
                &b.name,
            ))
    });
    Ok(nodes)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cordon {
    #[serde(flatten)]
    pub scope: Scope,
    pub name: String,
    /// True cordons; false lets pods be placed on it again.
    pub cordon: bool,
}

/// The kubectl arguments for cordoning or uncordoning a node, checked first.
pub fn cordon_args(input: &Cordon) -> KubeResult<Vec<String>> {
    let name = kubectl::object_name("node", &input.name)?;
    let verb = if input.cordon { "cordon" } else { "uncordon" };
    Ok(vec![verb.to_string(), name])
}

pub async fn cordon(input: Cordon) -> KubeResult<crate::actions::Done> {
    let args = cordon_args(&input)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let scope = Scope {
        context: input.scope.context,
        namespace: None,
    };
    let said = kubectl::run(&scope, &refs).await?;
    let message = said.trim().to_string();
    if message.is_empty() {
        return Err(KubeError::Response("kubectl said nothing".into()));
    }
    Ok(crate::actions::Done { message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quantities_are_read_as_kubernetes_writes_them() {
        assert_eq!(cpu_millis("4"), 4000);
        assert_eq!(cpu_millis("3800m"), 3800);
        assert_eq!(cpu_millis("0.5"), 500);
        assert_eq!(cpu_millis("250000000n"), 250);
        assert_eq!(bytes("6052828Ki"), 6_052_828 * 1024);
        assert_eq!(bytes("2Gi"), 2 * 1024 * 1024 * 1024);
        assert_eq!(bytes("512M"), 512_000_000);
        assert_eq!(bytes("1000"), 1000);
        assert_eq!(bytes("nonsense"), 0);
    }

    #[test]
    fn a_node_reads_its_roles_readiness_pressure_and_capacity() {
        let node = node_of(&json!({
            "metadata": { "name": "worker-2", "creationTimestamp": "2026-10-19T08:00:00Z",
                          "labels": { "node-role.kubernetes.io/worker": "", "kubernetes.io/os": "linux" } },
            "spec": { "unschedulable": true, "taints": [{ "key": "dedicated", "value": "gpu", "effect": "NoSchedule" }] },
            "status": {
                "conditions": [
                    { "type": "MemoryPressure", "status": "True" },
                    { "type": "DiskPressure", "status": "False" },
                    { "type": "Ready", "status": "False", "reason": "KubeletNotReady", "message": "container runtime network not ready" }
                ],
                "allocatable": { "cpu": "3800m", "memory": "7Gi", "pods": "110" },
                "capacity": { "cpu": "4", "memory": "8Gi", "pods": "110" },
                "nodeInfo": { "kubeletVersion": "v1.37.0", "osImage": "Debian GNU/Linux 13", "architecture": "arm64", "containerRuntimeVersion": "containerd://2.3.4" },
                "addresses": [{ "type": "Hostname", "address": "worker-2" }, { "type": "InternalIP", "address": "10.0.0.7" }]
            }
        }))
        .expect("reads");
        assert_eq!(node.roles, ["worker"]);
        assert_eq!(node.status, "NotReady");
        assert!(node.cordoned);
        assert_eq!(node.pressure, ["MemoryPressure"]);
        assert_eq!(
            node.reason.as_deref(),
            Some("container runtime network not ready")
        );
        assert_eq!(node.cpu_millis, 3800);
        assert_eq!(node.memory_bytes, 7 * 1024 * 1024 * 1024);
        assert_eq!(node.internal_ip.as_deref(), Some("10.0.0.7"));
        assert_eq!(node.taints, ["dedicated=gpu:NoSchedule"]);
    }

    #[test]
    fn a_ready_node_carries_no_reason() {
        let node = node_of(&json!({
            "metadata": { "name": "cp" },
            "status": { "conditions": [{ "type": "Ready", "status": "True", "reason": "KubeletReady", "message": "kubelet is posting ready status" }] }
        }))
        .expect("reads");
        assert_eq!(node.status, "Ready");
        assert_eq!(node.reason, None);
        assert!(!node.cordoned);
    }

    #[test]
    fn usage_is_read_from_kubectl_top() {
        let usage = usage_of("cp   212m   5%   1203Mi   20%\nworker-1   1.5   37%   3Gi   41%\n");
        assert_eq!(usage[0], ("cp".into(), 212, 1203 * 1024 * 1024));
        assert_eq!(usage[1], ("worker-1".into(), 1500, 3 * 1024 * 1024 * 1024));
    }

    #[test]
    fn cordoning_asks_kubectl_for_just_that() {
        let cordon = Cordon {
            scope: Scope::default(),
            name: "worker-2".into(),
            cordon: true,
        };
        assert_eq!(cordon_args(&cordon).unwrap(), ["cordon", "worker-2"]);
        let free = Cordon {
            scope: Scope::default(),
            name: "worker-2".into(),
            cordon: false,
        };
        assert_eq!(cordon_args(&free).unwrap(), ["uncordon", "worker-2"]);
        let sneaky = Cordon {
            scope: Scope::default(),
            name: "--all".into(),
            cordon: true,
        };
        assert!(cordon_args(&sneaky).is_err());
    }

    /// Reads the nodes of whatever cluster kubectl is pointed at. Reads only.
    #[tokio::test]
    #[ignore = "needs a running cluster"]
    async fn against_the_local_cluster() {
        let nodes = list(ListNodes::default()).await.expect("nodes");
        println!("{nodes:#?}");
        assert!(!nodes.is_empty());
        assert!(nodes
            .iter()
            .all(|node| node.cpu_millis > 0 && node.memory_bytes > 0));
    }
}
