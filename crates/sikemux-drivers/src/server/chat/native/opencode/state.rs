//! What OpenCode's events do to a chat, read one at a time from the server's event stream, or
//! from the session's own log after a gap in it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::update::{self, notice, TurnUsage};
use sikemux_fold::UpdateKind;
use sikemux_view::chat::StopReason;

use crate::acp::failure::Failure;
use crate::server::chat::states::{self, Subagent, UNNAMED_TASK};

use super::super::session::TurnEnd;
use super::tools::{self, Call};

/// What one event does to the chat.
pub enum Effect {
    Update(Value),
    /// What happens in a subagent's session, for its card.
    In(String, Value),
    End(u64, TurnEnd),
    WorkStarted,
    WorkEnded(StopReason),
    /// OpenCode asks the person's leave: the request as the chat puts it, and OpenCode's id
    /// for it.
    Ask {
        session: String,
        request: Value,
        id: String,
    },
    /// A permission was answered elsewhere, so the chat stops asking.
    Settled(String),
    /// The model was called with this many tokens of context.
    Called(u64),
    /// The session's model or mode changed, to be shown in its pickers.
    Selected,
    /// OpenCode asks the person something with a form in `session`, put as the ask tool's
    /// questions.
    Question {
        session: String,
        form: String,
        input: Value,
    },
    /// A compaction ended: the next call starts from its summary.
    Compacted,
    /// A compaction ended either way.
    CompactionOver,
}

/// The turn at work, and whose it is.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
enum Turn {
    #[default]
    Idle,
    /// Prompted and sent, with OpenCode yet to start on it.
    Sent(u64),
    Prompted(u64),
    /// One OpenCode began by itself, such as on a background subagent's report.
    Unprompted,
    /// The core's own compaction while the chat is idle, which shows no turn.
    Quiet,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Tokens {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
}

impl Tokens {
    fn read(tokens: &Value) -> Self {
        let field = |pointer: &str| tokens.pointer(pointer).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: field("/input"),
            output: field("/output"),
            reasoning: field("/reasoning"),
            cache_read: field("/cache/read"),
            cache_write: field("/cache/write"),
        }
    }

    fn add(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// The context the call read: everything it was sent, cached or not.
    fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    fn answer(&self) -> Value {
        TurnUsage {
            total: self.context() + self.output + self.reasoning,
            input: self.context(),
            cached: self.cache_read,
            output: self.output,
            thought: self.reasoning,
        }
        .answer()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    turn: Turn,
    stopping: bool,
    tools: HashMap<String, Call>,
    /// How many bytes of each text the chat was told, by message and part.
    told: HashMap<String, usize>,
    used: Tokens,
    finish: Option<String>,
    error: Option<Value>,
    asks: HashSet<String>,
    pub titled: bool,
    /// The context window of the model the session is on.
    pub window: u64,
    /// What the session has cost so far, in dollars.
    pub cost: f64,
    /// The last of the session's own events read, by its number in the session's log.
    pub seq: u64,
    /// The session whose feed holds this one's card, when this is a subagent's.
    parent: Option<String>,
    /// Every subagent below this session, however deep, by session.
    children: HashMap<String, Session>,
    /// Tokens a subagent's calls have taken, for its card.
    spent: u64,
    /// The compaction row open now.
    compacting: Option<String>,
    /// The next run is the core's compaction of an idle chat.
    quiet: bool,
    /// The forms already put to the person.
    forms: HashSet<String>,
    /// OpenCode's id for each message the chat sent, by the chat's id for it.
    pub messages: HashMap<String, String>,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

impl Session {
    pub fn new(id: String, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            ..Self::default()
        }
    }

    pub fn running(&self) -> bool {
        self.turn != Turn::Idle
    }

    /// Turn `number` is being sent. A turn OpenCode began by itself becomes this one, so it
    /// ends as the chat's.
    pub fn send(&mut self, number: u64) -> Vec<Effect> {
        let was = std::mem::replace(&mut self.turn, Turn::Sent(number));
        match was {
            Turn::Unprompted => {
                self.turn = Turn::Prompted(number);
                vec![Effect::WorkEnded(StopReason::EndTurn)]
            }
            _ => Vec::new(),
        }
    }

    /// OpenCode would not take turn `number`.
    pub fn refused(&mut self, number: u64, said: String) -> Vec<Effect> {
        if matches!(self.turn, Turn::Sent(sent) | Turn::Prompted(sent) if sent == number) {
            self.turn = Turn::Idle;
        }
        vec![Effect::End(number, TurnEnd::Refused(said))]
    }

    pub fn stop(&mut self) {
        if self.running() {
            self.stopping = true;
        }
    }

    /// The core compacts the idle session, so its run shows no turn.
    pub fn compact_quietly(&mut self) {
        self.quiet = !self.running();
    }

    fn holds(&self, session: &str) -> bool {
        session == self.id || self.children.contains_key(session)
    }

    /// The event `event`, from the stream or the session's log.
    pub fn handle(&mut self, event: &Value) -> Vec<Effect> {
        let data = &event["data"];
        let kind = text(event, "type");
        match kind {
            "permission.replied" => {
                let id = text(data, "requestID");
                let asked = self.asks.remove(id)
                    | self
                        .children
                        .values_mut()
                        .any(|child| child.asks.remove(id));
                return match asked {
                    true => vec![Effect::Settled(id.to_owned())],
                    false => Vec::new(),
                };
            }
            "form.created" => return self.form(&data["form"]),
            "session.created" => return self.spawned(data),
            _ => {}
        }
        let session = text(data, "sessionID");
        if session != self.id {
            return self.in_child(session, event);
        }
        if let Some(seq) = event.pointer("/durable/seq").and_then(Value::as_u64) {
            if seq <= self.seq {
                return Vec::new();
            }
            self.seq = seq;
        }
        match kind {
            "session.execution.started" => self.started(),
            "session.execution.succeeded" => self.ended(Ending::Succeeded),
            "session.execution.interrupted" => self.ended(Ending::Interrupted),
            "session.execution.failed" => {
                self.error = Some(data["error"].clone());
                self.ended(Ending::Failed)
            }
            "session.text.delta" => self.said(data, false, text(data, "delta")),
            "session.text.ended" => self.finished(data, false),
            "session.reasoning.delta" => self.said(data, true, text(data, "delta")),
            "session.reasoning.ended" => self.finished(data, true),
            "session.tool.input.started" => {
                let id = text(data, "id").to_owned();
                let name = text(data, "name");
                self.tools.insert(
                    id.clone(),
                    Call {
                        name: name.to_owned(),
                        ..Call::default()
                    },
                );
                if tools::is_todo(name) {
                    return Vec::new();
                }
                vec![Effect::Update(tools::pending(&id, name, &self.cwd))]
            }
            "session.tool.called" => {
                let id = text(data, "id");
                let call = self.tools.entry(id.to_owned()).or_default();
                call.input = data["input"].clone();
                if tools::is_todo(&call.name) {
                    return vec![Effect::Update(tools::todo_plan(&call.input))];
                }
                vec![Effect::Update(tools::running(id, call, &self.cwd))]
            }
            "session.tool.progress" => {
                let id = text(data, "id");
                let Some(call) = self
                    .tools
                    .get_mut(id)
                    .filter(|call| !tools::is_todo(&call.name))
                else {
                    return Vec::new();
                };
                call.metadata = data["metadata"].clone();
                vec![Effect::Update(tools::running(id, call, &self.cwd))]
            }
            "session.tool.success" => {
                let id = text(data, "id");
                let call = self.tools.remove(id).unwrap_or_default();
                if tools::is_todo(&call.name) {
                    return Vec::new();
                }
                vec![Effect::Update(tools::completed(
                    id,
                    &call,
                    &data["content"],
                    &data["metadata"],
                    &self.cwd,
                ))]
            }
            "session.tool.failed" => {
                let id = text(data, "id");
                let call = self.tools.remove(id).unwrap_or_default();
                if tools::is_todo(&call.name) {
                    return Vec::new();
                }
                let metadata = match &data["metadata"] {
                    Value::Null => &call.metadata,
                    metadata => metadata,
                };
                vec![Effect::Update(tools::failed(
                    id,
                    &call,
                    &data["content"],
                    metadata,
                    text(&data["error"], "message"),
                    &self.cwd,
                ))]
            }
            "session.step.ended" => {
                let tokens = Tokens::read(&data["tokens"]);
                self.used.add(tokens);
                self.spent += tokens.context() + tokens.output + tokens.reasoning;
                self.finish = data["finish"].as_str().map(str::to_owned);
                self.cost += data["cost"].as_f64().unwrap_or(0.0);
                self.stepped(tokens)
            }
            "session.step.failed" => {
                if data["tokens"].is_object() {
                    self.used.add(Tokens::read(&data["tokens"]));
                }
                self.error = Some(data["error"].clone());
                Vec::new()
            }
            "session.renamed" => {
                let title = text(data, "title");
                if title.is_empty() {
                    return Vec::new();
                }
                self.titled = true;
                vec![Effect::Update(update::session_title(title))]
            }
            "session.retry.scheduled" => {
                let said = format!(
                    "OpenCode is trying the model again: {}",
                    text(&data["error"], "message")
                );
                vec![Effect::Update(notice("warning", &said, None))]
            }
            "session.model.selected" | "session.agent.selected" => vec![Effect::Selected],
            "permission.asked" => {
                let id = text(data, "id").to_owned();
                if !self.asks.insert(id.clone()) {
                    return Vec::new();
                }
                let call = data["source"]["id"]
                    .as_str()
                    .and_then(|call| self.tools.get(call));
                let request = tools::permission(&self.id, data, call, &self.cwd);
                vec![Effect::Ask {
                    session: self.id.clone(),
                    request,
                    id,
                }]
            }
            "session.compaction.started" => self.compaction_started(),
            "session.compaction.ended" => {
                let mut effects = self.compaction_ended("completed", None);
                effects.push(Effect::Compacted);
                effects
            }
            "session.compaction.failed" => {
                let said = text(&data["error"], "message");
                let said = format!("Compaction failed: {said}");
                self.compaction_ended("failed", Some(&said))
            }
            _ => Vec::new(),
        }
    }

    /// A subagent's session began: a card for it in the feed of the session that started it.
    fn spawned(&mut self, data: &Value) -> Vec<Effect> {
        let child = text(data, "sessionID");
        let parent = text(data, "parentID");
        if parent.is_empty() || !self.holds(parent) || self.holds(child) {
            return Vec::new();
        }
        let mut session = Session::new(child.to_owned(), self.cwd.clone());
        session.parent = Some(parent.to_owned());
        self.children.insert(child.to_owned(), session);
        let name = match text(data, "agent") {
            "" => states::unnamed(child),
            agent => agent.to_owned(),
        };
        let task = match text(data, "title") {
            "" => UNNAMED_TASK,
            title => title,
        };
        vec![Effect::In(
            parent.to_owned(),
            states::spawned(child, &name, task),
        )]
    }

    /// An event of a subagent's session, onto its card.
    fn in_child(&mut self, session: &str, event: &Value) -> Vec<Effect> {
        let Some(child) = self.children.get_mut(session) else {
            return Vec::new();
        };
        let parent = child.parent.clone().unwrap_or_default();
        let spent = child.spent;
        let effects = child.handle(event);
        let failed = child.failure().is_some();
        let mut out: Vec<Effect> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Update(update) => Some(Effect::In(session.to_owned(), update)),
                Effect::WorkStarted => Some(Effect::In(
                    parent.clone(),
                    Subagent::Running.update(session),
                )),
                Effect::WorkEnded(reason) => {
                    let state = match reason {
                        StopReason::Cancelled => Subagent::Cancelled,
                        _ if failed => Subagent::Failed,
                        _ => Subagent::Completed,
                    };
                    Some(Effect::In(parent.clone(), state.update(session)))
                }
                Effect::In(..)
                | Effect::Ask { .. }
                | Effect::Settled(_)
                | Effect::Question { .. } => Some(effect),
                Effect::End(..)
                | Effect::Called(_)
                | Effect::Selected
                | Effect::Compacted
                | Effect::CompactionOver => None,
            })
            .collect();
        let spent_now = self
            .children
            .get(session)
            .map_or(spent, |child| child.spent);
        if spent_now != spent {
            out.push(Effect::In(
                parent,
                update::subagent_usage(session, spent_now),
            ));
        }
        out
    }

    /// OpenCode asks the person something, such as with its question tool.
    fn form(&mut self, form: &Value) -> Vec<Effect> {
        let session = text(form, "sessionID");
        let id = text(form, "id");
        if !self.holds(session) || !self.forms.insert(id.to_owned()) {
            return Vec::new();
        }
        vec![Effect::Question {
            session: session.to_owned(),
            form: id.to_owned(),
            input: tools::form_questions(form),
        }]
    }

    fn compaction_started(&mut self) -> Vec<Effect> {
        if self.turn == Turn::Quiet {
            return Vec::new();
        }
        let id = format!("opencode-compaction-{}", uuid::Uuid::now_v7());
        let open = update::compaction(&id, true, "in_progress", None);
        self.compacting = Some(id);
        vec![Effect::Update(open)]
    }

    fn compaction_ended(&mut self, status: &str, error: Option<&str>) -> Vec<Effect> {
        let mut effects = vec![Effect::CompactionOver];
        if let Some(id) = self.compacting.take() {
            effects.push(Effect::Update(update::compaction(
                &id, false, status, error,
            )));
        }
        effects
    }

    fn started(&mut self) -> Vec<Effect> {
        self.used = Tokens::default();
        self.finish = None;
        self.error = None;
        match self.turn {
            Turn::Sent(number) => {
                self.turn = Turn::Prompted(number);
                Vec::new()
            }
            Turn::Idle if std::mem::take(&mut self.quiet) => {
                self.turn = Turn::Quiet;
                Vec::new()
            }
            Turn::Idle => {
                self.turn = Turn::Unprompted;
                vec![Effect::WorkStarted]
            }
            Turn::Prompted(_) | Turn::Unprompted | Turn::Quiet => Vec::new(),
        }
    }

    fn ended(&mut self, ending: Ending) -> Vec<Effect> {
        let mut effects = self.abandon_tools();
        let reason = self.reason(ending);
        let turn = std::mem::take(&mut self.turn);
        self.stopping = false;
        match turn {
            Turn::Sent(number) | Turn::Prompted(number) => {
                let mut answer = update::turn_answer(reason);
                answer["usage"] = self.used.answer();
                let end = match (ending, self.failure()) {
                    (Ending::Failed, Some(failure)) | (Ending::Succeeded, Some(failure)) => {
                        TurnEnd::Failed {
                            failure,
                            response: Some(answer),
                        }
                    }
                    _ => TurnEnd::Completed(answer),
                };
                effects.push(Effect::End(number, end.read_offline()));
            }
            Turn::Unprompted => effects.push(Effect::WorkEnded(reason)),
            Turn::Quiet => effects.push(Effect::CompactionOver),
            Turn::Idle => {}
        }
        effects
    }

    /// The calls still open when a turn ends, which never will now.
    fn abandon_tools(&mut self) -> Vec<Effect> {
        let cwd = self.cwd.clone();
        self.tools
            .drain()
            .filter(|(_, call)| !tools::is_todo(&call.name))
            .map(|(id, call)| {
                Effect::Update(tools::failed(
                    &id,
                    &call,
                    &Value::Null,
                    &call.metadata,
                    "Cancelled",
                    &cwd,
                ))
            })
            .collect()
    }

    fn reason(&self, ending: Ending) -> StopReason {
        let error = self.error.as_ref().map(|error| text(error, "type"));
        if matches!(ending, Ending::Interrupted) || self.stopping || error == Some("aborted") {
            return StopReason::Cancelled;
        }
        match self.finish.as_deref() {
            Some("length") => StopReason::MaxTokens,
            Some("content-filter") => StopReason::Refusal,
            _ if error == Some("provider.content-filter") => StopReason::Refusal,
            _ => StopReason::EndTurn,
        }
    }

    /// What went wrong, unless the turn was stopped or the provider would not answer it.
    fn failure(&self) -> Option<Failure> {
        let error = self.error.as_ref()?;
        let message = error["message"]
            .as_str()
            .filter(|said| !said.is_empty())
            .unwrap_or("OpenCode's turn failed");
        match text(error, "type") {
            "aborted" | "provider.content-filter" | "" => None,
            "provider.auth" => Some(Failure::sign_in(message)),
            _ => Some(Failure::other(message)),
        }
    }

    fn stepped(&mut self, tokens: Tokens) -> Vec<Effect> {
        let context = tokens.context();
        if context == 0 {
            return Vec::new();
        }
        let mut effects = vec![Effect::Called(context)];
        if self.window > 0 {
            let mut usage = update::usage(context, self.window);
            usage["cost"] = json!({ "amount": self.cost, "currency": "USD" });
            effects.push(Effect::Update(usage));
        }
        effects
    }

    /// New text of a reply or of its reasoning.
    fn said(&mut self, data: &Value, thinking: bool, said: &str) -> Vec<Effect> {
        if said.is_empty() {
            return Vec::new();
        }
        let key = part_key(data, thinking);
        *self.told.entry(key).or_default() += said.len();
        vec![Effect::Update(chunk(data, thinking, said))]
    }

    /// A text or its reasoning ended: whatever of it the chat was not told, such as while the
    /// stream was down, is told now.
    fn finished(&mut self, data: &Value, thinking: bool) -> Vec<Effect> {
        let key = part_key(data, thinking);
        let told = self.told.remove(&key).unwrap_or(0);
        let whole = text(data, "text");
        match whole.get(told..) {
            Some(rest) if !rest.is_empty() => vec![Effect::Update(chunk(data, thinking, rest))],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum Ending {
    Succeeded,
    Interrupted,
    Failed,
}

fn message_id(data: &Value, thinking: bool) -> String {
    let message = text(data, "assistantMessageID");
    match thinking {
        true => format!("{message}:reasoning:{}", data["ordinal"]),
        false => message.to_owned(),
    }
}

fn part_key(data: &Value, thinking: bool) -> String {
    format!("{}#{}", message_id(data, thinking), data["ordinal"])
}

fn chunk(data: &Value, thinking: bool, said: &str) -> Value {
    let kind = match thinking {
        true => UpdateKind::AgentThoughtChunk,
        false => UpdateKind::AgentMessageChunk,
    };
    json!({
        "sessionUpdate": kind,
        "messageId": message_id(data, thinking),
        "content": { "type": "text", "text": said },
    })
}
