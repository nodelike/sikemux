mod bru;
mod collection;
mod error;
mod run;
mod scripts;
mod send;
mod vars;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Category, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
    PluginResult,
};

use crate::send::BruSendRequest;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Bruno {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Bruno {
    manifest: Manifest,
}

#[derive(Deserialize)]
struct SendParams {
    req: BruSendRequest,
}

impl Plugin for Bruno {
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
                "send" => {
                    let SendParams { req } = params(input)?;
                    reply(send::send(req).await?)
                }
                "collectionView" => on_disk(input, |ask| collection::collection(&ask)).await,
                "requestView" => on_disk(input, |ask| collection::request(&ask)).await,
                "workspacesView" => on_disk(input, |ask| collection::workspaces(&ask)).await,
                "saveRequest" => on_disk(input, collection::save).await,
                "newRequest" => on_disk(input, collection::new_request).await,
                "newFolder" => on_disk(input, collection::new_folder).await,
                "renameRequest" => on_disk(input, collection::rename).await,
                "deletePath" => on_disk(input, collection::delete).await,
                "run" => reply(run::run(params(input)?).await?),
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
}

/// Runs `work` on the asked params off the async threads, since it reads and writes files.
async fn on_disk<Ask, Out>(
    input: Value,
    work: impl FnOnce(Ask) -> error::BrunoResult<Out> + Send + 'static,
) -> PluginResult<Value>
where
    Ask: DeserializeOwned + Send + 'static,
    Out: serde::Serialize + Send + 'static,
{
    let ask: Ask = params(input)?;
    let done = tokio::task::spawn_blocking(move || work(ask))
        .await
        .map_err(|error| PluginError::new(Category::Internal, error.to_string()))?;
    reply(done?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn its_manifest_parses() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.bruno");
        assert_eq!(plugin.manifest().call_timeout_secs, Some(150));
    }

    #[tokio::test]
    async fn send_refuses_a_bad_url_before_touching_the_network() {
        let plugin = plugin().expect("manifest parses");
        let ctx = PluginContext::new(std::env::temp_dir());
        let req = json!({
            "method": "GET",
            "url": "file:///etc/passwd",
            "headers": [],
            "body": { "kind": "none" },
            "timeout_ms": 0,
            "skip_tls_verify": false,
            "trust": {
                "allow_private_network": false,
                "allow_file_read": false,
                "allow_insecure_tls": false,
                "file_root": null,
            },
        });
        let error = plugin
            .call(&ctx, "send", json!({ "req": req }))
            .await
            .expect_err("refused");
        assert_eq!(error.category, sikemux_plugin_api::Category::BadParams);
        assert_eq!(
            error.message,
            "invalid argument: only http(s) URLs are supported"
        );
    }
}
