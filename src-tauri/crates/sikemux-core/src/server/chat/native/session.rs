//! The part of a chat that is the same for every agent the core speaks to in
//! its own protocol: turns, permission requests, accounts and starting the
//! agent again under the chat.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::launch::PermissionMode;

use crate::acp::account::{self, Failure, FailureKind, SignIn};
use crate::acp::{permission, SessionEnd};
use crate::protocol::{ChatAccount, ChatContext, ChatLaunch, ChatStart};
use sikemux_fold::ChatEventKind;

use super::super::kept::Progress;
use super::super::rebind::{
    error_message, next_account, on_account, switch_notice, switches_at_limit, Carried, Outcome,
    Rebind, Replay,
};
use super::super::{reset, stall_later, Asked, Chat, ChatCommand, Delivery, Respond};
use super::process::{AgentRecord, Lines, Process, Released, Writer};

/// What the person sent, for the agent to read in its own form.
#[derive(Clone)]
pub(crate) struct Prompt {
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
    /// A message from another agent rather than the person.
    pub letter: bool,
}

impl From<&Replay> for Prompt {
    fn from(replay: &Replay) -> Self {
        Self {
            message_id: replay.message_id.clone(),
            text: replay.text.clone(),
            paths: replay.paths.clone(),
            context: replay.context.clone(),
            letter: false,
        }
    }
}

/// The most of any one text an update carries. The chat shows only the first
/// few hundred lines of what a tool printed, and a long chat loaded again
/// would otherwise send all of it.
const MAX_TEXT: usize = 128 * 1024;
/// The largest picture an update carries, as base64. The chat keeps none
/// bigger.
const MAX_IMAGE: usize = 2 * 1024 * 1024;

/// Cuts every text in `value` down to what the chat can show.
fn slim(value: &mut Value) {
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

/// How a turn this side started ended.
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

/// What woke the chat's loop.
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

/// What an agent's reader tells the chat. Cheap to clone, and safe to use
/// from any task.
#[derive(Clone)]
pub(crate) struct Sink {
    chat: Arc<Chat>,
    failed: mpsc::UnboundedSender<Failed>,
    broken: mpsc::UnboundedSender<String>,
    settled: mpsc::UnboundedSender<u64>,
    account: Option<String>,
    /// Quiet while a session loaded under a running chat replays history the
    /// chat already shows.
    quiet: Arc<std::sync::atomic::AtomicBool>,
}

impl Sink {
    /// Tells only `chat` what it is given, for a resumed chat's saved history shown before
    /// its agent starts.
    pub(crate) fn showing(chat: Arc<Chat>) -> Self {
        Self {
            chat,
            failed: mpsc::unbounded_channel().0,
            broken: mpsc::unbounded_channel().0,
            settled: mpsc::unbounded_channel().0,
            account: None,
            quiet: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn update(&self, session_id: &str, mut update: Value) {
        if self.quiet.load(Ordering::Acquire) {
            return;
        }
        self.chat.note_edits(&update);
        slim(&mut update);
        self.chat.emit(
            ChatEventKind::SessionUpdate,
            json!({ "sessionId": session_id, "update": update }),
        );
    }

    /// Says in the chat that something the person asked for went wrong.
    pub fn error(&self, message: impl std::fmt::Display) {
        self.chat.emit(ChatEventKind::Error, error_message(message));
    }

    /// Whether this start loads the session again under a running chat.
    pub fn reloading(&self) -> bool {
        self.quiet.load(Ordering::Acquire)
    }

    /// The session has loaded, so what the agent says from now on is new to
    /// the chat.
    pub fn loaded(&self) {
        self.quiet.store(false, Ordering::Release);
    }

    /// Whether the provider has a saved session to load.
    pub fn has_answered(&self) -> bool {
        self.chat.has_answered()
    }

    /// Whether a turn this side prompted is being answered.
    pub fn running(&self) -> bool {
        self.chat.running.load(Ordering::Acquire)
    }

    /// The agent's CLI said which version it is.
    pub fn cli_version(&self, version: &str) {
        self.chat.note_cli_version(version);
    }

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    pub fn work_started(&self) {
        self.chat.work_started();
    }

    /// The work nobody here asked for is over.
    pub fn work_ended(&self, stop_reason: &str) {
        self.chat.work_ended(stop_reason);
    }

    /// Settles turn `turn`, or hands a failure another account may fix to
    /// the chat's loop to decide on, with the turn still running.
    pub fn end_turn(&self, turn: u64, end: TurnEnd) {
        let chat = &self.chat;
        chat.unprompted.store(false, Ordering::Release);
        chat.cancel_permissions();
        let settle = || {
            chat.stop_running();
            let _ = self.settled.send(turn);
        };
        match end {
            TurnEnd::Completed(payload) => {
                settle();
                chat.answered.store(true, Ordering::Release);
                chat.emit(ChatEventKind::TurnCompleted, payload);
            }
            TurnEnd::Failed { failure, response } if failure.kind != FailureKind::Other => {
                let _ = self.failed.send(Failed {
                    turn,
                    failure,
                    response,
                });
            }
            TurnEnd::Failed { failure, response } => {
                settle();
                let failure = failure.payload(self.account.as_deref());
                chat.turn_failing(&failure);
                chat.emit(
                    ChatEventKind::TurnCompleted,
                    response.unwrap_or_else(|| json!({ "stopReason": "end_turn" })),
                );
                chat.emit(ChatEventKind::Error, failure);
            }
            TurnEnd::Refused(message) => {
                settle();
                chat.emit(ChatEventKind::Error, error_message(message));
            }
            TurnEnd::Broken(reason) => {
                settle();
                let _ = self.broken.send(reason);
            }
        }
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

    /// The person's message `text` went into the running turn.
    pub fn prompted(&self, text: &str, paths: &[String]) {
        self.chat.feed.prompted(None, text, paths, &[]);
    }

    /// Asks the person about a tool call. `request` is shaped as an ACP
    /// permission request: the session, the tool call and its `options`.
    /// `reply` is what the backend needs to answer the agent, kept with the
    /// question so a core that takes the chat over can answer it too.
    /// `respond` is told the chosen option's id, or none when it was
    /// cancelled.
    pub fn ask(&self, mut request: Value, reply: Value, respond: Respond) {
        if self.chat.approves() {
            if let Some(option_id) = permission::approval(&request) {
                respond(Some(option_id));
                return;
            }
        }
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
            .hold_permission(request_id, option_ids, respond, request.clone(), reply)
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

/// One agent the core speaks to in its own protocol.
pub(crate) trait Backend: Sized + Send {
    /// Starts the agent and opens the chat's session, or loads the one it
    /// resumes, replaying its history through `sink`.
    fn start(
        launch: &ChatLaunch,
        sink: Sink,
    ) -> impl std::future::Future<Output = Result<(Self, Opened), String>> + Send;

    /// Sends a prompt that begins turn `turn`. How it ends is told to the
    /// sink.
    fn prompt(
        &mut self,
        turn: u64,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Puts a message into the running turn. `injected` once it is in,
    /// `promptRequired` when the turn ended first. The answer is waited on
    /// apart from the chat's other commands, so a stop sent right after a
    /// steer is never held up behind it.
    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send + 'static;

    /// Asks the running turn to stop.
    fn cancel(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Applies one of the app's permission modes.
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
    fn responder(&self, reply: Value) -> Respond;
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
    let sink = |quiet: bool| Sink {
        chat: chat.clone(),
        failed: failed_tx.clone(),
        broken: broken_tx.clone(),
        settled: settled_tx.clone(),
        account: launch.account.as_ref().map(|account| account.id.clone()),
        quiet: Arc::new(std::sync::atomic::AtomicBool::new(quiet)),
    };
    let (mut backend, mut state, replay) = match opening {
        Begin::Start { rebind, notice } => {
            chat.emit(ChatEventKind::Status, json!({ "state": "starting" }));
            let signed_in_at_start = account::signed_in(&provider, &launch.env);
            let Carried {
                replay,
                exhausted,
                signed_in_again,
                notice: carried,
                quiet_load,
            } = Carried::from(rebind);
            let sink = sink(quiet_load || chat.saved_shown.swap(false, Ordering::AcqRel));
            chat.emit(ChatEventKind::Status, json!({ "state": "initializing" }));
            chat.feed.hold();
            let started = B::start(&launch, sink.clone()).await;
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
                chat.running.store(true, Ordering::Release);
            }
            chat.note_agent(backend.pid());
            if let Some(core) = chat.core.upgrade() {
                super::super::tryout_began(&core, &provider, &state.session_id);
            }
            let start = state.start();
            crate::server::chat::offered::remember(&provider, &start.setup);
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
                        let respond = backend.responder(asked.reply.clone());
                        chat.hold_again(asked, respond);
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
    if let Some(turn) = state.stopping.filter(|turn| *turn == state.turn) {
        if chat.running.load(Ordering::Acquire) {
            stall_later(turn, &stalled_tx);
        }
    }
    if let Some(replay) = replay {
        if replay.announce {
            chat.feed.prompted(
                replay.message_id.as_deref(),
                &replay.text,
                &replay.paths,
                &replay.context,
            );
        }
        state.turn += 1;
        begin(&chat, &mut backend, state.turn, Prompt::from(&replay)).await;
        state.last_prompt = Some(Replay {
            announce: false,
            ..replay
        });
    }

    let mut letter_turn = false;
    let end = loop {
        chat.keep(state.progress());
        chat.at_rest.store(true, Ordering::Release);
        // What the agent already said is taken in before the next command,
        // so a turn's failure or end is never cut off by letting go of the
        // agent.
        let woke = tokio::select! {
            biased;
            Some(reason) = broken_rx.recv() => Woke::Broken(reason),
            Some(failed) = failed_rx.recv() => Woke::Failed(failed),
            Some(stalled) = stalled_rx.recv() => Woke::Stalled(stalled),
            Some(settled) = settled_rx.recv() => Woke::Settled(settled),
            reason = backend.closed() => Woke::Closed(reason),
            command = commands.recv() => Woke::Command(command),
        };
        chat.at_rest.store(false, Ordering::Release);
        let command = match woke {
            Woke::Command(Some(command)) => command,
            Woke::Command(None) => break Outcome::Ended(SessionEnd::Requested),
            Woke::Closed(reason) => break stopped(&chat, &launch, &state, false, reason).await,
            Woke::Broken(reason) => break stopped(&chat, &launch, &state, true, reason).await,
            Woke::Failed(failed) => {
                if failed.turn != state.turn {
                    continue;
                }
                if let Some(switching) = state.switching.take() {
                    chat.stop_running();
                    if let Some(response) = failed.response {
                        chat.emit(ChatEventKind::TurnCompleted, response);
                    }
                    if let Some(outcome) = switching.carry_out(&chat, &provider, &launch) {
                        break outcome;
                    }
                    continue;
                }
                let current = launch.account.as_ref().map(|account| account.id.clone());
                let retry = match failed.failure.kind {
                    FailureKind::SignIn if state.reloadable && !state.signed_in_again => {
                        chat.relaunch(&launch).map(|launch| Rebind {
                            launch,
                            replay: state.last_prompt.clone(),
                            exhausted: state.exhausted.clone(),
                            signed_in_again: true,
                            notice: None,
                        })
                    }
                    FailureKind::Limit if state.reloadable && switches_at_limit(&chat) => {
                        let mut tried = state.exhausted.clone();
                        tried.extend(current.clone());
                        next_account(&provider, &launch, &tried).and_then(|next| {
                            let notice = switch_notice(&launch, next, "limit");
                            let moved = on_account(&provider, &launch, next);
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
                    break Outcome::Rebind(Box::new(rebind));
                }
                chat.stop_running();
                let failure = failed.failure.payload(current.as_deref());
                chat.turn_failing(&failure);
                if let Some(response) = failed.response {
                    chat.emit(ChatEventKind::TurnCompleted, response);
                }
                chat.emit(ChatEventKind::Error, failure);
                state.failed_prompt = state.last_prompt.take();
                if failed.failure.kind == FailureKind::Network && state.failed_prompt.is_some() {
                    chat.offline_failed.store(true, Ordering::Release);
                }
                if let (FailureKind::Limit, Some(prompt)) =
                    (failed.failure.kind, state.failed_prompt.clone())
                {
                    reset::schedule(&chat, &launch, prompt, failed.failure.resets_at);
                }
                continue;
            }
            Woke::Settled(settled) => {
                if settled != state.turn {
                    continue;
                }
                if let Some(switching) = state.switching.take() {
                    if let Some(outcome) = switching.carry_out(&chat, &provider, &launch) {
                        break outcome;
                    }
                }
                continue;
            }
            Woke::Stalled(stalled) => {
                if chat.running.load(Ordering::Acquire) && state.turn == stalled {
                    if let Some(switching) = state.switching.take() {
                        chat.stop_running();
                        chat.emit(
                            ChatEventKind::TurnCompleted,
                            json!({ "stopReason": "cancelled" }),
                        );
                        if let Some(outcome) = switching.carry_out(&chat, &provider, &launch) {
                            break outcome;
                        }
                        continue;
                    }
                    let why = "It was still at work a while after it was asked to stop.";
                    if state.reloadable {
                        chat.stop_running();
                        chat.emit(
                            ChatEventKind::TurnCompleted,
                            json!({ "stopReason": "cancelled" }),
                        );
                    }
                    let title = "This agent did not stop, so it was started again";
                    if let Some(rebind) = restart(&chat, &launch, &state, false, title, why).await {
                        break Outcome::Rebind(rebind);
                    }
                    let failure = error_message("The agent did not stop, so its session was ended");
                    chat.turn_failing(&failure);
                    chat.emit(ChatEventKind::Error, failure);
                    break Outcome::Ended(SessionEnd::Exited);
                }
                continue;
            }
        };
        let mut by_letter = false;
        let command = match command {
            ChatCommand::Letter { text, reply } if !chat.running.load(Ordering::Acquire) => {
                let _ = reply.send(Delivery::NewTurn);
                by_letter = true;
                ChatCommand::Prompt {
                    message_id: None,
                    text,
                    paths: Vec::new(),
                    context: Vec::new(),
                }
            }
            command => command,
        };
        match command {
            ChatCommand::Letter { text, reply } => {
                let prompt = Prompt {
                    message_id: None,
                    text,
                    paths: Vec::new(),
                    context: Vec::new(),
                    letter: true,
                };
                let steering = backend.steer(prompt.clone());
                let chat = chat.clone();
                tokio::spawn(async move {
                    let delivery = match steering.await.as_deref() {
                        Ok("injected") => {
                            chat.feed.prompted(None, &prompt.text, &[], &[]);
                            Delivery::Steered
                        }
                        Ok("promptRequired") => Delivery::TurnEnded,
                        _ => Delivery::Refused,
                    };
                    let _ = reply.send(delivery);
                });
            }
            ChatCommand::Prompt {
                message_id,
                text,
                paths,
                context,
            } => {
                if chat.running.swap(true, Ordering::AcqRel) {
                    // Another agent's message can start a turn just as the
                    // person sends one, so theirs goes next rather than lost.
                    if letter_turn {
                        let held = ChatCommand::Prompt {
                            message_id,
                            text,
                            paths,
                            context,
                        };
                        after_turn(chat.clone(), held);
                    } else if !chat.feed.enqueue(super::super::queue::Queued::new(
                        message_id, text, paths, context,
                    )) {
                        chat.emit(
                            ChatEventKind::Error,
                            error_message("too many messages are waiting for this turn"),
                        );
                    }
                    continue;
                }
                letter_turn = by_letter;
                state.fresh_turn(&chat);
                let replay = Replay {
                    announce: true,
                    message_id,
                    text,
                    paths,
                    context,
                };
                // A sign-in made elsewhere since the agent started is only
                // read by a new agent process.
                let changed = account::signed_in(&provider, &launch.env);
                if state.reloadable
                    && changed.is_some()
                    && state.signed_in_at_start.is_some()
                    && changed != state.signed_in_at_start
                {
                    if let Some(launch) = chat.relaunch(&launch) {
                        break Outcome::Rebind(Box::new(Rebind {
                            launch,
                            replay: Some(replay),
                            exhausted: Vec::new(),
                            signed_in_again: false,
                            notice: None,
                        }));
                    }
                }
                chat.feed.prompted(
                    replay.message_id.as_deref(),
                    &replay.text,
                    &replay.paths,
                    &replay.context,
                );
                state.turn += 1;
                begin(&chat, &mut backend, state.turn, Prompt::from(&replay)).await;
                state.last_prompt = Some(Replay {
                    announce: false,
                    ..replay
                });
            }
            ChatCommand::Edit {
                message_id,
                text,
                paths,
                context,
                reply,
            } => {
                if chat.running.load(Ordering::Acquire) {
                    let _ =
                        reply.send(Err("Stop the current turn before editing a message".into()));
                    continue;
                }
                let moved = match backend.rewind(&message_id).await {
                    Ok(moved) => moved,
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        continue;
                    }
                };
                chat.feed.rewound(&message_id);
                if let Some(moved) = moved {
                    state.session_id = moved;
                    let start = state.start();
                    chat.mark_ready(start.clone());
                    chat.emit(ChatEventKind::Ready, json!(start));
                }
                let _ = reply.send(Ok(()));
                state.fresh_turn(&chat);
                let replay = Replay {
                    announce: false,
                    message_id: Some(message_id),
                    text,
                    paths,
                    context,
                };
                chat.feed.prompted(
                    replay.message_id.as_deref(),
                    &replay.text,
                    &replay.paths,
                    &replay.context,
                );
                chat.running.store(true, Ordering::Release);
                letter_turn = false;
                state.turn += 1;
                begin(&chat, &mut backend, state.turn, Prompt::from(&replay)).await;
                state.last_prompt = Some(replay);
            }
            ChatCommand::SwitchAccount { account } => {
                if !state.reloadable {
                    chat.emit(
                        ChatEventKind::Error,
                        error_message("This agent cannot carry a chat to another account"),
                    );
                    continue;
                }
                if chat.running.load(Ordering::Acquire) {
                    let moves = launch
                        .account
                        .as_ref()
                        .is_none_or(|current| current.id != account.id);
                    if moves && state.switching.is_none() {
                        state.stopping = Some(state.turn);
                        let sent = backend.cancel().await;
                        chat.cancel_sent(sent, state.turn, &stalled_tx);
                    }
                    if moves {
                        let carry_on = state.switching.as_ref().is_none_or(|was| was.carry_on);
                        state.switching = Some(Switching { account, carry_on });
                    }
                    continue;
                }
                let replay = state.failed_prompt.take().or_else(|| {
                    chat.unprompted
                        .load(Ordering::Acquire)
                        .then(Switching::carry_on_prompt)
                });
                if let Some(outcome) = switch_to(&chat, &provider, &launch, &account, replay) {
                    break outcome;
                }
            }
            ChatCommand::SetPermissionMode { mode, reply } => {
                let result = if chat.running.load(Ordering::Acquire) {
                    Err("Stop the current turn before changing permissions".into())
                } else {
                    backend.set_permission_mode(mode).await
                };
                if result.is_ok() {
                    chat.set_permission_mode(mode);
                }
                let _ = reply.send(result);
            }
            ChatCommand::SetConfig {
                config_id,
                value,
                reply,
            } => {
                let result = if chat.running.load(Ordering::Acquire) {
                    Err("Stop the current turn before changing the model".into())
                } else {
                    backend.set_config(&config_id, &value).await
                };
                if let Ok(options) = &result {
                    state.setup["configOptions"] = options.clone();
                    chat.feed.set_setup(&state.setup);
                }
                let _ = reply.send(result.map(|options| json!({ "configOptions": options })));
            }
            ChatCommand::Steer {
                text,
                paths,
                context,
                reply,
            } => {
                if !chat.running.load(Ordering::Acquire) {
                    let _ = reply.send(Ok("promptRequired".to_owned()));
                    continue;
                }
                let prompt = Prompt {
                    message_id: None,
                    text,
                    paths,
                    context,
                    letter: false,
                };
                let steering = backend.steer(prompt.clone());
                let chat = chat.clone();
                tokio::spawn(async move {
                    let result = steering.await;
                    if result.as_deref() == Ok("injected") {
                        chat.feed
                            .prompted(None, &prompt.text, &prompt.paths, &prompt.context);
                    }
                    let _ = reply.send(result);
                });
            }
            ChatCommand::StopTask { task_id } => {
                if let Err(error) = backend.stop_task(&task_id).await {
                    chat.emit(ChatEventKind::Error, error_message(error));
                }
            }
            ChatCommand::Resume { prompt } => {
                if chat.running.swap(true, Ordering::AcqRel) {
                    continue;
                }
                letter_turn = false;
                state.fresh_turn(&chat);
                state.turn += 1;
                begin(&chat, &mut backend, state.turn, Prompt::from(&prompt)).await;
                state.last_prompt = Some(prompt);
            }
            ChatCommand::RetryFailed => {
                let Some(prompt) = state.failed_prompt.take() else {
                    continue;
                };
                if chat.running.swap(true, Ordering::AcqRel) {
                    state.failed_prompt = Some(prompt);
                    continue;
                }
                letter_turn = false;
                state.fresh_turn(&chat);
                state.turn += 1;
                begin(&chat, &mut backend, state.turn, Prompt::from(&prompt)).await;
                state.last_prompt = Some(prompt);
            }
            ChatCommand::Authenticate { method_id, reply } => {
                let _ = reply.send(backend.authenticate(&method_id).await);
            }
            ChatCommand::DiscardFailed => {
                state.failed_prompt = None;
                reset::cancel(&chat);
            }
            ChatCommand::Cancel => {
                if let Some(switching) = state.switching.as_mut() {
                    switching.carry_on = false;
                }
                if chat.running.load(Ordering::Acquire) {
                    state.stopping = Some(state.turn);
                }
                let sent = backend.cancel().await;
                chat.cancel_sent(sent, state.turn, &stalled_tx);
            }
            ChatCommand::HandOver { reply } => {
                let released = backend.release().await;
                let handed = released.map(|(backend, released)| Handed {
                    session: state,
                    backend,
                    released,
                });
                // Nobody waits for it any more, so the agent is stopped
                // rather than left running unread.
                if let Err(Ok(handed)) = reply.send(handed) {
                    super::process::kill_and_reap(handed.released.pid);
                }
                break Outcome::Released;
            }
        }
    };
    Ok(end)
}

impl Switching {
    fn carry_on_prompt() -> Replay {
        Replay {
            announce: false,
            message_id: None,
            text: "Your last turn was interrupted. Carry on where you left off.".into(),
            paths: Vec::new(),
            context: Vec::new(),
        }
    }

    /// Moves the chat to the account, now that its turn has stopped.
    fn carry_out(self, chat: &Arc<Chat>, provider: &str, launch: &ChatLaunch) -> Option<Outcome> {
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
        chat.running.store(true, Ordering::Release);
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
    if chat.stop_running() {
        chat.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "cancelled" }),
        );
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
            chat.emit(ChatEventKind::Error, error_message(error));
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
    let working = !broke && (chat.turn_running() || chat.held.load(Ordering::Acquire));
    let title = match (broke, working) {
        (true, _) => "This agent's session broke, and it was started again",
        (false, true) => "This agent stopped while it was working, and was started again",
        (false, false) => "This agent stopped, and was started again",
    };
    if let Some(rebind) = restart(chat, launch, state, working, title, &reason).await {
        return Outcome::Rebind(rebind);
    }
    if broke || chat.running.load(Ordering::Acquire) {
        let failure = error_message(&reason);
        chat.turn_failing(&failure);
        chat.emit(ChatEventKind::Error, failure);
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
    chat.stop_running();
    chat.cancel_permissions();
    chat.end_cut_turn();
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

/// Starts turn `turn`, or says why it could not start.
async fn begin<B: Backend>(chat: &Arc<Chat>, backend: &mut B, turn: u64, prompt: Prompt) {
    chat.unprompted.store(false, Ordering::Release);
    chat.emit(ChatEventKind::TurnStarted, json!({}));
    if let Err(error) = backend.prompt(turn, prompt).await {
        chat.stop_running();
        chat.emit(ChatEventKind::Error, error_message(error));
    }
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
