//! The agent's browser, workspace and plugin tools, served over MCP on stdio.
//! Sikemux itself answers them over the CLI broker socket and they act on the
//! tabs the person sees in the agent's pane. The one exception is the guide,
//! which this binary carries and serves on its own.

mod harness;
mod manifest;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use manifest::{Manifest, Tool};

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];
const PARENT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let agent_id = match agent_id() {
        Ok(agent_id) => agent_id,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    watch_parent();
    serve(Arc::new(Manifest::load()), agent_id);
    0
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
    let launcher = unsafe { libc::getppid() };
    std::thread::spawn(move || loop {
        std::thread::sleep(PARENT_CHECK_INTERVAL);
        if unsafe { libc::getppid() } != launcher {
            std::process::exit(0);
        }
    });
}

#[cfg(not(unix))]
fn watch_parent() {}

/// Only the app knows which plugins this build carries, so their tools are
/// asked for when an agent first lists tools. A failed ask is not remembered,
/// and the next listing tries again.
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
            if method == "notifications/initialized" {
                initialized.store(true, Ordering::Release);
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
                let (manifest, plugins, relay) = (
                    Arc::clone(&manifest),
                    Arc::clone(&plugins),
                    Arc::clone(&relay),
                );
                // A call waits on the app, so it runs off the read loop; a host
                // that pipelines a ping behind a navigation still gets answered.
                std::thread::spawn(move || {
                    emit(&reply(
                        id,
                        call(&manifest, &plugins, &*relay, &name, &arguments),
                    ));
                });
            }
        }
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
        "capabilities": { "experimental": {}, "tools": { "listChanged": false } },
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
        return content(vec![text(manifest.guide_text())], false);
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
/// JSON, so it travels as an image block with the page's name beside it.
fn content_for(name: &str, value: &Value) -> Vec<Value> {
    if name == "browser_screenshot" {
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
