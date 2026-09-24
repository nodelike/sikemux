pub mod agent;
mod builtin;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use dashmap::DashMap;
use semver::Version;
use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{AgentTool, Manifest, Plugin, PluginContext, PluginError, StreamSink};
use tauri::ipc::Channel;
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

use crate::error::{AppError, AppResult};

struct Loaded {
    plugin: Arc<dyn Plugin>,
    context: Arc<PluginContext>,
    call_timeout: Duration,
}

const PLUGIN_THREADS: usize = 2;
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Plugins run on threads of their own, so one that blocks or spins starves
/// other plugins but never the terminals and agents on the app's runtime.
pub struct PluginHost {
    plugins: BTreeMap<String, Loaded>,
    streams: Arc<DashMap<u32, JoinHandle<()>>>,
    next_stream: AtomicU32,
    runtime: Option<Runtime>,
    spawner: Handle,
    /// Switched off in Settings: still built in, but nothing reaches them.
    disabled: RwLock<HashSet<String>>,
}

impl PluginHost {
    pub fn with_builtins(data_root: &Path, sikemux: &Version) -> std::io::Result<Self> {
        Self::new(data_root, sikemux, builtin::plugins(), CALL_TIMEOUT)
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
            streams: Arc::default(),
            next_stream: AtomicU32::new(1),
            spawner: runtime.handle().clone(),
            runtime: Some(runtime),
            disabled: RwLock::default(),
        })
    }

    fn get(&self, id: &str) -> AppResult<&Loaded> {
        if !self.is_enabled(id) {
            return Err(AppError::Plugin {
                plugin: id.to_owned(),
                error: PluginError::new("disabled", format!("`{id}` is switched off in Settings")),
            });
        }
        self.plugins.get(id).ok_or_else(|| AppError::Plugin {
            plugin: id.to_owned(),
            error: PluginError::new("not-installed", format!("no plugin named `{id}`")),
        })
    }

    fn is_enabled(&self, id: &str) -> bool {
        !self
            .disabled
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
    }

    pub fn set_disabled(&self, ids: Vec<String>) {
        *self
            .disabled
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ids.into_iter().collect();
    }

    pub fn manifests(&self) -> Vec<Manifest> {
        self.plugins
            .values()
            .map(|loaded| loaded.plugin.manifest().clone())
            .collect()
    }

    /// Every tool the loaded plugins offer agents, beside the plugin that answers
    /// it. When two plugins name the same tool, the first keeps it.
    pub fn agent_tools(&self) -> Vec<(&str, &AgentTool)> {
        let mut offered: Vec<(&str, &AgentTool)> = Vec::new();
        for (id, loaded) in self.plugins.iter().filter(|(id, _)| self.is_enabled(id)) {
            for tool in &loaded.plugin.manifest().tools {
                if offered.iter().all(|(_, kept)| kept.name != tool.name) {
                    offered.push((id.as_str(), tool));
                }
            }
        }
        offered
    }

    /// Runs a tool by the name an agent knows it by. A plugin method that no
    /// tool names cannot be reached this way.
    pub async fn call_agent_tool(&self, name: &str, arguments: Value) -> AppResult<Value> {
        let (plugin, method) = self
            .agent_tools()
            .into_iter()
            .find(|(_, tool)| tool.name == name)
            .map(|(plugin, tool)| (plugin.to_owned(), tool.method.clone()))
            .ok_or_else(|| AppError::Other(format!("no plugin offers the tool `{name}`")))?;
        self.call(&plugin, &method, arguments).await
    }

    pub async fn call(&self, id: &str, method: &str, params: Value) -> AppResult<Value> {
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
            Ok(Err(stopped)) => Err(PluginError::new("stopped", stopped.to_string())),
            Err(_) => {
                abort.abort();
                Err(PluginError::new(
                    "timed-out",
                    format!(
                        "`{method}` did not answer within {}s",
                        call_timeout.as_secs()
                    ),
                ))
            }
        };
        outcome.map_err(|error| AppError::Plugin {
            plugin: id.to_owned(),
            error,
        })
    }

    pub fn start_stream(
        &self,
        id: &str,
        method: String,
        params: Value,
        on_event: Channel<StreamEvent>,
    ) -> AppResult<u32> {
        let loaded = self.get(id)?;
        let plugin = Arc::clone(&loaded.plugin);
        let context = Arc::clone(&loaded.context);
        let stream_id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let streams = Arc::clone(&self.streams);
        let items = on_event.clone();
        let sink = StreamSink::new(move |value| items.send(StreamEvent::Item { value }).is_ok());
        let (registered, is_registered) = tokio::sync::oneshot::channel::<()>();
        let task = self.spawner.spawn(async move {
            if is_registered.await.is_err() {
                return;
            }
            let finished = plugin.stream(&context, &method, params, sink).await;
            let last = match finished {
                Ok(()) => StreamEvent::End,
                Err(error) if error.category == PluginError::stream_closed().category => {
                    StreamEvent::End
                }
                Err(error) => StreamEvent::Error { error },
            };
            let _ = on_event.send(last);
            streams.remove(&stream_id);
        });
        self.streams.insert(stream_id, task);
        let _ = registered.send(());
        Ok(stream_id)
    }

    pub fn stop_stream(&self, stream_id: u32) {
        if let Some((_, task)) = self.streams.remove(&stream_id) {
            task.abort();
        }
    }

    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    pub fn drain(&self) {
        let ids: Vec<u32> = self.streams.iter().map(|entry| *entry.key()).collect();
        for stream_id in ids {
            self.stop_stream(stream_id);
        }
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StreamEvent {
    Item { value: Value },
    End,
    Error { error: PluginError },
}

#[tauri::command]
pub fn plugin_set_disabled(host: tauri::State<'_, PluginHost>, ids: Vec<String>) {
    host.set_disabled(ids);
}

#[tauri::command]
pub fn plugin_manifests(host: tauri::State<'_, PluginHost>) -> Vec<Manifest> {
    host.manifests()
}

#[tauri::command]
pub async fn plugin_call(
    host: tauri::State<'_, PluginHost>,
    plugin: String,
    method: String,
    params: Value,
) -> AppResult<Value> {
    host.call(&plugin, &method, params).await
}

#[tauri::command]
pub async fn plugin_stream_start(
    host: tauri::State<'_, PluginHost>,
    plugin: String,
    method: String,
    params: Value,
    on_event: Channel<StreamEvent>,
) -> AppResult<u32> {
    host.start_stream(&plugin, method, params, on_event)
}

#[tauri::command]
pub async fn plugin_stream_stop(
    host: tauri::State<'_, PluginHost>,
    stream_id: u32,
) -> AppResult<()> {
    host.stop_stream(stream_id);
    Ok(())
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
                        Err(PluginError::new("unconfigured", "sign in first").with_status(401))
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
    fn skips_incompatible_and_duplicate_plugins() {
        let host = host(vec![
            Echo::plugin("test.echo", ">=0.4"),
            Echo::plugin("test.echo", ">=0.4"),
            Echo::plugin("test.future", ">=9"),
        ]);
        let ids: Vec<String> = host.manifests().into_iter().map(|m| m.id).collect();
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
            json!({ "category": "unconfigured", "message": "test.echo: sign in first", "status": 401, "plugin": "test.echo" })
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
