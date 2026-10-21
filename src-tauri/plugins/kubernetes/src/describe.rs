//! One pod as `kubectl describe` tells it: each container's state and last crash,
//! its conditions, volumes and recent events. Environment variables come by name,
//! with where each is set from, never their values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KubeResult;
use crate::kubectl::{self, Scope};
use crate::pods::{pod_of, text, Pod};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(flatten)]
    pub pod: Pod,
    pub labels: Vec<(String, String)>,
    pub conditions: Vec<Condition>,
    /// Each container's state, kept apart from the pod's own `containers`, which counts them.
    pub container_states: Vec<Container>,
    pub volumes: Vec<Volume>,
    pub events: Vec<Event>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub kind: String,
    pub ok: bool,
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub name: String,
    pub image: String,
    pub init: bool,
    pub ready: bool,
    pub restarts: u32,
    /// running, waiting or terminated.
    pub state: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub exit_code: Option<i64>,
    pub since: Option<String>,
    /// How it ended the last time it stopped, which explains a crash loop.
    pub last: Option<Ended>,
    pub ports: Vec<String>,
    pub env: Vec<EnvName>,
    /// Whole ConfigMaps and Secrets read into its environment, by name.
    pub env_from: Vec<String>,
    pub requests: Vec<(String, String)>,
    pub limits: Vec<(String, String)>,
    pub probes: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    pub reason: Option<String>,
    pub exit_code: Option<i64>,
    pub finished: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvName {
    pub name: String,
    /// Where its value comes from: written in the spec, or a Secret's, ConfigMap's or field's.
    pub from: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub kind: String,
    pub source: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Normal or Warning.
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub count: u32,
    /// When it last happened, RFC 3339.
    pub last: Option<String>,
    /// What it happened to, such as Pod/web-1.
    pub object: String,
    pub namespace: Option<String>,
}

fn items<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    value
        .pointer(path)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn pairs(value: &Value, path: &str) -> Vec<(String, String)> {
    value
        .pointer(path)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn env_names(spec: &Value) -> Vec<EnvName> {
    items(spec, "/env")
        .into_iter()
        .filter_map(|variable| {
            let name = text(variable, "/name")?;
            let from = if let Some(secret) = text(variable, "/valueFrom/secretKeyRef/name") {
                format!(
                    "Secret {secret}/{}",
                    text(variable, "/valueFrom/secretKeyRef/key").unwrap_or_default()
                )
            } else if let Some(map) = text(variable, "/valueFrom/configMapKeyRef/name") {
                format!(
                    "ConfigMap {map}/{}",
                    text(variable, "/valueFrom/configMapKeyRef/key").unwrap_or_default()
                )
            } else if let Some(field) = text(variable, "/valueFrom/fieldRef/fieldPath") {
                format!("field {field}")
            } else if variable.pointer("/valueFrom/resourceFieldRef").is_some() {
                "resource".into()
            } else {
                "written in the spec".into()
            };
            Some(EnvName { name, from })
        })
        .collect()
}

fn probes(spec: &Value) -> Vec<String> {
    ["livenessProbe", "readinessProbe", "startupProbe"]
        .iter()
        .filter_map(|kind| {
            let probe = spec.pointer(&format!("/{kind}"))?;
            let how = if let Some(path) = text(probe, "/httpGet/path") {
                format!("GET {path}")
            } else if probe.pointer("/tcpSocket").is_some() {
                "TCP".into()
            } else if probe.pointer("/exec").is_some() {
                "command".into()
            } else {
                "check".into()
            };
            Some(format!("{} {how}", kind.trim_end_matches("Probe")))
        })
        .collect()
}

fn container_of(spec: &Value, status: Option<&Value>, init: bool) -> Option<Container> {
    let name = text(spec, "/name")?;
    let state = status
        .and_then(|status| status.pointer("/state"))
        .and_then(Value::as_object);
    let (state_name, detail) = state
        .and_then(|state| state.iter().next())
        .map_or(("waiting".to_string(), &Value::Null), |(name, detail)| {
            (name.clone(), detail)
        });
    let last = status
        .and_then(|status| status.pointer("/lastState/terminated"))
        .map(|ended| Ended {
            reason: text(ended, "/reason"),
            exit_code: ended.pointer("/exitCode").and_then(Value::as_i64),
            finished: text(ended, "/finishedAt"),
        });
    Some(Container {
        image: text(spec, "/image").unwrap_or_default(),
        init,
        ready: status
            .and_then(|status| status.pointer("/ready"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        restarts: status
            .and_then(|status| status.pointer("/restartCount"))
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or(0),
        reason: text(detail, "/reason"),
        message: text(detail, "/message"),
        exit_code: detail.pointer("/exitCode").and_then(Value::as_i64),
        since: text(detail, "/startedAt").or_else(|| text(detail, "/finishedAt")),
        state: state_name,
        last,
        ports: items(spec, "/ports")
            .into_iter()
            .filter_map(|port| {
                let number = port.pointer("/containerPort").and_then(Value::as_u64)?;
                Some(format!(
                    "{number}/{}",
                    text(port, "/protocol").unwrap_or_else(|| "TCP".into())
                ))
            })
            .collect(),
        env: env_names(spec),
        env_from: items(spec, "/envFrom")
            .into_iter()
            .filter_map(|source| {
                text(source, "/secretRef/name")
                    .map(|name| format!("Secret {name}"))
                    .or_else(|| {
                        text(source, "/configMapRef/name").map(|name| format!("ConfigMap {name}"))
                    })
            })
            .collect(),
        requests: pairs(spec, "/resources/requests"),
        limits: pairs(spec, "/resources/limits"),
        probes: probes(spec),
        name,
    })
}

fn volume_of(volume: &Value) -> Option<Volume> {
    let name = text(volume, "/name")?;
    let (kind, detail) = volume
        .as_object()?
        .iter()
        .find(|(key, _)| key.as_str() != "name")
        .map(|(key, detail)| (key.clone(), detail))?;
    let source = text(detail, "/claimName")
        .or_else(|| text(detail, "/secretName"))
        .or_else(|| text(detail, "/name"))
        .or_else(|| text(detail, "/path"));
    Some(Volume { name, kind, source })
}

pub fn event_of(event: &Value) -> Option<Event> {
    Some(Event {
        kind: text(event, "/type").unwrap_or_else(|| "Normal".into()),
        reason: text(event, "/reason").unwrap_or_default(),
        message: text(event, "/message")
            .unwrap_or_default()
            .trim()
            .to_string(),
        count: event
            .pointer("/count")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or(1),
        last: text(event, "/lastTimestamp")
            .or_else(|| text(event, "/eventTime"))
            .or_else(|| text(event, "/firstTimestamp"))
            .or_else(|| text(event, "/metadata/creationTimestamp")),
        object: format!(
            "{}/{}",
            text(event, "/involvedObject/kind").unwrap_or_default(),
            text(event, "/involvedObject/name").unwrap_or_default()
        ),
        namespace: text(event, "/involvedObject/namespace")
            .or_else(|| text(event, "/metadata/namespace")),
    })
}

/// Newest first.
pub fn events_of(list: &Value) -> Vec<Event> {
    let mut events: Vec<Event> = items(list, "/items")
        .into_iter()
        .filter_map(event_of)
        .collect();
    events.sort_by(|a, b| b.last.cmp(&a.last));
    events
}

pub fn detail_of(pod: &Value, events: &Value) -> Option<Detail> {
    let summary = pod_of(pod)?;
    let statuses = |path: &str| items(pod, path);
    let find = |path: &str, name: &str| {
        statuses(path)
            .into_iter()
            .find(|status| text(status, "/name").as_deref() == Some(name))
    };
    let mut containers: Vec<Container> = items(pod, "/spec/initContainers")
        .into_iter()
        .filter_map(|spec| {
            container_of(
                spec,
                find("/status/initContainerStatuses", &text(spec, "/name")?),
                true,
            )
        })
        .collect();
    containers.extend(
        items(pod, "/spec/containers")
            .into_iter()
            .filter_map(|spec| {
                container_of(
                    spec,
                    find("/status/containerStatuses", &text(spec, "/name")?),
                    false,
                )
            }),
    );
    Some(Detail {
        labels: pairs(pod, "/metadata/labels"),
        conditions: items(pod, "/status/conditions")
            .into_iter()
            .filter_map(|condition| {
                Some(Condition {
                    kind: text(condition, "/type")?,
                    ok: text(condition, "/status").as_deref() == Some("True"),
                    reason: text(condition, "/reason"),
                    message: text(condition, "/message"),
                })
            })
            .collect(),
        container_states: containers,
        volumes: items(pod, "/spec/volumes")
            .into_iter()
            .filter_map(volume_of)
            .filter(|volume| !volume.name.starts_with("kube-api-access"))
            .collect(),
        events: events_of(events),
        pod: summary,
    })
}

#[derive(Deserialize)]
pub struct Describe {
    #[serde(flatten)]
    pub scope: Scope,
    pub name: String,
}

pub async fn describe(input: Describe) -> KubeResult<Option<Detail>> {
    let name = kubectl::object_name("pod", &input.name)?;
    let selector = format!("--field-selector=involvedObject.kind=Pod,involvedObject.name={name}");
    let pod_args = ["get", "pod", name.as_str()];
    let event_args = ["get", "events", selector.as_str()];
    let (pod, events) = tokio::join!(
        kubectl::json::<Value>(&input.scope, &pod_args),
        kubectl::json::<Value>(&input.scope, &event_args),
    );
    Ok(detail_of(&pod?, &events.unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pods::tests::crashing;
    use serde_json::json;

    fn crashing_with_spec() -> Value {
        let mut pod = crashing();
        pod["spec"]["containers"] = json!([{
            "name": "worker", "image": "busybox:1.36",
            "ports": [{ "containerPort": 8080 }],
            "env": [
                { "name": "DB_PASSWORD", "valueFrom": { "secretKeyRef": { "name": "db", "key": "password" } } },
                { "name": "API_TOKEN", "value": "tok_live_123" },
                { "name": "POD_IP", "valueFrom": { "fieldRef": { "fieldPath": "status.podIP" } } }
            ],
            "envFrom": [{ "configMapRef": { "name": "web-config" } }, { "secretRef": { "name": "web-secrets" } }],
            "resources": { "limits": { "memory": "128Mi" }, "requests": { "cpu": "50m" } },
            "readinessProbe": { "httpGet": { "path": "/healthz", "port": 8080 } }
        }]);
        pod["spec"]["volumes"] = json!([
            { "name": "data", "persistentVolumeClaim": { "claimName": "worker-data" } },
            { "name": "kube-api-access-x", "projected": {} }
        ]);
        pod["status"]["conditions"] =
            json!([{ "type": "Ready", "status": "False", "reason": "ContainersNotReady" }]);
        pod
    }

    #[test]
    fn a_pod_describes_its_crash_and_never_an_env_value() {
        let events = json!({ "items": [
            { "type": "Normal", "reason": "Pulled", "message": "Container image already present", "count": 5, "lastTimestamp": "2026-10-19T08:01:00Z", "involvedObject": { "kind": "Pod", "name": "worker" } },
            { "type": "Warning", "reason": "BackOff", "message": "Back-off restarting failed container worker", "count": 12, "lastTimestamp": "2026-10-19T08:05:00Z", "involvedObject": { "kind": "Pod", "name": "worker" } }
        ] });
        let detail = detail_of(&crashing_with_spec(), &events).unwrap();
        let worker = &detail.container_states[0];
        assert_eq!(
            (worker.state.as_str(), worker.reason.as_deref()),
            ("waiting", Some("CrashLoopBackOff"))
        );
        assert_eq!(
            worker.last,
            Some(Ended {
                reason: Some("Error".into()),
                exit_code: Some(1),
                finished: None
            })
        );
        assert_eq!(
            worker.env,
            [
                EnvName {
                    name: "DB_PASSWORD".into(),
                    from: "Secret db/password".into()
                },
                EnvName {
                    name: "API_TOKEN".into(),
                    from: "written in the spec".into()
                },
                EnvName {
                    name: "POD_IP".into(),
                    from: "field status.podIP".into()
                },
            ]
        );
        assert_eq!(
            worker.env_from,
            ["ConfigMap web-config", "Secret web-secrets"]
        );
        assert_eq!(worker.limits, [("memory".to_string(), "128Mi".to_string())]);
        assert_eq!(worker.probes, ["readiness GET /healthz"]);
        assert_eq!(worker.ports, ["8080/TCP"]);
        let written = serde_json::to_value(&detail).unwrap();
        assert!(!written.to_string().contains("tok_live_123"));
        assert_eq!(written["containers"], 1);
        assert_eq!(written["containerStates"][0]["name"], "worker");
        assert_eq!(
            detail.volumes,
            [Volume {
                name: "data".into(),
                kind: "persistentVolumeClaim".into(),
                source: Some("worker-data".into())
            }]
        );
        assert_eq!(detail.events[0].reason, "BackOff");
        assert_eq!(detail.events[0].count, 12);
        assert!(!detail.conditions[0].ok);
    }
}
