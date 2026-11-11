// Google Play: an app's tracks, uploading a bundle to one, and the upload keystores
// that sign it, as agent tools.

mod auth;
mod client;
mod config;
mod error;
mod keystore;
mod tracks;

use serde_json::Value;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

sikemux_plugin_api::plugin!(GooglePlayPlugin);

impl Plugin for GooglePlayPlugin {
    fn vault(&self) -> Option<&'static dyn sikemux_plugin_api::vault::Book> {
        Some(&config::BOOK)
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
                "signOut" => answer(auth::sign_out(data_dir)).await,
                "tracks" => answer(tracks::tracks(data_dir, params(input)?)).await,
                "upload" => answer(tracks::upload(data_dir, params(input)?)).await,
                "keystore" => answer(keystore::keystores(data_dir, params(input)?)).await,
                "keystoreCreate" => answer(keystore::create(data_dir, params(input)?)).await,
                "keystoreAdd" => answer(keystore::add(data_dir, params(input)?)).await,
                "sign" => answer(keystore::sign(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Play is not tied to a repository, so its tools are offered wherever an account is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move {
            Ok(config::load(ctx.data_dir()).is_ok_and(|config| config.account.is_some()))
        })
    }
}
