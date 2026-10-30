//! A chat folded from what its agent says into the rows clients draw. The
//! events go in as the core's feed keeps them; what changed goes out at each
//! flush as [`sikemux_wire::chat::ChatChange`]s, so a client never reads an
//! agent's own messages and a streamed token costs only itself.

mod event;
mod find;
pub mod pickers;
mod prompt;
mod row;
mod send;
#[cfg(test)]
mod tests;
mod tool_rows;
mod tools;
pub mod trigger;
mod turns;
pub mod update;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};
use sikemux_markdown::Doc;
use sikemux_wire::protocol::PermissionMode;

use sikemux_wire::chat::{
    AccountMove, AgentNotice, AsyncTask, ChatCapabilities, ChatCommandInfo, ChatConnection,
    ChatDraft, ChatFailure, ChatHead, ChatMessage, ChatPart, ChatRole, ChatTrigger, ContentBlock,
    ContextUsage, FailureKind, MoveReason, NoticeSeverity, NoticeState, PermissionAsk,
    PermissionKind, PermissionOption, PlanEntry, QueuedMessage, Retrying, SentContext,
    SessionEnded, StopReason, SubagentCard, SubagentState, TaskNotice, TaskState, TaskUsage,
    ToolDetail, ToolStatus, TurnStart, UsageCost, WorkflowAgent,
};
use tools::WithRow;

pub use event::{ChatEvent, ChatEventKind};
pub use find::{found_in, matches, searchable, Found, Query};
pub use send::{with_trees, PageOf, PAGE_BYTES, TAIL_TURNS};
use update::stamped_at;
pub use update::UpdateKind;

/// Whether a call is one of our gestures, such as a reaction, which a chat shows
/// only by what it did.
pub fn is_gesture(title: &str) -> bool {
    tools::is_gesture(title)
}

/// The change a call made and what it printed, from everything said of it.
pub fn detail_of(raw: &Map<String, Value>, work: Option<(&Path, &Place)>) -> ToolDetail {
    let mut settled = tools::settle(raw);
    if let (Some(diff), Some((folder, place))) = (settled.diff.as_mut(), work) {
        tools::place(diff, raw, place);
        diff.path = tools::rooted(Some(folder), std::mem::take(&mut diff.path));
    }
    tools::detail(&settled)
}

/// Finds the line an edit's new text starts on in its file as the file is now, from the
/// path the agent gave and that text; none unless it is there exactly once.
pub type Place = std::sync::Arc<dyn Fn(&str, &str) -> Option<u64> + Send + Sync>;

/// A picture bigger than the transcript can reasonably hold onto is kept by
/// name instead of by its bytes: the row still says what it was.
const MAX_IMAGE_DATA_CHARS: usize = 2 * 1024 * 1024;
const MAX_COMMANDS: usize = 256;
/// An agent reacting more than this to one message is noise.
const MAX_REACTIONS: usize = 3;

/// What streamed into an answer and when, which its speed is worked out from.
#[derive(Clone, Copy, Default)]
struct Stream {
    started: u64,
    ended: u64,
    /// UTF-16 units, as clients count characters.
    chars: u64,
}

/// What clients were last sent of a part.
#[derive(Default)]
struct Side {
    /// Bytes of its text already sent.
    sent: usize,
    /// The tree of the text still being written, as its clients hold it.
    doc: Option<Doc>,
}

struct Row {
    msg: ChatMessage,
    /// The event it was opened by.
    index: u64,
    /// The change that last touched it.
    v: u64,
    /// About what it holds, as of the last flush.
    bytes: usize,
    /// A message a person sent, which the agent's echo never adds to.
    local: bool,
    /// What a message to the agent held as it came, before its context and
    /// letters were read out of it.
    raw: Vec<ChatPart>,
    /// The context it was sent with.
    sent_context: Vec<SentContext>,
    stream: Option<Stream>,
    sides: Vec<Side>,
    /// Opened since the last flush, so it goes out whole.
    fresh: bool,
    /// Parts added or replaced since the last flush.
    parts: Vec<usize>,
    fields: BTreeSet<&'static str>,
}

impl Row {
    fn new(id: String, role: ChatRole, index: u64) -> Self {
        Self {
            msg: ChatMessage {
                id,
                role,
                prompt_id: None,
                parts: Vec::new(),
                attachments: Vec::new(),
                context: Vec::new(),
                letters: Vec::new(),
                sent_at: None,
                ended_at: None,
                opens_turn: None,
                meta: None,
                fold: None,
                from_event: Some(index),
                reactions: Vec::new(),
            },
            index,
            v: 0,
            bytes: 0,
            local: false,
            raw: Vec::new(),
            sent_context: Vec::new(),
            stream: None,
            sides: Vec::new(),
            fresh: true,
            parts: Vec::new(),
            fields: BTreeSet::new(),
        }
    }

    fn push_part(&mut self, part: ChatPart) {
        let at = self.msg.parts.len();
        self.msg.parts.push(part);
        self.sides.push(Side::default());
        self.touch_part(at);
    }

    fn touch_part(&mut self, at: usize) {
        if !self.parts.contains(&at) {
            self.parts.push(at);
        }
        if let Some(side) = self.sides.get_mut(at) {
            side.doc = None;
        }
    }

    fn set(&mut self, field: &'static str) {
        self.fields.insert(field);
    }

    fn next_part_id(&self) -> String {
        format!("{}.{}", self.msg.id, self.msg.parts.len())
    }
}

/// The rows of the chat itself or of one subagent.
#[derive(Default)]
struct Transcript {
    rows: Vec<Row>,
    /// The row each of the agent's message ids opened.
    by_message: HashMap<String, String>,
    /// Rows changed since the last flush, by id, in the order they changed.
    touched: Vec<String>,
    /// What its rows hold, about, as of the last flush.
    bytes: usize,
    /// The number of the change it takes in now.
    now: u64,
}

impl Transcript {
    /// The id of a row the event `index` opens. Rows go in in the order of
    /// their events, so only the newest can have come from the same one.
    fn next_id(&self, index: u64) -> String {
        let same = self
            .rows
            .iter()
            .rev()
            .take_while(|row| row.index == index)
            .count();
        match same {
            0 => format!("m{index}"),
            same => format!("m{index}.{same}"),
        }
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.rows.iter().rposition(|row| row.msg.id == id)
    }

    fn touch(&mut self, at: usize) {
        self.rows[at].v = self.now;
        self.queue(at);
    }

    /// Sends the row's changes with the next flush, without counting it as changed now.
    fn queue(&mut self, at: usize) {
        let id = &self.rows[at].msg.id;
        if !self.touched.iter().any(|touched| touched == id) {
            self.touched.push(id.clone());
        }
    }

    fn push(&mut self, row: Row) -> usize {
        self.rows.push(row);
        let at = self.rows.len() - 1;
        self.touch(at);
        at
    }
}

/// Where a tool call's part is, and while it runs, everything said of it.
struct ToolPlace {
    within: Option<String>,
    row: String,
    part: usize,
    raw: Option<Map<String, Value>>,
}

/// The most of what tool calls did that the fold keeps to hand. Past it the
/// oldest is read again from the chat's events when a client opens its row.
const MAX_DETAIL_BYTES: usize = 4 * 1024 * 1024;

/// What settled tool calls did, line by line, newest last.
#[derive(Default)]
struct Details {
    held: HashMap<String, (ToolDetail, usize)>,
    order: std::collections::VecDeque<String>,
    bytes: usize,
}

impl Details {
    fn insert(&mut self, id: String, detail: ToolDetail) {
        let size = detail.diff.as_ref().map_or(0, |diff| {
            diff.lines.iter().map(|line| line.text.len() + 16).sum()
        }) + detail.output.as_ref().map_or(0, |output| {
            output.text.len() + output.image.as_ref().map_or(0, |image| image.data.len())
        });
        self.remove(&id);
        self.bytes += size;
        self.held.insert(id.clone(), (detail, size));
        self.order.push_back(id);
        while self.bytes > MAX_DETAIL_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, size)) = self.held.remove(&oldest) {
                self.bytes -= size;
            }
        }
    }

    fn remove(&mut self, id: &str) {
        if let Some((_, size)) = self.held.remove(id) {
            self.bytes -= size;
            self.order.retain(|held| held != id);
        }
    }

    fn get(&self, id: &str) -> Option<&ToolDetail> {
        self.held.get(id).map(|(detail, _)| detail)
    }
}

/// Where a subagent's card is.
struct CardPlace {
    within: Option<String>,
    row: String,
    part: usize,
}

/// A chat as the core folds it.
pub struct Fold {
    head: ChatHead,
    main: Transcript,
    subs: HashMap<String, Transcript>,
    cards: HashMap<String, CardPlace>,
    tools: HashMap<(Option<String>, String), ToolPlace>,
    /// Gesture calls by id: what has come of one still running, or nothing once it has finished.
    gestures: HashMap<(Option<String>, String), Option<Raw>>,
    details: Details,
    notices: HashSet<String>,
    suppress_user_echo: bool,
    /// Head fields changed since the last flush.
    head_dirty: BTreeSet<&'static str>,
    /// Rows from this one on were taken back since the last flush.
    dropped: Option<String>,
    /// The main rows from this position on may have new folds or meta.
    settle_from: Option<usize>,
    /// The number of the change it takes in now.
    seq: u64,
    /// The change that last said whether a turn runs.
    running_v: u64,
    /// The agent always offers an effort picker, empty until its session
    /// lists the levels.
    offers_effort: bool,
    /// The folder the agent works in, which the paths it names relative to are made whole against.
    folder: Option<PathBuf>,
    /// Where a finished edit's lines sit in its file, so they carry the file's numbers.
    place: Option<Place>,
    /// How many bytes each row's JSON is as a client opening the chat is sent it, by row id,
    /// with the change it was measured at.
    sent_bytes: RefCell<HashMap<String, (u64, usize)>>,
}

const TEXT_PART: &str = "text";
const THOUGHT_PART: &str = "thought";

fn text_of(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// History replayed from the agent's own files marks the first update of a
/// turn the agent began by itself, since no prompt shows where it starts.
fn replayed_turn_start(update: &Value) -> Option<TurnStart> {
    update::opens_turn(update).map(|at| TurnStart { at })
}

fn units(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn bound_content(content: &Value) -> ContentBlock {
    let mut block = ContentBlock::read(content);
    if let ContentBlock::Image { data, .. } = &mut block {
        if data
            .as_ref()
            .is_some_and(|data| data.len() > MAX_IMAGE_DATA_CHARS)
        {
            *data = None;
        }
    }
    block
}

fn notice_state(state: &str) -> Option<NoticeState> {
    match state {
        "completed" => Some(NoticeState::Completed),
        "failed" => Some(NoticeState::Failed),
        "stopped" | "killed" | "cancelled" => Some(NoticeState::Stopped),
        _ => None,
    }
}

fn subagent_state(state: &str) -> Option<SubagentState> {
    match state {
        "running" => Some(SubagentState::Running),
        "completed" => Some(SubagentState::Completed),
        "failed" => Some(SubagentState::Failed),
        "cancelled" => Some(SubagentState::Cancelled),
        "disconnected" => Some(SubagentState::Disconnected),
        _ => None,
    }
}

fn task_state(state: &str) -> Option<TaskState> {
    match state {
        "running" => Some(TaskState::Running),
        "paused" => Some(TaskState::Paused),
        "completed" => Some(TaskState::Completed),
        "failed" => Some(TaskState::Failed),
        "stopped" => Some(TaskState::Stopped),
        _ => None,
    }
}

fn context_usage(update: &Value) -> Option<ContextUsage> {
    let used = update.get("used")?.as_f64()?;
    let size = update.get("size")?.as_f64()?;
    if !used.is_finite() || !size.is_finite() || size <= 0.0 {
        return None;
    }
    let cost = update.get("cost").and_then(|cost| {
        let amount = cost
            .get("amount")?
            .as_f64()
            .filter(|amount| amount.is_finite())?;
        Some(UsageCost {
            amount,
            currency: text_of(cost.get("currency"))?.to_owned(),
        })
    });
    Some(ContextUsage { used, size, cost })
}

/// An agent says it is retrying as a warning-level failure, in the same shape
/// a failed turn uses. It is not a failure yet: the turn runs on.
fn retry_of(update: &Value) -> Option<Retrying> {
    let notice = update.pointer("/_meta/jetbrains/air/sessionFailure")?;
    if text_of(notice.get("severity")) != Some("warning") {
        return None;
    }
    Some(Retrying {
        attempt: notice
            .get("attempt")
            .and_then(Value::as_u64)
            .filter(|attempt| *attempt > 0),
        connection: text_of(notice.get("category")) == Some("connection"),
    })
}

fn failure_of(payload: &Value) -> Option<ChatFailure> {
    let failure = payload.get("failure")?;
    let kind = match text_of(failure.get("kind"))? {
        "signIn" => FailureKind::SignIn,
        "limit" => FailureKind::Limit,
        "network" => FailureKind::Network,
        "other" => FailureKind::Other,
        _ => return None,
    };
    Some(ChatFailure {
        kind,
        details: text_of(failure.get("details"))
            .filter(|details| !details.is_empty())
            .map(str::to_owned),
        account: text_of(failure.get("account")).map(str::to_owned),
        resumes_at: None,
    })
}

fn permission_ask(payload: &Value) -> Option<PermissionAsk> {
    let request_id = text_of(payload.get("requestId"))?;
    let session_id = text_of(payload.get("sessionId"))?;
    let tool_call = payload.get("toolCall")?.as_object()?;
    text_of(tool_call.get("toolCallId"))?;
    let mut call = tools::call(tool_call);
    if call.title.is_empty() {
        call.title = "Agent tool".into();
        call = call.with_row();
    }
    let options = payload
        .get("options")?
        .as_array()?
        .iter()
        .filter_map(|option| {
            Some(PermissionOption {
                option_id: text_of(option.get("optionId"))?.to_owned(),
                name: text_of(option.get("name"))?.to_owned(),
                kind: PermissionKind::deserialize(option.get("kind")?).ok()?,
            })
        })
        .collect();
    Some(PermissionAsk {
        request_id: request_id.to_owned(),
        session_id: session_id.to_owned(),
        diff: tools::tool_diff(tool_call.get("content"), tool_call.get("rawInput"))
            .as_ref()
            .map(tools::summary),
        tool_call: call,
        options,
    })
}

fn commands_of(update: &Value) -> Vec<ChatCommandInfo> {
    update
        .get("availableCommands")
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    Some(ChatCommandInfo {
                        name: text_of(command.get("name"))?.to_owned(),
                        description: text_of(command.get("description"))?.to_owned(),
                        hint: text_of(command.pointer("/input/hint")).map(str::to_owned),
                    })
                })
                .take(MAX_COMMANDS)
                .collect()
        })
        .unwrap_or_default()
}

fn queued_of(value: Option<&Value>) -> Vec<QueuedMessage> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let strings = |key: &str| -> Vec<String> {
                item.get(key)
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let context = item
                .get("context")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            Some(SentContext {
                                uri: text_of(item.get("uri"))?.to_owned(),
                                title: text_of(item.get("title"))?.to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(QueuedMessage {
                id: text_of(item.get("id"))?.to_owned(),
                text: text_of(item.get("text"))?.to_owned(),
                paths: strings("paths"),
                context,
            })
        })
        .collect()
}

fn workflow_agents(value: &Value) -> Option<Vec<WorkflowAgent>> {
    let agents = value.as_array()?;
    Some(
        agents
            .iter()
            .filter_map(|agent| serde_json::from_value(agent.clone()).ok())
            .collect(),
    )
}

/// Which transcript an update lands in: the chat's own, or a subagent's.
type Within = Option<String>;
type Raw = Map<String, Value>;

impl Fold {
    /// Makes the paths the agent names whole against `folder`, and numbers the lines of
    /// each edit that finishes from where `place` finds it in its file.
    pub fn work_in(&mut self, folder: PathBuf, place: Place) {
        self.folder = Some(folder);
        self.place = Some(place);
    }

    pub fn new(permission_mode: PermissionMode) -> Self {
        Self {
            head: ChatHead {
                permission_mode,
                capabilities: Default::default(),
                setup: Value::Object(Map::new()),
                ..ChatHead::default()
            },
            main: Transcript::default(),
            subs: HashMap::new(),
            cards: HashMap::new(),
            tools: HashMap::new(),
            gestures: HashMap::new(),
            details: Details::default(),
            notices: HashSet::new(),
            suppress_user_echo: false,
            head_dirty: BTreeSet::new(),
            dropped: None,
            settle_from: None,
            seq: 1,
            running_v: 0,
            offers_effort: false,
            folder: None,
            place: None,
            sent_bytes: RefCell::default(),
        }
    }

    /// Shows an effort picker from the start, as agents that offer effort on
    /// every model do.
    pub fn offer_effort(&mut self) {
        self.offers_effort = true;
        self.read_pickers();
    }

    /// The head's model and effort pickers, read from the session's options.
    fn read_pickers(&mut self) {
        let (model, mut effort) = pickers::pickers(&self.head.setup["configOptions"]);
        if effort.is_none() && self.offers_effort {
            effort = Some(sikemux_wire::protocol::ConfigPicker {
                id: "effort".into(),
                current: String::new(),
                choices: Vec::new(),
            });
        }
        if self.head.model != model {
            self.head.model = model;
            self.set_head("model");
        }
        if self.head.effort != effort {
            self.head.effort = effort;
            self.set_head("effort");
        }
    }

    /// Counts what it takes in from now on as change `seq`.
    pub fn number(&mut self, seq: u64) {
        self.seq = seq;
        self.main.now = seq;
    }

    fn set_head(&mut self, field: &'static str) {
        if field == "running" {
            self.running_v = self.seq;
        }
        self.head_dirty.insert(field);
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        if self.head.permission_mode != mode {
            self.head.permission_mode = mode;
            self.set_head("permissionMode");
        }
    }

    pub fn set_changing_mode(&mut self, changing: bool) {
        if self.head.changing_mode != changing {
            self.head.changing_mode = changing;
            self.set_head("changingMode");
        }
    }

    pub fn set_turned(&mut self, turned: bool) {
        if self.head.turned != turned {
            self.head.turned = turned;
            self.set_head("turned");
        }
    }

    pub fn set_running(&mut self, running: bool) {
        if self.head.running != running {
            self.head.running = running;
            self.set_head("running");
            self.settle_last_run();
        }
    }

    pub fn set_setup(&mut self, setup: &Value) {
        self.head.setup = setup.clone();
        self.set_head("setup");
        self.read_pickers();
    }

    pub fn set_config_options(&mut self, options: Value) {
        if !self.head.setup.is_object() {
            self.head.setup = Value::Object(Map::new());
        }
        if let Some(setup) = self.head.setup.as_object_mut() {
            setup.insert("configOptions".into(), options);
        }
        self.set_head("setup");
        self.read_pickers();
    }

    pub fn set_queued(&mut self, queued: Vec<QueuedMessage>) {
        self.head.queued = queued;
        self.set_head("queued");
    }

    pub fn set_triggers(&mut self, triggers: Vec<ChatTrigger>) {
        if self.head.triggers != triggers {
            self.head.triggers = triggers;
            self.set_head("triggers");
        }
    }

    pub fn set_draft(&mut self, draft: Option<ChatDraft>) {
        if self.head.draft != draft {
            self.head.draft = draft;
            self.set_head("draft");
        }
    }

    pub fn set_earlier_lost(&mut self) {
        if !self.head.earlier_lost {
            self.head.earlier_lost = true;
            self.set_head("earlierLost");
        }
    }

    pub fn forget_permission(&mut self, request_id: &str) {
        let before = self.head.permissions.len();
        self.head
            .permissions
            .retain(|ask| ask.request_id != request_id);
        if self.head.permissions.len() != before {
            self.set_head("permissions");
        }
    }

    fn transcript(&mut self, within: &Within) -> Option<&mut Transcript> {
        let now = self.seq;
        let transcript = match within {
            None => &mut self.main,
            Some(session) => self.subs.get_mut(session)?,
        };
        transcript.now = now;
        Some(transcript)
    }

    fn row_id(&mut self, index: u64, within: &Within) -> String {
        self.transcript(within).map_or_else(
            || format!("m{index}"),
            |transcript| transcript.next_id(index),
        )
    }

    fn note_main_change(&mut self, within: &Within, at: usize) {
        if within.is_none() {
            self.settle_from = Some(self.settle_from.map_or(at, |from| from.min(at)));
        }
    }

    /// Takes in one event the feed kept, `index` its place in the chat's
    /// history, `at` when it happened.
    pub fn apply(&mut self, index: u64, kind: ChatEventKind, payload: &Value, at: u64) {
        match kind {
            ChatEventKind::Status => self.status(payload, at),
            ChatEventKind::Ready => {
                self.head.connection = ChatConnection::Ready;
                if let Some(capabilities) = payload
                    .get("capabilities")
                    .and_then(|value| ChatCapabilities::deserialize(value).ok())
                {
                    self.head.capabilities = capabilities;
                }
                if let Some(setup) = payload.get("setup").filter(|value| value.is_object()) {
                    self.head.setup = setup.clone();
                }
                if let Some(session) = text_of(payload.get("sessionId")) {
                    self.head.session_id = Some(session.to_owned());
                    self.set_head("sessionId");
                }
                self.head.error = None;
                self.head.failure = None;
                for field in ["connection", "capabilities", "setup", "error", "failure"] {
                    self.set_head(field);
                }
                self.read_pickers();
            }
            ChatEventKind::SessionUpdate => {
                let updates: Vec<&Value> = match payload.get("updates").and_then(Value::as_array) {
                    Some(updates) => updates.iter().collect(),
                    None => vec![payload],
                };
                for notification in updates {
                    let (Some(session), Some(update)) = (
                        text_of(notification.get("sessionId")),
                        notification
                            .get("update")
                            .filter(|update| update.is_object()),
                    ) else {
                        continue;
                    };
                    let at = notification.get("at").and_then(Value::as_u64).unwrap_or(at);
                    let until = notification
                        .get("until")
                        .and_then(Value::as_u64)
                        .unwrap_or(at);
                    self.session_update(index, session, update, at, until);
                }
            }
            ChatEventKind::Prompt => self.prompted(index, payload, at),
            ChatEventKind::TurnStarted => {
                let at = payload.get("at").and_then(Value::as_u64);
                self.head.running = true;
                self.head.turn_opening = Some(TurnStart { at });
                self.head.retrying = None;
                self.head.stop_reason = None;
                self.head.error = None;
                self.head.failure = None;
                for field in [
                    "running",
                    "turnOpening",
                    "retrying",
                    "stopReason",
                    "error",
                    "failure",
                ] {
                    self.set_head(field);
                }
            }
            ChatEventKind::TurnCompleted => {
                let stop = payload
                    .get("stopReason")
                    .and_then(|reason| StopReason::deserialize(reason).ok());
                let at = payload.get("at").and_then(Value::as_u64).unwrap_or(at);
                self.end_turn(stop, at);
            }
            ChatEventKind::PermissionRequest => {
                if let Some(mut ask) = permission_ask(payload) {
                    let folder = self.folder.as_deref();
                    ask.tool_call = tools::root_call(folder, ask.tool_call);
                    if let Some(diff) = ask.diff.as_mut() {
                        diff.path = tools::rooted(folder, std::mem::take(&mut diff.path));
                    }
                    self.head
                        .permissions
                        .retain(|held| held.request_id != ask.request_id);
                    self.head.permissions.push(ask);
                    self.set_head("permissions");
                }
            }
            ChatEventKind::Error => {
                if self.head.connection != ChatConnection::Ready {
                    self.head.connection = ChatConnection::Error;
                }
                self.head.running = false;
                self.head.retrying = None;
                self.head.permissions.clear();
                self.head.error = Some(
                    text_of(payload.get("message"))
                        .unwrap_or("The agent stopped.")
                        .to_owned(),
                );
                self.head.failure = failure_of(payload);
                for field in [
                    "connection",
                    "running",
                    "retrying",
                    "permissions",
                    "error",
                    "failure",
                ] {
                    self.set_head(field);
                }
                self.settle_last_run();
            }
        }
    }

    fn status(&mut self, payload: &Value, at: u64) {
        let state = match text_of(payload.get("state")) {
            Some("starting") => ChatConnection::Starting,
            Some("initializing") => ChatConnection::Initializing,
            Some("ready") => ChatConnection::Ready,
            Some("stopped") => ChatConnection::Stopped,
            Some("error") => ChatConnection::Error,
            _ => ChatConnection::Connecting,
        };
        let ended = matches!(state, ChatConnection::Stopped | ChatConnection::Error);
        if ended {
            self.settle_transcript(&None, at, false);
            self.head.running = false;
            self.head.permissions.clear();
            self.head.tasks.clear();
            for field in ["running", "permissions", "tasks"] {
                self.set_head(field);
            }
            self.settle_last_run();
        }
        if state != ChatConnection::Error {
            self.head.error = None;
            self.head.failure = None;
            self.set_head("error");
            self.set_head("failure");
        }
        self.head.ended = ended.then(|| SessionEnded {
            reason: text_of(payload.get("reason"))
                .unwrap_or("requested")
                .to_owned(),
            message: text_of(payload.get("message")).map(str::to_owned),
        });
        self.set_head("ended");
        self.head.connection = state;
        self.set_head("connection");
    }

    /// A prompt from a person, on any device: shown as they sent it.
    fn prompted(&mut self, index: u64, payload: &Value, at: u64) {
        let Some(text) = text_of(payload.get("text")) else {
            return;
        };
        let paths: Vec<String> = payload
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let id = self.row_id(index, &None);
        let mut row = Row::new(id, ChatRole::User, index);
        row.local = true;
        row.msg.prompt_id = text_of(payload.get("messageId")).map(str::to_owned);
        row.msg.sent_at = Some(payload.get("at").and_then(Value::as_u64).unwrap_or(at));
        row.msg.attachments = paths;
        row.sent_context = payload
            .get("context")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(SentContext {
                            uri: text_of(item.get("uri"))?.to_owned(),
                            title: text_of(item.get("title"))?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !text.trim().is_empty() {
            row.raw.push(ChatPart::Text {
                id: String::new(),
                text: text.to_owned(),
                md: None,
                cut_bytes: None,
            });
        }
        read_sent(&mut row);
        let at = self.main.push(row);
        self.note_main_change(&None, at);
        self.suppress_user_echo = true;
        self.head.running = true;
        self.head.error = None;
        self.head.failure = None;
        for field in ["running", "error", "failure"] {
            self.set_head(field);
        }
    }

    /// The person took the chat back to before one of their messages: it
    /// goes, and everything after it.
    pub fn rewind(&mut self, message_id: &str) {
        let Some(at) = self.main.rows.iter().position(|row| {
            row.msg.role == ChatRole::User
                && row.msg.prompt_id.as_deref().unwrap_or(&row.msg.id) == message_id
        }) else {
            return;
        };
        let gone: Vec<Row> = self.main.rows.drain(at..).collect();
        for row in &gone {
            self.main.bytes -= row.bytes;
            self.forget_row(&None, row);
        }
        let first = gone.into_iter().next().map(|row| row.msg.id);
        let kept: HashSet<String> = self
            .main
            .rows
            .iter()
            .map(|row| row.msg.id.clone())
            .collect();
        self.main.touched.retain(|id| kept.contains(id));
        if let Some(first) = first {
            self.dropped = Some(match self.dropped.take() {
                Some(earlier) if self.main.rows.iter().all(|row| row.msg.id != earlier) => earlier,
                _ => first,
            });
        }
        self.head.plan = None;
        self.head.stop_reason = None;
        self.set_head("plan");
        self.set_head("stopReason");
        self.settle_from = Some(
            self.settle_from
                .map_or(at, |from| from.min(at))
                .min(self.main.rows.len()),
        );
    }

    fn forget_row(&mut self, within: &Within, row: &Row) {
        for part in &row.msg.parts {
            match part {
                ChatPart::Tool { tool, .. } => {
                    self.tools
                        .remove(&(within.clone(), tool.tool_call_id.clone()));
                    self.details.remove(&tool.tool_call_id);
                }
                ChatPart::Subagent { subagent, .. } => {
                    self.cards.remove(&subagent.session_id);
                    if let Some(sub) = self.subs.remove(&subagent.session_id) {
                        let session = Some(subagent.session_id.clone());
                        for row in &sub.rows {
                            self.forget_row(&session, row);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(transcript) = self.transcript(within) {
            transcript.by_message.retain(|_, id| *id != row.msg.id);
        }
    }

    fn session_update(&mut self, index: u64, session: &str, update: &Value, at: u64, until: u64) {
        let Some(kind) = UpdateKind::of(update) else {
            return;
        };
        match kind {
            UpdateKind::SubagentStateUpdate => return self.set_subagent_state(update, at),
            UpdateKind::SubagentUsage => {
                if let (Some(id), Some(tokens)) = (
                    text_of(update.get("subagentSessionId")),
                    update.get("tokens").and_then(Value::as_u64),
                ) {
                    self.patch_card(id, |card| card.tokens = Some(tokens));
                }
                return;
            }
            _ => {}
        }
        let main = self.head.session_id.as_deref() == Some(session);
        if !main && self.subs.contains_key(session) {
            let within = Some(session.to_owned());
            if kind == UpdateKind::SubagentSpawned {
                self.spawn_subagent(index, &within, update);
            } else {
                let timed = self.head.running;
                self.transcript_update(index, &within, update, timed, false, at, until);
            }
            return;
        }

        if kind == UpdateKind::UserMessageChunk && self.harness_notice(index, update) {
            return;
        }
        if kind == UpdateKind::UserMessageChunk && self.suppress_user_echo {
            return;
        }
        let opening = self
            .head
            .turn_opening
            .clone()
            .or_else(|| replayed_turn_start(update));
        let rows = self.main.rows.len();
        let timed = self.head.running;
        if self.transcript_update(index, &None, update, timed, opening.is_some(), at, until) {
            let opened = opening.is_some()
                && self.main.rows.len() > rows
                && self
                    .main
                    .rows
                    .last()
                    .is_some_and(|row| row.msg.role == ChatRole::Assistant);
            if opened {
                let last = self.main.rows.len() - 1;
                self.main.rows[last].msg.opens_turn = opening.clone();
                self.main.rows[last].set("opensTurn");
            }
            self.recorded_time(update, timed);
            let turn_opening = if opened { None } else { opening };
            if self.head.turn_opening != turn_opening {
                self.head.turn_opening = turn_opening;
                self.set_head("turnOpening");
            }
            self.suppress_user_echo = false;
            if kind != UpdateKind::UserMessageChunk && self.head.retrying.is_some() {
                self.head.retrying = None;
                self.set_head("retrying");
            }
            return;
        }

        match kind {
            UpdateKind::SubagentSpawned => {
                if self.spawn_subagent(index, &None, update) {
                    self.suppress_user_echo = false;
                }
            }
            UpdateKind::AsyncTaskSpawned => self.spawn_task(update),
            UpdateKind::AsyncTaskProgress | UpdateKind::AsyncTaskStateUpdate => {
                self.patch_task(index, update)
            }
            UpdateKind::TaskNotice => {
                let (Some(task), Some(state)) = (
                    text_of(update.get("taskId")),
                    text_of(update.get("state")).and_then(notice_state),
                ) else {
                    return;
                };
                let notice = TaskNotice {
                    name: text_of(update.get("name"))
                        .unwrap_or("Background task")
                        .to_owned(),
                    state,
                    summary: text_of(update.get("summary")).map(str::to_owned),
                    task_type: None,
                    subagent: text_of(update.get("subagentSessionId")).map(str::to_owned),
                };
                let between = !self.head.running;
                self.add_notice(index, task, notice, stamped_at(update), between);
            }
            UpdateKind::Notice => self.agent_notice(index, update),
            UpdateKind::Plan => {
                self.head.plan = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| PlanEntry::deserialize(entry).ok())
                            .collect()
                    });
                self.set_head("plan");
                self.suppress_user_echo = false;
            }
            UpdateKind::AvailableCommandsUpdate => {
                self.head.commands = commands_of(update);
                self.set_head("commands");
            }
            UpdateKind::ConfigOptionUpdate => {
                self.set_config_options(
                    update.get("configOptions").cloned().unwrap_or(Value::Null),
                );
            }
            UpdateKind::UsageUpdate => {
                if let Some(usage) = context_usage(update) {
                    self.head.usage = Some(usage);
                    self.set_head("usage");
                }
            }
            UpdateKind::AccountSwitched => {
                let Some(label) = text_of(update.get("label")) else {
                    return;
                };
                let moved = AccountMove {
                    label: label.to_owned(),
                    from: text_of(update.get("from")).map(str::to_owned),
                    reason: if text_of(update.get("reason")) == Some("limit") {
                        MoveReason::Limit
                    } else {
                        MoveReason::Chosen
                    },
                };
                self.append_part(index, &None, |id| ChatPart::Account { id, moved }, false);
            }
            UpdateKind::SessionInfoUpdate => {
                if let Some(title) = text_of(update.get("title")) {
                    self.head.title = Some(title.to_owned());
                    self.set_head("title");
                }
                if self.head.running {
                    if let Some(retrying) = retry_of(update) {
                        self.head.retrying = Some(retrying);
                        self.set_head("retrying");
                    }
                }
            }
            UpdateKind::QueuedMessages => self.set_queued(queued_of(update.get("messages"))),
            UpdateKind::EarlierTurnsLost => self.set_earlier_lost(),
            UpdateKind::MessageRewound => {
                if let Some(message) = text_of(update.get("messageId")) {
                    self.rewind(message);
                }
            }
            _ => {}
        }
    }

    /// History the agent kept says when each part of it happened. A turn
    /// watched live is timed as it runs; one rebuilt from history takes its
    /// prompt's time and its answer's last.
    fn recorded_time(&mut self, update: &Value, timed: bool) {
        let Some(at) = stamped_at(update) else {
            return;
        };
        if timed {
            return;
        }
        let Some(last) = self.main.rows.last_mut() else {
            return;
        };
        match last.msg.role {
            ChatRole::User if last.msg.sent_at.is_some() => return,
            ChatRole::User => {
                last.msg.sent_at = Some(at);
                last.set("sentAt");
            }
            ChatRole::Assistant => {
                last.msg.ended_at = Some(at);
                last.set("endedAt");
            }
        }
        let position = self.main.rows.len() - 1;
        self.main.touch(position);
        self.note_main_change(&None, position);
    }

    /// Applies the updates a session streams regardless of whose session it
    /// is. Answers whether the update was one of them.
    #[allow(clippy::too_many_arguments)]
    fn transcript_update(
        &mut self,
        index: u64,
        within: &Within,
        update: &Value,
        timed: bool,
        fresh: bool,
        at: u64,
        until: u64,
    ) -> bool {
        let Some(kind) = UpdateKind::of(update) else {
            return false;
        };
        match kind {
            UpdateKind::UserMessageChunk
            | UpdateKind::AgentMessageChunk
            | UpdateKind::AgentThoughtChunk => {
                let Some(content) = update.get("content").filter(|content| {
                    content.is_object() && content.get("type").is_some_and(Value::is_string)
                }) else {
                    return false;
                };
                let role = if kind == UpdateKind::UserMessageChunk {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                };
                if role == ChatRole::User {
                    if let Some(text) = text_of(content.get("text")) {
                        if prompt::is_markup_only(text) || prompt::is_interrupt_marker(text) {
                            return false;
                        }
                    }
                }
                let part = if kind == UpdateKind::AgentThoughtChunk {
                    THOUGHT_PART
                } else {
                    TEXT_PART
                };
                let message = text_of(update.get("messageId"));
                let fresh = fresh && role == ChatRole::Assistant;
                self.append_chunk(
                    index, within, role, part, content, message, timed, fresh, at, until,
                );
                true
            }
            UpdateKind::ToolCall => {
                let Some(raw) = update.as_object() else {
                    return false;
                };
                if text_of(raw.get("toolCallId")).is_none() || text_of(raw.get("title")).is_none() {
                    return false;
                }
                self.upsert_tool(index, within, raw, false, fresh, at)
            }
            UpdateKind::ToolCallUpdate => {
                let Some(raw) = update.as_object() else {
                    return false;
                };
                if text_of(raw.get("toolCallId")).is_none() {
                    return false;
                }
                self.upsert_tool(index, within, raw, true, fresh, at)
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn append_chunk(
        &mut self,
        index: u64,
        within: &Within,
        role: ChatRole,
        part_kind: &'static str,
        content: &Value,
        message: Option<&str>,
        timed: bool,
        fresh: bool,
        at: u64,
        until: u64,
    ) {
        let text = text_of(content.get("text"));
        let Some(transcript) = self.transcript(within) else {
            return;
        };
        let continues = transcript
            .rows
            .last()
            .is_some_and(|last| last.msg.role == role && !last.local && !fresh);
        let existing = match message {
            Some(message) => transcript
                .by_message
                .get(message)
                .and_then(|id| transcript.position(id)),
            None if continues => Some(transcript.rows.len() - 1),
            None => None,
        };
        let stream = timed && role == ChatRole::Assistant && text.is_some();
        let position = match existing {
            Some(position) => position,
            None => {
                let new_id = transcript.next_id(index);
                let mut row = Row::new(new_id.clone(), role, index);
                if role == ChatRole::User {
                    row.msg.prompt_id = message.map(str::to_owned);
                }
                if timed {
                    row.msg.sent_at = Some(at);
                }
                let position = transcript.push(row);
                if let Some(message) = message {
                    transcript.by_message.insert(message.to_owned(), new_id);
                }
                position
            }
        };
        let row = &mut transcript.rows[position];
        if role == ChatRole::User {
            match (text, row.raw.last_mut()) {
                (Some(more), Some(ChatPart::Text { text, .. })) => text.push_str(more),
                (Some(text), _) => row.raw.push(ChatPart::Text {
                    id: String::new(),
                    text: text.to_owned(),
                    md: None,
                    cut_bytes: None,
                }),
                (None, _) => row.raw.push(ChatPart::Content {
                    id: String::new(),
                    content: bound_content(content),
                }),
            }
            read_sent(row);
            transcript.touch(position);
            self.note_main_change(within, position);
            return;
        }
        // A task can end while the reply streams; its notice must not cut the reply's markdown in two.
        let mut open = row.msg.parts.len().saturating_sub(1);
        while open > 0
            && matches!(
                row.msg.parts[open],
                ChatPart::Notice { .. } | ChatPart::AgentNotice { .. }
            )
        {
            open -= 1;
        }
        let appended = match (text, row.msg.parts.get_mut(open)) {
            (Some(more), Some(ChatPart::Text { text, .. })) if part_kind == TEXT_PART => {
                text.push_str(more);
                true
            }
            (Some(more), Some(ChatPart::Thought { text, .. })) if part_kind == THOUGHT_PART => {
                text.push_str(more);
                true
            }
            _ => false,
        };
        if appended {
            let more = text.unwrap_or_default();
            if let Some(doc) = row.sides.get_mut(open).and_then(|side| side.doc.as_mut()) {
                doc.push(more);
            }
        } else {
            let id = row.next_part_id();
            let part = match text {
                Some(text) if part_kind == THOUGHT_PART => ChatPart::Thought {
                    id,
                    text: text.to_owned(),
                    md: None,
                    cut_bytes: None,
                },
                Some(text) => ChatPart::Text {
                    id,
                    text: text.to_owned(),
                    md: None,
                    cut_bytes: None,
                },
                None => ChatPart::Content {
                    id,
                    content: bound_content(content),
                },
            };
            row.push_part(part);
        }
        if stream {
            let chars = units(text.unwrap_or_default());
            let stream = row.stream.get_or_insert(Stream {
                started: at,
                ended: until,
                chars: 0,
            });
            stream.ended = until;
            stream.chars += chars;
        }
        transcript.touch(position);
        self.note_main_change(within, position);
    }

    /// Adds `part` to the agent's last message, or starts one for it.
    fn append_part(
        &mut self,
        index: u64,
        within: &Within,
        part: impl FnOnce(String) -> ChatPart,
        fresh: bool,
    ) -> Option<(String, usize)> {
        let transcript = self.transcript(within)?;
        let continues = transcript
            .rows
            .last()
            .is_some_and(|last| last.msg.role == ChatRole::Assistant && !last.local && !fresh);
        let position = if continues {
            transcript.rows.len() - 1
        } else {
            let new_id = transcript.next_id(index);
            transcript.push(Row::new(new_id, ChatRole::Assistant, index))
        };
        let row = &mut transcript.rows[position];
        let id = row.next_part_id();
        row.push_part(part(id));
        let place = (row.msg.id.clone(), row.msg.parts.len() - 1);
        transcript.touch(position);
        self.note_main_change(within, position);
        Some(place)
    }

    fn upsert_tool(
        &mut self,
        index: u64,
        within: &Within,
        update: &Map<String, Value>,
        merge: bool,
        fresh: bool,
        at: u64,
    ) -> bool {
        let id = text_of(update.get("toolCallId"))
            .unwrap_or_default()
            .to_owned();
        let key = (within.clone(), id.clone());
        if self.gestures.contains_key(&key)
            || text_of(update.get("title")).is_some_and(tools::is_gesture)
        {
            return self.gesture(index, within, key, update, merge);
        }
        if let Some(place) = self.tools.get_mut(&key) {
            let patch = update.iter().filter(|(key, value)| {
                !(value.is_null() || key.as_str() == "title" && value.as_str() == Some(""))
            });
            let raw = match place.raw.take() {
                Some(mut raw) if merge => {
                    for (key, value) in patch {
                        raw.insert(key.clone(), value.clone());
                    }
                    raw
                }
                Some(_) => update.clone(),
                None => {
                    // Ended already: what it said since only renames or restates it.
                    let (row, part, within) = (place.row.clone(), place.part, place.within.clone());
                    let patch: Map<String, Value> = patch
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    self.restate_tool(&within, &row, part, &patch);
                    return true;
                }
            };
            let (row, part, within) = (place.row.clone(), place.part, place.within.clone());
            let running = tools::running(text_of(raw.get("status")));
            if running {
                self.tools.get_mut(&key).expect("held").raw = Some(raw.clone());
                let call = tools::root_call(self.folder.as_deref(), tools::call(&raw));
                self.replace_tool(&within, &row, part, |part| {
                    if let ChatPart::Tool { tool, .. } = part {
                        *tool = call;
                    }
                });
            } else {
                self.settle_tool(&within, &row, part, &raw, at);
            }
            return true;
        }

        // An update can land for a call this side never saw open: the adapter
        // raises a subagent as its own session and drops the call that spawned
        // it. With nothing to merge into and no title to show, a new row would
        // be a blank line that spins forever.
        if text_of(update.get("title")).is_none_or(str::is_empty) {
            return false;
        }
        let ended = !tools::running(text_of(update.get("status")));
        // Reloading a session replays its whole history at once, so a call
        // already finished the first time this side sees it never ran here.
        let call = tools::root_call(self.folder.as_deref(), tools::call(update));
        let Some((row, part)) = self.append_part(
            index,
            within,
            |id| ChatPart::Tool {
                id,
                tool: call,
                diff: None,
                output: None,
                page: None,
                gif: None,
                failure: None,
                started_at: (!ended).then_some(at),
                ended_at: None,
            },
            fresh,
        ) else {
            return false;
        };
        self.tools.insert(
            key,
            ToolPlace {
                within: within.clone(),
                row: row.clone(),
                part,
                raw: Some(update.clone()),
            },
        );
        if ended {
            self.settle_tool(within, &row, part, update, at);
        }
        true
    }

    /// A gesture shows nothing while it runs, and once it has finished only what it did.
    fn gesture(
        &mut self,
        index: u64,
        within: &Within,
        key: (Within, String),
        update: &Map<String, Value>,
        merge: bool,
    ) -> bool {
        let slot = self.gestures.entry(key).or_insert_with(|| Some(Map::new()));
        let Some(held) = slot.as_mut() else {
            return false;
        };
        if !merge {
            held.clear();
        }
        for (field, value) in update {
            if !value.is_null() {
                held.insert(field.clone(), value.clone());
            }
        }
        if tools::running(text_of(held.get("status"))) {
            return false;
        }
        let Some(raw) = slot.take() else {
            return false;
        };
        match tools::gesture(&raw) {
            Some(tools::Gesture::React(emoji)) => self.react(within, emoji),
            Some(tools::Gesture::Gif(gif)) => {
                let call = tools::root_call(self.folder.as_deref(), tools::call(&raw));
                let part = |id| ChatPart::Tool {
                    id,
                    tool: call,
                    diff: None,
                    output: None,
                    page: None,
                    gif: Some(gif),
                    failure: None,
                    started_at: None,
                    ended_at: None,
                };
                self.append_part(index, within, part, false).is_some()
            }
            None => false,
        }
    }

    /// Puts `emoji` on the latest message to the agent, the one it is answering.
    fn react(&mut self, within: &Within, emoji: String) -> bool {
        let Some(transcript) = self.transcript(within) else {
            return false;
        };
        let Some(position) = transcript
            .rows
            .iter()
            .rposition(|row| row.msg.role == ChatRole::User)
        else {
            return false;
        };
        let row = &mut transcript.rows[position];
        if row.msg.reactions.contains(&emoji) || row.msg.reactions.len() >= MAX_REACTIONS {
            return false;
        }
        row.msg.reactions.push(emoji);
        row.set("reactions");
        transcript.touch(position);
        true
    }

    fn replace_tool(
        &mut self,
        within: &Within,
        row: &str,
        part: usize,
        change: impl FnOnce(&mut ChatPart),
    ) {
        let Some(transcript) = self.transcript(within) else {
            return;
        };
        let Some(position) = transcript.position(row) else {
            return;
        };
        let held = &mut transcript.rows[position];
        if let Some(found) = held.msg.parts.get_mut(part) {
            change(found);
            held.touch_part(part);
        }
        transcript.touch(position);
        self.note_main_change(within, position);
        self.note_card_tool(within);
    }

    /// A call that has ended has said everything it is going to say. Its
    /// change, output and failure are read once, and what they came from is
    /// let go of.
    fn settle_tool(
        &mut self,
        within: &Within,
        row: &str,
        part: usize,
        raw: &Map<String, Value>,
        at: u64,
    ) {
        let mut settled = tools::settle(raw);
        if let Some(diff) = settled.diff.as_mut() {
            if let Some(place) = &self.place {
                tools::place(diff, raw, place);
            }
            diff.path = tools::rooted(self.folder.as_deref(), std::mem::take(&mut diff.path));
        }
        let call = tools::root_call(self.folder.as_deref(), tools::call(raw));
        let detail = tools::detail(&settled);
        let id = call.tool_call_id.clone();
        if detail.diff.is_some() || detail.output.is_some() {
            self.details.insert(id.clone(), detail);
        }
        if let Some(place) = self.tools.get_mut(&(within.clone(), id)) {
            place.raw = None;
        }
        self.replace_tool(within, row, part, |part| {
            if let ChatPart::Tool {
                tool,
                diff,
                output,
                page,
                failure,
                ended_at,
                ..
            } = part
            {
                *tool = call;
                *diff = settled.diff.as_ref().map(tools::summary);
                *output = settled.output.as_ref().map(tools::output_summary);
                *page = settled.page;
                *failure = settled.failure;
                ended_at.get_or_insert(at);
            }
        });
    }

    fn restate_tool(
        &mut self,
        within: &Within,
        row: &str,
        part: usize,
        patch: &Map<String, Value>,
    ) {
        let folder = self.folder.clone();
        self.replace_tool(within, row, part, |part| {
            let ChatPart::Tool { tool, .. } = part else {
                return;
            };
            let restated = tools::call(patch);
            if !restated.title.is_empty() {
                tool.title = restated.title;
            }
            if restated.kind.is_some() {
                tool.kind = restated.kind;
            }
            if restated.status.is_some() {
                tool.status = restated.status;
            }
            if restated.path.is_some() {
                tool.path = restated.path;
            }
            if restated.raw_input.is_some() {
                tool.raw_input = restated.raw_input;
                tool.description = restated.description;
            }
            *tool = tools::root_call(folder.as_deref(), std::mem::take(tool).with_row());
        });
    }

    /// A subagent's card counts its calls and names the last one; a change to
    /// one of its tools shows on the card.
    fn note_card_tool(&mut self, within: &Within) {
        let Some(session) = within else {
            return;
        };
        let Some(sub) = self.subs.get(session) else {
            return;
        };
        let mut calls = 0;
        let mut last = None;
        for row in &sub.rows {
            for part in &row.msg.parts {
                if let ChatPart::Tool { tool, .. } = part {
                    calls += 1;
                    last = Some(tool.clone());
                }
            }
        }
        let session = session.clone();
        self.patch_card(&session, |card| {
            card.calls = calls;
            card.last_tool = last;
        });
    }

    /// The agent sends no last word for work it was cut off in, so it would
    /// spin forever. A subagent can outlive its turn, so only `end_subagents`
    /// stops one.
    fn settle_transcript(&mut self, within: &Within, at: u64, end_subagents: bool) {
        let mut running: Vec<(String, usize, String)> = Vec::new();
        let mut cards: Vec<(String, bool)> = Vec::new();
        if let Some(transcript) = self.transcript(within) {
            for row in &transcript.rows {
                for (position, part) in row.msg.parts.iter().enumerate() {
                    match part {
                        ChatPart::Tool { tool, .. }
                            if tool.status.is_none_or(ToolStatus::running) =>
                        {
                            running.push((row.msg.id.clone(), position, tool.tool_call_id.clone()));
                        }
                        ChatPart::Subagent { subagent, .. } => {
                            cards.push((
                                subagent.session_id.clone(),
                                subagent.state == SubagentState::Running,
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
        for (row, part, id) in running {
            let mut raw = self
                .tools
                .get_mut(&(within.clone(), id.clone()))
                .and_then(|place| place.raw.take())
                .unwrap_or_else(|| {
                    let mut raw = Map::new();
                    raw.insert("toolCallId".into(), Value::String(id.clone()));
                    raw
                });
            raw.insert("status".into(), Value::from("cancelled"));
            self.settle_tool(within, &row, part, &raw, at);
        }
        for (session, running) in cards {
            if running && !end_subagents {
                continue;
            }
            self.settle_transcript(&Some(session.clone()), at, true);
            if running {
                self.patch_card(&session, |card| card.state = SubagentState::Cancelled);
            }
        }
    }

    fn patch_card(&mut self, session: &str, patch: impl FnOnce(&mut SubagentCard)) {
        let Some(place) = self.cards.get(session) else {
            return;
        };
        let (within, row, part) = (place.within.clone(), place.row.clone(), place.part);
        let Some(transcript) = self.transcript(&within) else {
            return;
        };
        let Some(position) = transcript.position(&row) else {
            return;
        };
        let held = &mut transcript.rows[position];
        let Some(ChatPart::Subagent { subagent, .. }) = held.msg.parts.get_mut(part) else {
            return;
        };
        let before = subagent.clone();
        patch(subagent);
        if *subagent == before {
            return;
        }
        held.touch_part(part);
        transcript.touch(position);
        self.note_main_change(&within, position);
        if within.is_none() {
            self.refresh_running_subagents();
        }
    }

    fn refresh_running_subagents(&mut self) {
        let running: Vec<SubagentCard> = self
            .main
            .rows
            .iter()
            .flat_map(|row| row.msg.parts.iter())
            .filter_map(|part| match part {
                ChatPart::Subagent { subagent, .. } if subagent.state == SubagentState::Running => {
                    Some(subagent.clone())
                }
                _ => None,
            })
            .collect();
        if running != self.head.running_subagents {
            self.head.running_subagents = running;
            self.set_head("runningSubagents");
        }
    }

    fn spawn_subagent(&mut self, index: u64, within: &Within, update: &Value) -> bool {
        let Some(session) = text_of(update.get("subagentSessionId")) else {
            return false;
        };
        if self.cards.contains_key(session) {
            return false;
        }
        let card = SubagentCard {
            session_id: session.to_owned(),
            name: text_of(update.get("name")).unwrap_or("Subagent").to_owned(),
            task: text_of(update.get("task")).unwrap_or_default().to_owned(),
            state: SubagentState::Running,
            tokens: None,
            calls: 0,
            last_tool: None,
        };
        let Some((row, part)) = self.append_part(
            index,
            within,
            |id| ChatPart::Subagent { id, subagent: card },
            false,
        ) else {
            return false;
        };
        self.cards.insert(
            session.to_owned(),
            CardPlace {
                within: within.clone(),
                row,
                part,
            },
        );
        self.subs.insert(session.to_owned(), Transcript::default());
        if within.is_none() {
            self.refresh_running_subagents();
        }
        true
    }

    fn set_subagent_state(&mut self, update: &Value, at: u64) {
        let (Some(session), Some(state)) = (
            text_of(update.get("subagentSessionId")),
            text_of(update.get("state")).and_then(subagent_state),
        ) else {
            return;
        };
        if state != SubagentState::Running {
            self.settle_transcript(&Some(session.to_owned()), at, true);
        }
        self.patch_card(session, |card| card.state = state);
    }

    fn spawn_task(&mut self, update: &Value) {
        let Some(id) = text_of(update.get("asyncTaskId")) else {
            return;
        };
        if self.head.tasks.iter().any(|task| task.async_task_id == id) {
            return;
        }
        self.head.tasks.push(AsyncTask {
            async_task_id: id.to_owned(),
            name: text_of(update.get("name"))
                .unwrap_or("Background task")
                .to_owned(),
            task_type: text_of(update.get("taskType"))
                .unwrap_or_default()
                .to_owned(),
            description: text_of(update.get("description"))
                .unwrap_or_default()
                .to_owned(),
            state: TaskState::Running,
            can_stop: update.get("canStop") == Some(&Value::Bool(true)),
            summary: None,
            last_tool_name: None,
            output_file_path: text_of(update.get("outputFilePath")).map(str::to_owned),
            usage: None,
            owner: text_of(update.get("owner")).map(str::to_owned),
            agents: Vec::new(),
        });
        self.set_head("tasks");
    }

    fn patch_task(&mut self, index: u64, update: &Value) {
        let id = text_of(update.get("asyncTaskId"));
        let Some(position) = self
            .head
            .tasks
            .iter()
            .position(|task| Some(task.async_task_id.as_str()) == id)
        else {
            return;
        };
        let state = text_of(update.get("state")).and_then(task_state);
        // A task that reached its end has nothing left to watch or stop, so it
        // leaves the composer and says how it went in the transcript instead.
        if let Some(ended @ (TaskState::Completed | TaskState::Failed | TaskState::Stopped)) = state
        {
            let task = self.head.tasks.remove(position);
            self.set_head("tasks");
            let notice = TaskNotice {
                name: task.name.clone(),
                state: match ended {
                    TaskState::Completed => NoticeState::Completed,
                    TaskState::Failed => NoticeState::Failed,
                    _ => NoticeState::Stopped,
                },
                summary: text_of(update.get("summary"))
                    .map(str::to_owned)
                    .or(task.summary),
                task_type: (!task.task_type.is_empty()).then_some(task.task_type),
                subagent: None,
            };
            let between = !self.head.running;
            self.add_notice(
                index,
                &task.async_task_id,
                notice,
                stamped_at(update),
                between,
            );
            return;
        }
        let task = &mut self.head.tasks[position];
        let text = |key: &str| text_of(update.get(key)).map(str::to_owned);
        if let Some(state) = state {
            task.state = state;
        }
        if let Some(description) = text("description") {
            task.description = description;
        }
        task.summary = text("summary").or(task.summary.take());
        task.last_tool_name = text("lastToolName").or(task.last_tool_name.take());
        task.output_file_path = text("outputFilePath").or(task.output_file_path.take());
        if let Some(usage) = update.get("usage").and_then(|usage| {
            Some(TaskUsage {
                total_tokens: usage.get("totalTokens")?.as_u64()?,
                tool_uses: usage.get("toolUses")?.as_u64()?,
                duration_ms: usage.get("durationMs")?.as_u64()?,
            })
        }) {
            task.usage = Some(usage);
        }
        if let Some(agents) = update.get("agents").and_then(workflow_agents) {
            task.agents = agents;
        }
        self.set_head("tasks");
    }

    /// Work that ends while the agent is idle is what wakes it, so its notice
    /// stands between the turns, saying why the next one began, rather than at
    /// the end of the last one.
    fn add_notice(
        &mut self,
        index: u64,
        task: &str,
        notice: TaskNotice,
        at: Option<u64>,
        between: bool,
    ) {
        if !self.notices.insert(task.to_owned()) {
            return;
        }
        if !between {
            self.append_part(index, &None, |id| ChatPart::Notice { id, notice }, false);
            return;
        }
        let id = self.row_id(index, &None);
        let mut row = Row::new(id, ChatRole::Assistant, index);
        row.msg.opens_turn = Some(TurnStart { at });
        let part_id = row.next_part_id();
        row.push_part(ChatPart::Notice {
            id: part_id,
            notice,
        });
        let position = self.main.push(row);
        self.note_main_change(&None, position);
    }

    /// Claude Code tells itself a background task ended in a tag the person
    /// never sees. History keeps it, and it is the only record of why the next
    /// turn began.
    fn harness_notice(&mut self, index: u64, update: &Value) -> bool {
        let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
            return false;
        };
        if !text.contains("<task-notification>") {
            return false;
        }
        let tag = |name: &str| -> Option<String> {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            let start = text.find(&open)? + open.len();
            let len = text[start..].find(&close)?;
            Some(text[start..start + len].trim().to_owned())
        };
        let (Some(task), Some(state)) = (
            tag("task-id"),
            tag("status").as_deref().and_then(notice_state),
        ) else {
            return true;
        };
        let summary = tag("summary");
        let name = summary
            .as_deref()
            .and_then(|summary| {
                let start = summary.find('"')? + 1;
                let len = summary[start..].find('"')?;
                (len > 0).then(|| summary[start..start + len].to_owned())
            })
            .or(summary)
            .unwrap_or_else(|| "Background task".into());
        // History marks the notice that woke the agent; one that came mid-turn belongs to that turn.
        let woke = !self.head.running && replayed_turn_start(update).is_some();
        let notice = TaskNotice {
            name,
            state,
            summary: None,
            task_type: None,
            subagent: text_of(update.get("subagentSessionId")).map(str::to_owned),
        };
        self.add_notice(index, &task, notice, stamped_at(update), woke);
        true
    }

    fn agent_notice(&mut self, index: u64, update: &Value) {
        let Some(title) = text_of(update.get("title")) else {
            return;
        };
        // The stopped task's own line already says so.
        if title == "Task stopped by user" {
            return;
        }
        let severity = match text_of(update.get("severity")) {
            Some("warning") => NoticeSeverity::Warning,
            Some("error") => NoticeSeverity::Error,
            _ => NoticeSeverity::Info,
        };
        let notice = AgentNotice {
            severity,
            title: title.to_owned(),
            description: text_of(update.get("description")).map(str::to_owned),
        };
        // A chat a usage limit stopped says when it resumes, or that it no longer will.
        if let Some(failure) = self
            .head
            .failure
            .as_mut()
            .filter(|failure| failure.kind == FailureKind::Limit)
        {
            if update.get("resumesAt").is_some() {
                failure.resumes_at = update.get("resumesAt").and_then(Value::as_u64);
                self.head_dirty.insert("failure");
            }
        }
        self.append_part(
            index,
            &None,
            |id| ChatPart::AgentNotice { id, notice },
            false,
        );
    }

    /// A cancelled turn stops only the subagents it was waiting on, and the
    /// agent says which. Background ones run on and report their own end.
    fn end_turn(&mut self, stop_reason: Option<StopReason>, at: u64) {
        let was_running = self.head.running;
        self.settle_transcript(&None, at, false);
        if was_running {
            if let Some(last) = self
                .main
                .rows
                .last_mut()
                .filter(|row| row.msg.role == ChatRole::Assistant)
            {
                last.msg.ended_at = Some(at);
                last.set("endedAt");
                let position = self.main.rows.len() - 1;
                self.main.touch(position);
                self.note_main_change(&None, position);
            }
        }
        self.head.running = false;
        self.head.turn_opening = None;
        self.head.retrying = None;
        self.head.permissions.clear();
        self.head.stop_reason = stop_reason;
        self.suppress_user_echo = false;
        for field in [
            "running",
            "turnOpening",
            "retrying",
            "permissions",
            "stopReason",
        ] {
            self.set_head(field);
        }
        self.settle_last_run();
    }

    /// Whether the last run can now fold depends on the turn having ended.
    fn settle_last_run(&mut self) {
        if let Some(last) = self.main.rows.len().checked_sub(1) {
            self.settle_from = Some(self.settle_from.map_or(last, |from| from.min(last)));
        }
    }

    /// The change a tool call made and what it printed, line by line.
    pub fn tool_detail(&self, tool_call_id: &str) -> Option<ToolDetail> {
        self.details.get(tool_call_id).cloned()
    }
}

/// What a person sent with a message is shown by name: their own text stays
/// prose, an issue they handed over is a chip, and letters from other agents
/// are read out. A row read again goes out whole.
fn read_sent(row: &mut Row) {
    let mut context = row.sent_context.clone();
    let mut parts = Vec::new();
    for part in &row.raw {
        match part {
            ChatPart::Content {
                content: ContentBlock::Resource { resource },
                ..
            } => {
                let first = resource
                    .text
                    .as_deref()
                    .and_then(|text| text.split('\n').next())
                    .filter(|line| !line.is_empty())
                    .unwrap_or(&resource.uri);
                context.push(SentContext {
                    uri: resource.uri.clone(),
                    title: first.to_owned(),
                });
            }
            ChatPart::Text { text, .. } => {
                let (rest, found) = prompt::split_sent_context(text);
                context.extend(found);
                if !rest.is_empty() {
                    parts.push(ChatPart::Text {
                        id: String::new(),
                        text: rest,
                        md: None,
                        cut_bytes: None,
                    });
                }
            }
            _ => parts.push(part.clone()),
        }
    }
    let mut seen = HashSet::new();
    context.retain(|item| !prompt::is_mention(&item.uri) && seen.insert(item.uri.clone()));
    let whole: String = parts
        .iter()
        .map(|part| match part {
            ChatPart::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    let only_text = parts
        .iter()
        .all(|part| matches!(part, ChatPart::Text { .. }));
    let letters = only_text
        .then(|| prompt::agent_letters(&whole))
        .flatten()
        .unwrap_or_default();
    for (at, part) in parts.iter_mut().enumerate() {
        let id = format!("{}.{at}", row.msg.id);
        match part {
            ChatPart::Text { id: own, .. } | ChatPart::Content { id: own, .. } => *own = id,
            _ => {}
        }
    }
    row.sides = parts.iter().map(|_| Side::default()).collect();
    row.msg.parts = parts;
    row.msg.context = context;
    row.msg.letters = letters;
    row.fresh = true;
}
