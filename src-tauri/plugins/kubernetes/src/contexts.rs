//! The clusters in the person's kubeconfig, which one kubectl uses now, and each
//! cluster's namespaces. Only names and addresses leave here, never credentials.

use serde::Serialize;
use serde_json::Value;

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub name: String,
    pub cluster: String,
    /// Where the cluster's API answers, such as https://127.0.0.1:6443.
    pub server: Option<String>,
    /// The namespace kubectl uses in this context when none is named.
    pub namespace: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contexts {
    pub current: Option<String>,
    pub contexts: Vec<Context>,
}

fn text(value: &Value, path: &str) -> Option<String> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The kubeconfig's contexts, read from `kubectl config view`, leaving its users out.
pub fn contexts_of(config: &Value) -> Contexts {
    let servers: Vec<(String, String)> = config
        .pointer("/clusters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|cluster| Some((text(cluster, "/name")?, text(cluster, "/cluster/server")?)))
        .collect();
    let mut contexts: Vec<Context> = config
        .pointer("/contexts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|context| {
            let cluster = text(context, "/context/cluster").unwrap_or_default();
            Some(Context {
                name: text(context, "/name")?,
                server: servers
                    .iter()
                    .find(|(name, _)| *name == cluster)
                    .map(|(_, server)| server.clone()),
                namespace: text(context, "/context/namespace"),
                cluster,
            })
        })
        .collect();
    contexts.sort_by(|a, b| a.name.cmp(&b.name));
    Contexts {
        current: text(config, "/current-context"),
        contexts,
    }
}

pub async fn contexts() -> KubeResult<Contexts> {
    let config: Value = kubectl::json(&Scope::default(), &["config", "view"]).await?;
    Ok(contexts_of(&config))
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Namespace {
    pub name: String,
    /// Active, or Terminating while it is being deleted.
    pub phase: String,
}

pub fn namespaces_of(list: &Value) -> Vec<Namespace> {
    let mut found: Vec<Namespace> = list
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(Namespace {
                name: text(item, "/metadata/name")?,
                phase: text(item, "/status/phase").unwrap_or_else(|| "Active".into()),
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

pub async fn namespaces(scope: Scope) -> KubeResult<Vec<Namespace>> {
    let list: Value = kubectl::json(
        &Scope {
            namespace: None,
            ..scope
        },
        &["get", "namespaces"],
    )
    .await?;
    Ok(namespaces_of(&list))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// kubectl is installed and the kubeconfig names at least one context.
    pub ready: bool,
    pub message: Option<String>,
    pub category: Option<&'static str>,
    pub current: Option<String>,
    pub contexts: Vec<Context>,
}

/// Whether kubectl and a kubeconfig are here, without reaching any cluster.
pub async fn status() -> Status {
    match contexts().await {
        Ok(found) if found.contexts.is_empty() => Status {
            ready: false,
            message: Some(
                "Your kubeconfig has no clusters yet. Add one with your cloud's CLI, or start one locally with kind or minikube."
                    .into(),
            ),
            category: Some("unconfigured"),
            current: None,
            contexts: Vec::new(),
        },
        Ok(found) => Status {
            ready: true,
            message: None,
            category: None,
            current: found.current,
            contexts: found.contexts,
        },
        Err(error) => Status {
            ready: false,
            category: Some(match error {
                KubeError::CliMissing => "cli-missing",
                _ => "unconfigured",
            }),
            message: Some(error.to_string()),
            current: None,
            contexts: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contexts_read_with_their_cluster_address_and_never_their_users() {
        let config = json!({
            "current-context": "kind-sikemux",
            "clusters": [
                { "name": "kind-sikemux", "cluster": { "server": "https://127.0.0.1:6443", "certificate-authority-data": "DATA+OMITTED" } },
                { "name": "gke_acme_prod", "cluster": { "server": "https://34.1.2.3" } }
            ],
            "contexts": [
                { "name": "kind-sikemux", "context": { "cluster": "kind-sikemux", "user": "kind-sikemux" } },
                { "name": "gke_acme_prod", "context": { "cluster": "gke_acme_prod", "user": "gke_acme_prod", "namespace": "shop" } }
            ],
            "users": [{ "name": "kind-sikemux", "user": { "token": "s3cr3t" } }]
        });
        let read = contexts_of(&config);
        assert_eq!(read.current.as_deref(), Some("kind-sikemux"));
        assert_eq!(read.contexts[0].name, "gke_acme_prod");
        assert_eq!(read.contexts[0].namespace.as_deref(), Some("shop"));
        assert_eq!(
            read.contexts[1].server.as_deref(),
            Some("https://127.0.0.1:6443")
        );
        assert!(!serde_json::to_string(&read).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn namespaces_read_sorted_with_their_phase() {
        let list = json!({ "items": [
            { "metadata": { "name": "shop" }, "status": { "phase": "Active" } },
            { "metadata": { "name": "default" } },
            { "metadata": { "name": "old" }, "status": { "phase": "Terminating" } }
        ] });
        let read = namespaces_of(&list);
        assert_eq!(
            read.iter().map(|ns| ns.name.as_str()).collect::<Vec<_>>(),
            ["default", "old", "shop"]
        );
        assert_eq!(read[0].phase, "Active");
        assert_eq!(read[1].phase, "Terminating");
    }
}
