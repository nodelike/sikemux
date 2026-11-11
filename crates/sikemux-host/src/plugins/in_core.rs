//! The built-in plugins in the core's own process, the one place they run, so
//! the window, agents and the board share one set of sign-ins and what the
//! board does on code hosts and trackers goes on while the app is closed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};
use sikemux_core::server::plugins::{
    AgentTool, Answer, Gated, Listing, Manifest, Plugins, Provided, Sink,
};
use sikemux_plugin_api::{vault, StreamSink};
use sikemux_process::user_environment::UserEnvironment;

use super::host::PluginHost;

impl Plugins for PluginHost {
    fn manifests(&self) -> Vec<&Manifest> {
        self.manifests().collect()
    }

    fn set_disabled(&self, ids: Vec<String>) {
        self.set_disabled(ids);
    }

    fn disabled(&self) -> Vec<String> {
        self.disabled()
    }

    fn relay_sign_in(&self, landed: &str) -> Result<(), String> {
        sikemux_loopback::relay(landed).map_err(|error| error.0)
    }

    fn offers_agent_tools(&self, plugin: &str) -> bool {
        self.offers_agent_tools(plugin)
    }

    fn agent_environment(&self, project: &Path) -> BTreeMap<String, String> {
        self.agent_environment(project)
    }

    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value> {
        Box::pin(async move { self.call(plugin, method, params).await })
    }

    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()> {
        Box::pin(async move {
            PluginHost::stream(self, plugin, method, params, StreamSink::new(sink)).await
        })
    }

    fn agent_tools<'a>(&'a self, remotes: &'a [String]) -> Listing<'a> {
        Box::pin(async move { describe(self.agent_tools_for(remotes).await) })
    }

    fn gated(&self, name: &str) -> Option<Gated> {
        let (plugin, tool) = self.agent_tool(name)?;
        Some(Gated {
            plugin: plugin.to_owned(),
            method: tool.method.clone(),
            check: tool.approve.as_ref()?.check.clone(),
        })
    }

    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value> {
        Box::pin(async move { self.call_agent_tool(name, arguments).await })
    }

    fn vault_books(&self) -> Vec<(String, PathBuf, &'static dyn vault::Book)> {
        PluginHost::vault_books(self)
    }
}

fn describe(tools: Vec<(&str, &AgentTool)>) -> Vec<Value> {
    tools
        .into_iter()
        .map(|(plugin, tool)| {
            json!({
                "plugin": plugin,
                "name": tool.name,
                "method": tool.method,
                "description": tool.description,
                "properties": tool.properties,
                "required": tool.required,
            })
        })
        .collect()
}

static SHELL_VARIABLES: OnceLock<Vec<String>> = OnceLock::new();

/// What the core's children get on top of its own environment: what the
/// window gives its children, plus every variable a plugin reads that the
/// person's shell sets and the core was not started with.
fn core_environment() -> UserEnvironment {
    let mut environment = crate::environment::user_environment();
    let shell = sikemux_pty::user_shell::login_shell_environment();
    for name in SHELL_VARIABLES.get().into_iter().flatten() {
        if std::env::var_os(name).is_some() {
            continue;
        }
        if let Some(value) = shell.get(name) {
            environment
                .variables
                .entry(name.clone())
                .or_insert_with(|| value.clone());
        }
    }
    environment
}

/// The plugins for the core, which keep their sign-ins under `data_dir` as
/// the app always has, after giving the core the environment they read.
pub fn provide_to_core(data_dir: &Path) -> Option<Provided> {
    crate::install_tls_crypto();
    let plugins = match PluginHost::with_builtins(&data_dir.join("plugins")) {
        Ok(host) => {
            let _ = SHELL_VARIABLES.set(host.shell_variables());
            let plugins: Box<dyn Plugins> = Box::new(host);
            #[cfg(feature = "fake-plugins")]
            let plugins: Box<dyn Plugins> = Box::new(super::fake::Fake(plugins));
            Some(Provided(Box::leak(plugins)))
        }
        Err(error) => {
            eprintln!("sikemux core: the built-in plugins did not load: {error}");
            None
        }
    };
    sikemux_process::user_environment::provide(
        core_environment,
        sikemux_pty::user_shell::login_shell_generation,
    );
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tools_mcp` routes an agent's call by these fields, from another process.
    #[test]
    fn a_listed_tool_names_the_plugin_that_answers_it() {
        let tool: AgentTool = serde_json::from_value(json!({
            "name": "github_runs", "method": "runs", "description": "Runs.",
        }))
        .expect("a tool");
        let listed = describe(vec![("sikemux.github", &tool)]);
        assert_eq!(listed[0]["plugin"], "sikemux.github");
        assert_eq!(listed[0]["name"], "github_runs");
        assert_eq!(listed[0]["method"], "runs");
    }
}
