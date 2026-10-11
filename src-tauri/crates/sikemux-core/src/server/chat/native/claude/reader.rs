//! Reads what Claude Code writes on its output and tells the chat, as the
//! ACP session updates the app reads from every agent.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::task::AbortHandle;

use crate::acp::account::{Failure, FailureKind};

use super::super::session::{Sink, TurnEnd};
use super::control::Control;
use super::permission;
use super::tools::{self, TaskPlan};

/// A child session's updates are held until it is announced, but only so many.
const MAX_HELD_PER_CHILD: usize = 64;

/// Text Claude Code shows instead of an answer when the account is out of
/// usage, taken from the Agent SDK.
const USAGE_LIMIT_PREFIXES: [&str; 12] = [
    "You've hit your",
    "You've reached your",
    "You're out of usage credits",
    "Your org is out of usage · add funds to continue",
    "Your org is out of usage · contact your admin",
    "Your seat type doesn't include usage credits",
    "Your seat type doesn't include usage",
    "Your usage allocation has been disabled by your admin",
    "Your group's usage limit is set to $0",
    "Fable 5 requires usage credits",
    "You're out of extra usage",
    "Your seat type doesn't include extra usage",
];

/// Slash commands that only make sense in Claude Code's own terminal.
const TERMINAL_COMMANDS: [&str; 8] = [
    "clear",
    "cost",
    "keybindings-help",
    "login",
    "logout",
    "output-style:new",
    "release-notes",
    "todos",
];

/// The turn a prompt from this side started, as the reader and the driver
/// both see it.
pub(crate) struct Turn {
    pub number: u64,
    /// The last message sent into the turn: its prompt, or a message steered
    /// into it since. The result that answers it ends the turn.
    pub latest: String,
    pub cancelled: bool,
    pub failure: Option<Failure>,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub turn: Option<Turn>,
    /// Every message this side wrote, whose echoes are not shown again.
    pub sent: HashSet<String>,
    /// Claude Code's permission mode for the chat, which an approved plan
    /// goes back to.
    pub mode: String,
    /// What the chat's first message said, until Claude has named the chat
    /// from it.
    pub untitled: Option<String>,
}

struct Tool {
    name: String,
    input: Value,
    session: String,
}

struct Child {
    session_id: String,
    ended: bool,
}

struct Task {
    name: String,
    ended: bool,
}

pub(crate) struct Reader {
    sink: Sink,
    control: Control,
    cwd: PathBuf,
    session_id: String,
    shared: Arc<Mutex<Shared>>,
    /// Text and thinking streamed for the current top-level message, which
    /// the whole message repeats when it lands.
    streamed: Vec<(u64, &'static str, String)>,
    stream_message_id: Option<String>,
    tools: HashMap<String, Tool>,
    shown: HashSet<String>,
    /// Subagents by the tool call that spawned them.
    children: HashMap<String, Child>,
    generations: HashMap<String, u32>,
    held: HashMap<String, Vec<Value>>,
    tasks: HashMap<String, Task>,
    compaction: Option<String>,
    /// The list Claude's task tools keep, shown as the chat's plan.
    task_plan: TaskPlan,
    used: Option<u64>,
    size: u64,
    model: Option<String>,
    permissions: Arc<Mutex<HashMap<String, AbortHandle>>>,
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn usage_total(usage: &Value) -> Option<u64> {
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    usage.is_object().then(|| {
        field("input_tokens")
            + field("output_tokens")
            + field("cache_read_input_tokens")
            + field("cache_creation_input_tokens")
    })
}

/// A model with a million-token window says so in its name.
fn window_from_name(model: &str) -> Option<u64> {
    model
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == "1m")
        .then_some(1_000_000)
}

/// The commands the chat offers, from the list Claude Code sends.
pub(crate) fn available_commands(commands: &Value) -> Value {
    let commands: Vec<Value> = commands
        .as_array()
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    let name = text(command, "name")?;
                    if TERMINAL_COMMANDS.contains(&name) {
                        return None;
                    }
                    let name = match name.strip_suffix(" (MCP)") {
                        Some(name) => format!("mcp:{name}"),
                        None => name.to_owned(),
                    };
                    let hint = match command.get("argumentHint") {
                        Some(Value::String(hint)) if !hint.is_empty() => Some(hint.clone()),
                        Some(Value::Array(parts)) if !parts.is_empty() => Some(
                            parts
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                        _ => None,
                    };
                    Some(json!({
                        "name": name,
                        "description": command.get("description").and_then(Value::as_str).unwrap_or_default(),
                        "input": hint.map(|hint| json!({ "hint": hint })),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "sessionUpdate": "available_commands_update", "availableCommands": commands })
}

/// Why a turn failed, from an assistant message Claude Code wrote in place
/// of an answer.
fn failure_of(message: &Value) -> Option<Failure> {
    let inner = message.get("message")?;
    let said: String = inner
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| text(block, "text"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let synthetic = text(inner, "model") == Some("<synthetic>");
    let title = |fallback: &str| {
        if said.trim().is_empty() {
            fallback.to_owned()
        } else {
            said.trim().to_owned()
        }
    };
    if synthetic && said.contains("Please run /login") {
        return Some(Failure::sign_in(title("Sign in to continue using Claude.")));
    }
    if synthetic
        && USAGE_LIMIT_PREFIXES
            .iter()
            .any(|prefix| said.trim_start().starts_with(prefix))
    {
        return Some(Failure {
            kind: FailureKind::Limit,
            title: title("The Claude account has no available quota."),
            details: None,
        });
    }
    let error = text(message, "error")?;
    let kind = match error {
        "authentication_failed" | "oauth_org_not_allowed" => FailureKind::SignIn,
        "billing_error" | "account_on_hold" | "rate_limit" => FailureKind::Limit,
        _ => FailureKind::Other,
    };
    Some(Failure {
        kind,
        title: title("The model provider reported an error."),
        details: None,
    })
}

fn stop_reason(result: &Value) -> &'static str {
    match (text(result, "stop_reason"), text(result, "subtype")) {
        (Some("refusal"), _) => "refusal",
        (Some("max_tokens"), _) => "max_tokens",
        (
            _,
            Some(
                "error_max_turns" | "error_max_budget_usd" | "error_max_structured_output_retries",
            ),
        ) => "max_turn_requests",
        _ => "end_turn",
    }
}

/// Strips the tags Claude Code wraps a local command's output in.
fn local_output(content: &str) -> Option<String> {
    if !content.contains("<local-command-stdout>") {
        return None;
    }
    let output = content
        .replace("<local-command-stdout>", "")
        .replace("</local-command-stdout>", "");
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_owned())
}

fn task_state(status: &str) -> Option<&'static str> {
    Some(match status {
        "pending" | "running" => "running",
        "paused" => "paused",
        "completed" => "completed",
        "failed" => "failed",
        "killed" | "cancelled" | "stopped" => "stopped",
        _ => return None,
    })
}

fn subagent_state(status: &str) -> Option<&'static str> {
    Some(match status {
        "completed" => "completed",
        "failed" => "failed",
        "killed" | "cancelled" | "stopped" => "cancelled",
        "disconnected" => "disconnected",
        _ => return None,
    })
}

fn task_type(kind: Option<&str>) -> String {
    match kind {
        Some("local_bash") => "shell".into(),
        Some("local_workflow") => "workflow".into(),
        Some("local_monitor" | "mcp") => "monitor".into(),
        Some(other) => other.into(),
        None => "task".into(),
    }
}

impl Reader {
    pub fn new(
        sink: Sink,
        control: Control,
        cwd: PathBuf,
        session_id: String,
        shared: Arc<Mutex<Shared>>,
    ) -> Self {
        Self {
            sink,
            control,
            cwd,
            session_id,
            shared,
            streamed: Vec::new(),
            stream_message_id: None,
            tools: HashMap::new(),
            shown: HashSet::new(),
            children: HashMap::new(),
            generations: HashMap::new(),
            held: HashMap::new(),
            tasks: HashMap::new(),
            compaction: None,
            task_plan: TaskPlan::default(),
            used: None,
            size: 200_000,
            model: None,
            permissions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn note_model(&mut self, model: &str) {
        if let Some(size) = window_from_name(model) {
            self.size = size;
        }
    }

    fn turn_running(&self) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.turn.is_some())
            .unwrap_or(false)
    }

    /// Sends `update` to the session `parent` names: the chat's own, or the
    /// subagent a tool call spawned. A subagent not yet announced holds it.
    fn send(&mut self, parent: Option<&str>, update: Value) {
        let Some(parent) = parent else {
            if !self.turn_running() {
                self.sink.work_started();
            }
            self.sink.update(&self.session_id, update);
            return;
        };
        match self.children.get(parent) {
            Some(child) if child.ended => {}
            Some(child) => self.sink.update(&child.session_id, update),
            None => {
                let held = self.held.entry(parent.to_owned()).or_default();
                if held.len() < MAX_HELD_PER_CHILD {
                    held.push(update);
                }
            }
        }
    }

    /// The session a tool call spawned by `parent` belongs to.
    fn session_of(&self, parent: Option<&str>) -> String {
        parent
            .and_then(|parent| self.children.get(parent))
            .map_or_else(|| self.session_id.clone(), |child| child.session_id.clone())
    }

    pub fn handle(&mut self, message: Value) {
        match message.get("type").and_then(Value::as_str) {
            Some("control_response") => self.control.answered(&message),
            Some("control_request") => self.request(&message),
            Some("control_cancel_request") => {
                if let Some(id) = text(&message, "request_id") {
                    let handle = self
                        .permissions
                        .lock()
                        .ok()
                        .and_then(|mut permissions| permissions.remove(id));
                    if let Some(handle) = handle {
                        handle.abort();
                    }
                }
            }
            Some("stream_event") => self.stream_event(&message),
            Some("assistant") => self.assistant(&message),
            Some("user") => self.user(&message),
            Some("result") => self.result(&message),
            Some("system") => self.system(&message),
            Some("conversation_reset") => {
                self.task_plan.clear();
                self.send(None, self.task_plan.plan());
            }
            _ => {}
        }
    }

    fn request(&mut self, message: &Value) {
        let Some(request_id) = text(message, "request_id").map(str::to_owned) else {
            return;
        };
        let request = message.get("request").cloned().unwrap_or(Value::Null);
        match text(&request, "subtype") {
            Some("can_use_tool") => self.can_use_tool(request_id, request),
            Some("hook_callback") => self
                .control
                .respond(&request_id, json!({ "continue": true })),
            _ => self
                .control
                .refuse(&request_id, "Sikemux does not handle this request"),
        }
    }

    fn can_use_tool(&mut self, request_id: String, request: Value) {
        let tool_id = text(&request, "tool_use_id")
            .unwrap_or(&request_id)
            .to_owned();
        let name = text(&request, "tool_name").unwrap_or("Tool").to_owned();
        let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
        let parent = self
            .tools
            .get(&tool_id)
            .map(|tool| tool.session.clone())
            .filter(|session| *session != self.session_id);
        let session = parent.unwrap_or_else(|| self.session_id.clone());
        if !self.shown.contains(&tool_id) && !tools::is_plan_tool(&name) {
            self.shown.insert(tool_id.clone());
            self.tools.entry(tool_id.clone()).or_insert(Tool {
                name: name.clone(),
                input: input.clone(),
                session: session.clone(),
            });
            self.sink.update(
                &session,
                tools::tool_call(&tool_id, &name, &input, &self.cwd),
            );
        }
        let mode = self
            .shared
            .lock()
            .map(|shared| shared.mode.clone())
            .unwrap_or_default();
        let asked = permission::request(
            &session, &tool_id, &name, &input, &request, &self.cwd, &mode,
        );
        let sink = self.sink.clone();
        let control = self.control.clone();
        let permissions = self.permissions.clone();
        let id = request_id.clone();
        let task = tokio::spawn(async move {
            let chosen = sink.ask(asked).await;
            if let Ok(mut permissions) = permissions.lock() {
                permissions.remove(&id);
            }
            control.respond(
                &id,
                permission::answer(&tool_id, &input, &request, chosen.as_deref()),
            );
        });
        if let Ok(mut permissions) = self.permissions.lock() {
            permissions.insert(request_id, task.abort_handle());
        }
    }

    fn stream_event(&mut self, message: &Value) {
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        let Some(event) = message.get("event") else {
            return;
        };
        match text(event, "type") {
            Some("message_start") => {
                self.stream_message_id = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if parent.is_none() {
                    self.streamed.clear();
                    if let Some(model) = event.pointer("/message/model").and_then(Value::as_str) {
                        self.model = Some(model.to_owned());
                        self.note_model(model);
                    }
                    if let Some(used) = event.pointer("/message/usage").and_then(usage_total) {
                        self.used = Some(used);
                    }
                }
            }
            Some("content_block_start") => {
                let Some(block) = event.get("content_block") else {
                    return;
                };
                if matches!(
                    text(block, "type"),
                    Some("tool_use" | "server_tool_use" | "mcp_tool_use")
                ) {
                    self.tool_use(block, parent.as_deref(), true);
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                let (kind, chunk) = match text(delta, "type") {
                    Some("text_delta") => ("text", text(delta, "text")),
                    Some("thinking_delta") => ("thinking", text(delta, "thinking")),
                    _ => return,
                };
                let Some(chunk) = chunk.map(str::to_owned) else {
                    return;
                };
                if parent.is_none() {
                    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                    match self.streamed.last_mut() {
                        Some((last, last_kind, streamed))
                            if *last == index && *last_kind == kind =>
                        {
                            streamed.push_str(&chunk)
                        }
                        _ => self.streamed.push((index, kind, chunk.clone())),
                    }
                }
                let block = if kind == "text" {
                    json!({ "type": "text", "text": chunk })
                } else {
                    json!({ "type": "thinking", "thinking": chunk })
                };
                let message_id = self.stream_message_id.clone();
                if let Some(update) =
                    tools::content_chunk(&block, "assistant", message_id.as_deref())
                {
                    self.send(parent.as_deref(), update);
                }
            }
            Some("message_delta") if parent.is_none() => {
                if let Some(usage) = event.get("usage").filter(|usage| usage.is_object()) {
                    if let (Some(used), Some(output)) = (
                        self.used,
                        usage.get("output_tokens").and_then(Value::as_u64),
                    ) {
                        self.used = Some(used.max(output));
                    }
                }
            }
            _ => {}
        }
    }

    fn tool_use(&mut self, block: &Value, parent: Option<&str>, streaming: bool) {
        let Some(id) = text(block, "id").map(str::to_owned) else {
            return;
        };
        let name = text(block, "name").unwrap_or("Tool").to_owned();
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let session = self.session_of(parent);
        let known = self.tools.contains_key(&id);
        if !streaming || !known {
            self.tools.insert(
                id.clone(),
                Tool {
                    name: name.clone(),
                    input: input.clone(),
                    session,
                },
            );
        }
        if tools::is_plan_tool(&name) {
            if parent.is_none() && !streaming {
                if let Some(plan) = tools::todo_plan(&input) {
                    self.send(None, plan);
                }
            }
            return;
        }
        if tools::is_subagent_tool(&name) {
            return;
        }
        if self.shown.insert(id.clone()) {
            let update = tools::tool_call(&id, &name, &input, &self.cwd);
            self.send(parent, update);
        } else if !streaming {
            let update = tools::tool_call_refined(&id, &name, &input, &self.cwd);
            self.send(parent, update);
        }
    }

    fn assistant(&mut self, message: &Value) {
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        if parent.is_none() {
            if let Some(failure) = failure_of(message) {
                if let Ok(mut shared) = self.shared.lock() {
                    if let Some(turn) = shared.turn.as_mut() {
                        turn.failure = Some(failure);
                        return;
                    }
                }
            }
        }
        let Some(inner) = message.get("message") else {
            return;
        };
        if parent.is_none() {
            if let Some(model) = text(inner, "model").filter(|model| *model != "<synthetic>") {
                self.model = Some(model.to_owned());
                self.note_model(model);
            }
            if let Some(used) = inner.get("usage").and_then(usage_total) {
                self.used = Some(used);
            }
        }
        let message_id = text(inner, "id").map(str::to_owned);
        let Some(blocks) = inner.get("content").and_then(Value::as_array) else {
            return;
        };
        let mut position = 0;
        for block in blocks {
            match text(block, "type") {
                Some(kind @ ("text" | "thinking")) => {
                    let field = if kind == "text" { "text" } else { "thinking" };
                    let Some(full) = text(block, field) else {
                        continue;
                    };
                    let mut rest = full.to_owned();
                    if parent.is_none() {
                        if let Some((_, streamed_kind, streamed)) = self.streamed.get(position) {
                            if *streamed_kind == kind
                                && !streamed.is_empty()
                                && full.starts_with(streamed.as_str())
                            {
                                position += 1;
                                rest = full[streamed.len()..].to_owned();
                            }
                        }
                    }
                    if rest.is_empty() {
                        continue;
                    }
                    let mut block = block.clone();
                    block[field] = json!(rest);
                    if let Some(update) =
                        tools::content_chunk(&block, "assistant", message_id.as_deref())
                    {
                        self.send(parent.as_deref(), update);
                    }
                }
                Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                    self.tool_use(block, parent.as_deref(), false)
                }
                Some("image") => {
                    if let Some(update) =
                        tools::content_chunk(block, "assistant", message_id.as_deref())
                    {
                        self.send(parent.as_deref(), update);
                    }
                }
                _ => {}
            }
        }
        if parent.is_none() {
            self.streamed.clear();
        }
    }

    fn user(&mut self, message: &Value) {
        if let Some(uuid) = text(message, "uuid") {
            if self
                .shared
                .lock()
                .map(|shared| shared.sent.contains(uuid))
                .unwrap_or(false)
            {
                return;
            }
        }
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        let content = message.pointer("/message/content");
        let blocks = match content {
            Some(Value::String(said)) => {
                if let Some(output) = local_output(said) {
                    self.local_output(parent.as_deref(), output);
                }
                return;
            }
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => return,
        };
        let results: Vec<&Value> = blocks
            .iter()
            .filter(|block| {
                text(block, "type")
                    .is_some_and(|kind| kind == "tool_result" || kind.ends_with("_tool_result"))
            })
            .collect();
        if results.is_empty() {
            if let [only] = blocks.as_slice() {
                if let Some(output) = text(only, "text").and_then(local_output) {
                    self.local_output(parent.as_deref(), output);
                }
            }
            return;
        }
        let structured = (results.len() == 1)
            .then(|| message.get("tool_use_result"))
            .flatten();
        for result in results {
            let Some(id) = text(result, "tool_use_id").map(str::to_owned) else {
                continue;
            };
            let Some(tool) = self.tools.get(&id) else {
                continue;
            };
            let (name, input, session) =
                (tool.name.clone(), tool.input.clone(), tool.session.clone());
            if tools::is_plan_tool(&name) {
                if parent.is_none() {
                    if let Some(plan) = self
                        .task_plan
                        .apply_result(&name, &input, result, structured)
                    {
                        self.send(None, plan);
                    }
                }
                continue;
            }
            if tools::is_subagent_tool(&name) {
                let failed = result.get("is_error").and_then(Value::as_bool) == Some(true);
                if failed && !self.children.contains_key(&id) {
                    let mut update = tools::tool_call(&id, &name, &input, &self.cwd);
                    let completed =
                        tools::tool_result(&id, &name, &input, result, structured, &self.cwd);
                    if let (Some(update), Some(completed)) =
                        (update.as_object_mut(), completed.as_object())
                    {
                        for (key, value) in completed {
                            if key != "sessionUpdate" {
                                update.insert(key.clone(), value.clone());
                            }
                        }
                    }
                    self.sink.update(&session, update);
                }
                continue;
            }
            let update = tools::tool_result(&id, &name, &input, result, structured, &self.cwd);
            self.sink.update(&session, update);
            self.background_shell(&id, &input, result);
        }
    }

    /// What a slash command the person ran printed. Claude Code also says so
    /// when the chat changes its model or settings between turns, and that is
    /// no reply to anything, nor the agent starting work of its own.
    fn local_output(&mut self, parent: Option<&str>, output: String) {
        if !self.turn_running() {
            return;
        }
        self.send(
            parent,
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": output } }),
        );
    }

    /// A shell command sent to the background answers with where its output
    /// goes, and runs on as a task.
    fn background_shell(&mut self, tool_id: &str, input: &Value, result: &Value) {
        let said = match result.get("content") {
            Some(Value::String(said)) => said.clone(),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter_map(|block| text(block, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => return,
        };
        let Some(rest) = said.split("Command running in background with ID: ").nth(1) else {
            return;
        };
        let Some(task_id) = rest
            .split(|ch: char| ch == '.' || ch.is_whitespace())
            .next()
        else {
            return;
        };
        if task_id.is_empty() || self.tasks.contains_key(task_id) {
            return;
        }
        let output = said
            .split("Output is being written to: ")
            .nth(1)
            .and_then(|rest| rest.split(". You will be notified").next())
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let description = text(input, "command").unwrap_or("Shell").to_owned();
        self.tasks.insert(
            task_id.to_owned(),
            Task {
                name: description.clone(),
                ended: false,
            },
        );
        let mut update = json!({
            "sessionUpdate": "async_task_spawned",
            "asyncTaskId": task_id,
            "name": description,
            "taskType": "shell",
            "description": description,
            "showInTranscript": true,
            "canStop": true,
            "toolCallId": tool_id,
        });
        if let Some(output) = output {
            update["outputFilePath"] = json!(output);
        }
        self.sink.update(&self.session_id, update);
    }

    fn result(&mut self, result: &Value) {
        if let Some(size) = result
            .get("modelUsage")
            .and_then(Value::as_object)
            .and_then(|usage| self.window_of(usage))
        {
            self.size = size;
        }
        let used = self.used;
        if let Some(used) = used {
            let mut update =
                json!({ "sessionUpdate": "usage_update", "used": used, "size": self.size });
            if let Some(cost) = result.get("total_cost_usd").and_then(Value::as_f64) {
                update["cost"] = json!({ "amount": cost, "currency": "USD" });
            }
            self.sink.update(&self.session_id, update);
        }
        let answers = text(result, "user_message_uuid");
        let ended = {
            let Ok(mut shared) = self.shared.lock() else {
                return;
            };
            let ends = shared
                .turn
                .as_ref()
                .is_some_and(|turn| answers.is_none_or(|uuid| uuid == turn.latest));
            if ends {
                shared.turn.take()
            } else {
                None
            }
        };
        let Some(turn) = ended else {
            if !self.turn_running() {
                self.sink.work_ended("end_turn");
            }
            return;
        };
        let end = if turn.cancelled {
            TurnEnd::Completed(json!({ "stopReason": "cancelled" }))
        } else if let Some(failure) = turn.failure {
            TurnEnd::Failed {
                failure,
                response: Some(json!({ "stopReason": "end_turn" })),
            }
        } else if result.get("is_error").and_then(Value::as_bool) == Some(true) {
            let said = text(result, "result")
                .map(str::to_owned)
                .or_else(|| {
                    result
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|errors| {
                            errors
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                })
                .filter(|said| !said.is_empty())
                .unwrap_or_else(|| "Claude could not finish this turn".to_owned());
            let kind = if said.contains("Please run /login") {
                FailureKind::SignIn
            } else {
                FailureKind::Other
            };
            TurnEnd::Failed {
                failure: Failure {
                    kind,
                    title: said,
                    details: None,
                },
                response: Some(json!({ "stopReason": "end_turn" })),
            }
        } else {
            TurnEnd::Completed(json!({ "stopReason": stop_reason(result) }))
        };
        self.sink.end_turn(turn.number, end);
        self.name_chat();
    }

    /// Asks Claude for a short name for a new chat once its first turn is
    /// over, as Claude Code names its own.
    fn name_chat(&self) {
        let Some(first) = self
            .shared
            .lock()
            .ok()
            .and_then(|mut shared| shared.untitled.take())
        else {
            return;
        };
        let description: String = first.chars().take(1_000).collect();
        if description.trim().chars().count() < 10 {
            return;
        }
        let control = self.control.clone();
        let sink = self.sink.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let named = control
                .request(json!({ "subtype": "generate_session_title", "description": description, "persist": true }))
                .await;
            let title = named
                .ok()
                .and_then(|named| {
                    named
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|title| !title.is_empty());
            if let Some(title) = title {
                let title: String = title.chars().take(256).collect();
                sink.update(
                    &session_id,
                    json!({ "sessionUpdate": "session_info_update", "title": title }),
                );
            }
        });
    }

    fn window_of(&self, usage: &Map<String, Value>) -> Option<u64> {
        let model = self.model.as_deref().unwrap_or_default();
        let common = |key: &str| {
            key.chars()
                .zip(model.chars())
                .take_while(|(a, b)| a == b)
                .count()
        };
        usage
            .iter()
            .max_by_key(|(key, _)| common(key))
            .and_then(|(_, entry)| entry.get("contextWindow").and_then(Value::as_u64))
            .filter(|size| *size > 0)
    }

    fn system(&mut self, message: &Value) {
        match text(message, "subtype") {
            Some("session_state_changed") if text(message, "state") == Some("idle") => self.idle(),
            Some("status") => self.status(message),
            Some("compact_boundary") => {
                if let Some(used) = message
                    .pointer("/compact_metadata/post_tokens")
                    .and_then(Value::as_u64)
                {
                    self.used = Some(used);
                }
                self.compacted("completed", None);
            }
            Some("task_started") => self.task_started(message),
            Some("task_progress") => self.task_progress(message),
            Some("task_updated") => {
                let Some(id) = text(message, "task_id").map(str::to_owned) else {
                    return;
                };
                let patch = message.get("patch").cloned().unwrap_or(Value::Null);
                if let Some(status) = text(&patch, "status") {
                    self.task_ended(&id, status, None, text(&patch, "output_file"));
                }
            }
            Some("task_notification") => {
                let Some(id) = text(message, "task_id").map(str::to_owned) else {
                    return;
                };
                let status = text(message, "status").unwrap_or("completed").to_owned();
                self.task_ended(
                    &id,
                    &status,
                    text(message, "summary"),
                    text(message, "output_file"),
                );
            }
            Some("background_tasks_changed") => {
                let live: HashSet<&str> = message
                    .get("tasks")
                    .and_then(Value::as_array)
                    .map(|tasks| {
                        tasks
                            .iter()
                            .filter_map(|task| text(task, "task_id"))
                            .collect()
                    })
                    .unwrap_or_default();
                let gone: Vec<String> = self
                    .tasks
                    .iter()
                    .filter(|(id, task)| !task.ended && !live.contains(id.as_str()))
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in gone {
                    self.task_ended(&id, "stopped", None, None);
                }
            }
            Some("informational") if text(message, "level") == Some("warning") => {
                if let Some(content) = text(message, "content") {
                    self.notice("warning", content);
                }
            }
            Some("model_refusal_fallback") => {
                let original = text(message, "original_model").unwrap_or("The model");
                let fallback = text(message, "fallback_model").unwrap_or("another model");
                self.notice(
                    "warning",
                    &format!("{original} declined this request; retried with {fallback}."),
                );
            }
            Some("commands_changed") => {
                if let Some(commands) = message.get("commands") {
                    self.sink
                        .update(&self.session_id, available_commands(commands));
                }
            }
            Some("local_command_output") => {
                if let Some(content) = text(message, "content") {
                    self.send(
                        None,
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": content } }),
                    );
                }
            }
            Some("permission_denied") => {
                let Some(id) = text(message, "tool_use_id") else {
                    return;
                };
                if !self.shown.contains(id) {
                    return;
                }
                let reason = text(message, "decision_reason")
                    .or_else(|| text(message, "message"))
                    .unwrap_or("not allowed");
                let session = self
                    .tools
                    .get(id)
                    .map_or_else(|| self.session_id.clone(), |tool| tool.session.clone());
                self.sink.update(
                    &session,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": id,
                        "status": "failed",
                        "content": [{ "type": "content", "content": { "type": "text", "text": format!("Permission denied: {reason}") } }],
                    }),
                );
            }
            _ => {}
        }
    }

    fn notice(&mut self, severity: &str, content: &str) {
        let cleaned: String = content
            .replace("**", "")
            .replace("__", "")
            .replace('`', "")
            .lines()
            .map(|line| line.trim_start_matches('#').trim())
            .collect::<Vec<_>>()
            .join("\n");
        let cleaned = cleaned.trim();
        let (title, description) = match cleaned.split_once('\n') {
            Some((title, rest)) if title.len() <= 256 => {
                (title.to_owned(), Some(rest.trim().to_owned()))
            }
            _ if cleaned.len() <= 256 => (cleaned.to_owned(), None),
            _ => (
                "Claude reported a warning".to_owned(),
                Some(cleaned.to_owned()),
            ),
        };
        let mut update = json!({ "sessionUpdate": "notice", "severity": severity, "title": title });
        if let Some(description) = description.filter(|description| !description.is_empty()) {
            update["description"] = json!(description);
        }
        self.sink.update(&self.session_id, update);
    }

    /// Claude Code went idle: anything the turn still waited on is over.
    fn idle(&mut self) {
        let turn = self
            .shared
            .lock()
            .ok()
            .and_then(|mut shared| shared.turn.take());
        match turn {
            Some(turn) => {
                let end = match turn.failure {
                    Some(failure) if !turn.cancelled => TurnEnd::Failed {
                        failure,
                        response: Some(json!({ "stopReason": "end_turn" })),
                    },
                    _ => TurnEnd::Completed(json!({
                        "stopReason": if turn.cancelled { "cancelled" } else { "end_turn" },
                    })),
                };
                self.sink.end_turn(turn.number, end);
            }
            None => self.sink.work_ended("end_turn"),
        }
    }

    fn status(&mut self, message: &Value) {
        if text(message, "status") == Some("compacting") && self.compaction.is_none() {
            let id = text(message, "uuid")
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.compaction = Some(id.clone());
            self.send(
                None,
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": id,
                    "title": "Compact conversation",
                    "kind": "think",
                    "status": "in_progress",
                }),
            );
        }
        match text(message, "compact_result") {
            Some("success") => self.compacted("completed", None),
            Some("failed") => {
                let error = text(message, "compact_error")
                    .unwrap_or("unknown error")
                    .to_owned();
                self.compacted("failed", Some(format!("Compaction failed: {error}")));
            }
            _ => {}
        }
    }

    fn compacted(&mut self, status: &str, error: Option<String>) {
        let Some(id) = self.compaction.take() else {
            return;
        };
        let mut update =
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status });
        if let Some(error) = error {
            update["content"] =
                json!([{ "type": "content", "content": { "type": "text", "text": error } }]);
        }
        self.sink.update(&self.session_id, update);
    }

    fn task_started(&mut self, message: &Value) {
        let Some(task_id) = text(message, "task_id").map(str::to_owned) else {
            return;
        };
        let kind = text(message, "task_type");
        let spawner = text(message, "tool_use_id").map(str::to_owned);
        if text(message, "subagent_type").is_some() || kind == Some("local_agent") {
            if let Some(spawner) = spawner {
                self.spawn_child(&task_id, &spawner, message);
            }
            return;
        }
        let backgrounded = match message.get("is_backgrounded").and_then(Value::as_bool) {
            Some(backgrounded) => backgrounded,
            None => !matches!(kind, Some("local_bash" | "local_agent")),
        };
        if !backgrounded || self.tasks.contains_key(&task_id) {
            return;
        }
        let task_type = task_type(kind);
        let fallback = match task_type.as_str() {
            "shell" => "Shell",
            "workflow" => "Workflow",
            "monitor" => "Monitor",
            _ => "Background task",
        };
        let description = text(message, "description").unwrap_or(fallback).to_owned();
        let name = text(message, "workflow_name")
            .unwrap_or(&description)
            .to_owned();
        self.tasks.insert(
            task_id.clone(),
            Task {
                name: name.clone(),
                ended: false,
            },
        );
        let mut update = json!({
            "sessionUpdate": "async_task_spawned",
            "asyncTaskId": task_id,
            "name": name,
            "taskType": task_type,
            "description": description,
            "showInTranscript": message.get("skip_transcript").and_then(Value::as_bool) != Some(true),
            "canStop": true,
        });
        if let Some(spawner) = spawner {
            update["toolCallId"] = json!(spawner);
        }
        self.sink.update(&self.session_id, update);
    }

    fn spawn_child(&mut self, task_id: &str, spawner: &str, message: &Value) {
        if self.children.contains_key(spawner) {
            return;
        }
        let generation = self.generations.entry(task_id.to_owned()).or_insert(0);
        *generation += 1;
        let session_id = if *generation == 1 {
            task_id.to_owned()
        } else {
            format!("{task_id}:generation:{generation}")
        };
        let (input, parent_session) = self.tools.get(spawner).map_or_else(
            || (Value::Null, self.session_id.clone()),
            |tool| (tool.input.clone(), tool.session.clone()),
        );
        let first = |values: &[Option<&str>]| {
            values
                .iter()
                .flatten()
                .map(|value| value.trim())
                .find(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let name = first(&[
            text(&input, "name"),
            text(&input, "description"),
            text(&input, "subagent_type"),
            text(message, "description"),
            text(message, "subagent_type"),
        ])
        .unwrap_or_else(|| {
            let tail: String = task_id
                .chars()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("Agent {tail}")
        });
        let task = first(&[
            text(&input, "prompt"),
            text(&input, "description"),
            text(message, "prompt"),
            text(message, "description"),
        ])
        .unwrap_or_else(|| "Delegated task".to_owned());
        self.sink.update(
            &parent_session,
            json!({
                "sessionUpdate": "subagent_spawned",
                "subagentSessionId": session_id,
                "name": name,
                "task": task,
                "capabilities": {},
            }),
        );
        self.children.insert(
            spawner.to_owned(),
            Child {
                session_id: session_id.clone(),
                ended: false,
            },
        );
        for update in self.held.remove(spawner).unwrap_or_default() {
            self.sink.update(&session_id, update);
        }
    }

    fn task_progress(&mut self, message: &Value) {
        let Some(id) = text(message, "task_id") else {
            return;
        };
        if self.tasks.get(id).is_none_or(|task| task.ended) {
            return;
        }
        let mut update = json!({ "sessionUpdate": "async_task_progress", "asyncTaskId": id });
        for (from, to) in [
            ("description", "description"),
            ("summary", "summary"),
            ("last_tool_name", "lastToolName"),
        ] {
            if let Some(value) = text(message, from) {
                update[to] = json!(value);
            }
        }
        if let Some(usage) = message.get("usage").filter(|usage| usage.is_object()) {
            let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            update["usage"] = json!({
                "totalTokens": field("total_tokens"),
                "toolUses": field("tool_uses"),
                "durationMs": field("duration_ms"),
            });
        }
        self.sink.update(&self.session_id, update);
    }

    fn task_ended(
        &mut self,
        task_id: &str,
        status: &str,
        summary: Option<&str>,
        output: Option<&str>,
    ) {
        if let Some(child) = self.children.values_mut().find(|child| {
            child.session_id == task_id
                || child
                    .session_id
                    .starts_with(&format!("{task_id}:generation:"))
        }) {
            let Some(state) = subagent_state(status) else {
                return;
            };
            if child.ended {
                return;
            }
            child.ended = true;
            let session_id = child.session_id.clone();
            let parent = self
                .tools
                .iter()
                .find(|(id, _)| {
                    self.children
                        .get(*id)
                        .is_some_and(|child| child.session_id == session_id)
                })
                .map_or_else(|| self.session_id.clone(), |(_, tool)| tool.session.clone());
            self.sink.update(
                &parent,
                json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": session_id, "state": state }),
            );
            return;
        }
        let Some(state) = task_state(status) else {
            return;
        };
        let Some(task) = self.tasks.get_mut(task_id) else {
            return;
        };
        if task.ended || state == "running" || state == "paused" {
            return;
        }
        task.ended = true;
        let mut update = json!({ "sessionUpdate": "async_task_state_update", "asyncTaskId": task_id, "state": state });
        if let Some(summary) = summary {
            update["summary"] = json!(summary);
        }
        if let Some(output) = output {
            update["outputFilePath"] = json!(output);
        }
        self.sink.update(&self.session_id, update);
    }

    /// A task the person stopped from the app, said at once rather than when
    /// Claude Code gets round to it.
    pub fn task_stopped(&mut self, task_id: &str) {
        let Some(task) = self.tasks.get_mut(task_id).filter(|task| !task.ended) else {
            return;
        };
        task.ended = true;
        let name = task.name.clone();
        self.sink.update(
            &self.session_id,
            json!({ "sessionUpdate": "async_task_state_update", "asyncTaskId": task_id, "state": "stopped" }),
        );
        self.sink.update(
            &self.session_id,
            json!({ "sessionUpdate": "notice", "severity": "info", "title": "Task stopped by user", "description": format!("{name}.") }),
        );
    }

    /// Every subagent and task still running is gone with the agent.
    pub fn close(&mut self) {
        self.control.close();
        let live: Vec<(String, String)> = self
            .children
            .iter()
            .filter(|(_, child)| !child.ended)
            .map(|(spawner, child)| {
                let parent = self
                    .tools
                    .get(spawner)
                    .map_or_else(|| self.session_id.clone(), |tool| tool.session.clone());
                (parent, child.session_id.clone())
            })
            .collect();
        for (parent, session_id) in live {
            self.sink.update(
                &parent,
                json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": session_id, "state": "disconnected" }),
            );
        }
        for child in self.children.values_mut() {
            child.ended = true;
        }
        if let Ok(permissions) = self.permissions.lock() {
            for handle in permissions.values() {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usage_limit_banner_is_the_accounts_limit() {
        let message = json!({ "message": { "model": "<synthetic>", "content": [{ "type": "text", "text": "You've hit your limit · resets 3pm" }] } });
        let failure = failure_of(&message).unwrap();
        assert_eq!(failure.kind, FailureKind::Limit);
        assert_eq!(failure.title, "You've hit your limit · resets 3pm");
    }

    #[test]
    fn a_login_banner_asks_for_a_sign_in() {
        let message = json!({ "message": { "model": "<synthetic>", "content": [{ "type": "text", "text": "Invalid API key · Please run /login" }] } });
        assert_eq!(failure_of(&message).unwrap().kind, FailureKind::SignIn);
        let normal = json!({ "message": { "model": "claude-opus-5-5", "content": [{ "type": "text", "text": "hi" }] } });
        assert!(failure_of(&normal).is_none());
    }

    #[test]
    fn terminal_commands_are_left_out_and_mcp_ones_renamed() {
        let update = available_commands(&json!([
            { "name": "review", "description": "Review", "argumentHint": "[pr]" },
            { "name": "clear", "description": "Clear" },
            { "name": "github:issue (MCP)", "description": "Issue", "argumentHint": "" },
        ]));
        let names: Vec<&str> = update["availableCommands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|command| command["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["review", "mcp:github:issue"]);
        assert_eq!(update["availableCommands"][0]["input"]["hint"], "[pr]");
        assert!(update["availableCommands"][1]["input"].is_null());
    }

    #[test]
    fn a_million_token_model_says_so_in_its_name() {
        assert_eq!(window_from_name("claude-opus-5-5[1m]"), Some(1_000_000));
        assert_eq!(window_from_name("claude-opus-5-5"), None);
    }
}
