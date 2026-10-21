// Linear: the person's issues and a search, one issue with its comments, and
// the changes a person or their agents make, in a pane and as agent tools.
//
//   config — the workspaces signed in here, and their keys or tokens in the Keychain
//   client — the GraphQL client, the key or token, and Linear's errors in words
//   oauth  — signing in through the browser with a Linear account
//   auth   — signing in with the browser or a personal API key, and each workspace's status
//   issues — listing, reading, commenting on, changing and filing issues

mod auth;
mod client;
mod config;
mod error;
mod issues;
mod oauth;

use std::sync::Arc;

use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Linear {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Linear {
    manifest: Manifest,
}

impl Plugin for Linear {
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
                "search" => answer(issues::search(data_dir, params(input)?)).await,
                "issue" => answer(issues::issue(data_dir, params(input)?)).await,
                "comment" => answer(issues::comment(data_dir, params(input)?)).await,
                "update" => answer(issues::update(data_dir, params(input)?)).await,
                "create" => answer(issues::create(data_dir, params(input)?)).await,
                "teams" => answer(issues::teams(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        _input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "signInWithBrowser" => {
                    auth::sign_in_with_browser(data_dir, &sink).await?;
                    sink.send(reply(auth::status(data_dir).await)?)
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Linear is not tied to a repository, so its tools are offered wherever a workspace is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(!config::load(ctx.data_dir())?.workspaces.is_empty()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-linear-{name}-{}", std::process::id())),
        )
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
            .call(&ctx, "search", json!({}))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, "unconfigured");
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await.expect("answers"));
    }

    #[tokio::test]
    async fn a_key_that_is_not_linears_is_turned_away_before_any_request() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(&scratch("bad-key"), "signIn", json!({ "key": "xoxp-123" }))
            .await
            .expect_err("not a Linear key");
        assert_eq!(error.category, "bad-params");
        assert!(
            error.message.contains("Personal API keys"),
            "{}",
            error.message
        );
    }
}
