//! The built-in plugins with every browser sign-in stood in for: it ends after a moment as if
//! the person had finished it, sends no address to open and keeps nothing. Vercel's first try
//! fails, so a film shows a refused sign-in too.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_core::server::plugins::{
    Answer, Gated, Listing, Manifest, PluginFailure, Plugins, Sink,
};
use sikemux_plugin_api::codehost::asks::Merge;
use sikemux_plugin_api::credential::{Acting, Ask, Credential};
use sikemux_plugin_api::{vault, FailureCategory};

use super::host::PluginHost;

const FAILS_FIRST: &str = "sikemux.vercel";
const SIGNING_IN: Duration = Duration::from_secs(4);

pub struct StandIn {
    host: PluginHost,
    tried: Mutex<HashSet<String>>,
    signed_in: Mutex<HashSet<String>>,
}

impl StandIn {
    pub fn over(host: PluginHost) -> Self {
        Self {
            host,
            tried: Mutex::default(),
            signed_in: Mutex::default(),
        }
    }
}

impl Plugins for StandIn {
    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value> {
        let signed_in = (self
            .signed_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner))
        .contains(plugin);
        match method {
            "status" if signed_in => Box::pin(async {
                Ok(json!({ "configured": true, "ok": true, "authFailed": false, "message": null }))
            }),
            _ => Plugins::call(&self.host, plugin, method, params),
        }
    }

    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()> {
        if method != "signInWithBrowser" {
            return Plugins::stream(&self.host, plugin, method, params, sink);
        }
        Box::pin(async move {
            tokio::time::sleep(SIGNING_IN).await;
            let first = (self.tried.lock().unwrap_or_else(PoisonError::into_inner))
                .insert(plugin.to_owned());
            if first && plugin == FAILS_FIRST {
                return Err(PluginFailure::new(
                    plugin,
                    FailureCategory::Auth,
                    "Vercel refused the sign-in",
                ));
            }
            (self
                .signed_in
                .lock()
                .unwrap_or_else(PoisonError::into_inner))
            .insert(plugin.to_owned());
            Ok(())
        })
    }

    fn manifests(&self) -> Vec<&Manifest> {
        Plugins::manifests(&self.host)
    }

    fn set_disabled(&self, ids: Vec<String>) {
        Plugins::set_disabled(&self.host, ids);
    }

    fn disabled(&self) -> Vec<String> {
        Plugins::disabled(&self.host)
    }

    fn relay_sign_in(&self, landed: &str) -> Result<(), String> {
        Plugins::relay_sign_in(&self.host, landed)
    }

    fn offers_agent_tools(&self, plugin: &str) -> bool {
        Plugins::offers_agent_tools(&self.host, plugin)
    }

    fn agent_environment(&self, project: &Path) -> BTreeMap<String, String> {
        Plugins::agent_environment(&self.host, project)
    }

    fn agent_tools<'a>(&'a self, remotes: &'a [String]) -> Listing<'a> {
        Plugins::agent_tools(&self.host, remotes)
    }

    fn gated(&self, name: &str) -> Option<Gated> {
        Plugins::gated(&self.host, name)
    }

    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value> {
        Plugins::call_agent_tool(&self.host, name, arguments)
    }

    fn credential<'a>(&'a self, ask: Ask) -> Answer<'a, Option<Credential>> {
        Plugins::credential(&self.host, ask)
    }

    fn revoke<'a>(&'a self, remote: &'a str, secret: String) -> Answer<'a, ()> {
        Plugins::revoke(&self.host, remote, secret)
    }

    fn merge_as_agents<'a>(&'a self, remote: &'a str, merge: Merge) -> Answer<'a, bool> {
        Plugins::merge_as_agents(&self.host, remote, merge)
    }

    fn acts_as(&self, remote: &str) -> Option<Acting> {
        Plugins::acts_as(&self.host, remote)
    }

    fn vault_books(&self) -> Vec<(String, PathBuf, &'static dyn vault::Book)> {
        Plugins::vault_books(&self.host)
    }
}
