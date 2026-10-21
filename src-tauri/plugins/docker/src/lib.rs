// Docker: the engine running on this Mac, its containers, Compose stacks,
// images and volumes, in a pane and as agent tools.
//
//   engine — finding the engine's socket, and HTTP over it
//   status     — whether an engine is running, which one, and how much it holds
//   containers — listing and reading containers, starting, stopping and removing them
//   compose    — Compose stacks from their containers' labels, and acting on a whole stack
//   logs       — a container's logs, read once or followed as they are written
//   stats      — CPU, memory, network and disk of each running container
//   disk       — how much disk the engine uses, and how much could be freed
//   images     — images, the containers made from each, removing and pruning them
//   volumes    — volumes, their sizes and the containers that mount them, removing and pruning them

mod compose;
mod containers;
mod disk;
mod engine;
mod error;
mod images;
mod logs;
mod stats;
mod status;
mod volumes;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::engine::Engine;
use crate::error::DockerResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Docker {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Docker {
    manifest: Manifest,
}

async fn answer<T: Serialize, F>(work: impl FnOnce(Engine) -> F) -> Result<Value, PluginError>
where
    F: std::future::Future<Output = DockerResult<T>>,
{
    let engine = Engine::find()?;
    reply(work(engine).await?)
}

impl Plugin for Docker {
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
                "status" => reply(status::status().await),
                "containers" => {
                    let params = params(input)?;
                    answer(|engine| async move { containers::list(&engine, params).await }).await
                }
                "container" => {
                    let target = params(input)?;
                    answer(|engine| async move { containers::inspect(&engine, target).await }).await
                }
                "act" => {
                    let params = params(input)?;
                    answer(|engine| async move { containers::act(&engine, params).await }).await
                }
                "remove" => {
                    let params = params(input)?;
                    answer(|engine| async move { containers::remove(&engine, params).await }).await
                }
                "logs" => {
                    let params = params(input)?;
                    answer(|engine| async move { logs::read(&engine, params).await }).await
                }
                "images" => {
                    let params = params(input)?;
                    answer(|engine| async move { images::list(&engine, params).await }).await
                }
                "removeImage" => {
                    let params = params(input)?;
                    answer(|engine| async move { images::remove(&engine, params).await }).await
                }
                "pruneImages" => answer(|engine| async move { images::prune(&engine).await }).await,
                "stats" => {
                    let params = params(input)?;
                    answer(|engine| async move { stats::read(&engine, params).await }).await
                }
                "stacks" => answer(|engine| async move { compose::list(&engine).await }).await,
                "actOnStack" => {
                    let params = params(input)?;
                    answer(|engine| async move { compose::act(&engine, params).await }).await
                }
                "disk" => answer(|engine| async move { disk::read(&engine).await }).await,
                "volumes" => answer(|engine| async move { volumes::list(&engine).await }).await,
                "removeVolume" => {
                    let params = params(input)?;
                    answer(|engine| async move { volumes::remove(&engine, params).await }).await
                }
                "pruneVolumes" => {
                    answer(|engine| async move { volumes::prune(&engine).await }).await
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Docker isn't tied to a repository, so its tools are offered wherever an engine is installed.
    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async { Ok(Engine::find().is_ok()) })
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
                "followLogs" => logs::follow(&Engine::find()?, params(input)?, &sink).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the engine running on this Mac: `cargo test -p sikemux-plugin-docker -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a running Docker engine"]
    async fn against_the_local_engine() {
        let plugin = plugin().expect("plugin loads");
        let ctx = PluginContext::new(std::env::temp_dir());
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        println!("{status:#}");
        assert_eq!(status["running"], true);
        let containers = plugin
            .call(&ctx, "containers", serde_json::json!({}))
            .await
            .expect("containers");
        println!("{containers:#}");
        if let Some(first) = containers.get(0) {
            let detail = plugin
                .call(&ctx, "container", serde_json::json!({ "id": first["id"] }))
                .await
                .expect("container");
            println!("{detail:#}");
            let logs = plugin
                .call(
                    &ctx,
                    "logs",
                    serde_json::json!({ "id": first["id"], "tail": 5 }),
                )
                .await
                .expect("logs");
            println!("{logs:#}");
        }
        let volumes = plugin
            .call(&ctx, "volumes", Value::Null)
            .await
            .expect("volumes");
        println!("volumes: {}", volumes.as_array().map_or(0, Vec::len));
        let stats = plugin
            .call(&ctx, "stats", serde_json::json!({}))
            .await
            .expect("stats");
        println!("stats: {stats:#}");
    }

    #[test]
    fn its_manifest_parses_with_the_nine_tools() {
        let plugin = plugin().expect("manifest parses");
        let names: Vec<&str> = plugin
            .manifest()
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "docker_containers",
                "docker_container",
                "docker_logs",
                "docker_stats",
                "docker_act",
                "docker_stacks",
                "docker_stack_act",
                "docker_images",
                "docker_disk"
            ]
        );
    }

    #[test]
    fn its_tools_call_methods_it_answers() {
        let plugin = plugin().expect("manifest parses");
        let answered = [
            "containers",
            "container",
            "logs",
            "stats",
            "act",
            "stacks",
            "actOnStack",
            "images",
            "disk",
        ];
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
