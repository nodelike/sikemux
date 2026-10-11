//! The agent's browser, workspace and plugin tools, served over MCP on stdio.
//! Sikemux itself answers them over the CLI broker socket and they act on the
//! tabs the person sees in the agent's pane. The one exception is the guide,
//! which this binary carries and serves on its own.

mod harness;
mod manifest;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use manifest::{Manifest, Tool};
use sikemux_core::cli::protocol::{PLUGINS_CHANGED_METHOD, SIM_CANCEL_METHOD, SIM_OFFERED_METHOD};

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];
const PARENT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Asking whether to list the simulator tools must not hold up the host's handshake for long.
const OFFER_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait before asking again when Sikemux could not say whether plugins changed.
const WATCH_RETRY: Duration = Duration::from_secs(15);

pub fn run() -> i32 {
    let agent_id = match agent_id() {
        Ok(agent_id) => agent_id,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    watch_parent();
    let simulator = simulator_offered(&agent_id);
    serve(Arc::new(Manifest::load().offering(simulator)), agent_id);
    0
}

/// The app decides whether this agent gets the simulator tools, for chats it
/// started and chats a phone started alike, and remembers what it said.
fn simulator_offered(agent_id: &str) -> bool {
    harness::call_within(agent_id, SIM_OFFERED_METHOD, &json!({}), OFFER_TIMEOUT)
        .is_ok_and(|answer| answer["offered"] == true)
}

fn agent_id() -> Result<String, String> {
    validate_agent_id(&std::env::var("SIKEMUX_TOOLS_AGENT_ID").unwrap_or_default())
}

fn validate_agent_id(value: &str) -> Result<String, String> {
    let agent_id = value.trim();
    if agent_id.is_empty() {
        return Err("Missing SIKEMUX_TOOLS_AGENT_ID; launch this MCP through Sikemux".into());
    }
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-');
    if agent_id.len() > 128 || !agent_id.bytes().all(allowed) {
        return Err("Invalid SIKEMUX_TOOLS_AGENT_ID".into());
    }
    Ok(agent_id.to_owned())
}

/// macOS has no way to ask for a signal when the parent dies, so the sidecar
/// watches for the reparenting that follows instead. Without this an agent that
/// is killed rather than closed leaves its sidecar running forever.
#[cfg(unix)]
fn watch_parent() {
    // SAFETY: `getppid` takes nothing and cannot fail.
    let launcher = unsafe { libc::getppid() };
    std::thread::spawn(move || loop {
        std::thread::sleep(PARENT_CHECK_INTERVAL);
        // SAFETY: `getppid` takes nothing and cannot fail.
        if unsafe { libc::getppid() } != launcher {
            std::process::exit(0);
        }
    });
}

#[cfg(not(unix))]
fn watch_parent() {}

/// Only the app knows which plugins this build carries, so their tools are
/// asked for when an agent first lists tools, and again whenever the person
/// changes a plugin. A failed ask is not remembered, and the next listing
/// tries again.
#[derive(Default)]
struct PluginTools(Mutex<Option<Arc<Vec<Tool>>>>);

impl PluginTools {
    fn get(&self, manifest: &Manifest, relay: &Relay<'_>) -> Arc<Vec<Tool>> {
        if let Some(tools) = self.cached() {
            return tools;
        }
        let Ok(answer) = relay("plugins.tools", &json!({})) else {
            return Arc::default();
        };
        let tools = Arc::new(plugin_tools(manifest, answer));
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&tools));
        tools
    }

    /// Keeps what the app offers now, and whether that differs from what an
    /// agent was already given.
    fn replace(&self, tools: Vec<Tool>) -> bool {
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let declared = |tools: &[Tool]| tools.iter().map(Tool::declaration).collect::<Vec<_>>();
        let changed = held
            .as_ref()
            .is_some_and(|before| declared(before) != declared(&tools));
        *held = Some(Arc::new(tools));
        changed
    }

    fn cached(&self) -> Option<Arc<Vec<Tool>>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg(test)]
    fn with(tools: Vec<Tool>) -> Self {
        Self(Mutex::new(Some(Arc::new(tools))))
    }
}

/// A plugin tool that reuses a built-in name, or that this server cannot read,
/// is left out; the rest are still offered.
fn plugin_tools(manifest: &Manifest, answer: Value) -> Vec<Tool> {
    let Value::Array(offered) = answer else {
        return Vec::new();
    };
    offered
        .into_iter()
        .filter_map(|tool| serde_json::from_value::<Tool>(tool).ok())
        .filter(|tool| !manifest.declares(&tool.name))
        .collect()
}

/// One request to the app: a harness method and its params.
type Relay<'a> = dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'a;

/// Newline-delimited JSON-RPC, the framing every MCP stdio client speaks. The
/// loop ends when the host closes the pipe.
fn serve(manifest: Arc<Manifest>, agent_id: String) {
    let initialized = Arc::new(AtomicBool::new(false));
    let plugins = Arc::new(PluginTools::default());
    let relay: Arc<Relay<'static>> =
        Arc::new(move |method: &str, params: &Value| harness::call(&agent_id, method, params));
    let calls = Arc::new(Calls::default());
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            eprintln!("sikemux-tools-mcp: ignoring a line that is not JSON-RPC");
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = message.get("id").filter(|id| !id.is_null()).cloned() else {
            match method {
                "notifications/initialized" => {
                    if !initialized.swap(true, Ordering::AcqRel) {
                        let (manifest, plugins, relay) = (
                            Arc::clone(&manifest),
                            Arc::clone(&plugins),
                            Arc::clone(&relay),
                        );
                        std::thread::spawn(move || watch_plugins(&manifest, &plugins, &*relay));
                    }
                }
                "notifications/cancelled" => {
                    if let Some(name) = params.get("requestId").and_then(|id| calls.cancel(id)) {
                        if name.starts_with("sim_") {
                            let relay = Arc::clone(&relay);
                            std::thread::spawn(move || relay(SIM_CANCEL_METHOD, &json!({})));
                        }
                    }
                }
                _ => {}
            }
            continue;
        };
        match route(
            &manifest,
            initialized.load(Ordering::Acquire),
            id,
            method,
            params,
        ) {
            Route::Answer(answer) => emit(&answer),
            Route::List { id } => {
                let (manifest, plugins, relay) = (
                    Arc::clone(&manifest),
                    Arc::clone(&plugins),
                    Arc::clone(&relay),
                );
                std::thread::spawn(move || {
                    emit(&reply(
                        id,
                        json!({ "tools": list(&manifest, &plugins, &*relay) }),
                    ));
                });
            }
            Route::Call {
                id,
                name,
                arguments,
            } => {
                let (manifest, plugins, relay, calls) = (
                    Arc::clone(&manifest),
                    Arc::clone(&plugins),
                    Arc::clone(&relay),
                    Arc::clone(&calls),
                );
                calls.start(&id, &name);
                // A call waits on the app, so it runs off the read loop; a host
                // that pipelines a ping behind a navigation still gets answered.
                std::thread::spawn(move || {
                    let answer = call(&manifest, &plugins, &*relay, &name, &arguments);
                    if calls.finish(&id) {
                        emit(&reply(id, answer));
                    }
                });
            }
        }
    }
}

/// Tells the agent its tools changed when the person signs in to a plugin, or
/// switches one on or off, while it runs.
fn watch_plugins(manifest: &Manifest, plugins: &PluginTools, relay: &Relay<'_>) {
    let mut seen: Option<u64> = None;
    loop {
        match next_plugin_change(manifest, plugins, relay, seen) {
            Ok((version, changed)) => {
                if changed {
                    emit(
                        &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
                    );
                }
                seen = Some(version);
            }
            Err(_) => std::thread::sleep(WATCH_RETRY),
        }
    }
}

/// Waits for plugins to change after the `seen`th change, then lists their
/// tools again. Answers with the change it got to, and whether the tools differ.
fn next_plugin_change(
    manifest: &Manifest,
    plugins: &PluginTools,
    relay: &Relay<'_>,
    seen: Option<u64>,
) -> Result<(u64, bool), String> {
    let version = relay(PLUGINS_CHANGED_METHOD, &json!({ "seen": seen }))?["version"]
        .as_u64()
        .ok_or("Sikemux did not say how many times plugins changed")?;
    if seen == Some(version) {
        return Ok((version, false));
    }
    let answer = relay("plugins.tools", &json!({}))?;
    Ok((version, plugins.replace(plugin_tools(manifest, answer))))
}

/// Tool calls still running, by request id. A call the host cancelled is
/// not answered, as MCP asks.
#[derive(Default)]
struct Calls(Mutex<HashMap<String, (String, bool)>>);

impl Calls {
    fn start(&self, id: &Value, name: &str) {
        self.lock().insert(id.to_string(), (name.to_owned(), false));
    }

    /// Marks the call cancelled, and names its tool.
    fn cancel(&self, id: &Value) -> Option<String> {
        let mut calls = self.lock();
        let (name, cancelled) = calls.get_mut(&id.to_string())?;
        *cancelled = true;
        Some(name.clone())
    }

    /// Whether the finished call should still be answered.
    fn finish(&self, id: &Value) -> bool {
        self.lock()
            .remove(&id.to_string())
            .is_none_or(|(_, cancelled)| !cancelled)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, bool)>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

enum Route {
    Answer(Value),
    List {
        id: Value,
    },
    Call {
        id: Value,
        name: String,
        arguments: Value,
    },
}

fn route(manifest: &Manifest, initialized: bool, id: Value, method: &str, params: Value) -> Route {
    if method == "initialize" {
        return Route::Answer(reply(id, initialize(manifest, &params)));
    }
    if !initialized {
        return Route::Answer(failure(
            id,
            -32602,
            "Invalid request parameters",
            Some(Value::String(String::new())),
        ));
    }
    match method {
        "ping" => Route::Answer(reply(id, json!({}))),
        "tools/list" => Route::List { id },
        "tools/call" => match params.get("name").and_then(Value::as_str) {
            Some(name) => Route::Call {
                id,
                name: name.to_owned(),
                arguments: params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            },
            None => Route::Answer(failure(id, -32602, "Invalid request parameters", None)),
        },
        _ => Route::Answer(failure(id, -32601, "Method not found", None)),
    }
}

fn initialize(manifest: &Manifest, params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        LATEST_PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "experimental": {}, "tools": { "listChanged": true } },
        "serverInfo": { "name": "sikemux-tools", "version": env!("CARGO_PKG_VERSION") },
        "instructions": manifest.instructions(),
    })
}

fn list(manifest: &Manifest, plugins: &PluginTools, relay: &Relay<'_>) -> Vec<Value> {
    let mut tools = manifest.declarations();
    tools.extend(plugins.get(manifest, relay).iter().map(Tool::declaration));
    tools
}

fn call(
    manifest: &Manifest,
    plugins: &PluginTools,
    relay: &Relay<'_>,
    name: &str,
    arguments: &Value,
) -> Value {
    if name == manifest.guide_name() {
        let topic = arguments.get("topic").and_then(Value::as_str);
        return match manifest.guide_text(topic) {
            Ok(guide) => content(vec![text(guide)], false),
            Err(message) => invalid(&message),
        };
    }
    if let Some(tool) = manifest.tool(name) {
        if let Err(message) = tool.validate(arguments) {
            return invalid(&message);
        }
        return answer(name, relay(&tool.method, arguments));
    }
    let offered = plugins.get(manifest, relay);
    let Some(tool) = offered.iter().find(|tool| tool.name == name) else {
        return content(vec![text(&format!("Unknown tool: {name}"))], true);
    };
    if let Err(message) = tool.validate(arguments) {
        return invalid(&message);
    }
    answer(
        name,
        relay(
            "plugins.call",
            &json!({ "tool": name, "arguments": arguments }),
        ),
    )
}

fn invalid(message: &str) -> Value {
    content(
        vec![text(&format!("Input validation error: {message}"))],
        true,
    )
}

fn answer(name: &str, result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => content(content_for(name, &value), false),
        Err(message) => content(vec![text(&message)], true),
    }
}

/// A screenshot is the one answer an agent reads as a picture rather than as
/// JSON, so it travels as an image block with the page's name beside it. A
/// page preview's picture travels the same way, with the rest of its answer.
fn content_for(name: &str, value: &Value) -> Vec<Value> {
    if name == "page_preview" {
        if let Some(data) = value.get("data").and_then(Value::as_str) {
            let mut rest = value.clone();
            if let Some(fields) = rest.as_object_mut() {
                fields.remove("data");
                fields.remove("mimeType");
            }
            return vec![
                json!({ "type": "image", "data": data, "mimeType": value["mimeType"] }),
                text(&rest.to_string()),
            ];
        }
    }
    if name == "browser_screenshot" || name == "sim_screenshot" {
        if let Some(data) = value.get("data").and_then(Value::as_str) {
            let field = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default();
            let mime_type = value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let mut caption = format!("{} {}", field("title"), field("url"))
                .trim()
                .to_owned();
            if let Some(height) = value.get("cutAt").and_then(Value::as_f64) {
                caption.push_str(&format!("\n(cut at {height}px; the page is taller)"));
            }
            if !field("elements").is_empty() {
                caption.push('\n');
                caption.push_str(field("elements"));
            }
            let caption = caption.trim();
            return vec![
                json!({ "type": "image", "data": data, "mimeType": mime_type }),
                text(if caption.is_empty() {
                    "screenshot"
                } else {
                    caption
                }),
            ];
        }
    }
    vec![text(&value.to_string())]
}

fn text(body: &str) -> Value {
    json!({ "type": "text", "text": body })
}

fn content(blocks: Vec<Value>, is_error: bool) -> Value {
    json!({ "content": blocks, "isError": is_error })
}

fn reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: Value, code: i32, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn emit(message: &Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests;
