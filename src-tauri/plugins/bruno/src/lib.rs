mod error;
mod send;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
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
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
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
        assert_eq!(error.category, "bad-params");
        assert_eq!(
            error.message,
            "invalid argument: only http(s) URLs are supported"
        );
    }
}
