//! The built-in plugins in the core's own process, the one place they run, so
//! the window, agents and the board share one set of sign-ins and what the
//! board does on code hosts and trackers goes on while the app is closed.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::{json, Value};
use sikemux_core::server::plugins::{Answer, Listing, Plugins, Sink};
use sikemux_plugin_api::{AgentTool, StreamSink};
use sikemux_process::user_environment::UserEnvironment;

use super::host::PluginHost;

struct InCore(PluginHost);

impl Plugins for InCore {
    fn manifests(&self) -> Vec<Value> {
        self.0
            .manifests()
            .iter()
            .filter_map(|manifest| serde_json::to_value(manifest).ok())
            .collect()
    }

    fn set_disabled(&self, ids: Vec<String>) {
        self.0.set_disabled(ids);
    }

    fn disabled(&self) -> Vec<String> {
        self.0.disabled()
    }

    fn relay_sign_in(&self, landed: &str) -> Result<(), String> {
        sikemux_loopback::relay(landed).map_err(|error| error.0)
    }

    fn offers_agent_tools(&self, plugin: &str) -> bool {
        self.0.offers_agent_tools(plugin)
    }

    fn agent_environment(&self, project: &Path) -> BTreeMap<String, String> {
        self.0.agent_environment(project)
    }

    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value> {
        Box::pin(async move { self.0.call(plugin, method, params).await })
    }

    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()> {
        Box::pin(async move {
            self.0
                .stream(plugin, method, params, StreamSink::new(sink))
                .await
        })
    }

    fn agent_tools<'a>(&'a self, remotes: &'a [String]) -> Listing<'a> {
        Box::pin(async move { describe(self.0.agent_tools_for(remotes).await) })
    }

    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value> {
        Box::pin(async move { self.0.call_agent_tool(name, arguments).await })
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

/// Gives the core the plugins, which keep their sign-ins under `data_dir`
/// as the app always has, and the environment they read.
pub fn provide_to_core(data_dir: &Path) {
    crate::install_tls_crypto();
    let host = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(std::io::Error::other)
        .and_then(|version| PluginHost::with_builtins(&data_dir.join("plugins"), &version));
    match host {
        Ok(host) => {
            let _ = SHELL_VARIABLES.set(host.shell_variables());
            let plugins: Box<dyn Plugins> = Box::new(InCore(host));
            #[cfg(feature = "fake-plugins")]
            let plugins: Box<dyn Plugins> = Box::new(super::fake::Fake(plugins));
            sikemux_core::server::plugins::provide(plugins);
        }
        Err(error) => eprintln!("sikemux core: the built-in plugins did not load: {error}"),
    }
    sikemux_process::user_environment::provide(
        core_environment,
        sikemux_pty::user_shell::login_shell_generation,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
