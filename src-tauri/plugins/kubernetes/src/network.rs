//! How a namespace is reached: its Services with their ports and the pods they pick,
//! and its Ingresses with the hosts and paths that lead to each Service.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KubeResult;
use crate::kubectl::{self, Scope};
use crate::pods::text;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub namespace: String,
    /// ClusterIP, NodePort, LoadBalancer or ExternalName.
    pub kind: String,
    pub cluster_ip: Option<String>,
    /// Where it answers from outside the cluster, for a load balancer.
    pub external: Vec<String>,
    /// Such as 80→8080/TCP, with the node port when it has one.
    pub ports: Vec<String>,
    /// The label selector that picks its pods, such as app=web.
    pub selector: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ingress {
    pub name: String,
    pub namespace: String,
    pub class: Option<String>,
    pub rules: Vec<Route>,
    /// Hosts served over HTTPS.
    pub tls: Vec<String>,
    pub address: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub host: String,
    pub path: String,
    pub service: String,
    pub port: String,
}

fn items(list: &Value) -> Vec<&Value> {
    list.pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn addresses(value: &Value) -> Vec<String> {
    value
        .pointer("/status/loadBalancer/ingress")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| text(entry, "/hostname").or_else(|| text(entry, "/ip")))
        .collect()
}

fn port_of(port: &Value) -> Option<String> {
    let number = port.pointer("/port").and_then(Value::as_u64)?;
    let target = port
        .pointer("/targetPort")
        .map(|target| {
            target
                .as_str()
                .map_or_else(|| target.to_string(), str::to_string)
        })
        .filter(|target| target != "null" && *target != number.to_string());
    let protocol = text(port, "/protocol").unwrap_or_else(|| "TCP".into());
    let node = port.pointer("/nodePort").and_then(Value::as_u64);
    let mut label = number.to_string();
    if let Some(target) = target {
        label.push_str(&format!("→{target}"));
    }
    label.push_str(&format!("/{protocol}"));
    if let Some(node) = node {
        label.push_str(&format!(" (node {node})"));
    }
    Some(label)
}

pub fn service_of(item: &Value) -> Option<Service> {
    Some(Service {
        name: text(item, "/metadata/name")?,
        namespace: text(item, "/metadata/namespace").unwrap_or_default(),
        kind: text(item, "/spec/type").unwrap_or_else(|| "ClusterIP".into()),
        cluster_ip: text(item, "/spec/clusterIP").filter(|ip| ip != "None"),
        external: {
            let mut external = addresses(item);
            if let Some(name) = text(item, "/spec/externalName") {
                external.push(name);
            }
            external
        },
        ports: item
            .pointer("/spec/ports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(port_of)
            .collect(),
        selector: item
            .pointer("/spec/selector")
            .and_then(Value::as_object)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
                    .collect::<Vec<_>>()
                    .join(",")
            }),
    })
}

pub fn ingress_of(item: &Value) -> Option<Ingress> {
    let mut rules = Vec::new();
    for rule in item
        .pointer("/spec/rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let host = text(rule, "/host").unwrap_or_else(|| "*".into());
        for path in rule
            .pointer("/http/paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let backend = path.pointer("/backend/service").unwrap_or(&Value::Null);
            rules.push(Route {
                host: host.clone(),
                path: text(path, "/path").unwrap_or_else(|| "/".into()),
                service: text(backend, "/name").unwrap_or_default(),
                port: text(backend, "/port/name")
                    .or_else(|| {
                        backend
                            .pointer("/port/number")
                            .and_then(Value::as_u64)
                            .map(|port| port.to_string())
                    })
                    .unwrap_or_default(),
            });
        }
    }
    Some(Ingress {
        name: text(item, "/metadata/name")?,
        namespace: text(item, "/metadata/namespace").unwrap_or_default(),
        class: text(item, "/spec/ingressClassName"),
        tls: item
            .pointer("/spec/tls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|tls| {
                tls.pointer("/hosts")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|host| host.as_str().map(str::to_string))
            .collect(),
        address: addresses(item),
        rules,
    })
}

#[derive(Debug, Serialize)]
pub struct Network {
    pub services: Vec<Service>,
    pub ingresses: Vec<Ingress>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListNetwork {
    #[serde(flatten)]
    pub scope: Scope,
    pub all_namespaces: bool,
}

pub async fn list(input: ListNetwork) -> KubeResult<Network> {
    let mut args = vec!["get", "services,ingresses"];
    if input.all_namespaces {
        args.push("--all-namespaces");
    }
    let list: Value = kubectl::json(&input.scope, &args).await?;
    let kind_of = |item: &Value| text(item, "/kind").unwrap_or_default();
    let mut services: Vec<Service> = items(&list)
        .into_iter()
        .filter(|item| kind_of(item) == "Service")
        .filter_map(service_of)
        .collect();
    services.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    let ingresses = items(&list)
        .into_iter()
        .filter(|item| kind_of(item) == "Ingress")
        .filter_map(ingress_of)
        .collect();
    Ok(Network {
        services,
        ingresses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn services_read_with_their_ports_and_the_pods_they_pick() {
        let web = service_of(&json!({ "metadata": { "name": "web", "namespace": "shop" },
            "spec": { "type": "ClusterIP", "clusterIP": "10.96.4.2", "selector": { "app": "web" },
                      "ports": [{ "port": 80, "targetPort": 8080, "protocol": "TCP" }, { "port": 443, "targetPort": 443 }] } }))
        .unwrap();
        assert_eq!(web.ports, ["80→8080/TCP", "443/TCP"]);
        assert_eq!(web.selector.as_deref(), Some("app=web"));
        let lb = service_of(&json!({ "metadata": { "name": "edge" },
            "spec": { "type": "LoadBalancer", "clusterIP": "None", "ports": [{ "port": 80, "targetPort": "http", "nodePort": 30080 }] },
            "status": { "loadBalancer": { "ingress": [{ "hostname": "a1b2.elb.amazonaws.com" }] } } }))
        .unwrap();
        assert_eq!(lb.cluster_ip, None);
        assert_eq!(lb.external, ["a1b2.elb.amazonaws.com"]);
        assert_eq!(lb.ports, ["80→http/TCP (node 30080)"]);
    }

    #[test]
    fn ingresses_read_which_host_and_path_reach_which_service() {
        let ingress = ingress_of(&json!({ "metadata": { "name": "shop", "namespace": "shop" },
            "spec": { "ingressClassName": "nginx", "tls": [{ "hosts": ["shop.acme.dev"] }],
                      "rules": [{ "host": "shop.acme.dev", "http": { "paths": [
                          { "path": "/", "backend": { "service": { "name": "web", "port": { "number": 80 } } } },
                          { "path": "/api", "backend": { "service": { "name": "api", "port": { "name": "http" } } } } ] } }] } }))
        .unwrap();
        assert_eq!(ingress.rules.len(), 2);
        assert_eq!(
            ingress.rules[1],
            Route {
                host: "shop.acme.dev".into(),
                path: "/api".into(),
                service: "api".into(),
                port: "http".into()
            }
        );
        assert_eq!(ingress.tls, ["shop.acme.dev"]);
        assert_eq!(ingress.class.as_deref(), Some("nginx"));
    }
}
