//! The part of a chat that is the same for every agent the core speaks to in
//! its own protocol: turns, permission requests, accounts and starting the
//! agent again under the chat.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::os::fd::OwnedFd;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::launch::PermissionMode;

use crate::acp::failure::{sounds_offline, Failure, FailureKind};
use crate::agent_accounts::sign_in::{self, SignIn};
use crate::server::chat::permission;
use crate::server::chat::prompt::MAX_CONTEXT_ITEMS;
use crate::server::inbox::Fault;

use super::SessionEnd;
use sikemux_fold::update::turn_answer;
use sikemux_fold::{ChatEventKind, UpdateKind};
use sikemux_view::chat::{SessionRotation, StopReason};
use sikemux_view::questions::QuestionChoice;
use sikemux_wire::protocol::{AgentState, ChatAccount, ChatContext, ChatLaunch, ChatStart};

use super::super::feed::{Earlier, Reach};
use super::super::kept::Progress;
use super::super::rebind::{
    error_message, next_account, on_account, switch_notice, switches_at_limit, Carried, Outcome,
    Rebind, Replay,
};
use super::super::{cap, rotation};
use super::super::{reset, stall_later, Asked, Chat, ChatCommand, Delivery, Respond};
use super::process::{AgentRecord, Lines, Process, Released, Writer};
use crate::server::activity_log::{Compaction, ModelCall};
use crate::server::agent_accounts;

/// What the person sent, for the agent to read in its own form.
#[derive(Clone)]
pub(crate) struct Prompt {
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
    /// A message from another agent rather than the person.
    pub letter: bool,
    /// Sent before, to this agent or one that ran the session earlier, so its id may be taken.
    pub resent: bool,
}

impl From<&Replay> for Prompt {
    fn from(replay: &Replay) -> Self {
        Self {
            message_id: replay.message_id.clone(),
            text: replay.text.clone(),
            paths: replay.paths.clone(),
            context: replay.context.clone(),
            letter: false,
            resent: !replay.announce,
        }
    }
}

/// The most of any one text an update carries. The chat shows only the first
/// few hundred lines of what a tool printed, and a long chat loaded again
/// would otherwise send all of it.
const MAX_TEXT: usize = 128 * 1024;
/// Where a chat's brief says it is from, when it goes with the first message.
const BRIEF_URI: &str = "sikemux://memory";
/// The largest picture an update carries, as base64. The chat keeps none
/// bigger.
const MAX_IMAGE: usize = 2 * 1024 * 1024;

/// Cuts every text in `value` down to what the chat can show.
pub(crate) fn slim(value: &mut Value) {
    match value {
        Value::String(text) if text.len() > MAX_TEXT => {
            let mut end = MAX_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        Value::Array(items) => items.iter_mut().for_each(slim),
        Value::Object(fields) => {
            let picture = fields.get("data").and_then(Value::as_str).map(str::len);
            if picture.is_some_and(|len| len > MAX_IMAGE) {
                fields.remove("data");
            }
            for (key, field) in fields.iter_mut() {
                if key != "data" {
                    slim(field);
                }
            }
        }
        _ => {}
    }
}

pub(crate) enum TurnEnd {
    /// The `turn_completed` payload, such as `{"stopReason": "end_turn"}`.
    Completed(Value),
    /// The agent could not finish it, and said why.
    Failed {
        failure: Failure,
        response: Option<Value>,
    },
    /// The agent turned the prompt down with this error.
    Refused(String),
    /// The agent can take no more turns in this session, for this reason, and
    /// is started again.
    Broken(String),
}

impl TurnEnd {
    /// A failure or refusal whose words say the network was at fault reads as
    /// one, whichever agent it came from, so the prompt goes again once it is back.
    fn read_offline(self) -> Self {
        match self {
            Self::Failed { failure, response }
                if failure.kind == FailureKind::Other && sounds_offline(&failure.title) =>
            {
                Self::Failed {
                    failure: Failure::lost_connection(failure.title),
                    response,
                }
            }
            Self::Refused(message) if sounds_offline(&message) => Self::Failed {
                failure: Failure::lost_connection(message),
                response: None,
            },
            end => end,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Steered {
    Injected,
    /// The turn ended first, so it goes as a prompt instead.
    PromptRequired,
}

impl Steered {
    /// As ACP agents say it and as the phone's call answers it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::PromptRequired => "promptRequired",
        }
    }
}

enum Woke {
    Command(Option<ChatCommand>),
    Closed(String),
    Broken(String),
    Failed(Failed),
    Stalled(u64),
    Settled(u64),
}

/// A turn that failed in a way a new agent process or another account may fix.
pub(crate) struct Failed {
    turn: u64,
    failure: Failure,
    response: Option<Value>,
}

/// What tells a chat a session's history, the one thing a chat's saved history
/// needs shown before its agent starts.
#[derive(Clone)]
pub(crate) struct Telling {
    chat: Arc<Chat>,
    /// Quiet while a session loaded under a running chat replays history the
    /// chat already shows.
    quiet: Arc<std::sync::atomic::AtomicBool>,
}

impl Telling {
    pub(crate) fn new(chat: Arc<Chat>) -> Self {
        Self {
            chat,
            quiet: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn update(&self, session_id: &str, mut update: Value) {
        // The commands an agent lists are what it offers now, never history it replays.
        if self.quiet.load(Ordering::Acquire)
            && UpdateKind::of(&update) != Some(UpdateKind::AvailableCommandsUpdate)
        {
            return;
        }
        self.chat.note_edits(&update);
        slim(&mut update);
        self.chat.emit(
            ChatEventKind::SessionUpdate,
            json!({ "sessionId": session_id, "update": update }),
        );
    }

    /// Whether this start loads the session again under a running chat.
    pub fn reloading(&self) -> bool {
        self.quiet.load(Ordering::Acquire)
    }

    /// What the agent says until [`Telling::loaded`] is history the chat already shows.
    pub fn hush(&self) {
        self.quiet.store(true, Ordering::Release);
    }

    /// The session has loaded, so what the agent says from now on is new to
    /// the chat.
    pub fn loaded(&self) {
        self.quiet.store(false, Ordering::Release);
    }

    pub fn reach(&self, session: &str) -> Option<Reach> {
        self.chat.feed.reach(session)
    }

    pub fn leave_room(&self) {
        self.chat.feed.leave_room();
    }

    pub fn tell_earlier(&self, earlier: Earlier) {
        self.chat.feed.tell_earlier(earlier);
    }

    pub fn reached(&self, session: &str, reach: Reach) {
        self.chat.feed.reached(session, reach);
    }

    /// Whether the chat's session is kept small.
    pub fn capped(&self) -> bool {
        self.chat.capped()
    }

    /// The core's own compaction has ended, done or not.
    pub fn compaction_over(&self) {
        self.chat.meter.compaction_over();
    }

    /// What the session's compaction is told to keep.
    pub fn keep_list(&self) -> String {
        match self.chat.core.upgrade() {
            Some(core) => cap::keep_list(&core, &self.chat),
            None => String::new(),
        }
    }

    /// What the session is told after its compaction.
    pub fn after_compaction(&self) -> Option<String> {
        let core = self.chat.core.upgrade()?;
        cap::after_compaction(&core, &self.chat)
    }

    /// The session called the model with `context` tokens.
    pub fn called(&self, context: u64) {
        self.chat.meter.called(context);
    }

    /// The session was compacted down to `context` tokens.
    pub fn compacted(&self, context: u64) {
        self.chat.meter.compacted(context);
    }

    pub fn session_used(&self, calls: Vec<ModelCall>, compactions: Vec<Compaction>) {
        if let Some(core) = self.chat.core.upgrade() {
            let project = core.workspaces.project_of(&self.chat.launch.cwd);
            core.activity_log
                .session_used(self.chat.agent_id(), &project, calls, compactions);
        }
    }
}

/// What an agent's reader tells the chat. Cheap to clone, and safe to use
/// from any task.
#[derive(Clone)]
pub(crate) struct Sink {
    telling: Telling,
    failed: mpsc::UnboundedSender<Failed>,
    broken: mpsc::UnboundedSender<String>,
    settled: mpsc::UnboundedSender<u64>,
    account: Option<String>,
}

impl std::ops::Deref for Sink {
    type Target = Telling;

    fn deref(&self) -> &Telling {
        &self.telling
    }
}

impl Sink {
    pub fn error(&self, message: impl std::fmt::Display) {
        self.chat.error(error_message(message));
    }

    /// Whether the provider has a saved session to load.
    pub fn has_answered(&self) -> bool {
        self.chat.has_answered()
    }

    /// Whether a turn this side prompted is being answered.
    pub fn running(&self) -> bool {
        self.chat.prompted()
    }

    pub fn cli_version(&self, version: &str) {
        self.chat.note_cli_version(version);
    }

    /// Keeps the plan limits the agent reported as it worked as its account's reading.
    pub fn plan_limits(&self, live: Vec<crate::agent_accounts::usage::UsageWindow>) {
        if let Some(core) = self.chat.core.upgrade() {
            let provider = self.chat.provider();
            agent_accounts::note_live(&core, provider, self.account.as_deref(), live);
        }
    }

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    pub fn work_started(&self) {
        self.chat.work_started();
    }

    pub fn work_ended(&self, stop_reason: StopReason) {
        self.chat.work_ended(stop_reason);
    }

    /// Settles turn `turn`, or hands a failure another account may fix to
    /// the chat's loop to decide on, with the turn still running.
    pub fn end_turn(&self, turn: u64, end: TurnEnd) {
        let end = end.read_offline();
        let chat = &self.chat;
        chat.cancel_permissions();
        let answer = match &end {
            TurnEnd::Completed(payload) => Some(payload),
            TurnEnd::Failed { response, .. } => response.as_ref(),
            _ => None,
        };
        if let (Some(answer), Some(core)) = (answer, chat.core.upgrade()) {
            core.activity_log
                .turn_answered(&chat.current_launch(), answer);
        }
        match end {
            TurnEnd::Completed(payload) => {
                chat.note_answered();
                chat.finish_turn(payload);
            }
            TurnEnd::Failed { failure, response } if failure.kind != FailureKind::Other => {
                chat.end_unprompted();
                let _ = self.failed.send(Failed {
                    turn,
                    failure,
                    response,
                });
                return;
            }
            TurnEnd::Failed { failure, response } => {
                let failure = failure.payload(self.account.as_deref());
                chat.turn_failing(&failure, Fault::Broke);
                chat.finish_turn(response.unwrap_or_else(|| turn_answer(StopReason::EndTurn)));
                chat.fail_turn(failure);
            }
            TurnEnd::Refused(message) => chat.fail_turn(error_message(message)),
            TurnEnd::Broken(reason) => {
                chat.cut_turn();
                let _ = self.settled.send(turn);
                let _ = self.broken.send(reason);
                return;
            }
        }
        let _ = self.settled.send(turn);
    }

    /// A message from another agent went into the running turn but the agent
    /// dropped it unread, so it waits for the agent's next turn instead.
    pub fn letter_unread(&self, text: String) {
        self.chat.letter_unread(text);
    }

    /// The agent asked a question with its CLI's own tool. Answers what it
    /// is told in place of the answer, which comes later as a message.
    pub fn question(&self, input: &Value) -> String {
        self.chat.ask_question(input)
    }

    /// Puts to the person a dialog the agent is blocked on, whose answer comes
    /// back through [`Backend::answer_dialog`].
    pub fn ask_dialog(
        &self,
        dialog: String,
        header: &str,
        text: &str,
        choices: Vec<QuestionChoice>,
    ) -> Result<(), String> {
        self.chat.ask_dialog(dialog, header, text, choices)
    }

    pub fn withdraw_dialogs(&self, only: Option<&str>) {
        self.chat.withdraw_dialogs(only);
    }

    pub fn withdraw_permission(&self, tool_call_id: &str) {
        self.chat.withdraw_permission(tool_call_id);
    }

    /// `text` went into the running turn; `person` when the person sent it.
    pub fn prompted(&self, text: &str, paths: &[String], person: bool) {
        self.chat.feed.prompted(None, text, paths, &[], person);
    }

    /// Asks the person about a tool call. `request` is shaped as an ACP
    /// permission request: the session, the tool call and its `options`.
    /// `reply` is what the backend needs to answer the agent, kept with the
    /// question so a core that takes the chat over can answer it too.
    /// `responder` makes from it what is told the chosen option's id, or none
    /// when it was cancelled.
    pub fn ask<R: Serialize>(
        &self,
        request: Value,
        reply: R,
        responder: impl FnOnce(R) -> Respond,
    ) {
        if self.chat.approves() {
            if let Some(option_id) = permission::approval(&request) {
                responder(reply)(Some(option_id));
                return;
            }
        }
        self.ask_person(request, reply, responder);
    }

    /// Like `ask`, for what only the person may answer, even under YOLO.
    pub fn ask_person<R: Serialize>(
        &self,
        mut request: Value,
        reply: R,
        responder: impl FnOnce(R) -> Respond,
    ) {
        let kept = serde_json::to_value(&reply).unwrap_or(Value::Null);
        let respond = responder(reply);
        let option_ids = request
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| Some(option.get("optionId")?.as_str()?.to_owned()))
            .collect();
        let request_id = Uuid::new_v4().to_string();
        if let Some(object) = request.as_object_mut() {
            object.insert("requestId".into(), Value::String(request_id.clone()));
        }
        if self
            .chat
            .hold_permission(request_id, option_ids, respond, request.clone(), kept)
        {
            self.chat.emit(ChatEventKind::PermissionRequest, request);
        }
    }
}

/// Sends `command` to the chat's loop again once the running turn has ended.
fn after_turn(chat: Arc<Chat>, command: ChatCommand) {
    tokio::spawn(async move {
        tokio::select! {
            () = chat.turn_ended() => {
                let _ = chat.send(command);
            }
            () = chat.commands.closed() => {}
        }
    });
}

/// How many subagents' histories a resumed chat reads.
const MAX_SUBAGENT_HISTORIES: usize = 64;

/// The histories of the subagents `first` names and of theirs, each read by
/// `read`, whose subagents `children` names.
pub(crate) async fn subagent_histories<F>(
    first: Vec<String>,
    read: impl Fn(String) -> F,
    children: impl Fn(&[Value]) -> Vec<String>,
) -> HashMap<String, Vec<Value>>
where
    F: std::future::Future<Output = Vec<Value>>,
{
    let mut histories = HashMap::new();
    let mut queue = first;
    while let Some(child) = queue.pop() {
        if histories.contains_key(&child) || histories.len() >= MAX_SUBAGENT_HISTORIES {
            continue;
        }
        let said = read(child.clone()).await;
        queue.extend(children(&said));
        histories.insert(child, said);
    }
    histories
}

/// The session an agent opened, as the chat reads it.
pub(crate) struct Opened {
    pub session_id: String,
    pub capabilities: Value,
    pub setup: Value,
}

impl Opened {
    /// What the chat can do with an agent the core speaks to in that agent's own protocol.
    pub fn native_capabilities() -> Value {
        json!({
            "loadSession": true,
            "steering": true,
            "editing": true,
            "promptCapabilities": { "image": true, "embeddedContext": true },
        })
    }
}

/// A chat's brief as context handed over with its first message, titled by its first line.
fn brief_context(brief: String) -> ChatContext {
    ChatContext {
        uri: BRIEF_URI.to_owned(),
        title: brief.lines().next().unwrap_or_default().to_owned(),
        text: brief,
    }
}

/// One agent the core speaks to in its own protocol.
pub(crate) trait Backend: Sized + Send {
    /// What it needs kept with a question to answer the agent.
    type Reply: DeserializeOwned;

    /// The agent takes instructions of the core's own as it starts, so a
    /// chat's brief goes there rather than with its first message.
    const INSTRUCTIONS: bool = false;

    /// The agent takes a new effort while a turn runs, so the person need not stop it first.
    const EFFORT_MID_TURN: bool = false;

    /// Starts the agent and opens the chat's session, or loads the one it
    /// resumes, replaying its history through `sink`, with `brief` among its
    /// instructions when it takes them.
    fn start(
        launch: &ChatLaunch,
        brief: Option<&str>,
        sink: Sink,
    ) -> impl std::future::Future<Output = Result<(Self, Opened), String>> + Send;

    /// Sends a prompt that begins turn `turn`. How it ends is told to the
    /// sink.
    fn prompt(
        &mut self,
        turn: u64,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Puts a message into the running turn. The answer is waited on apart
    /// from the chat's other commands, so a stop sent right after a steer is
    /// never held up behind it.
    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static;

    fn cancel(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send;

    fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Changes a session option, answering with the options as they now are.
    fn set_config(
        &mut self,
        config_id: &str,
        value: &str,
    ) -> impl std::future::Future<Output = Result<Value, String>> + Send;

    fn stop_task(
        &mut self,
        task_id: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Compacts the session between turns, with no turn in the chat to show for it.
    fn compact(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send {
        async { Ok(()) }
    }

    /// Asks the session between turns what a fresh one would need to know, with no turn in
    /// the chat to show for it, and answers with what it wrote.
    fn handoff(&mut self) -> impl std::future::Future<Output = Result<String, String>> + Send {
        async { Err("This agent writes no handoff note".to_owned()) }
    }

    /// The person answered a dialog the agent is blocked on.
    fn answer_dialog(&mut self, _id: &str, _answer: String, _choice: Option<usize>) {}

    /// Asks the agent to sign in by itself, by `method_id`, one of the
    /// methods it offered.
    fn authenticate(
        &mut self,
        _method_id: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send {
        async { Err("This agent signs in only through its own sign-in".to_string()) }
    }

    /// Takes the conversation back to before the person's message `message_id`,
    /// answering with the session it goes on in when that is a new one.
    fn rewind(
        &mut self,
        message_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + Send;

    /// Resolves once the agent is gone, with why.
    fn closed(&self) -> impl std::future::Future<Output = String> + Send;

    /// The agent's process, which leads its own process group.
    fn pid(&self) -> u32;

    /// Whether a new agent process can load this session, so the chat can go
    /// on in one after a sign-in or on another account.
    fn reloadable(&self) -> bool {
        true
    }

    /// How far into its own record of the session the agent has written, all of which the
    /// chat has been told.
    fn reach(&self) -> Option<Reach> {
        None
    }

    /// Lets go of the agent without stopping it, answering with what a core
    /// that takes it up needs: this side's state as plain data, and the
    /// agent's process. On failure the agent is stopped.
    fn release(self)
        -> impl std::future::Future<Output = Result<(Value, Released), String>> + Send;

    /// Takes up an agent an earlier core released, from the state `release`
    /// answered.
    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        agent: (Process, Writer, Lines),
    ) -> Result<Self, String>;

    /// What answers the agent with the person's choice for a question whose
    /// backend data is `reply`.
    fn responder(&self, reply: Self::Reply) -> Respond;
}

/// Where the chat's loop stands, as plain data, for a core that takes the
/// chat over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionState {
    session_id: String,
    setup: Value,
    capabilities: Value,
    turn: u64,
    last_prompt: Option<Replay>,
    failed_prompt: Option<Replay>,
    exhausted: Vec<String>,
    signed_in_again: bool,
    signed_in_at_start: Option<SignIn>,
    reloadable: bool,
    /// The turn asked to stop, which is given a while before the agent is
    /// taken to be stuck.
    stopping: Option<u64>,
    /// The account the chat moves to once its running turn has stopped.
    switching: Option<Switching>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Switching {
    account: ChatAccount,
    /// The agent is told to carry on with the stopped turn, unless the person
    /// stopped it too.
    carry_on: bool,
}

/// A chat let go of by its loop.
pub(crate) struct Handed {
    pub session: SessionState,
    pub backend: Value,
    pub released: Released,
}

/// A chat another core let go of, to take up.
pub(crate) struct Adopted {
    pub session: SessionState,
    pub backend: Value,
    pub agent: AgentRecord,
    pub pipes: [OwnedFd; 3],
    pub asked: Vec<Asked>,
}

/// How the chat's loop gets its agent.
pub(crate) enum Begin {
    /// Starts the agent, or starts it again under the chat. `notice` is said
    /// in the transcript once the agent is up.
    Start {
        rebind: Option<Box<Rebind>>,
        notice: Option<Value>,
    },
    Adopt(Box<Adopted>),
}

pub(crate) async fn run<B: Backend>(
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    opening: Begin,
) -> Result<Outcome, String> {
    chat.at_rest.store(false, Ordering::Release);
    let provider = launch.provider.clone();
    let (failed_tx, mut failed_rx) = mpsc::unbounded_channel::<Failed>();
    let (broken_tx, mut broken_rx) = mpsc::unbounded_channel::<String>();
    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<u64>();
    // The agent signs in as whoever owns the folder it runs in, so that folder names the
    // account its live limits and failures belong to, whatever the chat was labelled.
    let account = (chat.core.upgrade())
        .and_then(|core| {
            let folder = crate::launch::named_account(&provider, &launch.env);
            let book = core.workspaces.accounts().unwrap_or_default();
            Some(book.account_in(&provider, folder)?.id)
        })
        .or_else(|| launch.account.as_ref().map(|account| account.id.clone()));
    let sink = |quiet: bool| Sink {
        telling: Telling {
            chat: chat.clone(),
            quiet: Arc::new(std::sync::atomic::AtomicBool::new(quiet)),
        },
        failed: failed_tx.clone(),
        broken: broken_tx.clone(),
        settled: settled_tx.clone(),
        account: account.clone(),
    };
    let (backend, state, replay) = match opening {
        Begin::Start { rebind, notice } => {
            let first = rebind.is_none();
            chat.emit(ChatEventKind::Status, json!({ "state": "starting" }));
            let signed_in_at_start = sign_in::signed_in(&provider, &launch.env);
            let Carried {
                replay,
                exhausted,
                signed_in_again,
                notice: carried,
                quiet_load,
            } = Carried::from(rebind);
            let sink = sink(quiet_load || chat.saved_shown.load(Ordering::Acquire));
            chat.emit(ChatEventKind::Status, json!({ "state": "initializing" }));
            chat.feed.hold();
            // With no history to come, a message sent while the agent starts shows at once.
            if first && chat.history_in_place() {
                chat.feed.let_out();
            }
            let brief = B::INSTRUCTIONS.then(|| chat.brief()).flatten();
            let reaching_provider = chat.with_model_token(&launch)?;
            let started = B::start(&reaching_provider, brief.as_deref(), sink.clone()).await;
            chat.feed.let_out();
            let (backend, opened) = started?;
            sink.loaded();
            let state = SessionState {
                session_id: opened.session_id,
                setup: opened.setup,
                capabilities: opened.capabilities,
                turn: 0,
                last_prompt: None,
                failed_prompt: None,
                exhausted,
                signed_in_again,
                signed_in_at_start,
                reloadable: backend.reloadable(),
                stopping: None,
                switching: None,
            };
            let replay = replay.or_else(|| reset::take_due(&chat));
            // Running before it reads as ready, so no client sees a turn about to go on as over.
            if replay.is_some() {
                chat.start_prompted();
            }
            chat.note_agent(backend.pid());
            let start = state.start();
            chat.mark_ready(start.clone());
            chat.emit(ChatEventKind::Ready, json!(start));
            for update in carried.into_iter().chain(notice) {
                sink.update(&state.session_id, update);
            }
            (backend, state, replay)
        }
        Begin::Adopt(adopted) => {
            let Adopted {
                session,
                backend: kept,
                agent,
                pipes,
                asked,
            } = *adopted;
            let [stdin, stdout, stderr] = pipes;
            let adopted = Process::adopt(&agent, &launch.agent_id, stdin, stdout, stderr)
                .inspect_err(|_| super::process::kill_and_reap(agent.pid))
                .and_then(|process| B::adopt(&launch, sink(false), kept, process));
            match adopted {
                Ok(backend) => {
                    chat.note_agent(backend.pid());
                    for asked in asked {
                        match serde_json::from_value(asked.reply.clone()) {
                            Ok(reply) => chat.hold_again(asked, backend.responder(reply)),
                            Err(error) => eprintln!(
                                "sikemux core: a question handed over could not be read: {error}"
                            ),
                        }
                    }
                    chat.mark_ready(session.start());
                    chat.finish_held();
                    (backend, session, None)
                }
                Err(error) => return Ok(not_taken_over(&chat, &launch, &error)),
            }
        }
    };

    let (stalled_tx, mut stalled_rx) = mpsc::unbounded_channel::<u64>();
    let telling = sink(false);
    let mut running = Running {
        chat,
        launch: &launch,
        provider: &provider,
        backend,
        state,
        stalled: stalled_tx,
        letter_turn: false,
        sink: telling,
        rotation_brief: None,
        fresh_from: None,
    };
    running.chat.rotation.runs_on(running.footing());
    running.opened(replay).await;
    let end = loop {
        if !running.chat.prompted() && commands.is_empty() {
            running.ready_rotation().await;
        }
        running.rest();
        // What the agent already said is taken in before the next command,
        // so a turn's failure or end is never cut off by letting go of the
        // agent.
        let woke = tokio::select! {
            biased;
            Some(reason) = broken_rx.recv() => Woke::Broken(reason),
            Some(failed) = failed_rx.recv() => Woke::Failed(failed),
            Some(stalled) = stalled_rx.recv() => Woke::Stalled(stalled),
            Some(settled) = settled_rx.recv() => Woke::Settled(settled),
            reason = running.backend.closed() => Woke::Closed(reason),
            command = commands.recv() => Woke::Command(command),
        };
        running.chat.at_rest.store(false, Ordering::Release);
        let next = match woke {
            Woke::Command(Some(ChatCommand::HandOver { reply })) => {
                return Ok(running.hand_over(reply).await)
            }
            Woke::Command(Some(command)) => running.command(command).await,
            Woke::Command(None) => ControlFlow::Break(Outcome::Ended(SessionEnd::Requested)),
            Woke::Closed(reason) => ControlFlow::Break(running.stopped(false, reason).await),
            Woke::Broken(reason) => ControlFlow::Break(running.stopped(true, reason).await),
            Woke::Failed(failed) => running.failed(failed),
            Woke::Settled(settled) => running.settled(settled),
            Woke::Stalled(stalled) => running.stalled(stalled).await,
        };
        if let ControlFlow::Break(end) = next {
            break end;
        }
    };
    running.tell_reach();
    Ok(end)
}

/// A chat's loop while its agent runs, which takes what woke it one at a time.
struct Running<'a, B> {
    chat: Arc<Chat>,
    launch: &'a ChatLaunch,
    provider: &'a str,
    backend: B,
    state: SessionState,
    stalled: mpsc::UnboundedSender<u64>,
    /// The running turn was started by another agent's message.
    letter_turn: bool,
    /// Tells the chat what a fresh session under it says.
    sink: Sink,
    /// What a fresh session is told with its first message.
    rotation_brief: Option<ChatContext>,
    /// The session the chat ran on before a fresh one that has not answered yet.
    fresh_from: Option<String>,
}

impl<B: Backend> Running<'_, B> {
    /// Waits out a stop asked before this core took the chat over, and sends the prompt the
    /// agent was started again to carry on with.
    async fn opened(&mut self, replay: Option<Replay>) {
        if let Some(turn) = self.state.stopping.filter(|turn| *turn == self.state.turn) {
            if self.chat.prompted() {
                stall_later(turn, &self.stalled);
            }
        }
        if let Some(replay) = replay {
            if replay.announce {
                self.show(&replay);
            }
            self.send(replay).await;
        }
    }

    /// The session the chat runs on now, and its model and account.
    fn footing(&self) -> rotation::Footing {
        rotation::Footing {
            session: self.state.session_id.clone(),
            model: self
                .chat
                .relaunch(self.launch)
                .and_then(|launch| launch.model),
            account: self.chat.current_launch().account.map(|account| account.id),
        }
    }

    /// At rest after a turn, a session grown full writes its handoff note now, while its cache
    /// is warm; the fresh session starts with the next message. Once a fresh session has
    /// answered its first message, the chat goes on in it.
    async fn ready_rotation(&mut self) {
        if let Some(from) = self.fresh_from.take() {
            let start = self.state.start();
            if start.session_id != from {
                self.chat.mark_ready(start.clone());
                self.chat.emit(ChatEventKind::Ready, json!(start));
            }
        }
        let Some(core) = self.chat.core.upgrade() else {
            return;
        };
        if self.chat.state_now() != AgentState::Idle || !rotation::full_at_rest(&core, &self.chat) {
            return;
        }
        let note = self.backend.handoff().await.ok();
        rotation::readied(&self.chat, note);
    }

    /// Starts a fresh session under the chat as a message is about to start a turn, when the old
    /// one is full, cold or on another model or account, from a brief the core builds; the
    /// message carries the brief. The old one carries on when the fresh one does not start.
    async fn rotate_if_due(&mut self) {
        const STARTING: std::time::Duration = std::time::Duration::from_secs(60);
        let Some(core) = self.chat.core.upgrade() else {
            return;
        };
        let Some(reason) = rotation::due(&core, &self.chat, &self.footing()) else {
            return;
        };
        let brief = rotation::brief(&core, &self.chat);
        let Some(mut launch) = self.chat.relaunch(self.launch) else {
            return;
        };
        launch.resume_id = None;
        let started = match self.chat.with_model_token(&launch) {
            Ok(launch) => tokio::time::timeout(
                STARTING,
                B::start(&launch, Some(&brief.instructions), self.sink.clone()),
            )
            .await
            .unwrap_or_else(|_| Err("it took too long to start".to_owned())),
            Err(error) => Err(error),
        };
        let (backend, opened) = match started {
            Ok(started) => started,
            Err(error) => {
                eprintln!("sikemux core: the chat goes on in its old session, as a fresh one did not start: {error}");
                self.chat.rotation.failed();
                return;
            }
        };
        self.sink.loaded();
        let from = std::mem::replace(&mut self.state.session_id, opened.session_id);
        self.state.setup = opened.setup;
        self.state.capabilities = opened.capabilities;
        self.backend = backend;
        self.state.reloadable = self.backend.reloadable();
        self.chat.note_agent(self.backend.pid());
        self.chat.meter.started_fresh();
        self.chat.rotation.rotated(self.footing());
        // Until the fresh session has answered, a restart takes the old one up again.
        self.fresh_from = Some(from.clone());
        let at = sikemux_process::clock::epoch_ms();
        rotation::keep(&core, self.chat.agent_id(), at, &brief.whole);
        let rotation = SessionRotation {
            at,
            reason,
            from_session: from,
            to_session: self.state.session_id.clone(),
            brief: rotation::shown(&brief.whole),
        };
        self.sink.update(
            &self.state.session_id,
            json!({ "sessionUpdate": UpdateKind::SessionRotated, "rotation": rotation }),
        );
        self.rotation_brief = Some(brief.message);
    }

    /// Writes down where the chat stands before the loop waits for what comes next.
    fn rest(&self) {
        self.chat.keep(self.state.progress());
        if !self.chat.prompted() {
            self.tell_reach();
        }
        self.chat.at_rest.store(true, Ordering::Release);
    }

    fn tell_reach(&self) {
        if let Some(reach) = self.backend.reach() {
            self.chat.feed.reached(&self.state.session_id, reach);
        }
    }

    fn show(&self, replay: &Replay) {
        self.chat.feed.prompted(
            replay.message_id.as_deref(),
            &replay.text,
            &replay.paths,
            &replay.context,
            replay.person,
        );
    }

    /// Starts the next turn with `replay`, which is kept to go again should the agent be
    /// started again under the chat, or says why it could not start.
    async fn send(&mut self, mut replay: Replay) {
        self.state.turn += 1;
        self.chat.begin_turn();
        if !crate::server::chat::prompt::is_command(&replay.text) {
            replay.context.extend(self.rotation_brief.take());
            if let Some(core) = self.chat.core.upgrade() {
                replay
                    .context
                    .extend(core.chats.handoffs.take(self.chat.agent_id()));
            }
        }
        // A slash command takes what follows it as its arguments, so the brief waits for
        // the next message. Kept on the replay, it goes again with the message.
        let carries = !B::INSTRUCTIONS
            && replay.context.len() < MAX_CONTEXT_ITEMS
            && !crate::server::chat::prompt::is_command(&replay.text);
        if carries {
            replay
                .context
                .extend(self.chat.take_brief().map(brief_context));
        }
        if let Err(error) = self
            .backend
            .prompt(self.state.turn, Prompt::from(&replay))
            .await
        {
            self.chat.fail_turn(error_message(error));
        }
        self.state.last_prompt = Some(Replay {
            announce: false,
            ..replay
        });
    }

    async fn stopped(&self, broke: bool, reason: String) -> Outcome {
        stopped(&self.chat, self.launch, &self.state, broke, reason).await
    }

    /// A turn failed: the agent is started again signed in anew or on another account when
    /// that may fix it, else the chat says why.
    fn failed(&mut self, failed: Failed) -> ControlFlow<Outcome> {
        let chat = &self.chat;
        let state = &mut self.state;
        if failed.turn != state.turn {
            return ControlFlow::Continue(());
        }
        if let Some(switching) = state.switching.take() {
            match failed.response {
                Some(response) => chat.finish_turn(response),
                None => chat.cut_turn(),
            }
            return switching
                .carry_out(chat, self.provider, self.launch)
                .map_or(ControlFlow::Continue(()), ControlFlow::Break);
        }
        let current = self
            .launch
            .account
            .as_ref()
            .map(|account| account.id.clone());
        let retry = match failed.failure.kind {
            FailureKind::SignIn if state.reloadable && !state.signed_in_again => {
                chat.relaunch(self.launch).map(|launch| Rebind {
                    launch,
                    replay: state.last_prompt.clone(),
                    exhausted: state.exhausted.clone(),
                    signed_in_again: true,
                    notice: None,
                })
            }
            FailureKind::Limit if state.reloadable && switches_at_limit(chat) => {
                let mut tried = state.exhausted.clone();
                tried.extend(current.clone());
                next_account(chat, self.provider, self.launch, &tried).and_then(|next| {
                    let notice = switch_notice(self.launch, next, "limit");
                    let moved = on_account(self.provider, self.launch, next);
                    chat.relaunch(&moved).map(|launch| Rebind {
                        launch,
                        replay: state.last_prompt.clone(),
                        exhausted: tried,
                        signed_in_again: false,
                        notice: Some(notice),
                    })
                })
            }
            _ => None,
        };
        if let Some(rebind) = retry {
            return ControlFlow::Break(Outcome::Rebind(Box::new(rebind)));
        }
        state.failed_prompt = state.last_prompt.take();
        let fault = match (failed.failure.kind, &state.failed_prompt) {
            (FailureKind::Limit | FailureKind::Network, Some(_)) => Fault::Retrying,
            (FailureKind::SignIn | FailureKind::Limit, _) => Fault::Account,
            (FailureKind::Network | FailureKind::Other, _) => Fault::Broke,
        };
        let failure = failed.failure.payload(current.as_deref());
        chat.turn_failing(&failure, fault);
        if let Some(response) = failed.response {
            chat.finish_turn(response);
        }
        chat.fail_turn(failure);
        if let (FailureKind::SignIn, Some(core)) = (failed.failure.kind, chat.core.upgrade()) {
            agent_accounts::chat_refused(&core, self.provider, current.as_deref());
        }
        if failed.failure.kind == FailureKind::Network && state.failed_prompt.is_some() {
            chat.offline_failed.store(true, Ordering::Release);
        }
        if failed.failure.kind == FailureKind::Limit {
            crate::server::chat::handoff::offer_another_agent(chat, self.launch);
        }
        if let (FailureKind::Limit, Some(prompt)) =
            (failed.failure.kind, state.failed_prompt.clone())
        {
            reset::schedule(chat, self.launch, prompt, failed.failure.resets_at);
        }
        ControlFlow::Continue(())
    }

    /// Turn `settled` is over in the agent too, so a move to another account waiting on it
    /// goes ahead.
    fn settled(&mut self, settled: u64) -> ControlFlow<Outcome> {
        if settled != self.state.turn {
            return ControlFlow::Continue(());
        }
        self.state
            .switching
            .take()
            .and_then(|switching| switching.carry_out(&self.chat, self.provider, self.launch))
            .map_or(ControlFlow::Continue(()), ControlFlow::Break)
    }

    /// Turn `stalled` was asked to stop a while ago and is still at work.
    async fn stalled(&mut self, stalled: u64) -> ControlFlow<Outcome> {
        let chat = &self.chat;
        if !chat.prompted() || self.state.turn != stalled {
            return ControlFlow::Continue(());
        }
        if let Some(switching) = self.state.switching.take() {
            chat.finish_turn(turn_answer(StopReason::Cancelled));
            return switching
                .carry_out(chat, self.provider, self.launch)
                .map_or(ControlFlow::Continue(()), ControlFlow::Break);
        }
        let why = "It was still at work a while after it was asked to stop.";
        if self.state.reloadable {
            chat.finish_turn(turn_answer(StopReason::Cancelled));
        }
        let title = "This agent did not stop, so it was started again";
        if let Some(rebind) = restart(chat, self.launch, &self.state, false, title, why).await {
            return ControlFlow::Break(Outcome::Rebind(rebind));
        }
        let failure = error_message("The agent did not stop, so its session was ended");
        chat.turn_failing(&failure, Fault::Broke);
        chat.fail_for_good(failure);
        ControlFlow::Break(Outcome::Ended(SessionEnd::Exited))
    }

    async fn command(&mut self, command: ChatCommand) -> ControlFlow<Outcome> {
        match command {
            ChatCommand::Letter {
                text,
                paths,
                person,
                reply,
            } if !self.chat.prompted() => {
                let _ = reply.send(Delivery::NewTurn);
                return self
                    .prompt(None, text, paths, Vec::new(), false, Some(person))
                    .await;
            }
            ChatCommand::Letter {
                text,
                paths,
                person,
                reply,
            } => self.letter(text, paths, person, reply),
            ChatCommand::Prompt {
                message_id,
                text,
                paths,
                context,
                shown,
            } => {
                return self
                    .prompt(message_id, text, paths, context, shown, None)
                    .await
            }
            ChatCommand::Edit {
                message_id,
                text,
                paths,
                context,
                reply,
            } => self.edit(message_id, text, paths, context, reply).await,
            ChatCommand::SwitchAccount { account, moved } => {
                return self.switch_account(account, moved).await
            }
            ChatCommand::SetPermissionMode { mode, reply } => {
                self.set_permission_mode(mode, reply).await
            }
            ChatCommand::SetConfig {
                config_id,
                value,
                reply,
            } => self.set_config(config_id, value, reply).await,
            ChatCommand::Steer {
                text,
                paths,
                context,
                reply,
            } => self.steer(text, paths, context, reply),
            ChatCommand::StopTask { task_id } => {
                if let Err(error) = self.backend.stop_task(&task_id).await {
                    self.chat.error(error_message(error));
                }
            }
            ChatCommand::Resume { prompt } => {
                if !self.chat.start_prompted() {
                    self.again(prompt).await;
                }
            }
            ChatCommand::RetryFailed => self.retry_failed().await,
            ChatCommand::Authenticate { method_id, reply } => {
                let _ = reply.send(self.backend.authenticate(&method_id).await);
            }
            ChatCommand::DiscardFailed => {
                self.state.failed_prompt = None;
                reset::cancel(&self.chat);
            }
            ChatCommand::AnswerDialog { id, answer, choice } => {
                self.backend.answer_dialog(&id, answer, choice)
            }
            ChatCommand::Cancel => self.cancel().await,
            ChatCommand::TakeHandoff => {
                if !self.chat.prompted() {
                    match self.backend.handoff().await {
                        Ok(note) => self.chat.rotation.note_handoff(note),
                        Err(error) => eprintln!("sikemux core: no handoff note was taken: {error}"),
                    }
                }
            }
            ChatCommand::Compact => {
                if !self.chat.prompted() {
                    if let Err(error) = self.backend.compact().await {
                        eprintln!("sikemux core: an idle chat was not compacted: {error}");
                    }
                }
            }
            ChatCommand::HandOver { .. } => unreachable!("the loop hands the chat over itself"),
        }
        ControlFlow::Continue(())
    }

    fn letter(
        &mut self,
        text: String,
        paths: Vec<String>,
        person: bool,
        reply: tokio::sync::oneshot::Sender<Delivery>,
    ) {
        let prompt = Prompt {
            message_id: None,
            text,
            paths,
            context: Vec::new(),
            letter: true,
            resent: false,
        };
        let steering = self.backend.steer(prompt.clone());
        let chat = self.chat.clone();
        tokio::spawn(async move {
            let delivery = match steering.await {
                Ok(Steered::Injected) => {
                    chat.feed
                        .prompted(None, &prompt.text, &prompt.paths, &[], person);
                    Delivery::Steered
                }
                Ok(Steered::PromptRequired) => Delivery::TurnEnded,
                Err(_) => Delivery::Refused,
            };
            let _ = reply.send(delivery);
        });
    }

    /// Starts a turn with what was sent, or holds it for after the running turn. `letter` when
    /// a message starts it rather than the person's prompt, saying whether it carries the
    /// person's own words.
    async fn prompt(
        &mut self,
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        shown: bool,
        letter: Option<bool>,
    ) -> ControlFlow<Outcome> {
        let by_letter = letter.is_some();
        let chat = &self.chat;
        if chat.start_prompted() {
            // Another agent's message can start a turn just as the
            // person sends one, so theirs goes next rather than lost.
            if self.letter_turn || shown {
                let held = ChatCommand::Prompt {
                    message_id,
                    text,
                    paths,
                    context,
                    shown,
                };
                after_turn(chat.clone(), held);
            } else if !chat.feed.enqueue(super::super::queue::Queued::new(
                message_id, text, paths, context,
            )) {
                chat.error(error_message("too many messages are waiting for this turn"));
            }
            return ControlFlow::Continue(());
        }
        self.letter_turn = by_letter;
        self.state.fresh_turn(chat);
        let replay = Replay {
            announce: true,
            message_id,
            text,
            paths,
            context,
            person: letter.unwrap_or(true),
        };
        // A sign-in made elsewhere since the agent started is only
        // read by a new agent process.
        let changed = sign_in::signed_in(self.provider, &self.launch.env);
        if self.state.reloadable
            && changed.is_some()
            && self.state.signed_in_at_start.is_some()
            && changed != self.state.signed_in_at_start
        {
            if let Some(launch) = chat.relaunch(self.launch) {
                return ControlFlow::Break(Outcome::Rebind(Box::new(Rebind {
                    launch,
                    replay: Some(replay),
                    exhausted: Vec::new(),
                    signed_in_again: false,
                    notice: None,
                })));
            }
        }
        if !shown {
            self.show(&replay);
        }
        self.rotate_if_due().await;
        self.send(replay).await;
        ControlFlow::Continue(())
    }

    /// Takes the conversation back to before the person's message `message_id` and sends
    /// its new text in its place.
    async fn edit(
        &mut self,
        message_id: String,
        text: String,
        paths: Vec<String>,
        mut context: Vec<ChatContext>,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    ) {
        if self.chat.prompted() {
            let _ = reply.send(Err("Stop the current turn before editing a message".into()));
            return;
        }
        context.extend(self.chat.feed.for_agent_of(&message_id));
        let moved = match self.backend.rewind(&message_id).await {
            Ok(moved) => moved,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        self.chat.feed.rewound(&message_id);
        if let Some(moved) = moved {
            self.state.session_id = moved;
            let start = self.state.start();
            self.chat.mark_ready(start.clone());
            self.chat.emit(ChatEventKind::Ready, json!(start));
        }
        let _ = reply.send(Ok(()));
        self.state.fresh_turn(&self.chat);
        let replay = Replay {
            announce: false,
            message_id: Some(message_id),
            text,
            paths,
            context,
            person: true,
        };
        self.show(&replay);
        self.chat.start_prompted();
        self.letter_turn = false;
        self.send(replay).await;
    }

    /// Moves the chat to `account`, once the running turn has stopped. Asked for the account
    /// it is still on while a move waits for that turn, it stays: the latest choice wins.
    async fn switch_account(
        &mut self,
        account: ChatAccount,
        moved: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) -> ControlFlow<Outcome> {
        let say = |going: Result<(), &str>| {
            if let Some(moved) = moved {
                let _ = moved.send(going.map_err(str::to_owned));
            }
        };
        let chat = &self.chat;
        if !self.state.reloadable {
            let refused = "This agent cannot carry a chat to another account";
            chat.error(error_message(refused));
            say(Err(refused));
            return ControlFlow::Continue(());
        }
        if chat.prompted() {
            let moves = self
                .launch
                .account
                .as_ref()
                .is_none_or(|current| current.id != account.id);
            if moves && self.state.switching.is_none() {
                self.state.stopping = Some(self.state.turn);
                let sent = self.backend.cancel().await;
                chat.cancel_sent(sent, self.state.turn, &self.stalled);
            }
            if moves || self.state.switching.is_some() {
                let carry_on = self.state.switching.as_ref().is_none_or(|was| was.carry_on);
                chat.moving_on.store(carry_on, Ordering::Release);
                self.state.switching = Some(Switching { account, carry_on });
            }
            say(Ok(()));
            return ControlFlow::Continue(());
        }
        let replay = self
            .state
            .failed_prompt
            .take()
            .or_else(|| chat.unprompted().then(Switching::carry_on_prompt));
        match switch_to(chat, self.provider, self.launch, &account, replay) {
            Some(outcome) => {
                say(Ok(()));
                ControlFlow::Break(outcome)
            }
            None => {
                say(Err(
                    "This chat has not begun yet, so it has nothing to carry over",
                ));
                ControlFlow::Continue(())
            }
        }
    }

    async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    ) {
        let result = if self.chat.prompted() {
            Err("Stop the current turn before changing permissions".into())
        } else {
            self.backend.set_permission_mode(mode).await
        };
        if result.is_ok() {
            self.chat.set_permission_mode(mode);
        }
        let _ = reply.send(result);
    }

    async fn set_config(
        &mut self,
        config_id: String,
        value: String,
        reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
    ) {
        let result = if self.chat.prompted() && !(B::EFFORT_MID_TURN && self.is_effort(&config_id))
        {
            Err("Stop the current turn before changing the model".into())
        } else {
            self.backend.set_config(&config_id, &value).await
        };
        if let Ok(options) = &result {
            self.state.setup["configOptions"] = options.clone();
            self.chat.feed.set_setup(&self.state.setup);
        }
        let _ = reply.send(result.map(|options| json!({ "configOptions": options })));
    }

    fn is_effort(&self, config_id: &str) -> bool {
        (self.state.setup["configOptions"]
            .as_array()
            .into_iter()
            .flatten())
        .any(|row| row["id"] == config_id && sikemux_fold::pickers::is_effort(row))
    }

    fn steer(
        &mut self,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        reply: tokio::sync::oneshot::Sender<Result<Steered, String>>,
    ) {
        if !self.chat.prompted() {
            let _ = reply.send(Ok(Steered::PromptRequired));
            return;
        }
        let prompt = Prompt {
            message_id: None,
            text,
            paths,
            context,
            letter: false,
            resent: false,
        };
        let steering = self.backend.steer(prompt.clone());
        let chat = self.chat.clone();
        tokio::spawn(async move {
            let result = steering.await;
            if result == Ok(Steered::Injected) {
                chat.feed
                    .prompted(None, &prompt.text, &prompt.paths, &prompt.context, true);
            }
            let _ = reply.send(result);
        });
    }

    /// Sends a prompt that ran before as a new turn.
    async fn again(&mut self, prompt: Replay) {
        self.letter_turn = false;
        self.state.fresh_turn(&self.chat);
        self.send(prompt).await;
    }

    async fn retry_failed(&mut self) {
        let Some(prompt) = self.state.failed_prompt.take() else {
            return;
        };
        if self.chat.start_prompted() {
            self.state.failed_prompt = Some(prompt);
            return;
        }
        self.again(prompt).await;
    }

    async fn cancel(&mut self) {
        if let Some(switching) = self.state.switching.as_mut() {
            switching.carry_on = false;
        }
        self.chat.moving_on.store(false, Ordering::Release);
        if self.chat.prompted() {
            self.state.stopping = Some(self.state.turn);
        }
        let sent = self.backend.cancel().await;
        self.chat.cancel_sent(sent, self.state.turn, &self.stalled);
    }

    /// Lets go of the agent for a core that takes the chat over.
    async fn hand_over(
        self,
        reply: tokio::sync::oneshot::Sender<Result<Handed, String>>,
    ) -> Outcome {
        let released = self.backend.release().await;
        let handed = released.map(|(backend, released)| Handed {
            session: self.state,
            backend,
            released,
        });
        // Nobody waits for it any more, so the agent is stopped
        // rather than left running unread.
        if let Err(Ok(handed)) = reply.send(handed) {
            super::process::kill_and_reap(handed.released.pid);
        }
        Outcome::Released
    }
}

impl Switching {
    fn carry_on_prompt() -> Replay {
        super::super::carry_on_prompt("Your chat moved to another account")
    }

    /// Moves the chat to the account, now that its turn has stopped.
    fn carry_out(self, chat: &Arc<Chat>, provider: &str, launch: &ChatLaunch) -> Option<Outcome> {
        chat.moving_on.store(false, Ordering::Release);
        let replay = self.carry_on.then(Self::carry_on_prompt);
        switch_to(chat, provider, launch, &self.account, replay)
    }
}

/// Starts the agent again on `account`, which then answers `replay`.
fn switch_to(
    chat: &Arc<Chat>,
    provider: &str,
    launch: &ChatLaunch,
    account: &ChatAccount,
    replay: Option<Replay>,
) -> Option<Outcome> {
    let moves = launch
        .account
        .as_ref()
        .is_none_or(|current| current.id != account.id);
    let notice = moves.then(|| switch_notice(launch, account, "chosen"));
    reset::cancel(chat);
    let moved = on_account(provider, launch, account);
    let launch = chat.relaunch(&moved)?;
    if replay.is_some() {
        chat.start_prompted();
    }
    Some(Outcome::Rebind(Box::new(Rebind {
        launch,
        replay,
        exhausted: Vec::new(),
        signed_in_again: false,
        notice,
    })))
}

impl SessionState {
    fn progress(&self) -> Progress<'_> {
        Progress {
            prompt: self.last_prompt.as_ref(),
            failed: self.failed_prompt.as_ref(),
            reloadable: self.reloadable,
        }
    }

    /// Forgets how the last turn failed, as a new one starts in its place.
    fn fresh_turn(&mut self, chat: &Chat) {
        self.exhausted.clear();
        self.signed_in_again = false;
        self.failed_prompt = None;
        chat.offline_failed.store(false, Ordering::Release);
        reset::cancel(chat);
    }

    fn start(&self) -> ChatStart {
        ChatStart {
            session_id: self.session_id.clone(),
            capabilities: self.capabilities.clone(),
            setup: self.setup.clone(),
        }
    }
}

/// A chat whose agent this core could not take over: it goes on in a new
/// agent process on the same session, and says so.
fn not_taken_over(chat: &Chat, launch: &ChatLaunch, error: &str) -> Outcome {
    if chat.prompted() {
        chat.finish_turn(turn_answer(StopReason::Cancelled));
    }
    chat.cancel_permissions();
    let notice = super::super::update_notice(error);
    match chat.relaunch(launch) {
        Some(launch) => Outcome::Rebind(Box::new(Rebind {
            launch,
            replay: None,
            exhausted: Vec::new(),
            signed_in_again: false,
            notice: Some(notice),
        })),
        None => {
            chat.error(error_message(error));
            Outcome::Ended(SessionEnd::Exited)
        }
    }
}

/// The agent stopped by itself, or its session `broke`: starts it again on
/// the chat's session, or ends the chat saying why.
async fn stopped(
    chat: &Arc<Chat>,
    launch: &ChatLaunch,
    state: &SessionState,
    broke: bool,
    reason: String,
) -> Outcome {
    let working = !broke && chat.working();
    let title = match (broke, working) {
        (true, _) => "This agent's session broke, and it was started again",
        (false, true) => "This agent stopped while it was working, and was started again",
        (false, false) => "This agent stopped, and was started again",
    };
    if let Some(rebind) = restart(chat, launch, state, working, title, &reason).await {
        return Outcome::Rebind(rebind);
    }
    if broke || chat.prompted() {
        let failure = error_message(&reason);
        chat.turn_failing(&failure, Fault::Broke);
        chat.fail_for_good(failure);
    }
    Outcome::Ended(SessionEnd::Exited)
}

/// Starts the agent again on the chat's session after it stopped by itself,
/// saying so in the chat, and carrying on the turn it was cut off in when
/// `carry_on`. None when it cannot load its session, or has stopped too often
/// lately.
async fn restart(
    chat: &Arc<Chat>,
    launch: &ChatLaunch,
    state: &SessionState,
    carry_on: bool,
    title: &str,
    why: &str,
) -> Option<Box<Rebind>> {
    if !state.reloadable {
        return None;
    }
    let launch = chat.relaunch(launch)?;
    let wait = chat.restart_after()?;
    chat.cancel_permissions();
    chat.cut_turn();
    tokio::time::sleep(wait).await;
    Some(Box::new(Rebind {
        launch,
        replay: carry_on.then(|| {
            super::super::carry_on_prompt(
                "Your process stopped while you were working and Sikemux started you again",
            )
        }),
        exhausted: Vec::new(),
        signed_in_again: false,
        notice: Some(super::super::restart_notice(title, why)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_texts_are_cut_and_pictures_kept_whole() {
        let long = "é".repeat(MAX_TEXT);
        let picture = "A".repeat(MAX_TEXT * 2);
        let mut update = json!({
            "rawOutput": long,
            "content": [{ "type": "image", "data": picture, "mimeType": "image/png" }],
            "huge": [{ "type": "image", "source": { "type": "base64", "data": "A".repeat(MAX_IMAGE + 1) } }],
        });
        slim(&mut update);
        let cut = update["rawOutput"].as_str().unwrap();
        assert!(cut.len() <= MAX_TEXT + '…'.len_utf8());
        assert!(cut.ends_with('…'));
        assert_eq!(
            update["content"][0]["data"].as_str().map(str::len),
            Some(MAX_TEXT * 2)
        );
        assert!(update["huge"][0]["source"].get("data").is_none());
    }
}
