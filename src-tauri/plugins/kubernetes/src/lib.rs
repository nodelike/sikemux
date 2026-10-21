// Kubernetes: the clusters in the person's kubeconfig, their workloads, pods,
// logs and events, in a pane and as agent tools. It runs the person's own kubectl,
// so every way their clusters sign in keeps working.
//
//   actions  — restarting and scaling workloads, replacing a pod, running or pausing a CronJob
//   kubectl  — running kubectl in a context and namespace, and its errors in words
//   contexts — the kubeconfig's clusters, the current one, and each one's namespaces
//   pods     — pods with their status as kubectl words it, restarts and owning workload
//   logs     — a container's logs, read once, from before its last restart, or followed
//   workloads — Deployments, StatefulSets, DaemonSets, Jobs and CronJobs, and what is wrong with them
//   events   — a namespace's recent events, warnings first in mind
//   network  — Services with their ports and selectors, and Ingresses with their routes
//   config   — ConfigMaps and Secrets by key name and size; no value is ever read
//   describe — one pod's containers, last crash, conditions, volumes and events; env by name only

mod actions;
mod config;
mod contexts;
mod describe;
mod error;
mod events;
mod kubectl;
mod logs;
mod network;
mod pods;
mod workloads;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::KubeResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Kubernetes {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Kubernetes {
    manifest: Manifest,
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = KubeResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Kubernetes {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            match method {
                "status" => reply(contexts::status().await),
                "contexts" => answer(contexts::contexts()).await,
                "namespaces" => answer(contexts::namespaces(params(input)?)).await,
                "pods" => answer(pods::list(params(input)?)).await,
                "pod" => answer(describe::describe(params(input)?)).await,
                "logs" => answer(logs::read(params(input)?)).await,
                "workloads" => answer(workloads::list(params(input)?)).await,
                "events" => answer(events::list(params(input)?)).await,
                "network" => answer(network::list(params(input)?)).await,
                "config" => answer(config::list(params(input)?)).await,
                "act" => answer(actions::act(params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Kubernetes isn't tied to a repository, so its tools are offered wherever kubectl has a cluster to talk to.
    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async { Ok(contexts::status().await.ready) })
    }

    fn stream<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            match method {
                "followLogs" => logs::follow(params(input)?, &sink).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the clusters in this Mac's kubeconfig: `cargo test -p sikemux-plugin-kubernetes -- --ignored`.
    #[tokio::test]
    #[ignore = "needs kubectl and a reachable cluster"]
    async fn against_the_local_cluster() {
        let plugin = plugin().expect("plugin loads");
        let ctx = PluginContext::new(std::env::temp_dir());
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        println!("{status:#}");
        assert_eq!(status["ready"], true);
        let namespaces = plugin
            .call(&ctx, "namespaces", serde_json::json!({}))
            .await
            .expect("namespaces");
        println!("{namespaces:#}");
        let workloads = plugin
            .call(
                &ctx,
                "workloads",
                serde_json::json!({ "namespace": "shop" }),
            )
            .await
            .expect("workloads");
        let events = plugin
            .call(
                &ctx,
                "events",
                serde_json::json!({ "namespace": "shop", "warningsOnly": true, "limit": 3 }),
            )
            .await
            .expect("events");
        let network = plugin
            .call(&ctx, "network", serde_json::json!({ "namespace": "shop" }))
            .await
            .expect("network");
        println!("network {network}");
        let config = plugin
            .call(&ctx, "config", serde_json::json!({ "namespace": "shop" }))
            .await
            .expect("config");
        println!("config {config}");
        for each in events.as_array().into_iter().flatten() {
            println!(
                "event {} {} x{} {}",
                each["object"], each["reason"], each["count"], each["message"]
            );
        }
        for each in workloads.as_array().into_iter().flatten() {
            println!(
                "workload {} {} {} {}/{} {}",
                each["kind"],
                each["name"],
                each["health"],
                each["ready"],
                each["desired"],
                each["note"]
            );
        }
        let pods = plugin
            .call(&ctx, "pods", serde_json::json!({ "allNamespaces": true }))
            .await
            .expect("pods");
        for pod in pods.as_array().into_iter().flatten() {
            println!(
                "{} {} {} {}/{} restarts {} {}",
                pod["namespace"],
                pod["name"],
                pod["status"],
                pod["ready"],
                pod["containers"],
                pod["restarts"],
                pod["owner"]
            );
        }
        if let Some(first) = pods.get(0) {
            let detail = plugin
                .call(
                    &ctx,
                    "pod",
                    serde_json::json!({ "namespace": first["namespace"], "name": first["name"] }),
                )
                .await
                .expect("pod");
            println!("{detail:#}");
            let logs = plugin
                .call(
                    &ctx,
                    "logs",
                    serde_json::json!({ "namespace": first["namespace"], "pod": first["name"], "previous": true, "tail": 5 }),
                )
                .await
                .expect("logs");
            println!("previous logs: {logs:#}");
            let (tell, mut heard) = tokio::sync::mpsc::unbounded_channel();
            let sink = sikemux_plugin_api::StreamSink::new(move |item| tell.send(item).is_ok());
            let follow = plugin.stream(
                &ctx,
                "followLogs",
                serde_json::json!({ "namespace": "shop", "pod": "web-56fd9f5c8b-fgprx", "tail": 3 }),
                sink,
            );
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), follow).await;
            while let Ok(batch) = heard.try_recv() {
                println!("followed: {batch}");
            }
        }
    }

    #[test]
    fn its_tools_call_methods_it_answers() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.kubernetes");
        let answered = [
            "contexts",
            "pods",
            "pod",
            "logs",
            "workloads",
            "events",
            "network",
            "config",
            "act",
        ];
        let names: Vec<&str> = plugin
            .manifest()
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "k8s_contexts",
                "k8s_pods",
                "k8s_pod",
                "k8s_logs",
                "k8s_workloads",
                "k8s_events",
                "k8s_network",
                "k8s_config",
                "k8s_act"
            ]
        );
        for tool in &plugin.manifest().tools {
            assert!(
                answered.contains(&tool.method.as_str()),
                "{} calls {}",
                tool.name,
                tool.method
            );
        }
    }
}
