// Slack: read a thread a person shares, search, reply, and say who someone is,
// as tools for the person's agents.

mod auth;
mod client;
mod config;
mod error;
mod slack;
mod text;

use serde_json::Value;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

sikemux_plugin_api::plugin!(Slack);

impl Plugin for Slack {
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
                "thread" => answer(slack::thread(data_dir, params(input)?)).await,
                "search" => answer(slack::search(data_dir, params(input)?)).await,
                "post" => answer(slack::post(data_dir, params(input)?)).await,
                "people" => answer(slack::people(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Slack is not tied to a repository, so its tools are offered wherever a workspace is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(store::has_account::<config::SlackConfig>(ctx.data_dir())) })
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

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out_and_offers_no_tools() {
        let plugin = plugin();
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
            .call(&ctx, "search", json!({ "query": "deploy" }))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, sikemux_plugin_api::Category::Unconfigured);
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await.expect("answers"));
    }

    #[tokio::test]
    async fn a_token_that_is_not_slacks_is_refused_before_asking_slack() {
        let plugin = plugin();
        let error = plugin
            .call(&scratch("token"), "signIn", json!({ "token": "glpat-abc" }))
            .await
            .expect_err("refused");
        assert_eq!(error.category, sikemux_plugin_api::Category::BadParams);
    }

    #[tokio::test]
    async fn an_unknown_method_is_refused() {
        let plugin = plugin();
        let error = plugin
            .call(&scratch("unknown"), "nonsense", Value::Null)
            .await
            .expect_err("refused");
        assert_eq!(error.category, sikemux_plugin_api::Category::UnknownMethod);
    }
}
