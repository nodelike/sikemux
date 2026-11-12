//! Reads what Claude Code writes on its output and tells the chat, as the
//! session updates the fold reads from every agent.

use crate::launch::claude_model;
use sikemux_fold::update::{agent_text, compaction, notice, text_content, thought, turn_answer};
use sikemux_fold::UpdateKind;
use sikemux_view::chat::StopReason;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;

use crate::acp::failure::{Failure, FailureKind};
use crate::server::activity_log::{Compaction, ModelCall};

use super::super::super::cap;
use super::super::super::feed;
use super::super::super::reset::{month_day, twelve_hour, WallClock, Zone};
use super::super::super::states::{self, Held, Subagent, Task as TaskState};
use super::super::super::Respond;
use super::super::json::{self, text};
use super::super::pending::{Answered, Purpose};
use super::super::process::Reads;
use super::super::rpc::error_text;
use super::super::session::{Sink, TurnEnd};
use super::apps::{self, Apps, Shows};
use super::compaction::AutoCompact;
use super::control::Control;
use super::failure;
use super::messages::{
    CompactMetadata, Delta, Message, StreamEvent, System, SystemEvent, TurnResult,
};
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
pub struct Turn {
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
pub struct Shared {
    pub turn: Option<Turn>,
    /// Every message this side wrote, whose echoes are not shown again.
    pub sent: HashSet<String>,
    /// Claude Code's permission mode for the chat, which an approved plan
    /// goes back to.
    pub mode: String,
    /// Set until the chat's first message is sent, which Claude names the
    /// chat from.
    pub untitled: Option<String>,
    /// The core's own compaction runs on this message, so the chat sees no turn for it.
    pub quiet: Option<String>,
    /// The quiet turn is the core's ask for a handoff note, so nothing it says reaches the chat.
    #[serde(default)]
    pub hushed: bool,
    /// The handoff note it wrote, once its turn ended.
    pub handed: Option<String>,
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

pub struct Reader {
    sink: Sink,
    control: Control,
    shared: Arc<Mutex<Shared>>,
    asking: Asking,
    data: ReaderState,
    apps: Apps,
    /// The last turn to end was the person's and it finished, so what Claude predicts they
    /// send next is shown.
    suggests: bool,
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
    #[serde(default)]
    auto_compact: AutoCompact,
    /// The session's calls to the model and its compactions since Claude last answered.
    #[serde(default)]
    calls: Vec<ModelCall>,
    #[serde(default)]
    compactions: Vec<Compaction>,
    /// Subagents or background work called the model since Claude last answered, so what a
    /// compaction cost cannot be told apart from what they did.
    #[serde(default)]
    others_called: bool,
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
    json!({ "sessionUpdate": UpdateKind::AvailableCommandsUpdate, "availableCommands": commands })
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
    let (hour, minute) = twelve_hour(clock, pm)?;
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

fn stop_reason(result: &TurnResult) -> StopReason {
    match (result.stop_reason.as_deref(), result.subtype.as_deref()) {
        (Some("refusal"), _) => StopReason::Refusal,
        (Some("max_tokens"), _) => StopReason::MaxTokens,
        (
            _,
            Some(
                "error_max_turns" | "error_max_budget_usd" | "error_max_structured_output_retries",
            ),
        ) => StopReason::MaxTurnRequests,
        _ => StopReason::EndTurn,
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
        apps: Apps,
    ) -> Self {
        Self::restored(
            sink,
            control,
            shared,
            apps,
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
                auto_compact: AutoCompact::default(),
                calls: Vec::new(),
                compactions: Vec::new(),
                others_called: false,
            },
        )
    }

    /// Reads on from where an earlier reader of the same Claude stopped.
    pub fn restored(
        sink: Sink,
        control: Control,
        shared: Arc<Mutex<Shared>>,
        apps: Apps,
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
            apps,
            suggests: false,
        }
    }

    /// What the reader knows, and the state it shares with the driver.
    pub fn into_state(self) -> (ReaderState, Arc<Mutex<Shared>>) {
        (self.data, self.shared)
    }

    pub fn note_model(&mut self, model: &str) {
        if claude_model::names_long_window(model) {
            self.data.size = claude_model::LONG_WINDOW;
        }
    }

    pub fn compacts(&mut self, auto_compact: AutoCompact) {
        self.data.auto_compact = auto_compact;
    }

    fn turn_running(&self) -> bool {
        self.shared.locked().turn.is_some()
    }

    /// Sends `update` to the session `parent` names: the chat's own, or the
    /// subagent a tool call spawned. A subagent not yet announced holds it.
    fn send(&mut self, parent: Option<&str>, update: Value) {
        if self.shared.locked().hushed {
            return;
        }
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

    pub fn handle(&mut self, message: Message) {
        let said = matches!(
            message,
            Message::StreamEvent { .. }
                | Message::Assistant(_)
                | Message::User(_)
                | Message::System(_)
        );
        if said && self.shared.locked().hushed {
            return;
        }
        match message {
            Message::ControlResponse { response } => {
                if let Some((purpose, result)) = self.control.answered(response) {
                    self.purpose_answered(&purpose, result);
                }
            }
            Message::ControlRequest {
                request_id,
                request,
            } => self.request(request_id, request),
            Message::ControlCancelRequest { request_id } => self.asking.take_back(&request_id),
            Message::StreamEvent {
                parent_tool_use_id,
                event,
            } => self.stream_event(parent_tool_use_id, event),
            Message::Assistant(message) => self.assistant(&message),
            Message::User(message) => self.user(&message),
            Message::Result(result) => self.result(&result),
            Message::System(message) => self.system(message),
            Message::CommandLifecycle {
                state,
                command_uuid,
            } => {
                if state.as_deref() == Some("queued") {
                    return;
                }
                let mut unread = None;
                if let Some(uuid) = command_uuid {
                    if let Some(turn) = self.shared.locked().turn.as_mut() {
                        turn.waiting.remove(&uuid);
                        let letter = turn.letters.remove(&uuid);
                        if matches!(state.as_deref(), Some("cancelled" | "dropped")) {
                            unread = letter;
                        }
                    }
                }
                if let Some(letter) = unread {
                    self.sink.letter_unread(letter);
                }
            }
            Message::RateLimitEvent { rate_limit_info } => self
                .sink
                .plan_limits(crate::agent_accounts::usage::claude_live(&rate_limit_info)),
            Message::ConversationReset => {
                self.data.task_plan.clear();
                self.send(None, self.data.task_plan.plan());
            }
            Message::PromptSuggestion { suggestion } => {
                if std::mem::take(&mut self.suggests) && !self.turn_running() {
                    self.sink.update(
                        &self.data.session_id,
                        sikemux_fold::update::prompt_suggestion(&suggestion),
                    );
                }
            }
            Message::Other => {}
        }
    }

    fn request(&mut self, request_id: String, request: Value) {
        const HANDOFF_NO_TOOLS: &str = "Write the handoff note only, with no tools";
        match text(&request, "subtype") {
            Some("can_use_tool") if self.shared.locked().hushed => {
                self.control.refuse(&request_id, HANDOFF_NO_TOOLS)
            }
            Some("can_use_tool") => self.can_use_tool(request_id, request),
            Some("hook_callback") => {
                let answer = match text(&request, "callback_id") {
                    Some(cap::KEEP_HOOK) => {
                        json!({ "continue": true, "systemMessage": self.sink.keep_list() })
                    }
                    Some(cap::AFTER_COMPACT_HOOK) => match self.sink.after_compaction() {
                        Some(context) => json!({
                            "continue": true,
                            "hookSpecificOutput": {
                                "hookEventName": "SessionStart",
                                "additionalContext": context,
                            },
                        }),
                        None => json!({ "continue": true }),
                    },
                    Some(cap::TOOL_HOOK) if self.shared.locked().hushed => json!({
                        "continue": true,
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": HANDOFF_NO_TOOLS,
                        },
                    }),
                    _ => json!({ "continue": true }),
                };
                self.control.respond(&request_id, answer)
            }
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
        if name == "AskUserQuestion" && permission::is_claude_code_question(&tool_id) {
            let asked = permission::claude_code_question(&session, &tool_id, &input);
            let reply = Asked {
                request_id,
                tool_id,
                input,
                request,
            };
            self.sink
                .ask_person(asked, reply, |reply| self.asking.responder(reply));
            return;
        }
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
        let reply = Asked {
            request_id,
            tool_id,
            input,
            request,
        };
        self.sink
            .ask(asked, reply, |reply| self.asking.responder(reply));
    }

    pub fn asking(&self) -> Asking {
        self.asking.clone()
    }

    fn purpose_answered(&self, purpose: &Purpose, result: Answered) {
        const MODE_ASKS: u8 = 3;
        if let (Purpose::Mode(asked_before), Err(error)) = (purpose, &result) {
            let mode = self.shared.locked().mode.clone();
            let asked = asked_before + 1;
            if asked < MODE_ASKS && self.control.restore_mode(&mode, asked).is_ok() {
                return;
            }
            eprintln!(
                "sikemux core: Claude kept refusing to go back to its permission mode {mode} after a handoff note: {}",
                error_text(error)
            );
            return;
        }
        if *purpose == Purpose::Title {
            let title = result
                .ok()
                .and_then(|named| feed::title(text(&named, "title")?));
            if let Some(title) = title {
                self.sink.update(
                    &self.data.session_id,
                    sikemux_fold::update::session_title(&title),
                );
            }
        }
    }

    fn stream_event(&mut self, parent: Option<String>, event: StreamEvent) {
        match event {
            StreamEvent::MessageStart { message } => {
                self.data.stream_message_id = message.id;
                if parent.is_none() {
                    self.data.streamed.clear();
                    if let Some(model) = message.model {
                        self.note_model(&model);
                        self.data.model = Some(model);
                    }
                    if let Some(used) = message.usage.as_ref().and_then(usage_total) {
                        self.data.used = Some(used);
                    }
                    if let Some(usage) = message.usage.as_ref() {
                        self.called(usage);
                    }
                } else {
                    self.data.others_called = true;
                }
            }
            StreamEvent::ContentBlockStart { content_block } => {
                if matches!(
                    text(&content_block, "type"),
                    Some("tool_use" | "server_tool_use" | "mcp_tool_use")
                ) {
                    self.tool_use(&content_block, parent.as_deref(), true);
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let (kind, chunk) = match delta {
                    Delta::Text { text } => ("text", text),
                    Delta::Thinking { thinking } => ("thinking", thinking),
                    Delta::Other => return,
                };
                if chunk.is_empty() {
                    return;
                }
                if parent.is_none() {
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
                let message_id = self
                    .data
                    .stream_message_id
                    .as_deref()
                    .filter(|id| !id.is_empty());
                let update = if kind == "text" {
                    agent_text(&chunk, message_id)
                } else {
                    thought(&chunk, message_id)
                };
                self.send(parent.as_deref(), update);
            }
            StreamEvent::MessageDelta { usage } if parent.is_none() => {
                let output = usage.and_then(|usage| usage.output_tokens);
                if let (Some(used), Some(output)) = (self.data.used, output) {
                    self.data.used = Some(used.max(output));
                }
                if let (Some(call), Some(output)) = (self.data.calls.last_mut(), output) {
                    call.output = call.output.max(output);
                }
            }
            StreamEvent::MessageDelta { .. } | StreamEvent::Other => {}
        }
    }

    fn tool_use(&mut self, block: &Value, parent: Option<&str>, streaming: bool) {
        let Some(id) = text(block, "id").map(str::to_owned) else {
            return;
        };
        let name = text(block, "name").unwrap_or("Tool").to_owned();
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let session = self.session_of(parent);
        self.data.others_called |= tools::is_subagent_tool(&name);
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
        for meta in message
            .get("tool_use_meta")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(id), Some(server), Some(tool)) = (
                text(meta, "id"),
                text(meta, "server_display_name"),
                text(meta, "tool_name"),
            ) {
                self.apps.called(id, server, tool);
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
            let ended = self.apps.ended(&id);
            match ended {
                Shows::MaybeApp { server, tool }
                    if update.get("status").and_then(Value::as_str) == Some("completed") =>
                {
                    let call_result = apps::call_result(structured, result);
                    self.show_app(
                        id.clone(),
                        session,
                        update,
                        server,
                        tool,
                        input,
                        call_result,
                    );
                }
                _ => self.sink.update(&session, update),
            }
            // Only the call's name and session are read once it has its result, and its input
            // can hold whole files, such as a Write's.
            if let Some(tool) = self.data.tools.get_mut(&id) {
                tool.input = Value::Null;
            }
        }
    }

    /// Reads and keeps the page the call names before its end is told, so the chat shows the
    /// page in its place; a page that cannot be read leaves the call as any other.
    #[allow(clippy::too_many_arguments)]
    fn show_app(
        &self,
        id: String,
        session: String,
        update: Value,
        server: String,
        tool: String,
        input: Value,
        result: Value,
    ) {
        let (apps, control, sink) = (self.apps.clone(), self.control.clone(), self.sink.clone());
        tokio::spawn(async move {
            let read = tokio::time::timeout(
                apps::READ_WAIT,
                apps.read(&control, &server, &tool, input, result),
            )
            .await
            .unwrap_or_else(|_| Err("its server took too long".into()));
            match read {
                Ok(Some(record)) => {
                    if let Err(error) = sink.keep_app(&id, &record) {
                        eprintln!(
                            "sikemux: the app of {server}'s {tool} could not be kept: {error}"
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("sikemux: the app of {server}'s {tool} could not be read: {error}")
                }
            }
            sink.update(&session, update);
        });
    }

    /// What a slash command the person ran printed. Claude Code also says so
    /// when the chat changes its model or settings between turns, and that is
    /// no reply to anything, nor the agent starting work of its own.
    fn local_output(&mut self, parent: Option<&str>, output: String) {
        if !self.turn_running() || self.quiet() {
            return;
        }
        // A capped chat's compaction hooks answer with what it keeps, which is not for the chat.
        let output = match output.split_once(" PreCompact [") {
            Some((said, _)) if self.sink.capped() => said.to_owned(),
            _ => output,
        };
        self.send(
            parent,
            json!({ "sessionUpdate": UpdateKind::AgentMessageChunk, "content": { "type": "text", "text": output } }),
        );
    }

    fn result(&mut self, result: &TurnResult) {
        self.suggests = false;
        if let Some(size) = result
            .model_usage
            .as_ref()
            .and_then(|usage| self.window_of(usage))
        {
            self.data.size = size;
        }
        let used = self.data.used;
        if let Some(used) = used {
            let mut update = sikemux_fold::update::usage(used, self.data.size);
            let model = self.data.model.as_deref();
            if let Some(at) = self.data.auto_compact.at(model, self.data.size) {
                update["compactAt"] = json!(at);
            }
            if let Some(cost) = result.total_cost_usd {
                update["cost"] = json!({ "amount": cost, "currency": "USD" });
            }
            self.sink.update(&self.data.session_id, update);
        }
        self.tally(result);
        let answers = result.user_message_uuid.as_deref();
        let quiet = {
            let mut shared = self.shared.locked();
            let quiet = answers.is_some() && shared.quiet.as_deref() == answers;
            if quiet {
                shared.quiet = None;
                if std::mem::take(&mut shared.hushed) {
                    shared.handed = Some(result.result.clone().unwrap_or_default());
                    let _ = self.control.restore_mode(&shared.mode, 0);
                }
            }
            quiet
        };
        if quiet {
            self.sink.compaction_over();
            return;
        }
        let woken = result.origin.is_some();
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
                self.sink.work_ended(StopReason::EndTurn);
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
        let end = if let Some(end) = self.ended_already(&mut turn) {
            end
        } else if result.is_error {
            let said = result
                .result
                .clone()
                .or_else(|| {
                    let errors: Vec<&str> =
                        result.errors.iter().filter_map(Value::as_str).collect();
                    (!errors.is_empty()).then(|| errors.join(", "))
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
                response: Some(turn_answer(StopReason::EndTurn)),
            }
        } else {
            TurnEnd::Completed(turn_answer(stop_reason(result)))
        };
        self.data.offline_retries = false;
        self.suggests = !turn.cancelled && matches!(end, TurnEnd::Completed(_));
        self.sink.end_turn(turn.number, end);
    }

    fn quiet(&self) -> bool {
        self.shared.locked().quiet.is_some()
    }

    /// A call the session made to the model, as it starts streaming. The call that writes a
    /// compaction's summary is the compaction's, not the chat's.
    fn called(&mut self, usage: &Value) {
        if self.data.compaction.is_some() || self.quiet() {
            return;
        }
        let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let call = ModelCall {
            at_ms: sikemux_process::clock::epoch_ms(),
            input: field("input_tokens"),
            cache_read: field("cache_read_input_tokens"),
            cache_write: field("cache_creation_input_tokens"),
            output: 0,
        };
        self.sink.called(call.context());
        self.data.calls.push(call);
    }

    fn boundary(&mut self, metadata: CompactMetadata) {
        let after = metadata.post_tokens.unwrap_or(0);
        if metadata.post_tokens.is_some() {
            self.data.used = Some(after);
        }
        self.sink.compacted(after);
        let trigger = if self.quiet() {
            "idle".to_owned()
        } else {
            metadata.trigger.unwrap_or_else(|| "auto".to_owned())
        };
        self.data.compactions.push(Compaction {
            at_ms: sikemux_process::clock::epoch_ms(),
            trigger,
            before: metadata.pre_tokens.unwrap_or(0),
            after,
            ms: metadata.duration_ms.unwrap_or(0),
            cost: None,
        });
    }

    /// Keeps what the session's calls carried since Claude last answered. What Claude counted
    /// beyond those calls went on writing a compaction's summary, when one ran alone; beside a
    /// subagent's calls or another compaction its cost is unknown.
    fn tally(&mut self, result: &TurnResult) {
        let calls = std::mem::take(&mut self.data.calls);
        let mut compactions = std::mem::take(&mut self.data.compactions);
        let others = std::mem::take(&mut self.data.others_called);
        let counted = result.model_usage_since_last_result.as_ref();
        if let ([compaction], Some(counted), false) = (compactions.as_mut_slice(), counted, others)
        {
            let total = |key: &str| -> u64 {
                counted
                    .values()
                    .filter_map(|usage| usage.get(key).and_then(Value::as_u64))
                    .sum()
            };
            let seen = |part: fn(&ModelCall) -> u64| calls.iter().map(part).sum::<u64>();
            compaction.cost = Some(ModelCall {
                at_ms: compaction.at_ms,
                input: total("inputTokens").saturating_sub(seen(|call| call.input)),
                cache_read: total("cacheReadInputTokens")
                    .saturating_sub(seen(|call| call.cache_read)),
                cache_write: total("cacheCreationInputTokens")
                    .saturating_sub(seen(|call| call.cache_write)),
                output: total("outputTokens").saturating_sub(seen(|call| call.output)),
            });
        }
        self.sink.session_used(calls, compactions);
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

    fn system(&mut self, message: System) {
        if let Some(version) = message.claude_code_version {
            self.sink.cli_version(&version);
        }
        match message.event {
            SystemEvent::SessionStateChanged { state } if state.as_deref() == Some("idle") => {
                self.idle()
            }
            SystemEvent::Init { capabilities } => {
                self.apps.started_turn(&capabilities);
                if !self.quiet() {
                    self.sink.work_started();
                }
            }
            SystemEvent::Status {
                status,
                uuid,
                compact_result,
                compact_error,
            } => self.status(status, uuid, compact_result, compact_error),
            SystemEvent::ApiRetry {
                attempt,
                error_status,
            } => self.retrying(attempt.unwrap_or(1), error_status),
            SystemEvent::CompactBoundary { compact_metadata } => {
                if let Some(metadata) = compact_metadata {
                    self.boundary(metadata);
                }
                self.compacted("completed", None);
            }
            SystemEvent::TaskStarted(message) => self.task_started(&message),
            SystemEvent::TaskProgress(message) => self.task_progress(&message),
            SystemEvent::TaskUpdated { task_id, patch } => {
                if patch.is_backgrounded == Some(true) {
                    self.backgrounded(&task_id);
                }
                if let Some(status) = &patch.status {
                    self.task_ended(&task_id, status, None, patch.output_file.as_deref());
                }
            }
            SystemEvent::TaskNotification {
                task_id,
                status,
                summary,
                output_file,
            } => {
                let status = status.unwrap_or_else(|| "completed".to_owned());
                let summary = summary.as_deref();
                // Work started outside any tracked task still wakes the agent.
                if !self.data.tasks.contains_key(&task_id)
                    && !self.data.subagents.contains_key(&task_id)
                {
                    // A resumed session reports every task its last process left behind as stopped.
                    if self.data.foreground.contains_key(&task_id)
                        || states::outcome(&status) == Some(states::Outcome::Stopped)
                    {
                        return;
                    }
                    let name = summary
                        .and_then(|summary| summary.split('"').nth(1))
                        .or(summary)
                        .unwrap_or("Background task")
                        .to_owned();
                    self.task_notice(&task_id, &name, &status);
                    return;
                }
                self.task_ended(&task_id, &status, summary, output_file.as_deref());
            }
            SystemEvent::BackgroundTasksChanged { tasks } => {
                let live: HashSet<&str> = tasks
                    .iter()
                    .filter_map(|task| task.task_id.as_deref())
                    .collect();
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
            SystemEvent::Informational {
                level,
                content: Some(content),
            } if level.as_deref() == Some("warning") => self.notice("warning", &content),
            SystemEvent::ModelRefusalFallback {
                original_model,
                fallback_model,
            } => {
                let original = original_model.as_deref().unwrap_or("The model");
                let fallback = fallback_model.as_deref().unwrap_or("another model");
                self.notice(
                    "warning",
                    &format!("{original} declined this request; retried with {fallback}."),
                );
            }
            SystemEvent::CommandsChanged {
                commands: Some(commands),
            } => self
                .sink
                .update(&self.data.session_id, available_commands(&commands)),
            SystemEvent::LocalCommandOutput {
                content: Some(content),
            } => self.send(None, agent_text(&content, None)),
            SystemEvent::PermissionDenied {
                tool_use_id,
                decision_reason,
                message,
            } => {
                if !self.data.shown.contains(&tool_use_id) {
                    return;
                }
                let reason = decision_reason
                    .or(message)
                    .unwrap_or_else(|| "not allowed".to_owned());
                let session = self
                    .data
                    .tools
                    .get(&tool_use_id)
                    .map_or_else(|| self.data.session_id.clone(), |tool| tool.session.clone());
                self.sink.update(
                    &session,
                    json!({
                        "sessionUpdate": UpdateKind::ToolCallUpdate,
                        "toolCallId": tool_use_id,
                        "status": "failed",
                        "content": [text_content(&format!("Permission denied: {reason}"))],
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
        self.sink.update(
            &self.data.session_id,
            notice(severity, &title, description.as_deref()),
        );
    }

    /// A failure Claude gave no reason for reads as the network's when the turn
    /// was retrying with no answer from Anthropic.
    fn offline(&self, failure: Failure) -> Failure {
        if failure.kind == FailureKind::Other && self.data.offline_retries {
            Failure::lost_connection(failure.title)
        } else {
            failure
        }
    }

    /// How a turn ends whatever Claude said of it: stopped, or failed already.
    fn ended_already(&self, turn: &mut Turn) -> Option<TurnEnd> {
        if turn.cancelled {
            return Some(TurnEnd::Completed(turn_answer(StopReason::Cancelled)));
        }
        turn.failure.take().map(|failure| TurnEnd::Failed {
            failure: self.offline(failure),
            response: Some(turn_answer(StopReason::EndTurn)),
        })
    }

    /// Claude Code went idle: anything the turn still waited on is over.
    fn idle(&mut self) {
        if self.quiet() {
            return;
        }
        self.sink.compaction_over();
        let turn = self.shared.locked().turn.take();
        match turn {
            Some(mut turn) => {
                let end = self
                    .ended_already(&mut turn)
                    .unwrap_or_else(|| TurnEnd::Completed(turn_answer(StopReason::EndTurn)));
                self.data.offline_retries = false;
                self.sink.end_turn(turn.number, end);
            }
            None => self.sink.work_ended(StopReason::EndTurn),
        }
    }

    fn status(
        &mut self,
        status: Option<String>,
        uuid: Option<String>,
        compact_result: Option<String>,
        compact_error: Option<String>,
    ) {
        if status.as_deref() == Some("compacting") && self.data.compaction.is_none() {
            let id = uuid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.data.compaction = Some(id.clone());
            self.send(None, compaction(&id, true, "in_progress", None));
        }
        match compact_result.as_deref() {
            Some("success") => self.compacted("completed", None),
            Some("failed") => {
                let error = compact_error.as_deref().unwrap_or("unknown error");
                self.compacted("failed", Some(format!("Compaction failed: {error}")));
                self.shared.locked().quiet = None;
                self.sink.compaction_over();
            }
            _ => {}
        }
    }

    /// Claude Code retries a failed request on its own, for up to minutes.
    fn retrying(&mut self, attempt: u64, status: Option<u64>) {
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
        let update = crate::acp::failure::retrying("api_retry", category, attempt, &detail);
        self.sink.update(&self.data.session_id, update);
    }

    fn compacted(&mut self, status: &str, error: Option<String>) {
        let Some(id) = self.data.compaction.take() else {
            return;
        };
        let update = compaction(&id, false, status, error.as_deref());
        self.sink.update(&self.data.session_id, update);
    }

    fn task_started(&mut self, message: &Value) {
        self.data.others_called = true;
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
            "sessionUpdate": UpdateKind::AsyncTaskSpawned,
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
        let mut spawned = states::spawned(&session_id, &name, &task);
        spawned["canStop"] = json!(true);
        self.sink.update(&parent_session, spawned);
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
                let update = sikemux_fold::update::subagent_usage(id, tokens);
                self.sink.update(&child.parent, update);
            }
            return;
        }
        let Some(task) = self.data.tasks.get_mut(id).filter(|task| !task.ended) else {
            return;
        };
        let mut update =
            json!({ "sessionUpdate": UpdateKind::AsyncTaskProgress, "asyncTaskId": id });
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
        let mut update = json!({ "sessionUpdate": UpdateKind::TaskNotice, "taskId": task_id, "name": name, "state": state.as_str() });
        if self.data.subagents.contains_key(task_id) {
            update["subagentSessionId"] = json!(task_id);
        }
        self.sink.update(&self.data.session_id, update);
    }

    /// A task or subagent the person stopped from the app, said at once rather
    /// than when Claude Code gets round to it.
    pub fn task_stopped(&mut self, task_id: &str) {
        if let Some(child) = self.data.subagents.get_mut(task_id) {
            if !child.ended {
                child.ended = true;
                let parent = child.parent.clone();
                self.sink
                    .update(&parent, Subagent::Cancelled.update(task_id));
            }
            return;
        }
        let Some(task) = self.data.tasks.get_mut(task_id).filter(|task| !task.ended) else {
            return;
        };
        task.ended = true;
        let name = task.name.clone();
        self.sink
            .update(&self.data.session_id, TaskState::Stopped.update(task_id));
        self.sink.update(
            &self.data.session_id,
            notice("info", "Task stopped by user", Some(&format!("{name}."))),
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
    type Message = Message;

    fn message(&mut self, message: Message) {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    request_id: String,
    tool_id: String,
    input: Value,
    request: Value,
}

/// Claude's permission requests still waiting on the person, by Claude's own request id.
#[derive(Clone)]
pub struct Asking {
    control: Control,
    open: Arc<Mutex<HashSet<String>>>,
}

impl Asking {
    /// Answers the request `reply` holds with the person's choice, unless
    /// Claude has taken the request back by then.
    pub fn responder(&self, reply: Asked) -> Respond {
        self.open.locked().insert(reply.request_id.clone());
        let asking = self.clone();
        Box::new(move |chosen| {
            let still_open = asking.open.locked().remove(&reply.request_id);
            if !still_open {
                return;
            }
            let answer = permission::answer(
                &reply.tool_id,
                &reply.input,
                &reply.request,
                chosen.as_deref(),
            );
            asking.control.respond(&reply.request_id, answer);
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
            text("You've hit your weekly limit · resets Oct 17, 2099, 3:30pm (UTC)"),
            Some(4_095_934_200_000)
        );
        let next_three = |now: u64| {
            let three = now - now % 86_400 + 15 * 3600;
            if three > now {
                three
            } else {
                three + 86_400
            }
        };
        let utc = text("You've hit your session limit · resets 3pm (UTC)");
        let later = sikemux_process::clock::epoch_secs();
        assert!(
            utc == Some(next_three(now) * 1000) || utc == Some(next_three(later) * 1000),
            "{utc:?}"
        );
        let local = text("You've hit your limit · resets 3pm").unwrap();
        let a_day_with_a_clock_change = 25 * 3600;
        assert!(local > now * 1000 && local <= (later + a_day_with_a_clock_change) * 1000);
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
}
