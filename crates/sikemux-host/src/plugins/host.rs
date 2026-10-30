use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use semver::Version;
use serde_json::Value;
use sikemux_plugin_api::{
    AgentTool, Category, Manifest, Plugin, PluginContext, PluginError, PluginResult, StreamSink,
};
use sikemux_process::sync::{Locked, Shared};
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

use super::builtin;
use sikemux_core::server::plugins::Failure;

struct Loaded {
    plugin: Arc<dyn Plugin>,
    context: Arc<PluginContext>,
    call_timeout: Duration,
}

const PLUGIN_THREADS: usize = 2;
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const AGENT_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Plugins run on threads of their own, so one that blocks or spins starves
/// other plugins but never the terminals and agents on the core's runtime.
pub struct PluginHost {
    plugins: BTreeMap<String, Loaded>,
    runtime: Option<Runtime>,
    spawner: Handle,
    /// Switched off in Settings: still built in, but nothing reaches them.
    disabled: RwLock<HashSet<String>>,
    disabled_file: Option<PathBuf>,
    /// Each plugin's last answer for a set of remotes, kept for when it is slow to answer
    /// again, so an agent's tools do not come and go with a flaky check.
    offered_before: Mutex<OfferedBefore>,
}

/// Whether a plugin offered tools, by plugin id and the remotes it was asked about.
type OfferedBefore = HashMap<(String, Arc<[String]>), bool>;

const DISABLED_FILE: &str = "disabled.json";

impl PluginHost {
    pub fn with_builtins(data_root: &Path, sikemux: &Version) -> std::io::Result<Self> {
        let mut host = Self::new(data_root, sikemux, builtin::plugins(), CALL_TIMEOUT)?;
        host.disabled_file = Some(data_root.join(DISABLED_FILE));
        host.read_disabled();
        Ok(host)
    }

    fn new(
        data_root: &Path,
        sikemux: &Version,
        plugins: Vec<Arc<dyn Plugin>>,
        default_call_timeout: Duration,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(PLUGIN_THREADS)
            .thread_name("sikemux-plugin")
            .enable_all()
            .build()?;
        let mut loaded = BTreeMap::new();
        for plugin in plugins {
            let manifest = plugin.manifest();
            if !manifest.supports(sikemux) {
                eprintln!(
                    "plugin {} needs sikemux {} and this is {sikemux}; not loading it",
                    manifest.id, manifest.sikemux
                );
                continue;
            }
            if loaded.contains_key(&manifest.id) {
                eprintln!(
                    "plugin {} is registered twice; keeping the first",
                    manifest.id
                );
                continue;
            }
            let context = Arc::new(PluginContext::new(data_root.join(&manifest.id)));
            let call_timeout = manifest
                .call_timeout_secs
                .map_or(default_call_timeout, Duration::from_secs);
            loaded.insert(
                manifest.id.clone(),
                Loaded {
                    plugin,
                    context,
                    call_timeout,
                },
            );
        }
        Ok(Self {
            plugins: loaded,
            spawner: runtime.handle().clone(),
            runtime: Some(runtime),
            disabled: RwLock::default(),
            disabled_file: None,
            offered_before: Mutex::default(),
        })
    }

    fn get(&self, id: &str) -> Result<&Loaded, Failure> {
        if !self.is_enabled(id) {
            return Err(failure(
                id,
                PluginError::new(
                    Category::Disabled,
                    format!("`{id}` is switched off in Settings"),
                ),
            ));
        }
        self.plugins.get(id).ok_or_else(|| {
            failure(
                id,
                PluginError::new(Category::NotInstalled, format!("no plugin named `{id}`")),
            )
        })
    }

    fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.read_locked().contains(id)
    }

    pub fn set_disabled(&self, ids: Vec<String>) {
        if let Some(file) = &self.disabled_file {
            let written = serde_json::to_vec(&ids)
                .map_err(std::io::Error::from)
                .and_then(|bytes| {
                    if let Some(folder) = file.parent() {
                        std::fs::create_dir_all(folder)?;
                    }
                    sikemux_process::files::write_atomic(file, &bytes, false)
                });
            if let Err(error) = written {
                eprintln!("the switched-off plugins were not saved: {error}");
            }
        }
        self.keep_disabled(ids);
    }

    pub fn disabled(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.disabled.read_locked().iter().cloned().collect();
        ids.sort();
        ids
    }

    fn keep_disabled(&self, ids: Vec<String>) {
        *self.disabled.write_locked() = ids.into_iter().collect();
    }

    fn read_disabled(&self) {
        let Some(file) = &self.disabled_file else {
            return;
        };
        let ids = std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default();
        self.keep_disabled(ids);
    }

    pub fn offers_agent_tools(&self, id: &str) -> bool {
        self.plugins
            .get(id)
            .is_some_and(|loaded| !loaded.plugin.manifest().tools.is_empty())
    }

    /// Every shell variable a loaded plugin reads.
    pub fn shell_variables(&self) -> Vec<String> {
        self.plugins
            .values()
            .flat_map(|loaded| loaded.plugin.manifest().environment.clone())
            .collect()
    }

    /// What the enabled plugins set for an agent or terminal started in `project`.
    pub fn agent_environment(&self, project: &Path) -> BTreeMap<String, String> {
        self.plugins
            .iter()
            .filter(|(id, _)| self.is_enabled(id))
            .flat_map(|(_, loaded)| loaded.plugin.agent_environment(&loaded.context, project))
            .collect()
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.plugins.values().map(|loaded| loaded.plugin.manifest())
    }

    /// Every tool the loaded plugins offer agents, beside the plugin that answers
    /// it. When two plugins name the same tool, the first keeps it.
    pub fn agent_tools(&self) -> Vec<(&str, &AgentTool)> {
        self.tools_of(|id, _| self.is_enabled(id))
    }

    /// The tools an agent in a repository with these git remotes can use: only
    /// those of plugins that say they would work there.
    pub async fn agent_tools_for(&self, remotes: &[String]) -> Vec<(&str, &AgentTool)> {
        let remotes: Arc<[String]> = remotes.into();
        let checks: Vec<(&str, JoinHandle<PluginResult<bool>>)> = self
            .plugins
            .iter()
            .filter(|(id, loaded)| {
                self.is_enabled(id) && !loaded.plugin.manifest().tools.is_empty()
            })
            .map(|(id, loaded)| {
                let plugin = Arc::clone(&loaded.plugin);
                let context = Arc::clone(&loaded.context);
                let remotes = Arc::clone(&remotes);
                let check = self
                    .spawner
                    .spawn(async move { plugin.offers_agent_tools(&context, &remotes).await });
                (id.as_str(), check)
            })
            .collect();
        let mut offering = HashSet::new();
        for (id, mut check) in checks {
            let key = (id.to_string(), Arc::clone(&remotes));
            let answer = match tokio::time::timeout(AGENT_CHECK_TIMEOUT, &mut check).await {
                Ok(Ok(Ok(offers))) => Some(offers),
                Ok(_) => None,
                Err(_) => {
                    check.abort();
                    None
                }
            };
            let mut before = self.offered_before.locked();
            let offers = match answer {
                Some(offers) => {
                    before.insert(key, offers);
                    offers
                }
                None => before.get(&key).copied().unwrap_or(false),
            };
            if offers {
                offering.insert(id);
            }
        }
        self.tools_of(|id, tool| offering.contains(id) || (tool.always && self.is_enabled(id)))
    }

    fn tools_of(&self, include: impl Fn(&str, &AgentTool) -> bool) -> Vec<(&str, &AgentTool)> {
        let mut offered: Vec<(&str, &AgentTool)> = Vec::new();
        for (id, loaded) in &self.plugins {
            for tool in &loaded.plugin.manifest().tools {
                if include(id, tool) && offered.iter().all(|(_, kept)| kept.name != tool.name) {
                    offered.push((id.as_str(), tool));
                }
            }
        }
        offered
    }

    /// The tool an agent knows by `name`, beside the plugin that answers it.
    pub fn agent_tool(&self, name: &str) -> Option<(&str, &AgentTool)> {
        self.agent_tools()
            .into_iter()
            .find(|(_, tool)| tool.name == name)
    }

    /// Runs a tool by the name an agent knows it by. A plugin method that no
    /// tool names cannot be reached this way, nor can one the person approves.
    pub async fn call_agent_tool(&self, name: &str, arguments: Value) -> Result<Value, Failure> {
        let (plugin, tool) = self.agent_tool(name).ok_or_else(|| {
            Failure::new(
                "sikemux",
                Category::own("other"),
                format!("no plugin offers the tool `{name}`"),
            )
        })?;
        if tool.approve.is_some() {
            return Err(Failure::new(
                plugin,
                Category::own("other"),
                format!("`{name}` runs only once the person approves it"),
            ));
        }
        let (plugin, method) = (plugin.to_owned(), tool.method.clone());
        self.call(&plugin, &method, arguments).await
    }

    pub async fn call(&self, id: &str, method: &str, params: Value) -> Result<Value, Failure> {
        let loaded = self.get(id)?;
        let plugin = Arc::clone(&loaded.plugin);
        let context = Arc::clone(&loaded.context);
        let call_timeout = loaded.call_timeout;
        let owned_method = method.to_owned();
        let task = self
            .spawner
            .spawn(async move { plugin.call(&context, &owned_method, params).await });
        let abort = task.abort_handle();
        let outcome = match tokio::time::timeout(call_timeout, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(stopped)) => Err(PluginError::new(Category::Stopped, stopped.to_string())),
            Err(_) => {
                abort.abort();
                Err(PluginError::new(
                    Category::TimedOut,
                    format!(
                        "`{method}` did not answer within {}s",
                        call_timeout.as_secs()
                    ),
                ))
            }
        };
        outcome.map_err(|error| failure(id, error))
    }

    /// Runs a stream until it ends. Dropping the future stops it.
    pub async fn stream(
        &self,
        id: &str,
        method: &str,
        params: Value,
        sink: StreamSink,
    ) -> Result<(), Failure> {
        let loaded = self.get(id)?;
        let plugin = Arc::clone(&loaded.plugin);
        let context = Arc::clone(&loaded.context);
        let method = method.to_owned();
        let task = self
            .spawner
            .spawn(async move { plugin.stream(&context, &method, params, sink).await });
        let _stopping = StopOnDrop(task.abort_handle());
        let outcome = match task.await {
            Ok(Err(error)) if error.category == Category::StreamClosed => Ok(()),
            Ok(result) => result,
            Err(stopped) => Err(PluginError::new(Category::Stopped, stopped.to_string())),
        };
        outcome.map_err(|error| failure(id, error))
    }
}

fn failure(plugin: &str, error: PluginError) -> Failure {
    Failure {
        plugin: plugin.to_owned(),
        category: error.category,
        message: error.message,
        status: error.status,
    }
}

struct StopOnDrop(tokio::task::AbortHandle);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sikemux_plugin_api::PluginFuture;

    struct Echo(Manifest);

    impl Echo {
        fn plugin(id: &str, sikemux: &str) -> Arc<dyn Plugin> {
            let manifest = Manifest::from_json(
                &json!({ "id": id, "name": "Echo", "version": "1.0.0", "sikemux": sikemux })
                    .to_string(),
            )
            .expect("test manifest parses");
            Arc::new(Self(manifest))
        }
    }

    impl Plugin for Echo {
        fn manifest(&self) -> &Manifest {
            &self.0
        }

        fn call<'a>(
            &'a self,
            _ctx: &'a PluginContext,
            method: &'a str,
            params: Value,
        ) -> PluginFuture<'a, Value> {
            Box::pin(async move {
                match method {
                    "echo" => Ok(params),
                    "block" => {
                        std::thread::sleep(Duration::from_millis(300));
                        Ok(Value::Null)
                    }
                    "hang" => std::future::pending().await,
                    "fail" => {
                        Err(PluginError::new(Category::Unconfigured, "sign in first")
                            .with_status(401))
                    }
                    _ => Err(PluginError::unknown_method(method)),
                }
            })
        }
    }

    fn host(plugins: Vec<Arc<dyn Plugin>>) -> PluginHost {
        PluginHost::new(
            Path::new("/tmp/plugins"),
            &Version::new(0, 4, 0),
            plugins,
            Duration::from_millis(100),
        )
        .expect("plugin runtime starts")
    }

    #[test]
    #[cfg(all(feature = "github", feature = "signoz"))]
    fn collects_the_shell_variables_plugins_read() {
        let data = tempfile::tempdir().expect("tempdir");
        let version = Version::parse(env!("CARGO_PKG_VERSION")).expect("a version");
        let host = PluginHost::with_builtins(data.path(), &version).expect("plugin runtime starts");
        let names = host.shell_variables();
        for name in [
            "GH_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GH_HOST",
            "SIGNOZ_API_KEY",
        ] {
            assert!(names.iter().any(|found| found == name), "{name} is missing");
        }
    }

    #[test]
    fn skips_incompatible_and_duplicate_plugins() {
        let host = host(vec![
            Echo::plugin("test.echo", ">=0.4"),
            Echo::plugin("test.echo", ">=0.4"),
            Echo::plugin("test.future", ">=9"),
        ]);
        let ids: Vec<String> = host.manifests().map(|m| m.id.clone()).collect();
        assert_eq!(ids, ["test.echo"]);
    }

    #[tokio::test]
    async fn routes_calls_and_tags_errors_with_the_plugin() {
        let host = host(vec![Echo::plugin("test.echo", "*")]);
        assert_eq!(
            host.call("test.echo", "echo", json!({ "a": 1 })).await.ok(),
            Some(json!({ "a": 1 }))
        );

        let error = host
            .call("test.echo", "fail", Value::Null)
            .await
            .expect_err("fails");
        let wire = serde_json::to_value(&error).expect("serializes");
        assert_eq!(
            wire,
            json!({ "category": "unconfigured", "message": "sign in first", "status": 401, "plugin": "test.echo" })
        );

        let missing = host
            .call("test.nope", "echo", Value::Null)
            .await
            .expect_err("fails");
        assert_eq!(
            serde_json::to_value(&missing).expect("serializes")["category"],
            "not-installed"
        );
    }

    #[tokio::test]
    async fn agents_reach_only_the_methods_a_plugin_names_as_tools() {
        let manifest = Manifest::from_json(
            &json!({
                "id": "test.echo", "name": "Echo", "version": "1.0.0", "sikemux": "*",
                "tools": [{ "name": "echo_back", "method": "echo", "description": "Echo." }],
            })
            .to_string(),
        )
        .expect("test manifest parses");
        let host = host(vec![Arc::new(Echo(manifest))]);

        let offered: Vec<(&str, &str)> = host
            .agent_tools()
            .into_iter()
            .map(|(plugin, tool)| (plugin, tool.name.as_str()))
            .collect();
        assert_eq!(offered, [("test.echo", "echo_back")]);
        assert_eq!(
            host.call_agent_tool("echo_back", json!({ "a": 1 }))
                .await
                .ok(),
            Some(json!({ "a": 1 }))
        );
        assert!(host.call_agent_tool("fail", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn a_switched_off_plugin_answers_nothing_and_offers_no_tools() {
        let manifest = Manifest::from_json(
            &json!({
                "id": "test.echo", "name": "Echo", "version": "1.0.0", "sikemux": "*",
                "tools": [{ "name": "echo_back", "method": "echo", "description": "Echo." }],
            })
            .to_string(),
        )
        .expect("test manifest parses");
        let host = host(vec![Arc::new(Echo(manifest))]);

        host.set_disabled(vec!["test.echo".into()]);
        let refused = host
            .call("test.echo", "echo", Value::Null)
            .await
            .expect_err("switched off");
        assert_eq!(
            serde_json::to_value(&refused).expect("serializes")["category"],
            "disabled"
        );
        assert!(host.agent_tools().is_empty());

        host.set_disabled(Vec::new());
        assert!(host.call("test.echo", "echo", Value::Null).await.is_ok());
        assert_eq!(host.agent_tools().len(), 1);
    }

    /// Offers its tools only to agents whose repository has a remote on `host`.
    struct Picky {
        manifest: Manifest,
        host: &'static str,
    }

    impl Picky {
        fn plugin(id: &str, host: &'static str) -> Arc<dyn Plugin> {
            let manifest = Manifest::from_json(
                &json!({
                    "id": id, "name": "Picky", "version": "1.0.0", "sikemux": "*",
                    "tools": [{ "name": "picky_look", "method": "look", "description": "Look." }],
                })
                .to_string(),
            )
            .expect("test manifest parses");
            Arc::new(Self { manifest, host })
        }
    }

    impl Plugin for Picky {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }

        fn call<'a>(
            &'a self,
            _ctx: &'a PluginContext,
            method: &'a str,
            _params: Value,
        ) -> PluginFuture<'a, Value> {
            Box::pin(async move { Err(PluginError::unknown_method(method)) })
        }

        fn offers_agent_tools<'a>(
            &'a self,
            _ctx: &'a PluginContext,
            remotes: &'a [String],
        ) -> PluginFuture<'a, bool> {
            Box::pin(async move { Ok(remotes.iter().any(|remote| remote.contains(self.host))) })
        }
    }

    fn offered_to(host: &PluginHost, remotes: &[&str]) -> Vec<String> {
        let remotes: Vec<String> = remotes.iter().map(|remote| (*remote).to_owned()).collect();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(host.agent_tools_for(&remotes))
            .into_iter()
            .map(|(plugin, _)| plugin.to_owned())
            .collect()
    }

    #[test]
    fn an_agent_is_offered_only_the_tools_that_would_work_in_its_repository() {
        let host = host(vec![
            Picky::plugin("test.github", "github.com"),
            Picky::plugin("test.bitbucket", "bitbucket.org"),
        ]);
        assert_eq!(
            offered_to(&host, &["git@bitbucket.org:team/app.git"]),
            ["test.bitbucket"]
        );
        assert!(offered_to(&host, &[]).is_empty());

        host.set_disabled(vec!["test.bitbucket".into()]);
        assert!(offered_to(&host, &["git@bitbucket.org:team/app.git"]).is_empty());
    }

    #[test]
    fn a_tool_name_passes_to_the_next_plugin_when_the_first_is_not_offered() {
        let host = host(vec![
            Picky::plugin("test.a", "github.com"),
            Picky::plugin("test.b", "bitbucket.org"),
        ]);
        assert_eq!(
            offered_to(&host, &["https://bitbucket.org/team/app"]),
            ["test.b"]
        );
        assert_eq!(host.agent_tools().len(), 1);
    }

    #[test]
    fn the_first_plugin_keeps_a_tool_name_two_plugins_claim() {
        let claiming = |id: &str| -> Arc<dyn Plugin> {
            let manifest = Manifest::from_json(
                &json!({
                    "id": id, "name": "Echo", "version": "1.0.0", "sikemux": "*",
                    "tools": [{ "name": "echo_back", "method": "echo", "description": "Echo." }],
                })
                .to_string(),
            )
            .expect("test manifest parses");
            Arc::new(Echo(manifest))
        };
        let host = host(vec![claiming("test.b"), claiming("test.a")]);
        let offered: Vec<&str> = host
            .agent_tools()
            .into_iter()
            .map(|(plugin, _)| plugin)
            .collect();
        assert_eq!(offered, ["test.a"]);
    }

    #[tokio::test]
    async fn a_plugin_that_never_answers_times_out() {
        let host = host(vec![Echo::plugin("test.echo", "*")]);
        let error = host
            .call("test.echo", "hang", Value::Null)
            .await
            .expect_err("times out");
        assert_eq!(
            serde_json::to_value(&error).expect("serializes")["category"],
            "timed-out"
        );
    }

    #[tokio::test]
    async fn a_plugin_gets_the_call_timeout_it_declares() {
        let manifest = Manifest::from_json(
            &json!({
                "id": "test.slow", "name": "Slow", "version": "1.0.0", "sikemux": "*",
                "callTimeoutSecs": 1,
            })
            .to_string(),
        )
        .expect("test manifest parses");
        let host = host(vec![
            Arc::new(Echo(manifest)),
            Echo::plugin("test.echo", "*"),
        ]);
        assert_eq!(
            host.call("test.slow", "block", Value::Null).await.ok(),
            Some(Value::Null)
        );
        assert!(host.call("test.echo", "block", Value::Null).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blocking_plugin_does_not_hold_up_the_caller() {
        let host = host(vec![Echo::plugin("test.echo", "*")]);
        let started = std::time::Instant::now();
        let error = host
            .call("test.echo", "block", Value::Null)
            .await
            .expect_err("times out");
        assert_eq!(
            serde_json::to_value(&error).expect("serializes")["category"],
            "timed-out"
        );
        assert!(started.elapsed() < Duration::from_millis(250));
    }
}
