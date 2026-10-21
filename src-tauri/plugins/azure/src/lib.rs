// Azure: a subscription's resources, its virtual machines and apps with what they
// are doing, its storage and vaults, and what has happened lately.
//
//   cli       — the Azure CLI, which holds the sign-in, and its words for a failure
//   auth      — who is signed in, the subscriptions, and the access tokens calls use
//   client    — the Resource Manager, its pages, and Azure's errors in words
//   resources — resource groups, and everything in the subscription through Resource Graph
//   compute   — virtual machines, and starting, stopping and restarting one
//   web       — App Service and Functions apps, their settings by name, and their log stream
//   storage   — storage accounts with their containers, and key vaults by name
//   monitor   — the activity log, and the numbers one resource reports

mod auth;
mod cli;
mod client;
mod compute;
mod error;
mod monitor;
mod resources;
mod storage;
mod web;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::AzureResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Azure {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Azure {
    manifest: Manifest,
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = AzureResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Azure {
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
                "status" => reply(auth::status().await),
                "signIn" => answer(auth::sign_in(params(input)?)).await,
                "useSubscription" => answer(auth::use_subscription(params(input)?)).await,
                "groups" => answer(resources::groups(params(input)?)).await,
                "resources" => answer(resources::resources(params(input)?)).await,
                "vms" => answer(compute::list(params(input)?)).await,
                "vm" => answer(compute::machine(params(input)?)).await,
                "vmAct" => answer(compute::act(params(input)?)).await,
                "apps" => answer(web::list(params(input)?)).await,
                "app" => answer(web::app(params(input)?)).await,
                "appAct" => answer(web::act(params(input)?)).await,
                "storage" => answer(storage::accounts(params(input)?)).await,
                "containers" => answer(storage::containers(params(input)?)).await,
                "vaults" => answer(storage::vaults(params(input)?)).await,
                "activity" => answer(monitor::activity(params(input)?)).await,
                "metrics" => answer(monitor::metrics(params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
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
                "tailAppLogs" => Ok(web::tail_logs(params(input)?, &sink).await?),
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Azure's tools are offered once the CLI has an account signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(auth::status().await.ready) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch() -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-azure-{}", std::process::id())),
        )
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
                "azure_resources",
                "azure_vms",
                "azure_vm_act",
                "azure_apps",
                "azure_app",
                "azure_app_act",
                "azure_storage",
                "azure_activity",
                "azure_metrics"
            ]
        );
    }

    #[tokio::test]
    async fn a_method_it_does_not_have_is_turned_away() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(&scratch(), "deleteEverything", json!({}))
            .await
            .expect_err("no such method");
        assert_eq!(error.category, "unknown-method");
    }

    #[tokio::test]
    async fn a_name_that_could_reach_another_path_is_turned_away() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(
                &scratch(),
                "vmAct",
                json!({ "subscription": "s1", "group": "g", "name": "../../other", "action": "start" }),
            )
            .await
            .expect_err("not a name");
        assert_eq!(error.category, "bad-params");
    }

    /// Runs against whatever the Azure CLI on this Mac is signed in to. Reads only.
    #[tokio::test]
    #[ignore = "needs an Azure sign-in"]
    async fn against_the_signed_in_subscription() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let plugin = plugin().expect("plugin loads");
        let ctx = scratch();
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(status["ready"].as_bool(), Some(true), "{status}");
        let groups = plugin
            .call(&ctx, "groups", json!({}))
            .await
            .expect("groups");
        assert!(
            groups.as_array().is_some_and(|groups| !groups.is_empty()),
            "{groups}"
        );
        let found = plugin
            .call(&ctx, "resources", json!({}))
            .await
            .expect("resources");
        assert!(
            found["resources"]
                .as_array()
                .is_some_and(|all| !all.is_empty()),
            "{found}"
        );
        let machines = plugin.call(&ctx, "vms", json!({})).await.expect("vms");
        for machine in machines.as_array().into_iter().flatten() {
            assert_ne!(machine["power"].as_str(), Some("unknown"), "{machine}");
        }
        let activity = plugin
            .call(&ctx, "activity", json!({ "hours": 24 }))
            .await
            .expect("activity");
        assert!(activity.is_array(), "{activity}");
    }
}
