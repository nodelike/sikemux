// Slack: read a thread a person shares, search, reply, and say who someone is,
// for the person in a pane of their own and for their agents as tools.
//
//   config — the workspaces signed in here, and their tokens in the Keychain
//   client — the HTTP client, the token, and Slack's `ok: false` answers
//   auth   — signing in with a token, and the status of each workspace
//   text   — message links, and Slack's markup as markdown
//   slack  — channels, history, threads, search, posts and people

mod auth;
mod client;
mod config;
mod error;
mod slack;
mod text;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::SlackResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Slack {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Slack {
    manifest: Manifest,
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = SlackResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Slack {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "status" => reply(auth::status(data_dir).await),
                "signIn" => {
                    auth::sign_in(data_dir, params(input)?).await?;
                    reply(auth::status(data_dir).await)
                }
                "signOut" => answer(auth::sign_out(data_dir, params(input)?)).await,
                "setDefault" => answer(auth::set_default(data_dir, params(input)?)).await,
                "channels" => answer(slack::channels(data_dir, params(input)?)).await,
                "history" => answer(slack::history(data_dir, params(input)?)).await,
                "thread" => answer(slack::thread(data_dir, params(input)?)).await,
                "search" => answer(slack::search(data_dir, params(input)?)).await,
                "post" => answer(slack::post(data_dir, params(input)?)).await,
                "people" => answer(slack::people(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        _input: Value,
        _sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move { Err(PluginError::unknown_method(method)) })
    }

    /// Slack is not tied to a repository, so its tools are offered wherever a workspace is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(!config::load(ctx.data_dir()).workspaces.is_empty()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-slack-{name}-{}", std::process::id())),
        )
    }

    #[test]
    fn its_manifest_parses_with_the_four_tools() {
        let plugin = plugin().expect("manifest parses");
        let names: Vec<&str> = plugin
            .manifest()
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["slack_thread", "slack_search", "slack_post", "slack_user"]
        );
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out_and_offers_no_tools() {
        let plugin = plugin().expect("plugin loads");
        let ctx = scratch("status");
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(
            (status["configured"].as_bool(), status["ok"].as_bool()),
            (Some(false), Some(false))
        );
        let error = plugin
            .call(&ctx, "channels", json!({}))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, "unconfigured");
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await.expect("answers"));
    }

    #[tokio::test]
    async fn a_token_that_is_not_slacks_is_refused_before_asking_slack() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(&scratch("token"), "signIn", json!({ "token": "glpat-abc" }))
            .await
            .expect_err("refused");
        assert_eq!(error.category, "bad-params");
    }

    #[tokio::test]
    async fn an_unknown_method_is_refused() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(&scratch("unknown"), "nonsense", Value::Null)
            .await
            .expect_err("refused");
        assert_eq!(error.category, "unknown-method");
    }
}
