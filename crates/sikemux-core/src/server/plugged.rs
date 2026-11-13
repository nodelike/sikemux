//! The plugins the binary that runs the core provides, as what stands under them calls them:
//! the registry they are reached through, their manifests, and a call to one of them.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_calls::plugins::PluginFailure;
use sikemux_plugin_api::FailureCategory;

pub mod manifest;
use manifest::Manifest;

use super::repo_state::RepoParts;
use super::Core;

/// One line of a plugin stream on the tool endpoint.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StreamEvent {
    Item { value: Value },
    End,
    Error { error: PluginFailure },
}

pub type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, PluginFailure>> + Send + 'a>>;
pub type Listing<'a> = Pin<Box<dyn Future<Output = Vec<Value>> + Send + 'a>>;
/// Takes a stream's next item; false once nobody reads them, which ends it.
pub type Sink = Box<dyn Fn(Value) -> bool + Send + Sync>;

/// A plugin tool whose every call the person approves first: `check` answers
/// what they are asked, and `method` runs once they approve.
pub struct Gated {
    pub plugin: String,
    pub method: String,
    pub check: String,
}

pub trait Plugins: Send + Sync {
    fn manifests(&self) -> Vec<&Manifest>;
    fn set_disabled(&self, ids: Vec<String>);
    fn disabled(&self) -> Vec<String>;
    /// Hands a waiting browser sign-in the address the browser came back to on
    /// another machine.
    fn relay_sign_in(&self, landed: &str) -> Result<(), String>;
    fn offers_agent_tools(&self, plugin: &str) -> bool;
    /// What the plugins set for an agent or terminal started in `project`.
    fn agent_environment(&self, project: &Path) -> BTreeMap<String, String>;
    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value>;
    /// Runs until the stream ends. Dropping the answer stops it.
    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()>;
    /// The tools an agent in a repository with these git remotes is offered.
    fn agent_tools<'a>(&'a self, remotes: &'a [String]) -> Listing<'a>;
    /// The tool an agent knows by `name`, when the person approves each call to it.
    fn gated(&self, name: &str) -> Option<Gated>;
    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value>;
    /// Each plugin's sign-ins, for the account vault, with the folder the plugin keeps them in.
    fn vault_books(
        &self,
    ) -> Vec<(
        String,
        PathBuf,
        &'static dyn sikemux_plugin_api::vault::Book,
    )> {
        Vec::new()
    }
}

/// The plugins the binary that runs the core provides. They live as long as
/// the process does.
#[derive(Clone, Copy)]
pub struct Provided(pub &'static dyn Plugins);

impl std::fmt::Debug for Provided {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Provided")
    }
}

/// The plugins this core was started with, none in tests.
pub(crate) fn provided(core: &Core) -> Option<Provided> {
    core.plugins()
}

pub(crate) fn plugins(core: &Core, plugin: &str) -> Result<&'static dyn Plugins, PluginFailure> {
    present(provided(core), plugin)
}

pub(crate) fn present(
    plugins: Option<Provided>,
    plugin: &str,
) -> Result<&'static dyn Plugins, PluginFailure> {
    plugins.map(|provided| provided.0).ok_or_else(|| {
        PluginFailure::new(
            plugin,
            FailureCategory::NotInstalled,
            format!("Sikemux's {plugin} plugin is not available to its background process"),
        )
    })
}

pub(crate) async fn call(
    core: &Core,
    plugin: &str,
    method: &str,
    params: Value,
) -> Result<Value, PluginFailure> {
    call_on(provided(core), plugin, method, params).await
}

/// Calls `method` of `plugin` among `plugins`, for what holds them without the core.
pub(crate) async fn call_on(
    plugins: Option<Provided>,
    plugin: &str,
    method: &str,
    params: Value,
) -> Result<Value, PluginFailure> {
    present(plugins, plugin)?.call(plugin, method, params).await
}

pub(crate) fn manifests(core: &Core) -> Vec<&'static Manifest> {
    provided(core)
        .map(|Provided(plugins)| plugins.manifests())
        .unwrap_or_default()
}

pub(crate) fn manifest(core: &Core, plugin: &str) -> Option<&'static Manifest> {
    manifests(core)
        .into_iter()
        .find(|manifest| manifest.id == plugin)
}

/// An agent cannot sign in on the person's behalf, so a signed-out plugin says
/// who can fix it and where.
pub(crate) fn explain(failure: &PluginFailure) -> String {
    let signed_out = matches!(
        failure.category,
        FailureCategory::Unconfigured | FailureCategory::Auth
    ) || matches!(failure.status, Some(401 | 403));
    if signed_out {
        format!(
            "{} is not signed in ({}). Ask the person to sign in from its pane in Sikemux, then try again; do not switch to another tool or MCP server for it.",
            failure.plugin, failure.message
        )
    } else {
        format!("{}: {}", failure.plugin, failure.message)
    }
}
