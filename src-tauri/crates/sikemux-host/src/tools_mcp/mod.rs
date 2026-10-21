//! The agent's browser, workspace and plugin tools, served over MCP on stdio.
//! Sikemux itself answers them over the CLI broker socket and they act on the
//! tabs the person sees in the agent's pane. The one exception is the guide,
//! which this binary serves from what Sikemux handed it.
//!
//! An agent keeps this process across a Sikemux update, so everything a tool
//! is comes from the running Sikemux: its name, schema, method and how its
//! answer is shown. This binary's own copy is used only when Sikemux cannot be
//! reached.

mod harness;
mod manifest;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use manifest::{Manifest, Tool};
use sikemux_core::cli::protocol::{
    PLUGINS_CHANGED_METHOD, REVIEWER_OFFER, SIM_CANCEL_METHOD, SIM_OFFERED_METHOD,
    TOOLS_MANIFEST_METHOD, TOOLS_OFFER_ENV,
};

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
    let reviewer = std::env::var(TOOLS_OFFER_ENV).is_ok_and(|offer| offer == REVIEWER_OFFER);
    serve(Arc::new(Offer::new(simulator, reviewer)), agent_id);
    0
}

/// The app decides whether this agent gets the simulator tools, for chats it
/// started and chats a phone started alike, and remembers what it said.
fn simulator_offered(agent_id: &str) -> bool {
    harness::call_within(
        agent_id,
        SIM_OFFERED_METHOD,
        &json!({}),
        OFFER_TIMEOUT,
        Duration::ZERO,
    )
    .is_ok_and(|answer| answer["offered"] == true)
}

fn agent_id() -> Result<String, String> {
    validate_agent_id(&std::env::var(sikemux_process::vars::TOOLS_AGENT_ID).unwrap_or_default())
}

fn validate_agent_id(value: &str) -> Result<String, String> {
    let agent_id = value.trim();
    if agent_id.is_empty() {
        return Err("Missing SIKEMUX_TOOLS_AGENT_ID; launch this MCP through Sikemux".into());
    }
    if !sikemux_core::cli::protocol::is_tool_agent_id(agent_id) {
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

/// The tools this agent is offered: the built-in ones as the running Sikemux
/// describes them, and the ones its plugins offer. Listing and calling ask
/// Sikemux only for what it has not answered yet, without waiting for it, and
/// fall back to this binary's own tools and no plugin tools. The watcher waits
/// out a restart and asks again.
struct Offer {
    simulator: bool,
    reviewer: bool,
    built: Arc<Manifest>,
    served: Mutex<Option<Arc<Manifest>>>,
    plugins: Mutex<Option<Arc<Vec<Tool>>>>,
    /// What the agent was last given by `tools/list`.
    given: Mutex<Option<Vec<Value>>>,
    /// The guide as the agent was last told it reads, once the app served one.
    told_guide: Mutex<Option<String>>,
    /// Parts of the guide that changed since, to tell the agent with its
    /// next tool answer: `None` for the opening, else a topic.
    guide_news: Mutex<Vec<Option<String>>>,
}

impl Offer {
    fn new(simulator: bool, reviewer: bool) -> Self {
        let built = Manifest::load().offering(simulator, reviewer);
        Self {
            simulator,
            reviewer,
            told_guide: Mutex::default(),
            built: Arc::new(built),
            served: Mutex::default(),
            plugins: Mutex::default(),
            given: Mutex::default(),
            guide_news: Mutex::default(),
        }
    }

    #[cfg(test)]
    fn with(manifest: Manifest, plugins: Option<Vec<Tool>>) -> Self {
        Self {
            simulator: true,
            reviewer: false,
            built: Arc::new(Manifest::load()),
            told_guide: Mutex::new(Some(manifest.guide_source().to_owned())),
            served: Mutex::new(Some(Arc::new(manifest))),
            plugins: Mutex::new(plugins.map(Arc::new)),
            given: Mutex::default(),
            guide_news: Mutex::default(),
        }
    }

    fn built_in(&self, relay: &Relay<'_>) -> Arc<Manifest> {
        held(&self.served)
            .or_else(|| Some(self.hold(self.ask_manifest(relay)?)))
            .unwrap_or_else(|| Arc::clone(&self.built))
    }

    fn plugin_tools(&self, relay: &Relay<'_>) -> Arc<Vec<Tool>> {
        held(&self.plugins)
            .or_else(|| Some(keep(&self.plugins, self.ask_plugins(relay)?)))
            .unwrap_or_default()
    }

    fn ask_manifest(&self, relay: &Relay<'_>) -> Option<Manifest> {
        let answer = relay(TOOLS_MANIFEST_METHOD, &json!({})).ok()?;
        Some(Manifest::served(&answer)?.offering(self.simulator, self.reviewer))
    }

    /// Holds what the app serves, noting the parts of its guide that changed
    /// since the agent was last told.
    fn hold(&self, manifest: Manifest) -> Arc<Manifest> {
        let mut told = self.told_guide.locked();
        let changed = told
            .as_deref()
            .map(|told| manifest.guide_changes(told))
            .unwrap_or_default();
        if told.is_none() || !changed.is_empty() {
            *told = Some(manifest.guide_source().to_owned());
            let mut news = self.guide_news.locked();
            for part in changed {
                if !news.contains(&part) {
                    news.push(part);
                }
            }
        }
        drop(told);
        keep(&self.served, manifest)
    }

    /// Tells the agent once which parts of the guide to read again.
    fn take_guide_news(&self) -> Option<String> {
        let news = std::mem::take(&mut *self.guide_news.locked());
        if news.is_empty() {
            return None;
        }
        let topics: Vec<&str> = news.iter().flatten().map(String::as_str).collect();
        let mut parts = Vec::new();
        if news.iter().any(Option::is_none) {
            parts.push("its opening".to_owned());
        }
        match topics.as_slice() {
            [] => {}
            [topic] => parts.push(format!("the {topic} topic")),
            [rest @ .., last] => parts.push(format!("the {} and {last} topics", rest.join(", "))),
        }
        Some(format!(
            "Sikemux was updated since you started and its {guide} changed: read {} again with {guide}.",
            parts.join(" and "),
            guide = self.manifest().guide_name(),
        ))
    }

    fn ask_plugins(&self, relay: &Relay<'_>) -> Option<Vec<Tool>> {
        relay("plugins.tools", &json!({})).ok().map(plugin_tools)
    }

    /// Asks the app again for everything, keeping what it could not answer.
    /// Answers whether the agent should list its tools again, and whether
    /// the built-in tools came from the app.
    fn refresh(&self, relay: &Relay<'_>) -> (bool, bool) {
        let manifest = self.ask_manifest(relay);
        let served = manifest.is_some();
        if let Some(manifest) = manifest {
            self.hold(manifest);
        }
        if let Some(tools) = self.ask_plugins(relay) {
            keep(&self.plugins, tools);
        }
        let mut given = self.given.locked();
        let Some(before) = given.as_ref() else {
            return (false, served);
        };
        let now = self.declarations();
        let changed = *before != now;
        *given = Some(now);
        (changed, served)
    }

    /// What `tools/list` answers with what is held now.
    fn declarations(&self) -> Vec<Value> {
        declarations(&self.manifest(), &held(&self.plugins).unwrap_or_default())
    }

    fn manifest(&self) -> Arc<Manifest> {
        held(&self.served).unwrap_or_else(|| Arc::clone(&self.built))
    }
}

fn held<T>(slot: &Mutex<Option<Arc<T>>>) -> Option<Arc<T>> {
    slot.locked().clone()
}

fn keep<T>(slot: &Mutex<Option<Arc<T>>>, value: T) -> Arc<T> {
    let value = Arc::new(value);
    *slot.locked() = Some(Arc::clone(&value));
    value
}

/// A plugin tool this server cannot read is left out; the rest are still
/// offered.
fn plugin_tools(answer: Value) -> Vec<Tool> {
    let Value::Array(offered) = answer else {
        return Vec::new();
    };
    offered
        .into_iter()
        .filter_map(|tool| serde_json::from_value::<Tool>(tool).ok())
        .collect()
}

fn declarations(manifest: &Manifest, plugins: &[Tool]) -> Vec<Value> {
    let mut tools = manifest.declarations();
    tools.extend(
        plugins
            .iter()
            .filter(|tool| !manifest.declares(&tool.name))
            .map(Tool::declaration),
    );
    tools
}

/// One request to the app: a harness method and its params.
type Relay<'a> = dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'a;

/// A tool call waits for Sikemux to come back from a restart. Anything that
/// has an answer without Sikemux, such as listing tools, asks only once.
struct Relays {
    patient: Box<Relay<'static>>,
    quick: Box<Relay<'static>>,
}

impl Relays {
    fn new(agent_id: String) -> Self {
        let quick_id = agent_id.clone();
        Self {
            patient: Box::new(move |method: &str, params: &Value| {
                harness::call(&agent_id, method, params)
            }),
            quick: Box::new(move |method: &str, params: &Value| {
                harness::call_within(
                    &quick_id,
                    method,
                    params,
                    harness::TOOLS_REPLY_TIMEOUT,
                    Duration::ZERO,
                )
            }),
        }
    }
}

/// Newline-delimited JSON-RPC, the framing every MCP stdio client speaks. The
/// loop ends when the host closes the pipe.
fn serve(offer: Arc<Offer>, agent_id: String) {
    let initialized = Arc::new(AtomicBool::new(false));
    let relays = Arc::new(Relays::new(agent_id));
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
                        let (offer, relays) = (Arc::clone(&offer), Arc::clone(&relays));
                        std::thread::spawn(move || watch(&offer, &*relays.patient, &*relays.quick));
                    }
                }
                "notifications/cancelled" => {
                    if let Some(name) = params.get("requestId").and_then(|id| calls.cancel(id)) {
                        if name.starts_with("sim_") {
                            let relays = Arc::clone(&relays);
                            std::thread::spawn(move || {
                                (relays.quick)(SIM_CANCEL_METHOD, &json!({}))
                            });
                        }
                    }
                }
                _ => {}
            }
            continue;
        };
        match route(
            &offer.manifest(),
            initialized.load(Ordering::Acquire),
            id,
            method,
            params,
        ) {
            Route::Answer(answer) => emit(&answer),
            Route::List { id } => {
                let (offer, relays) = (Arc::clone(&offer), Arc::clone(&relays));
                std::thread::spawn(move || {
                    emit(&reply(id, json!({ "tools": list(&offer, &*relays.quick) })));
                });
            }
            Route::Call {
                id,
                name,
                arguments,
            } => {
                let (offer, relays, calls) =
                    (Arc::clone(&offer), Arc::clone(&relays), Arc::clone(&calls));
                calls.start(&id, &name);
                // A call waits on the app, so it runs off the read loop; a host
                // that pipelines a ping behind a navigation still gets answered.
                std::thread::spawn(move || {
                    let mut answer =
                        call(&offer, &*relays.quick, &*relays.patient, &name, &arguments);
                    if calls.finish(&id) {
                        if let Some(blocks) = answer["content"].as_array_mut() {
                            if let Some(news) = offer.take_guide_news() {
                                blocks.push(text(&news));
                            }
                        }
                        emit(&reply(id, answer));
                    }
                });
            }
        }
    }
}

/// Tells the agent its tools changed when the person signs in to a plugin, or
/// switches one on or off, while it runs, and when a different Sikemux, such
/// as an updated one, takes over the endpoint. It first asks as this server
/// starts, waiting out a Sikemux that is restarting.
fn watch(offer: &Offer, patient: &Relay<'_>, quick: &Relay<'_>) {
    let mut seen: Option<u64> = None;
    let mut instance = None;
    loop {
        let now = harness::instance();
        if now != instance {
            seen = None;
            let (changed, served) = offer.refresh(patient);
            if changed {
                tell_tools_changed();
            }
            if served {
                instance = now;
            }
        }
        match next_plugin_change(offer, quick, seen) {
            Ok((version, changed)) => {
                if changed {
                    tell_tools_changed();
                }
                seen = Some(version);
            }
            Err(_) => std::thread::sleep(WATCH_RETRY),
        }
    }
}

fn tell_tools_changed() {
    emit(&json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }));
}

/// Waits for plugins to change after the `seen`th change, then asks for the
/// tools again. Answers with the change it got to, and whether the tools differ.
/// Without a change seen yet there is nothing to compare, since the tools were
/// asked for when this Sikemux was first seen.
fn next_plugin_change(
    offer: &Offer,
    relay: &Relay<'_>,
    seen: Option<u64>,
) -> Result<(u64, bool), String> {
    let version = relay(PLUGINS_CHANGED_METHOD, &json!({ "seen": seen }))?["version"]
        .as_u64()
        .ok_or("Sikemux did not say how many times plugins changed")?;
    if seen.is_none_or(|seen| seen == version) {
        return Ok((version, false));
    }
    Ok((version, offer.refresh(relay).0))
}

/// Tool calls still running, by request id. A call the host cancelled is
/// not answered, as MCP asks.
#[derive(Default)]
struct Calls(Mutex<HashMap<String, (String, bool)>>);

impl Calls {
    fn start(&self, id: &Value, name: &str) {
        self.0
            .locked()
            .insert(id.to_string(), (name.to_owned(), false));
    }

    /// Marks the call cancelled, and names its tool.
    fn cancel(&self, id: &Value) -> Option<String> {
        let mut calls = self.0.locked();
        let (name, cancelled) = calls.get_mut(&id.to_string())?;
        *cancelled = true;
        Some(name.clone())
    }

    /// Whether the finished call should still be answered.
    fn finish(&self, id: &Value) -> bool {
        self.0
            .locked()
            .remove(&id.to_string())
            .is_none_or(|(_, cancelled)| !cancelled)
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

fn list(offer: &Offer, relay: &Relay<'_>) -> Vec<Value> {
    let tools = declarations(&offer.built_in(relay), &offer.plugin_tools(relay));
    *offer.given.locked() = Some(tools.clone());
    tools
}

/// `quick` asks for the tools, `patient` makes the call.
fn call(
    offer: &Offer,
    quick: &Relay<'_>,
    patient: &Relay<'_>,
    name: &str,
    arguments: &Value,
) -> Value {
    if name == offer.manifest().guide_name() {
        let topic = arguments.get("topic").and_then(Value::as_str);
        return match offer.manifest().guide_text(topic) {
            Ok(guide) => content(vec![text(guide)], false),
            Err(message) => invalid(&message),
        };
    }
    let manifest = offer.built_in(quick);
    if let Some(tool) = manifest.tool(name) {
        if let Err(message) = tool.validate(arguments) {
            return invalid(&message);
        }
        return answer(tool, patient(&tool.method, arguments));
    }
    let plugins = offer.plugin_tools(quick);
    let Some(tool) = plugins
        .iter()
        .find(|tool| tool.name == name && !manifest.declares(name))
    else {
        return content(vec![text(&format!("Unknown tool: {name}"))], true);
    };
    if let Err(message) = tool.validate(arguments) {
        return invalid(&message);
    }
    answer(
        tool,
        patient(
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

fn answer(tool: &Tool, result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => content(content_for(tool.answer.as_deref(), &value), false),
        Err(message) => content(vec![text(&message)], true),
    }
}

/// A tool whose answer is a picture says so in the manifest. A `screenshot`
/// travels as an image block with the page's name beside it; any other kind,
/// including ones newer than this binary, as [`picture`].
fn content_for(kind: Option<&str>, value: &Value) -> Vec<Value> {
    if kind.is_none() {
        return vec![text(&value.to_string())];
    }
    if kind == Some("screenshot") {
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
    picture(value)
}

/// The image in an answer, with the rest of its fields beside it. An answer
/// that is not a picture is shown without its data, which would only spend
/// the agent's context.
fn picture(value: &Value) -> Vec<Value> {
    let Some(data) = value.get("data").and_then(Value::as_str) else {
        return vec![text(&value.to_string())];
    };
    let mime_type = value.get("mimeType").and_then(Value::as_str);
    let mut rest = value.clone();
    if let Some(fields) = rest.as_object_mut() {
        fields.remove("data");
        fields.remove("mimeType");
    }
    match mime_type {
        Some(mime_type) if mime_type.starts_with("image/") => vec![
            json!({ "type": "image", "data": data, "mimeType": mime_type }),
            text(&rest.to_string()),
        ],
        _ => vec![text(&format!(
            "{rest} ({} bytes of {} left out)",
            data.len(),
            mime_type.unwrap_or("data")
        ))],
    }
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
