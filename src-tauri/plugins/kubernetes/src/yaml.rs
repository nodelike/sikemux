//! An object's YAML to read, as `kubectl get -o yaml` shows it, less what the pane
//! never shows: env values, ConfigMap data, the copy `kubectl apply` keeps in an
//! annotation, and the field-manager bookkeeping. Secrets are refused outright.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};

const HIDDEN: &str = "(hidden)";
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// The resource kubectl knows a kind by, for the kinds the pane opens.
pub fn resource_of(kind: &str) -> KubeResult<&'static str> {
    match kind.to_lowercase().as_str() {
        "pod" | "pods" | "po" => Ok("pod"),
        "deployment" | "deployments" | "deploy" => Ok("deployment"),
        "statefulset" | "statefulsets" | "sts" => Ok("statefulset"),
        "daemonset" | "daemonsets" | "ds" => Ok("daemonset"),
        "job" | "jobs" => Ok("job"),
        "cronjob" | "cronjobs" | "cj" => Ok("cronjob"),
        "service" | "services" | "svc" => Ok("service"),
        "ingress" | "ingresses" | "ing" => Ok("ingress"),
        "configmap" | "configmaps" | "cm" => Ok("configmap"),
        "node" | "nodes" | "no" => Ok("node"),
        "secret" | "secrets" => Err(KubeError::BadArg(
            "Sikemux doesn't show a Secret's YAML; its key names are on the Config page".into(),
        )),
        other => Err(KubeError::BadArg(format!(
            "Sikemux doesn't open a {other}'s YAML"
        ))),
    }
}

/// Takes out what the pane never shows, wherever in the object it sits.
pub fn redact(value: &mut Value) {
    if let Some(metadata) = value.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("managedFields");
        if let Some(annotations) = metadata
            .get_mut("annotations")
            .and_then(Value::as_object_mut)
        {
            annotations.remove(LAST_APPLIED);
            if annotations.is_empty() {
                metadata.remove("annotations");
            }
        }
    }
    let is_config_map = value.get("kind").and_then(Value::as_str) == Some("ConfigMap");
    if is_config_map {
        for field in ["data", "binaryData"] {
            if let Some(data) = value.get_mut(field).and_then(Value::as_object_mut) {
                for entry in data.values_mut() {
                    *entry = Value::String(HIDDEN.into());
                }
            }
        }
    }
    hide_env(value);
}

/// Every `env` list's literal values, in containers, init containers and templates alike.
fn hide_env(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if key == "env" {
                    for variable in field.as_array_mut().into_iter().flatten() {
                        if let Some(literal) = variable.get_mut("value") {
                            *literal = Value::String(HIDDEN.into());
                        }
                    }
                } else {
                    hide_env(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(hide_env),
        _ => {}
    }
}

/// Whether a string can be written bare in YAML and read back as the same string.
fn plain(text: &str) -> bool {
    const WORDS: [&str; 11] = [
        "true", "false", "yes", "no", "on", "off", "null", "y", "n", "~", "",
    ];
    if WORDS.contains(&text.to_lowercase().as_str()) || text.parse::<f64>().is_ok() {
        return false;
    }
    let first_ok = text
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/'));
    first_ok
        && !text.ends_with(' ')
        && !text.ends_with(':')
        && !text.contains(": ")
        && !text.contains(" #")
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _./-:@+=,()".contains(c))
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) if plain(text) => text.clone(),
        Value::String(text) => serde_json::to_string(text).unwrap_or_default(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn key(text: &str) -> String {
    if plain(text) {
        text.to_string()
    } else {
        serde_json::to_string(text).unwrap_or_default()
    }
}

fn is_block(value: &Value) -> bool {
    match value {
        Value::Object(fields) => !fields.is_empty(),
        Value::Array(items) => !items.is_empty(),
        _ => false,
    }
}

fn inline(value: &Value) -> String {
    match value {
        Value::Object(_) => "{}".into(),
        Value::Array(_) => "[]".into(),
        other => scalar(other),
    }
}

fn write_object(fields: &Map<String, Value>, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for (name, value) in fields {
        if is_block(value) {
            out.push_str(&format!("{pad}{}:\n", key(name)));
            let deeper = if value.is_array() { indent } else { indent + 2 };
            write_value(value, deeper, out);
        } else {
            out.push_str(&format!("{pad}{}: {}\n", key(name), inline(value)));
        }
    }
}

fn write_value(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(fields) => write_object(fields, indent, out),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Object(fields) if !fields.is_empty() => {
                        let mut nested = String::new();
                        write_object(fields, indent + 2, &mut nested);
                        let body = nested.get(indent + 2..).unwrap_or_default();
                        out.push_str(&format!("{pad}- {body}"));
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        out.push_str(&format!("{pad}-\n"));
                        write_value(item, indent + 2, out);
                    }
                    other => out.push_str(&format!("{pad}- {}\n", inline(other))),
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", inline(other))),
    }
}

/// The object as YAML, in the order kubectl keeps its fields.
pub fn to_yaml(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YamlOf {
    #[serde(flatten)]
    pub scope: Scope,
    pub kind: String,
    pub name: String,
}

pub async fn yaml(input: YamlOf) -> KubeResult<String> {
    let resource = resource_of(&input.kind)?;
    let name = kubectl::object_name("name", &input.name)?;
    let scope = if resource == "node" {
        Scope {
            context: input.scope.context,
            namespace: None,
        }
    } else {
        input.scope
    };
    let mut object: Value = kubectl::json(&scope, &["get", resource, &name]).await?;
    redact(&mut object);
    Ok(to_yaml(&object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn env_values_config_data_and_the_applied_copy_are_taken_out() {
        let mut deployment = json!({
            "kind": "Deployment",
            "metadata": { "name": "web", "managedFields": [{ "manager": "kubectl" }],
                          "annotations": { LAST_APPLIED: "{\"spec\":{\"env\":[{\"value\":\"hunter2\"}]}}" } },
            "spec": { "template": { "spec": {
                "initContainers": [{ "name": "migrate", "env": [{ "name": "DB_PASSWORD", "value": "hunter2" }] }],
                "containers": [{ "name": "web", "env": [
                    { "name": "LOG_LEVEL", "value": "debug" },
                    { "name": "DB_URL", "valueFrom": { "secretKeyRef": { "name": "db", "key": "url" } } }
                ] }]
            } } }
        });
        redact(&mut deployment);
        let text = to_yaml(&deployment);
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("debug"));
        assert!(!text.contains("managedFields"));
        assert!(!text.contains("last-applied"));
        assert!(text.contains("name: DB_PASSWORD"));
        assert!(text.contains("secretKeyRef:"));
        let mut config = json!({ "kind": "ConfigMap", "metadata": { "name": "app" }, "data": { "API_URL": "https://internal.acme" } });
        redact(&mut config);
        let text = to_yaml(&config);
        assert!(text.contains("API_URL: \"(hidden)\""));
        assert!(!text.contains("internal.acme"));
    }

    #[test]
    fn secrets_are_refused_and_only_known_kinds_open() {
        assert!(resource_of("Secret").is_err());
        assert!(resource_of("secrets").is_err());
        assert_eq!(resource_of("Deployment").ok(), Some("deployment"));
        assert_eq!(resource_of("svc").ok(), Some("service"));
        assert!(resource_of("ClusterRole").is_err());
    }

    #[test]
    fn yaml_is_written_as_kubectl_writes_it() {
        let object = json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": { "name": "web-1", "labels": { "app.kubernetes.io/name": "web", "tier": "front" }, "annotations": {} },
            "spec": {
                "containers": [
                    { "name": "web", "image": "nginx:1.27", "ports": [{ "containerPort": 80, "protocol": "TCP" }], "args": ["--port", "80"] },
                    { "name": "sidecar", "image": "envoy:v1", "command": [] }
                ],
                "nodeSelector": { "kubernetes.io/os": "linux" },
                "restartPolicy": "Always",
                "terminationGracePeriodSeconds": 30,
                "hostNetwork": false
            },
            "status": { "phase": "Running", "message": "probe failed: dial tcp: connection refused", "reason": null, "count": "10", "flag": "yes" }
        });
        let expected = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: web-1\n  labels:\n    app.kubernetes.io/name: web\n    tier: front\n  annotations: {}\nspec:\n  containers:\n  - name: web\n    image: nginx:1.27\n    ports:\n    - containerPort: 80\n      protocol: TCP\n    args:\n    - \"--port\"\n    - \"80\"\n  - name: sidecar\n    image: envoy:v1\n    command: []\n  nodeSelector:\n    kubernetes.io/os: linux\n  restartPolicy: Always\n  terminationGracePeriodSeconds: 30\n  hostNetwork: false\nstatus:\n  phase: Running\n  message: \"probe failed: dial tcp: connection refused\"\n  reason: null\n  count: \"10\"\n  flag: \"yes\"\n";
        assert_eq!(to_yaml(&object), expected);
    }

    /// Reads the shop namespace's web Deployment on whatever cluster kubectl points at. Reads only.
    #[tokio::test]
    #[ignore = "needs a running cluster"]
    async fn against_the_local_cluster() {
        let text = yaml(YamlOf {
            scope: Scope {
                context: None,
                namespace: Some("shop".into()),
            },
            kind: "Deployment".into(),
            name: "web".into(),
        })
        .await
        .expect("yaml");
        println!("{text}");
        assert!(text.starts_with("apiVersion: apps/v1\nkind: Deployment\n"));
        assert!(!text.contains("last-applied-configuration"));
        assert!(!text.contains("managedFields"));
    }
}
