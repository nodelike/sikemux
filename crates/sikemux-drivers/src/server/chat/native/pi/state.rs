//! What the chat knows of a Pi session while it runs. Each event comes back as
//! the effects it has on the chat.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::update::{self, agent_text, compaction, notice, thought};
use uuid::Uuid;

use crate::acp::failure::{Failure, FailureKind};

use super::super::super::states::{self, Outcome, Subagent};
use super::super::json::{text, text_or_empty};
use super::super::pending::Purpose;
use super::super::rpc_driver::{self, RpcSession};
use super::super::turns::{Ended, Prompted, Started, Turns};
use super::super::words::spoken;
use super::tools;

pub type Effect = rpc_driver::Effect<Own>;

/// How often a running command's output is told to the chat.
const PARTIAL_EVERY: Duration = Duration::from_millis(250);

/// Pi refuses a prompt with these words when it races a compaction already under way.
pub(super) const COMPACTING: &str = "while compaction is in progress";

pub enum Own {
    Send {
        command: Value,
        purpose: Purpose,
    },
    /// These dialogs no longer wait, so Pi and the person are told.
    CloseDialogs(Vec<String>),
}

fn send(command: Value, purpose: Purpose) -> Effect {
    Effect::Own(Own::Send { command, purpose })
}

/// A prompt or steer waiting for a compaction to end, which Pi takes none during.
#[derive(Serialize, Deserialize)]
pub struct Held {
    command: Value,
    purpose: Purpose,
}

#[derive(Default, Serialize, Deserialize)]
struct Used {
    input: u64,
    cached: u64,
    output: u64,
    thought: u64,
}

impl Used {
    fn add(&mut self, usage: &Value) {
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        self.input += count("input") + count("cacheRead") + count("cacheWrite");
        self.cached += count("cacheRead");
        self.output += count("output");
        self.thought += count("reasoning");
    }

    fn answer(&self) -> Value {
        update::TurnUsage {
            total: self.input + self.output,
            input: self.input,
            cached: self.cached,
            output: self.output,
            thought: self.thought,
        }
        .answer()
    }
}

#[derive(Serialize, Deserialize)]
struct Call {
    tool: String,
    args: Value,
}

/// The card a tool's nested calls show under.
#[derive(Serialize, Deserialize)]
struct Nest {
    child: String,
    ended: bool,
}

/// What a turn's error says the person can do about it.
pub fn failure(said: &str) -> Failure {
    let lower = said.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let kind = if says(&[
        "no api key",
        "unauthorized",
        "invalid api key",
        "authentication",
        "/login",
        "no model",
    ]) {
        FailureKind::SignIn
    } else if says(&["rate limit", "quota", "usage limit", "too many requests"]) {
        FailureKind::Limit
    } else {
        FailureKind::Other
    };
    Failure {
        kind,
        title: said.to_owned(),
        details: None,
        resets_at: None,
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub cwd: String,
    pub turns: Turns,
    pub context_window: u64,
    /// The latest usage Pi's `get_session_stats` gave, for `/status`.
    pub stats: Value,
    /// The app's message that began the running turn.
    pub turn_message: Option<String>,
    /// The last prompt written to Pi, to send again if it raced a compaction.
    pub sent_prompt: Option<Value>,
    /// The last session entry the chat has read.
    pub leaf: Option<String>,
    /// The entry each of the app's messages became, for going back to before it.
    pub message_entries: HashMap<String, String>,
    /// What the person chose to always allow for this session.
    pub allowed: HashSet<String>,
    /// The dialogs waiting on words from the person, and the text each opened with.
    pub word_dialogs: HashMap<String, Option<String>>,
    /// The person stopped, so a dialog an extension goes on to open is declined.
    pub stopped: bool,
    /// The id of the assistant message streaming now, and which parts of it spoke.
    message: Option<String>,
    spoken_parts: HashSet<u64>,
    /// The compaction row open now.
    compacting: Option<String>,
    /// A compaction row has said how the running turn's compaction went.
    compaction_shown: bool,
    held: Vec<Held>,
    /// Held messages went to Pi as the compaction ended, so the turn goes on.
    released: bool,
    /// Pi said it settled, and the answer to whether it is really idle is awaited.
    settling: bool,
    /// Counts what can change that answer, so an older answer is ignored.
    generation: u64,
    failed: Option<String>,
    used: Used,
    calls: HashMap<String, Call>,
    nested: HashMap<String, Nest>,
    #[serde(skip)]
    partial_at: HashMap<String, Instant>,
}

impl Session {
    pub fn new(session_id: String, cwd: String) -> Self {
        Self {
            session_id,
            cwd,
            ..Self::default()
        }
    }

    pub fn is_compacting(&self) -> bool {
        self.compacting.is_some()
    }

    /// Keeps a prompt for when the compaction under way ends.
    pub fn hold(&mut self, command: Value, purpose: Purpose) {
        self.held.push(Held { command, purpose });
    }

    /// A prompt or steer sent since Pi said it settled may have started another run.
    pub fn touched(&mut self) {
        self.generation += 1;
    }

    pub fn probe_if_settling(&mut self) -> Vec<Effect> {
        if self.settling {
            vec![self.probe()]
        } else {
            Vec::new()
        }
    }

    fn probe(&mut self) -> Effect {
        self.generation += 1;
        send(
            json!({ "type": "get_state" }),
            Purpose::Settle(self.generation),
        )
    }

    /// Pi answered whether it is idle, which a newer `generation` has made out of date.
    pub fn probed(&mut self, generation: u64, state: Result<Value, Value>) -> Vec<Effect> {
        if generation != self.generation || !self.settling {
            return Vec::new();
        }
        let busy = state.as_ref().is_ok_and(|state| {
            state["isStreaming"] == true
                || state["isCompacting"] == true
                || state["pendingMessageCount"].as_u64().unwrap_or(0) > 0
        });
        if busy {
            return Vec::new();
        }
        self.finish(Outcome::Completed)
    }

    /// Pi took a prompt the person sent, in `disposition`.
    pub fn accepted(&mut self, disposition: &str) -> Vec<Effect> {
        if disposition == "handled" {
            self.settling = true;
            return vec![self.probe()];
        }
        Vec::new()
    }

    /// Pi answered the person's `/compact`, which is turn `turn`.
    pub fn compact_answered(&mut self, turn: u64, result: Result<Value, String>) -> Vec<Effect> {
        if self.turns.active() != Some(turn_id(turn)) {
            return Vec::new();
        }
        if std::mem::take(&mut self.released) {
            self.settling = true;
            return Vec::new();
        }
        match result {
            Err(error) if !self.compaction_shown => {
                self.failed = Some(error);
                self.finish(Outcome::Failed)
            }
            _ => self.finish(Outcome::Completed),
        }
    }

    pub fn close_dialogs(&mut self) -> Option<Effect> {
        let ids: Vec<String> = self.word_dialogs.drain().map(|(id, _)| id).collect();
        (!ids.is_empty()).then(|| Effect::Own(Own::CloseDialogs(ids)))
    }

    fn finish(&mut self, outcome: Outcome) -> Vec<Effect> {
        self.settling = false;
        self.compaction_shown = false;
        self.released = false;
        self.message = None;
        let said = self.failed.take();
        let mut effects: Vec<Effect> = self.close_dialogs().into_iter().collect();
        while let Some(id) = self.turns.active() {
            match self.turns.completed(&id) {
                Ended::Prompted(turn) => {
                    effects.extend(self.end_turn(turn, outcome, said.as_deref()))
                }
                Ended::Unprompted => effects.extend(self.end_unprompted(outcome)),
                Ended::Neither => break,
            }
        }
        self.used = Used::default();
        self.calls.clear();
        effects.extend(self.close_nests(match outcome {
            Outcome::Completed => Subagent::Completed,
            Outcome::Failed => Subagent::Failed,
            Outcome::Stopped => Subagent::Cancelled,
        }));
        effects.push(send(json!({ "type": "get_session_stats" }), Purpose::Usage));
        let mut entries = json!({ "type": "get_entries" });
        if let Some(leaf) = &self.leaf {
            entries["since"] = json!(leaf);
        }
        effects.push(send(entries, Purpose::Entries(self.turn_message.take())));
        effects
    }

    fn end_turn(&mut self, turn: Prompted, outcome: Outcome, said: Option<&str>) -> Vec<Effect> {
        let mut payload = self.payload();
        payload["usage"] = self.used.answer();
        let outcome = if outcome == Outcome::Completed && said.is_some() {
            Outcome::Failed
        } else {
            outcome
        };
        self.end_prompted(turn, outcome, payload, || {
            failure(said.unwrap_or("Pi could not finish the turn"))
        })
    }

    fn close_nests(&mut self, state: Subagent) -> Vec<Effect> {
        let session = self.session_id.clone();
        self.nested
            .values_mut()
            .filter(|nest| !nest.ended)
            .map(|nest| {
                nest.ended = true;
                Effect::Update(session.clone(), state.update(&nest.child))
            })
            .collect()
    }

    /// Tells the chat what `get_session_stats` said.
    pub fn stats(&mut self, stats: Value) -> Vec<Effect> {
        let context = &stats["contextUsage"];
        let mut effects = Vec::new();
        if let (Some(used), Some(size)) = (
            context["tokens"].as_u64(),
            context["contextWindow"].as_u64(),
        ) {
            effects.push(Effect::Update(
                self.session_id.clone(),
                update::usage(used, size),
            ));
        }
        self.stats = stats;
        effects
    }

    /// `entries` were written since the chat last read; the first user message
    /// among them is the one `message` began its turn with.
    pub fn entries(&mut self, read: &Value, message: Option<String>) {
        let listed = read["entries"].as_array().map_or(&[][..], Vec::as_slice);
        let leaf = read["leafId"].as_str();
        let walked: Vec<&Value> = super::history::branch(listed, leaf);
        if let (Some(message), Some(entry)) = (message, super::history::first_user_entry(&walked)) {
            self.message_entries.insert(message, entry);
        }
        self.leaf = leaf.map(str::to_owned);
    }

    /// The conversation went on in a fork, which starts with its own entries.
    pub fn forked(&mut self, session_id: String) {
        self.session_id = session_id;
        self.leaf = None;
    }

    fn where_tool(&mut self, event: &Value) -> (String, Vec<Effect>) {
        let Some(parent) = text(event, "parentToolCallId") else {
            return (self.session_id.clone(), Vec::new());
        };
        let parent = parent.to_owned();
        if let Some(nest) = self.nested.get(&parent) {
            return (nest.child.clone(), Vec::new());
        }
        let child = format!("pi-tool-{parent}");
        let name = self
            .calls
            .get(&parent)
            .map_or_else(|| "Tool".to_owned(), |call| spoken(&call.tool));
        let opened = Effect::Update(self.session_id.clone(), states::spawned(&child, &name, ""));
        self.nested.insert(
            parent,
            Nest {
                child: child.clone(),
                ended: false,
            },
        );
        (child, vec![opened])
    }

    fn tool_started(&mut self, event: &Value) -> Vec<Effect> {
        let Some(id) = text(event, "toolCallId").map(str::to_owned) else {
            return Vec::new();
        };
        let tool = text_or_empty(event, "toolName").to_owned();
        let (session, mut effects) = self.where_tool(event);
        let call = tools::call(&id, &tool, &event["args"], &self.cwd, "in_progress");
        self.calls.insert(
            id,
            Call {
                tool,
                args: event["args"].clone(),
            },
        );
        effects.push(Effect::Update(session, call));
        effects
    }

    fn tool_progress(&mut self, event: &Value) -> Vec<Effect> {
        let Some(id) = text(event, "toolCallId") else {
            return Vec::new();
        };
        let now = Instant::now();
        if self
            .partial_at
            .get(id)
            .is_some_and(|at| now - *at < PARTIAL_EVERY)
        {
            return Vec::new();
        }
        let tool = self.calls.get(id).map_or_else(
            || text_or_empty(event, "toolName").to_owned(),
            |call| call.tool.clone(),
        );
        let Some(partial) = tools::partial(id, &tool, &event["partialResult"]) else {
            return Vec::new();
        };
        self.partial_at.insert(id.to_owned(), now);
        let (session, effects) = self.where_tool(event);
        effects
            .into_iter()
            .chain([Effect::Update(session, partial)])
            .collect()
    }

    fn tool_ended(&mut self, event: &Value) -> Vec<Effect> {
        let Some(id) = text(event, "toolCallId").map(str::to_owned) else {
            return Vec::new();
        };
        let (session, mut effects) = self.where_tool(event);
        let failed = event["isError"] == true;
        let tool = self.calls.remove(&id).map_or_else(
            || text_or_empty(event, "toolName").to_owned(),
            |call| call.tool,
        );
        self.partial_at.remove(&id);
        effects.push(Effect::Update(
            session,
            tools::done(&id, &tool, &event["result"], failed),
        ));
        if let Some(nest) = self.nested.get_mut(&id).filter(|nest| !nest.ended) {
            nest.ended = true;
            let state = if failed {
                Subagent::Failed
            } else {
                Subagent::Completed
            };
            effects.push(Effect::Update(
                self.session_id.clone(),
                state.update(&nest.child),
            ));
        }
        effects
    }

    fn said(&mut self, part: &Value, delta: &str, thinking: bool) -> Vec<Effect> {
        if delta.is_empty() {
            return Vec::new();
        }
        let index = part["contentIndex"].as_u64().unwrap_or(0);
        self.spoken_parts.insert(index);
        self.part(index, delta, thinking)
    }

    /// A part whose whole text came only at its end, from a provider that does not stream.
    fn part_ended(&mut self, part: &Value, whole: Option<&str>, thinking: bool) -> Vec<Effect> {
        let index = part["contentIndex"].as_u64().unwrap_or(0);
        match whole.filter(|whole| !whole.is_empty()) {
            Some(whole) if self.spoken_parts.insert(index) => self.part(index, whole, thinking),
            _ => Vec::new(),
        }
    }

    fn part(&mut self, index: u64, said: &str, thinking: bool) -> Vec<Effect> {
        let message = self
            .message
            .get_or_insert_with(|| Uuid::now_v7().to_string());
        let id = format!("{message}:{index}");
        let chunk = if thinking {
            thought(said, Some(&id))
        } else {
            agent_text(said, Some(&id))
        };
        vec![Effect::Update(self.session_id.clone(), chunk)]
    }

    fn message_update(&mut self, event: &Value) -> Vec<Effect> {
        let part = &event["assistantMessageEvent"];
        match part["type"].as_str() {
            Some("text_delta") => self.said(part, text_or_empty(part, "delta"), false),
            Some("thinking_delta") => self.said(part, text_or_empty(part, "delta"), true),
            Some("text_end") => self.part_ended(part, text(part, "content"), false),
            Some("thinking_end") => {
                let whole = text(part, "content").or_else(|| text(part, "thinking"));
                self.part_ended(part, whole, true)
            }
            _ => Vec::new(),
        }
    }

    fn message_ended(&mut self, message: &Value) -> Vec<Effect> {
        if message["role"] != "assistant" {
            return Vec::new();
        }
        self.message = None;
        self.spoken_parts.clear();
        self.used.add(&message["usage"]);
        if message["stopReason"] == "error" && self.failed.is_none() {
            self.failed = Some(
                text(message, "errorMessage")
                    .unwrap_or("Pi reported a model error")
                    .to_owned(),
            );
        }
        let used = message["usage"]["totalTokens"].as_u64().unwrap_or(0);
        if used > 0 && self.context_window > 0 {
            return vec![Effect::Update(
                self.session_id.clone(),
                update::usage(used, self.context_window),
            )];
        }
        Vec::new()
    }

    fn compaction_started(&mut self) -> Vec<Effect> {
        self.generation += 1;
        self.compaction_shown = true;
        let id = format!("pi-compaction-{}", Uuid::now_v7());
        let open = compaction(&id, true, "in_progress", None);
        self.compacting = Some(id);
        vec![Effect::Update(self.session_id.clone(), open)]
    }

    fn compaction_ended(&mut self, event: &Value) -> Vec<Effect> {
        let id = self
            .compacting
            .take()
            .unwrap_or_else(|| format!("pi-compaction-{}", Uuid::now_v7()));
        let (status, error) = if !event["result"].is_null() {
            ("completed", None)
        } else if event["aborted"] == true {
            ("failed", Some("Stopped"))
        } else {
            (
                "failed",
                text(event, "errorMessage").or(Some("Pi could not compact the conversation")),
            )
        };
        if event["willRetry"] == true {
            self.failed = None;
        }
        self.released = !self.held.is_empty();
        let mut effects: Vec<Effect> = std::mem::take(&mut self.held)
            .into_iter()
            .map(|held| send(held.command, held.purpose))
            .collect();
        effects.push(Effect::Update(
            self.session_id.clone(),
            compaction(&id, false, status, error),
        ));
        effects.extend(self.probe_if_settling());
        effects
    }

    /// The person stopped the turn: what waits for a compaction waits for nothing.
    pub fn drop_held(&mut self) {
        self.held.clear();
    }

    pub fn handle(&mut self, event: &Value) -> Vec<Effect> {
        match event["type"].as_str() {
            Some("agent_start") => {
                self.stopped = false;
                self.generation += 1;
                self.settling = false;
                if self.turns.active().is_some() {
                    return Vec::new();
                }
                let id = format!("pi-{}", Uuid::now_v7());
                match self.turns.began(&id) {
                    Started::Unprompted => vec![Effect::WorkStarted],
                    _ => Vec::new(),
                }
            }
            Some("message_start") if event["message"]["role"] == "assistant" => {
                self.message = None;
                self.spoken_parts.clear();
                Vec::new()
            }
            Some("message_update") => self.message_update(event),
            Some("message_end") => self.message_ended(&event["message"]),
            Some("tool_execution_start") => self.tool_started(event),
            Some("tool_execution_update") => self.tool_progress(event),
            Some("tool_execution_end") => self.tool_ended(event),
            Some("compaction_start") => self.compaction_started(),
            Some("compaction_end") => self.compaction_ended(event),
            Some("auto_retry_start") => {
                let said = format!(
                    "Retrying (attempt {} of {}): {}",
                    event["attempt"],
                    event["maxAttempts"],
                    text_or_empty(event, "errorMessage"),
                );
                vec![Effect::Update(
                    self.session_id.clone(),
                    notice("warning", &said, None),
                )]
            }
            Some("auto_retry_end") => {
                self.failed = if event["success"] == true {
                    None
                } else {
                    text(event, "finalError")
                        .map(str::to_owned)
                        .or(self.failed.take())
                };
                Vec::new()
            }
            Some("agent_settled") if event["aborted"] == true => self.finish(Outcome::Stopped),
            Some("agent_settled") => {
                if self.turns.active().is_none() {
                    return Vec::new();
                }
                self.settling = true;
                vec![self.probe()]
            }
            Some("extension_error") => {
                let extension = text_or_empty(event, "extensionPath");
                let name = extension_name(extension);
                let title = match text(event, "event") {
                    Some(during) => format!("{name} failed during {during}"),
                    None => format!("{name} failed"),
                };
                vec![Effect::Update(
                    self.session_id.clone(),
                    notice("error", &title, text(event, "error")),
                )]
            }
            _ => Vec::new(),
        }
    }
}

/// An extension's name: a file's stem, or for a folder's index file, the folder.
fn extension_name(path: &str) -> String {
    let mut parts = path.rsplit('/').filter(|part| !part.is_empty());
    let file = parts.next().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let name = if stem == "index" {
        parts.next().unwrap_or(stem)
    } else {
        stem
    };
    if name.is_empty() {
        "A Pi extension".to_owned()
    } else {
        name.to_owned()
    }
}

/// This side's name for prompted turn `number`, since Pi names no turns.
pub fn turn_id(number: u64) -> String {
    format!("pi-turn-{number}")
}

impl RpcSession for Session {
    type Own = Own;
    const NAME: &'static str = "Pi";

    fn id(&self) -> &str {
        &self.session_id
    }

    fn turns(&mut self) -> &mut Turns {
        &mut self.turns
    }

    fn finish_subagents(&mut self, state: Subagent) -> Vec<Effect> {
        self.close_nests(state)
    }
}
