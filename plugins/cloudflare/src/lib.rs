mod auth;
mod client;
mod config;
mod dns;
mod edge;
mod error;
mod oauth;
mod zones;

use serde_json::Value;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{
    answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

sikemux_plugin_api::plugin!(Cloudflare);

impl Plugin for Cloudflare {
    fn vault(&self) -> Option<&'static dyn sikemux_plugin_api::vault::Book> {
        Some(&oauth::BOOK)
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
                "accounts" => answer(zones::accounts(data_dir)).await,
                "zones" => answer(zones::zones(data_dir, params(input)?)).await,
                "zone_create" => answer(zones::zone_create(data_dir, params(input)?)).await,
                "dns_list" => answer(dns::dns_list(data_dir, params(input)?)).await,
                "dns_upsert" => answer(dns::dns_upsert(data_dir, params(input)?)).await,
                "dns_delete" => answer(dns::dns_delete(data_dir, params(input)?)).await,
                "dns_batch" => answer(dns::dns_batch(data_dir, params(input)?)).await,
                "workers" => answer(edge::workers(data_dir, params(input)?)).await,
                "pages" => answer(edge::pages(data_dir, params(input)?)).await,
                "purge_cache" => answer(zones::purge_cache(data_dir, params(input)?)).await,
                "resolveZone" => answer(zones::resolve_zone(data_dir, params(input)?)).await,
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

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move {
            Ok(store::has_account::<config::CloudflareConfig>(
                ctx.data_dir(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-cloudflare-{name}-{}", std::process::id())),
        )
    }

    #[tokio::test]
    async fn signed_out_status_and_no_tools_offered() -> Result<(), PluginError> {
        let plugin = plugin();
        let ctx = scratch("status");
        let status = plugin.call(&ctx, "status", Value::Null).await?;
        assert_eq!(
            (status["configured"].as_bool(), status["ok"].as_bool()),
            (Some(false), Some(false))
        );
        let error = plugin.call(&ctx, "zones", json!({})).await;
        assert!(matches!(
            error,
            Err(error) if error.category == sikemux_plugin_api::FailureCategory::Unconfigured
        ));
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await?);
        Ok(())
    }

    #[tokio::test]
    async fn empty_token_is_rejected_before_network() {
        let plugin = plugin();
        let error = plugin
            .call(&scratch("empty-token"), "signIn", json!({ "token": "   " }))
            .await;
        assert!(matches!(
            error,
            Err(error) if error.category == sikemux_plugin_api::FailureCategory::BadParams
        ));
    }

    #[tokio::test]
    async fn resolve_zone_returns_normalized_domain_without_network() -> Result<(), PluginError> {
        let plugin = plugin();
        let resolved = plugin
            .call(
                &scratch("resolve-domain"),
                "resolveZone",
                json!({ "zone": "Sikemux.COM." }),
            )
            .await?;
        assert_eq!(resolved["name"].as_str(), Some("sikemux.com"));
        Ok(())
    }
}
