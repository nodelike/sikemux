//! Reads what Claude Code writes on its output and tells the chat, as the
//! ACP session updates the app reads from every agent.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;

use crate::acp::account::{Failure, FailureKind};

use super::super::super::feed;
use super::super::super::reset::{month_day, WallClock, Zone};
use super::super::super::states::{self, Held, Subagent, Task as TaskState};
use super::super::super::Respond;
use super::super::json::{self, text};
use super::super::pending::{Answered, Purpose};
use super::super::process::Reads;
use super::super::session::{Sink, TurnEnd};
use super::control::Control;
use super::failure;
use super::permission;
use super::tools::{self, TaskPlan};

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
#[derive(Serialize, Deserialize)]
pub(crate) struct Turn {
    pub number: u64,
    /// Every message sent into the turn: its prompt and any steered in since.
    /// Claude Code answers a steered message folded into the turn with the
    /// result for the prompt, and one it held until the turn ended with its own.
    pub messages: HashSet<String>,
    /// Steered messages Claude Code has queued but not yet taken in, whose
    /// own result is still to come.
    pub waiting: HashSet<String>,
    /// The text of each steered message that came from another agent, by
    /// its id, until Claude Code takes it in.
    pub letters: HashMap<String, String>,
    pub cancelled: bool,
    pub failure: Option<Failure>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Shared {
    pub turn: Option<Turn>,
    /// Every message this side wrote, whose echoes are not shown again.
    pub sent: HashSet<String>,
    /// Claude Code's permission mode for the chat, which an approved plan
    /// goes back to.
    pub mode: String,
    /// Set until the chat's first message is sent, which Claude names the
    /// chat from.
    pub untitled: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Tool {
    name: String,
    input: Value,
    session: String,
}

#[derive(Serialize, Deserialize)]
struct Child {
    name: String,
    /// The session whose transcript holds this subagent's card.
    parent: String,
    ended: bool,
    background: bool,
}

#[derive(Serialize, Deserialize)]
struct Task {
    name: String,
    ended: bool,
    /// The agents a workflow runs, which Claude Code leaves off most progress.
    agents: Option<Value>,
}

pub(crate) struct Reader {
    sink: Sink,
    control: Control,
    shared: Arc<Mutex<Shared>>,
    asking: Asking,
    data: ReaderState,
}

/// What the reader knows of the session, as plain data, for a core that
/// takes the chat over to read on from.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReaderState {
    cwd: PathBuf,
    session_id: String,
    /// Text and thinking streamed for the current top-level message, which
    /// the whole message repeats when it lands.
    streamed: Vec<(u64, String, String)>,
    stream_message_id: Option<String>,
    tools: HashMap<String, Tool>,
    shown: HashSet<String>,
    /// The subagent each spawning or resuming tool call speaks for, by the call.
    children: HashMap<String, String>,
    /// Subagents by their session, which is the task id Claude gave them.
    subagents: HashMap<String, Child>,
    held: HashMap<String, Held<Value>>,
    tasks: HashMap<String, Task>,
    /// Shell commands Claude Code waits on in the turn, as it reported them
    /// starting. It reports their end like background work's, but their tool
    /// result already says it. One moved to the background is shown then.
    foreground: HashMap<String, Value>,
    compaction: Option<String>,
    /// Claude Code retried this turn's request with no answer from Anthropic,
    /// so a failure that ends it was the network's.
    offline_retries: bool,
    /// The list Claude's task tools keep, shown as the chat's plan.
    task_plan: TaskPlan,
    used: Option<u64>,
    size: u64,
    model: Option<String>,
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
    let said = json::joined_text(inner.get("content").filter(|content| content.is_array()));
    let synthetic = failure::is_synthetic(inner);
    let title = |fallback: &str| {
        if said.trim().is_empty() {
            fallback.to_owned()
        } else {
            said.trim().to_owned()
        }
    };
    if synthetic && failure::asks_sign_in(&said) {
        return Some(Failure::sign_in(title("Sign in to continue using Claude.")));
    }
    if synthetic && failure::out_of_usage(&said) {
        return Some(Failure {
            kind: FailureKind::Limit,
            title: title("The Claude account has no available quota."),
            details: None,
            resets_at: limit_resets_at(message, &said),
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
        resets_at: (kind == FailureKind::Limit)
            .then(|| limit_resets_at(message, &said))
            .flatten(),
    })
}

/// When the limit that stopped a turn resets, in Unix milliseconds, from the
/// limit Claude Code attaches to the message, else from its text.
fn limit_resets_at(message: &Value, said: &str) -> Option<u64> {
    let Some(limit) = message.pointer("/api_error_params/rate_limit_info") else {
        return resets_in_text(said);
    };
    let rejected = |status, at| {
        (text(limit, status) == Some("rejected"))
            .then(|| limit.get(at)?.as_u64())
            .flatten()
    };
    let seconds = [
        rejected("status", "resetsAt"),
        rejected("overageStatus", "overageResetsAt"),
    ];
    seconds.into_iter().flatten().min()?.checked_mul(1000)
}

/// The reset time in a limit's text: Unix seconds after a `|`, or a clock
/// such as "resets 3pm (Europe/London)" or "resets Oct 17, 2027, 3:30pm".
fn resets_in_text(said: &str) -> Option<u64> {
    let epoch = said.trim().rsplit_once('|');
    if let Some(seconds) = epoch.and_then(|(_, seconds)| seconds.parse::<u64>().ok()) {
        return seconds.checked_mul(1000);
    }
    let (_, rest) = said.split_once("resets ")?;
    let rest = rest.strip_prefix("at ").unwrap_or(rest);
    let end = ["am", "pm"]
        .iter()
        .filter_map(|half| rest.find(half))
        .min()?
        + 2;
    let (stamp, after) = rest.split_at(end);
    let zone = match after
        .strip_prefix(" (")
        .and_then(|after| after.split_once(')'))
    {
        None => Zone::Host,
        Some(("UTC", _)) => Zone::Utc,
        Some((zone, _)) if host_zone().as_deref() == Some(zone) => Zone::Host,
        Some(_) => return None,
    };
    let mut parts: Vec<&str> = stamp.split(", ").collect();
    let clock = parts.pop()?;
    let (clock, pm) = match clock.strip_suffix("pm") {
        Some(clock) => (clock, true),
        None => (clock.strip_suffix("am")?, false),
    };
    let (hour, minute) = clock.split_once(':').unwrap_or((clock, "0"));
    let (hour, minute) = (hour.parse::<i32>().ok()?, minute.parse::<i32>().ok()?);
    if !(1..=12).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    let date = match parts.as_slice() {
        [] => None,
        [day] | [day, _] => {
            let (month, day) = month_day(day)?;
            let year = match parts.get(1) {
                Some(year) => Some(year.parse::<i32>().ok()?),
                None => None,
            };
            Some((year, month, day))
        }
        _ => return None,
    };
    let hour = hour % 12 + if pm { 12 } else { 0 };
    WallClock {
        zone,
        date,
        hour,
        minute,
    }
    .unix_ms()
}

/// The IANA name of this host's time zone, as Claude Code labels its times.
fn host_zone() -> Option<String> {
    if let Ok(zone) = std::env::var("TZ") {
        return Some(zone.trim_start_matches(':').to_owned());
    }
    let link = std::fs::read_link("/etc/localtime").ok()?;
    Some(link.to_str()?.split_once("zoneinfo/")?.1.to_owned())
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

/// A workflow's agents, in the order it started them, as the chat shows them.
fn workflow_agents(progress: &[Value]) -> Value {
    let mut agents: Vec<&Value> = progress
        .iter()
        .filter(|entry| text(entry, "type") == Some("workflow_agent"))
        .collect();
    agents.sort_by_key(|agent| agent.get("index").and_then(Value::as_u64));
    let agents = agents
        .into_iter()
        .map(|agent| {
            let state = match text(agent, "state") {
                Some("done") => "completed",
                Some("error") => "failed",
                _ if agent.get("startedAt").is_some_and(|at| !at.is_null()) => "running",
                Some("progress") => "running",
                _ => "queued",
            };
            let mut shown = json!({
                "index": agent.get("index"),
                "label": text(agent, "label").unwrap_or("agent"),
                "state": state,
            });
            for (from, to) in [
                ("phaseTitle", "phase"),
                ("lastToolSummary", "activity"),
                ("error", "error"),
            ] {
                if let Some(value) = text(agent, from) {
                    shown[to] = json!(value);
                }
            }
            if shown.get("activity").is_none() {
                if let Some(tool) = text(agent, "lastToolName") {
                    shown["activity"] = json!(tool);
                }
            }
            for key in ["tokens", "toolCalls"] {
                if let Some(count) = agent.get(key).and_then(Value::as_u64) {
                    shown[key] = json!(count);
                }
            }
            shown
        })
        .collect();
    Value::Array(agents)
}

fn task_type(kind: Option<&str>) -> String {
    match kind {
        Some("local_bash") => "shell".into(),
        Some("local_workflow") => "workflow".into(),
        Some("monitor_mcp" | "monitor_ws") => "monitor".into(),
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
        Self::restored(
            sink,
            control,
            shared,
            ReaderState {
                cwd,
                session_id,
                streamed: Vec::new(),
                stream_message_id: None,
                tools: HashMap::new(),
                shown: HashSet::new(),
                children: HashMap::new(),
                subagents: HashMap::new(),
                held: HashMap::new(),
                tasks: HashMap::new(),
                foreground: HashMap::new(),
                compaction: None,
                offline_retries: false,
                task_plan: TaskPlan::default(),
                used: None,
                size: 200_000,
                model: None,
            },
        )
    }

    /// Reads on from where an earlier reader of the same Claude stopped.
    pub fn restored(
        sink: Sink,
        control: Control,
        shared: Arc<Mutex<Shared>>,
        data: ReaderState,
    ) -> Self {
        Self {
            sink,
            asking: Asking {
                control: control.clone(),
                open: Arc::default(),
            },
            control,
            shared,
            data,
        }
    }

    /// What the reader knows, and the state it shares with the driver.
    pub fn into_state(self) -> (ReaderState, Arc<Mutex<Shared>>) {
        (self.data, self.shared)
    }

    pub fn note_model(&mut self, model: &str) {
        if let Some(size) = window_from_name(model) {
            self.data.size = size;
        }
    }

    fn turn_running(&self) -> bool {
        self.shared.locked().turn.is_some()
    }

    /// Sends `update` to the session `parent` names: the chat's own, or the
    /// subagent a tool call spawned. A subagent not yet announced holds it.
    fn send(&mut self, parent: Option<&str>, update: Value) {
        let Some(parent) = parent else {
            self.sink.update(&self.data.session_id, update);
            return;
        };
        match self.data.children.get(parent) {
            Some(session)
                if self
                    .data
                    .subagents
                    .get(session)
                    .is_none_or(|child| child.ended) => {}
            Some(session) => self.sink.update(session, update),
            None => self
                .data
                .held
                .entry(parent.to_owned())
                .or_default()
                .push(update),
        }
    }

    /// The session a tool call spawned by `parent` belongs to.
    fn session_of(&self, parent: Option<&str>) -> String {
        parent
            .and_then(|parent| self.data.children.get(parent))
            .map_or_else(|| self.data.session_id.clone(), Clone::clone)
    }

    pub fn handle(&mut self, message: Value) {
        match message.get("type").and_then(Value::as_str) {
            Some("control_response") => {
                if let Some((purpose, result)) = self.control.answered(&message) {
                    self.purpose_answered(&purpose, result);
                }
            }
            Some("control_request") => self.request(&message),
            Some("control_cancel_request") => {
                if let Some(id) = text(&message, "request_id") {
                    self.asking.take_back(id);
                }
            }
            Some("stream_event") => self.stream_event(&message),
            Some("assistant") => self.assistant(&message),
            Some("user") => self.user(&message),
            Some("result") => self.result(&message),
            Some("system") => self.system(&message),
            Some("command_lifecycle") => {
                let state = text(&message, "state");
                if state == Some("queued") {
                    return;
                }
                let mut unread = None;
                if let Some(uuid) = text(&message, "command_uuid") {
                    if let Some(turn) = self.shared.locked().turn.as_mut() {
                        turn.waiting.remove(uuid);
                        let letter = turn.letters.remove(uuid);
                        if matches!(state, Some("cancelled" | "dropped")) {
                            unread = letter;
                        }
                    }
                }
                if let Some(letter) = unread {
                    self.sink.letter_unread(letter);
                }
            }
            Some("conversation_reset") => {
                self.data.task_plan.clear();
                self.send(None, self.data.task_plan.plan());
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
            .data
            .tools
            .get(&tool_id)
            .map(|tool| tool.session.clone())
            .filter(|session| *session != self.data.session_id);
        let session = parent.unwrap_or_else(|| self.data.session_id.clone());
        if name == "AskUserQuestion" {
            let told = self.sink.question(&input);
            self.control.respond(
                &request_id,
                json!({ "behavior": "deny", "message": told, "toolUseID": tool_id }),
            );
            return;
        }
        if !self.data.shown.contains(&tool_id) && !tools::is_plan_tool(&name) {
            self.data.shown.insert(tool_id.clone());
            self.data.tools.entry(tool_id.clone()).or_insert(Tool {
                name: name.clone(),
                input: input.clone(),
                session: session.clone(),
            });
            self.sink.update(
                &session,
                tools::tool_call(&tool_id, &name, &input, &self.data.cwd),
            );
        }
        let mode = self.shared.locked().mode.clone();
        let asked = permission::request(
            &session,
            &tool_id,
            &name,
            &input,
            &request,
            &self.data.cwd,
            &mode,
        );
        let reply = json!({
            "requestId": request_id,
            "toolId": tool_id,
            "input": input,
            "request": request,
        });
        self.sink
            .ask(asked, reply.clone(), self.asking.responder(reply));
    }

    /// What answers Claude's permission requests.
    pub fn asking(&self) -> Asking {
        self.asking.clone()
    }

    /// The answer to a control request sent with `purpose`.
    fn purpose_answered(&self, purpose: &Purpose, result: Answered) {
        if *purpose == Purpose::Title {
            let title = result
                .ok()
                .and_then(|named| feed::title(text(&named, "title")?));
            if let Some(title) = title {
                self.sink.update(
                    &self.data.session_id,
                    json!({ "sessionUpdate": "session_info_update", "title": title }),
                );
            }
        }
    }

    fn stream_event(&mut self, message: &Value) {
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        let Some(event) = message.get("event") else {
            return;
        };
        match text(event, "type") {
            Some("message_start") => {
                self.data.stream_message_id = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if parent.is_none() {
                    self.data.streamed.clear();
                    if let Some(model) = event.pointer("/message/model").and_then(Value::as_str) {
                        self.data.model = Some(model.to_owned());
                        self.note_model(model);
                    }
                    if let Some(used) = event.pointer("/message/usage").and_then(usage_total) {
                        self.data.used = Some(used);
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
                    match self.data.streamed.last_mut() {
                        Some((last, last_kind, streamed))
                            if *last == index && *last_kind == kind =>
                        {
                            streamed.push_str(&chunk)
                        }
                        _ => self
                            .data
                            .streamed
                            .push((index, kind.to_owned(), chunk.clone())),
                    }
                }
                let block = if kind == "text" {
                    json!({ "type": "text", "text": chunk })
                } else {
                    json!({ "type": "thinking", "thinking": chunk })
                };
                let message_id = self.data.stream_message_id.clone();
                if let Some(update) =
                    tools::content_chunk(&block, "assistant", message_id.as_deref())
                {
                    self.send(parent.as_deref(), update);
                }
            }
            Some("message_delta") if parent.is_none() => {
                if let Some(usage) = event.get("usage").filter(|usage| usage.is_object()) {
                    if let (Some(used), Some(output)) = (
                        self.data.used,
                        usage.get("output_tokens").and_then(Value::as_u64),
                    ) {
                        self.data.used = Some(used.max(output));
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
        let known = self.data.tools.contains_key(&id);
        if !streaming || !known {
            self.data.tools.insert(
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
        if self.data.shown.insert(id.clone()) {
            let update = tools::tool_call(&id, &name, &input, &self.data.cwd);
            self.send(parent, update);
        } else if !streaming {
            let update = tools::tool_call_refined(&id, &name, &input, &self.data.cwd);
            self.send(parent, update);
        }
    }

    fn assistant(&mut self, message: &Value) {
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        if parent.is_none() {
            if let Some(failure) = failure_of(message) {
                let mut shared = self.shared.locked();
                if let Some(turn) = shared.turn.as_mut() {
                    turn.failure = Some(failure);
                    return;
                }
            }
        }
        let Some(inner) = message.get("message") else {
            return;
        };
        if parent.is_none() {
            if let Some(model) =
                text(inner, "model").filter(|model| *model != failure::SYNTHETIC_MODEL)
            {
                self.data.model = Some(model.to_owned());
                self.note_model(model);
            }
            if let Some(used) = inner.get("usage").and_then(usage_total) {
                self.data.used = Some(used);
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
                        if let Some((_, streamed_kind, streamed)) = self.data.streamed.get(position)
                        {
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
            self.data.streamed.clear();
        }
    }

    fn user(&mut self, message: &Value) {
        if let Some(uuid) = text(message, "uuid") {
            if self.shared.locked().sent.contains(uuid) {
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
            let Some(tool) = self.data.tools.get(&id) else {
                continue;
            };
            let (name, input, session) =
                (tool.name.clone(), tool.input.clone(), tool.session.clone());
            if tools::is_plan_tool(&name) {
                if parent.is_none() {
                    if let Some(plan) = self
                        .data
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
                if failed && !self.data.children.contains_key(&id) {
                    let mut update = tools::tool_call(&id, &name, &input, &self.data.cwd);
                    let completed =
                        tools::tool_result(&id, &name, &input, result, structured, &self.data.cwd);
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
            let update = tools::tool_result(&id, &name, &input, result, structured, &self.data.cwd);
            self.sink.update(&session, update);
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

    fn result(&mut self, result: &Value) {
        if let Some(size) = result
            .get("modelUsage")
            .and_then(Value::as_object)
            .and_then(|usage| self.window_of(usage))
        {
            self.data.size = size;
        }
        let used = self.data.used;
        if let Some(used) = used {
            let mut update =
                json!({ "sessionUpdate": "usage_update", "used": used, "size": self.data.size });
            if let Some(cost) = result.get("total_cost_usd").and_then(Value::as_f64) {
                update["cost"] = json!({ "amount": cost, "currency": "USD" });
            }
            self.sink.update(&self.data.session_id, update);
        }
        let answers = text(result, "user_message_uuid");
        // A turn that background work started, rather than a message, carries its origin.
        let woken = result.get("origin").is_some();
        let ended = {
            let mut shared = self.shared.locked();
            let ends = shared.turn.as_ref().is_some_and(|turn| match answers {
                Some(uuid) => turn.messages.contains(uuid) && turn.waiting.is_empty(),
                None => !woken,
            });
            if ends {
                shared.turn.take()
            } else {
                None
            }
        };
        let Some(mut turn) = ended else {
            if !self.turn_running() {
                self.sink.work_ended("end_turn");
            }
            return;
        };
        if turn.cancelled {
            for uuid in &turn.waiting {
                if let Some(letter) = turn.letters.remove(uuid) {
                    self.sink.letter_unread(letter);
                }
            }
        }
        let end = if turn.cancelled {
            TurnEnd::Completed(json!({ "stopReason": "cancelled" }))
        } else if let Some(failure) = turn.failure {
            TurnEnd::Failed {
                failure: self.offline(failure),
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
            let kind = if failure::asks_sign_in(&said) {
                FailureKind::SignIn
            } else {
                FailureKind::Other
            };
            TurnEnd::Failed {
                failure: self.offline(Failure {
                    kind,
                    title: said,
                    details: None,
                    resets_at: None,
                }),
                response: Some(json!({ "stopReason": "end_turn" })),
            }
        } else {
            TurnEnd::Completed(json!({ "stopReason": stop_reason(result) }))
        };
        self.data.offline_retries = false;
        self.sink.end_turn(turn.number, end);
    }

    fn window_of(&self, usage: &Map<String, Value>) -> Option<u64> {
        let model = self.data.model.as_deref().unwrap_or_default();
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
        if let Some(version) = text(message, "claude_code_version") {
            self.sink.cli_version(version);
        }
        match text(message, "subtype") {
            Some("session_state_changed") if text(message, "state") == Some("idle") => self.idle(),
            // Claude Code opens every turn with this, including the ones background work starts.
            Some("init") if !self.turn_running() => self.sink.work_started(),
            Some("status") => self.status(message),
            Some("api_retry") => self.retrying(message),
            Some("compact_boundary") => {
                if let Some(used) = message
                    .pointer("/compact_metadata/post_tokens")
                    .and_then(Value::as_u64)
                {
                    self.data.used = Some(used);
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
                if patch.get("is_backgrounded").and_then(Value::as_bool) == Some(true) {
                    self.backgrounded(&id);
                }
                if let Some(status) = text(&patch, "status") {
                    self.task_ended(&id, status, None, text(&patch, "output_file"));
                }
            }
            Some("task_notification") => {
                let Some(id) = text(message, "task_id").map(str::to_owned) else {
                    return;
                };
                let status = text(message, "status").unwrap_or("completed").to_owned();
                let summary = text(message, "summary");
                // Work started outside any tracked task still wakes the agent.
                if !self.data.tasks.contains_key(&id) && !self.data.subagents.contains_key(&id) {
                    // A resumed session reports every task its last process left behind as stopped.
                    if self.data.foreground.contains_key(&id)
                        || states::outcome(&status) == Some(states::Outcome::Stopped)
                    {
                        return;
                    }
                    let name = summary
                        .and_then(|summary| summary.split('"').nth(1))
                        .or(summary)
                        .unwrap_or("Background task")
                        .to_owned();
                    self.task_notice(&id, &name, &status);
                    return;
                }
                self.task_ended(&id, &status, summary, text(message, "output_file"));
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
                    .data
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
                        .update(&self.data.session_id, available_commands(commands));
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
                if !self.data.shown.contains(id) {
                    return;
                }
                let reason = text(message, "decision_reason")
                    .or_else(|| text(message, "message"))
                    .unwrap_or("not allowed");
                let session = self
                    .data
                    .tools
                    .get(id)
                    .map_or_else(|| self.data.session_id.clone(), |tool| tool.session.clone());
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
        self.sink.update(&self.data.session_id, update);
    }

    /// A failure Claude gave no reason for reads as the network's when the turn
    /// was retrying with no answer from Anthropic, or Claude's words say so.
    fn offline(&self, failure: Failure) -> Failure {
        let networked = failure.kind == FailureKind::Other
            && (self.data.offline_retries || crate::acp::account::sounds_offline(&failure.title));
        if networked {
            Failure::lost_connection(failure.title)
        } else {
            failure
        }
    }

    /// Claude Code went idle: anything the turn still waited on is over.
    fn idle(&mut self) {
        let turn = self.shared.locked().turn.take();
        match turn {
            Some(turn) => {
                let end = match turn.failure {
                    Some(failure) if !turn.cancelled => TurnEnd::Failed {
                        failure: self.offline(failure),
                        response: Some(json!({ "stopReason": "end_turn" })),
                    },
                    _ => TurnEnd::Completed(json!({
                        "stopReason": if turn.cancelled { "cancelled" } else { "end_turn" },
                    })),
                };
                self.data.offline_retries = false;
                self.sink.end_turn(turn.number, end);
            }
            None => self.sink.work_ended("end_turn"),
        }
    }

    fn status(&mut self, message: &Value) {
        if text(message, "status") == Some("compacting") && self.data.compaction.is_none() {
            let id = text(message, "uuid")
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.data.compaction = Some(id.clone());
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

    /// Claude Code retries a failed request on its own, for up to minutes.
    fn retrying(&mut self, message: &Value) {
        let attempt = message.get("attempt").and_then(Value::as_u64).unwrap_or(1);
        let status = message.get("error_status").and_then(Value::as_u64);
        let category = if status.is_none() {
            "connection"
        } else {
            "service"
        };
        let detail = match status {
            Some(status) => format!("Anthropic answered {status}"),
            None => "No answer from Anthropic".to_owned(),
        };
        self.data.offline_retries |= status.is_none();
        let update = crate::acp::account::retrying("api_retry", category, attempt, &detail);
        self.sink.update(&self.data.session_id, update);
    }

    fn compacted(&mut self, status: &str, error: Option<String>) {
        let Some(id) = self.data.compaction.take() else {
            return;
        };
        let mut update =
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status });
        if let Some(error) = error {
            update["content"] =
                json!([{ "type": "content", "content": { "type": "text", "text": error } }]);
        }
        self.sink.update(&self.data.session_id, update);
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
        if !backgrounded {
            self.data.foreground.insert(task_id, message.clone());
            return;
        }
        if self.data.tasks.contains_key(&task_id) {
            return;
        }
        // Claude Code reports a monitor as a shell; only the tool that started it tells them apart.
        let monitor = spawner
            .as_deref()
            .and_then(|id| self.data.tools.get(id))
            .is_some_and(|tool| tool.name == "Monitor");
        let task_type = if monitor {
            "monitor".to_owned()
        } else {
            task_type(kind)
        };
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
        self.data.tasks.insert(
            task_id.clone(),
            Task {
                name: name.clone(),
                ended: false,
                agents: None,
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
        if let Some(owner) = self.owner_of(spawner.as_deref()) {
            update["owner"] = json!(owner);
        }
        if let Some(spawner) = spawner {
            update["toolCallId"] = json!(spawner);
        }
        self.sink.update(&self.data.session_id, update);
    }

    /// The subagent whose tool call started a background task, since the task
    /// is listed with the chat's own and would otherwise look like the chat's.
    fn owner_of(&self, tool_id: Option<&str>) -> Option<String> {
        let session = &self.data.tools.get(tool_id?)?.session;
        (session != &self.data.session_id)
            .then(|| {
                self.data
                    .subagents
                    .get(session)
                    .map(|child| child.name.clone())
            })
            .flatten()
    }

    fn spawn_child(&mut self, task_id: &str, spawner: &str, message: &Value) {
        let session_id = task_id.to_owned();
        // An agent that finished with its own background work still running is
        // started again by that work, under the call that first spawned it.
        if self.data.subagents.contains_key(task_id) {
            self.resume_child(&session_id, spawner, message);
            return;
        }
        if self.data.children.contains_key(spawner) {
            return;
        }
        let (input, parent_session) = self.data.tools.get(spawner).map_or_else(
            || (Value::Null, self.data.session_id.clone()),
            |tool| (tool.input.clone(), tool.session.clone()),
        );
        let name = states::first_text(&[
            text(&input, "name"),
            text(&input, "description"),
            text(&input, "subagent_type"),
            text(message, "description"),
            text(message, "subagent_type"),
        ])
        .unwrap_or_else(|| states::unnamed(task_id));
        let task = states::first_text(&[
            text(&input, "prompt"),
            text(&input, "description"),
            text(message, "prompt"),
            text(message, "description"),
        ])
        .unwrap_or_else(|| states::UNNAMED_TASK.to_owned());
        self.sink
            .update(&parent_session, states::spawned(&session_id, &name, &task));
        let background = message
            .get("is_backgrounded")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                input.get("run_in_background").and_then(Value::as_bool) == Some(true)
            });
        self.data
            .children
            .insert(spawner.to_owned(), session_id.clone());
        self.data.subagents.insert(
            session_id.clone(),
            Child {
                name: name.clone(),
                parent: parent_session,
                ended: false,
                background,
            },
        );
        for update in self.data.held.remove(spawner).unwrap_or_default() {
            self.sink.update(&session_id, update);
        }
    }

    /// A message sent to a subagent wakes the one already on screen, so its
    /// card picks up where it left off rather than a second card starting.
    fn resume_child(&mut self, session_id: &str, spawner: &str, message: &Value) {
        self.data
            .children
            .insert(spawner.to_owned(), session_id.to_owned());
        let Some(child) = self.data.subagents.get_mut(session_id) else {
            return;
        };
        if let Some(background) = message.get("is_backgrounded").and_then(Value::as_bool) {
            child.background = background;
        }
        let parent = child.parent.clone();
        if child.ended {
            child.ended = false;
            self.sink
                .update(&parent, Subagent::Running.update(session_id));
        }
        for update in self.data.held.remove(spawner).unwrap_or_default() {
            self.sink.update(session_id, update);
        }
    }

    fn task_progress(&mut self, message: &Value) {
        let Some(id) = text(message, "task_id") else {
            return;
        };
        if let Some(child) = self.data.subagents.get(id).filter(|child| !child.ended) {
            let tokens = message
                .get("usage")
                .and_then(|usage| usage.get("total_tokens"))
                .and_then(Value::as_u64);
            if let Some(tokens) = tokens {
                let update = json!({ "sessionUpdate": "subagent_usage", "subagentSessionId": id, "tokens": tokens });
                self.sink.update(&child.parent, update);
            }
            return;
        }
        let Some(task) = self.data.tasks.get_mut(id).filter(|task| !task.ended) else {
            return;
        };
        let mut update = json!({ "sessionUpdate": "async_task_progress", "asyncTaskId": id });
        if let Some(progress) = message.get("workflow_progress").and_then(Value::as_array) {
            task.agents = Some(workflow_agents(progress));
        }
        if let Some(agents) = &task.agents {
            update["agents"] = agents.clone();
        }
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
        self.sink.update(&self.data.session_id, update);
    }

    fn task_ended(
        &mut self,
        task_id: &str,
        status: &str,
        summary: Option<&str>,
        output: Option<&str>,
    ) {
        if let Some(child) = self.data.subagents.get_mut(task_id) {
            let Some(state) = states::outcome(status).map(Subagent::from) else {
                return;
            };
            if child.ended {
                return;
            }
            child.ended = true;
            let (parent, background, name) =
                (child.parent.clone(), child.background, child.name.clone());
            // An agent whose spawner has already finished reports to the chat itself.
            let reports_to_chat = parent == self.data.session_id
                || self
                    .data
                    .subagents
                    .get(&parent)
                    .is_some_and(|spawner| spawner.ended);
            let woke = (background && reports_to_chat).then_some(name);
            self.sink.update(&parent, state.update(task_id));
            if let Some(name) = woke {
                self.task_notice(task_id, &name, status);
            }
            return;
        }
        let Some(state) = TaskState::from_status(status).filter(|state| state.ended()) else {
            return;
        };
        let Some(task) = self.data.tasks.get_mut(task_id) else {
            return;
        };
        if task.ended {
            return;
        }
        task.ended = true;
        let mut update = state.update(task_id);
        if let Some(summary) = summary {
            update["summary"] = json!(summary);
        }
        if let Some(output) = output {
            update["outputFilePath"] = json!(output);
        }
        self.sink.update(&self.data.session_id, update);
    }

    /// Background work Claude Code reports ending, which wakes the agent; the
    /// transcript shows it so the turn that follows has a reason.
    fn task_notice(&mut self, task_id: &str, name: &str, status: &str) {
        let Some(state) = TaskState::from_status(status).filter(|state| state.ended()) else {
            return;
        };
        let mut update = json!({ "sessionUpdate": "task_notice", "taskId": task_id, "name": name, "state": state.as_str() });
        if self.data.subagents.contains_key(task_id) {
            update["subagentSessionId"] = json!(task_id);
        }
        self.sink.update(&self.data.session_id, update);
    }

    /// A task the person stopped from the app, said at once rather than when
    /// Claude Code gets round to it.
    pub fn task_stopped(&mut self, task_id: &str) {
        let Some(task) = self.data.tasks.get_mut(task_id).filter(|task| !task.ended) else {
            return;
        };
        task.ended = true;
        let name = task.name.clone();
        self.sink
            .update(&self.data.session_id, TaskState::Stopped.update(task_id));
        self.sink.update(
            &self.data.session_id,
            json!({ "sessionUpdate": "notice", "severity": "info", "title": "Task stopped by user", "description": format!("{name}.") }),
        );
    }

    /// Work the turn was waiting on that Claude Code moved to the background,
    /// which then runs on past the turn.
    fn backgrounded(&mut self, task_id: &str) {
        if let Some(child) = self.data.subagents.get_mut(task_id) {
            child.background = true;
            return;
        }
        if let Some(mut started) = self.data.foreground.remove(task_id) {
            started["is_backgrounded"] = json!(true);
            self.task_started(&started);
        }
    }
}

impl Reads for Reader {
    /// A task the person stopped from the app.
    type Nudge = String;

    fn message(&mut self, message: Value) {
        self.handle(message);
    }

    fn nudged(&mut self, task_id: String) {
        self.task_stopped(&task_id);
    }

    /// Claude Code is gone: nothing it asked can be answered any more.
    fn closed(&mut self) {
        self.control.close();
        self.asking.close();
    }
}

/// Claude's permission requests still waiting on the person, by Claude's
/// own request id, and how the answer goes back.
#[derive(Clone)]
pub(crate) struct Asking {
    control: Control,
    open: Arc<Mutex<HashSet<String>>>,
}

impl Asking {
    /// Answers the request `reply` holds with the person's choice, unless
    /// Claude has taken the request back by then.
    pub fn responder(&self, reply: Value) -> Respond {
        let Some(id) = reply["requestId"].as_str().map(str::to_owned) else {
            return Box::new(|_| {});
        };
        self.open.locked().insert(id.clone());
        let asking = self.clone();
        Box::new(move |chosen| {
            let still_open = asking.open.locked().remove(&id);
            if !still_open {
                return;
            }
            let answer = permission::answer(
                reply["toolId"].as_str().unwrap_or_default(),
                &reply["input"],
                &reply["request"],
                chosen.as_deref(),
            );
            asking.control.respond(&id, answer);
        })
    }

    fn take_back(&self, id: &str) {
        let mut open = self.open.locked();
        open.remove(id);
    }

    fn close(&self) {
        let mut open = self.open.locked();
        open.clear();
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
    fn a_usage_limit_says_when_it_resets() {
        let now = sikemux_process::clock::epoch_secs();
        let resets = |text: &str, params: Value| {
            let message = json!({
                "type": "assistant", "error": "rate_limit", "api_error_params": params,
                "message": { "model": "<synthetic>", "content": [{ "type": "text", "text": text }] },
            });
            failure_of(&message).unwrap().resets_at
        };
        let reported = json!({ "rate_limit_info": {
            "status": "rejected", "resetsAt": now + 3600, "rateLimitType": "five_hour",
            "overageStatus": "rejected", "overageDisabledReason": "org_level_disabled",
        } });
        assert_eq!(
            resets(
                "You've hit your session limit · resets 3pm (America/Los_Angeles)",
                reported
            ),
            Some((now + 3600) * 1000)
        );

        let text = |text: &str| resets(text, Value::Null);
        assert_eq!(
            text("You've hit your weekly limit · resets Oct 17, 2027, 3:30pm (UTC)"),
            Some(1_823_787_000_000)
        );
        let three = now - now % 86_400 + 15 * 3600;
        let next_three = if three > now { three } else { three + 86_400 };
        assert_eq!(
            text("You've hit your session limit · resets 3pm (UTC)"),
            Some(next_three * 1000)
        );
        let local = text("You've hit your limit · resets 3pm").unwrap();
        assert!(local > now * 1000 && local <= (now + 86_400) * 1000);
        assert_eq!(
            text("Claude AI usage limit reached|1823787000"),
            Some(1_823_787_000_000)
        );
        assert_eq!(
            text("You've hit your limit · resets 3pm (Mars/Olympus)"),
            None
        );
        assert_eq!(text("Your org is out of usage · contact your admin"), None);
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
