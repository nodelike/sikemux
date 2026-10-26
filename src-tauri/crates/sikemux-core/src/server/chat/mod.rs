//! Chat agents: processes the core starts and speaks the Agent Client
//! Protocol with, so a turn keeps going while the window is closed and a
//! permission request waits for the person to come back.

pub(crate) mod feed;
pub(crate) mod find;
mod history;
mod kept;
pub(crate) mod mentions;
mod native;
pub(crate) mod offered;
pub(crate) mod queue;
mod rebind;
pub(crate) mod reset;
mod states;
mod usage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::sync::{mpsc, oneshot, watch};

use crate::acp::{bounded_text, current_choice, native as acp_native, permission, SessionEnd};
use crate::launch::providers::ROUTED_ENV;
use crate::launch::PermissionMode;
use crate::protocol::{
    Attention, AttentionKind, ChatAccount, ChatAttachment, ChatContext, ChatInfo, ChatLaunch,
    ChatMark, ChatStart, ChatState, Event, RequestId, Response,
};

use super::activity::Activity;
use super::connection::{ClientConn, ClientId};
use super::spawn::Ending;
use super::{Core, CoreError, CoreResult};
use feed::{Feed, FeedState, Standing};
use kept::{Keeper, KeptAgent, KeptChat};
use native::Begin;
pub(crate) use sikemux_fold::ChatEventKind;
use sikemux_process::clock::epoch_ms;

const MAX_AGENT_ID: usize = 200;
const START_TIMEOUT: Duration = Duration::from_secs(150);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const NOT_RUNNING: &str = "ACP session is not running";
const STOPPED: &str = "ACP session stopped";

/// How long a stop waits for an agent's process to be gone.
const STOP_SETTLE: Duration = Duration::from_secs(2);
/// How long a stopped turn may keep running before the agent is killed. The
/// agent only reads a stop between steps, and a wedged tool never gets there.
const CANCEL_GRACE: Duration = Duration::from_secs(10);
/// How long after its last subagent ends a chat waits for the agent to take
/// up the subagent's report before it counts as finished.
const SUBAGENT_WAKE_GRACE: Duration = Duration::from_secs(3);
/// How long a chat has to let go of its agent for an update before it is
/// stopped and started again on its session instead.
const RELEASE_WAIT: Duration = Duration::from_secs(5);
/// An agent that stops by itself is started again, after a wait that doubles
/// each time, until it has stopped this often within the window.
const RESTART_WAIT: Duration = Duration::from_secs(1);
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);

pub(crate) enum ChatCommand {
    Prompt {
        /// Names the message for a later edit of it.
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
    SetPermissionMode {
        mode: PermissionMode,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetConfig {
        config_id: String,
        value: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Steer {
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// A message from another agent: steered into the running turn, or the
    /// start of a new one when none runs.
    Letter {
        text: String,
        reply: oneshot::Sender<Delivery>,
    },
    StopTask {
        task_id: String,
    },
    SwitchAccount {
        account: ChatAccount,
    },
    /// Takes the conversation back to before the person's message
    /// `message_id` and sends `text` in its place.
    Edit {
        message_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Cancel,
    /// The prompt a usage limit stopped, sent again once the limit reset.
    Resume {
        prompt: rebind::Replay,
    },
    /// Sends the prompt the last failed turn stopped again, if one is kept.
    RetryFailed,
    /// Asks the agent to sign in by itself, by one of the methods it offered.
    Authenticate {
        method_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Lets go of the prompt the last failed turn stopped.
    DiscardFailed,
    /// Lets go of the agent, still running, for a core that replaces this one.
    HandOver {
        reply: oneshot::Sender<Result<native::Handed, String>>,
    },
}

/// What became of a message from another agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    NewTurn,
    Steered,
    /// The turn ended as it was steered; sending it again starts a new one.
    TurnEnded,
    /// The agent cannot take it mid-turn.
    Refused,
}

#[derive(Clone)]
enum Readiness {
    Starting,
    Ready(ChatStart),
    Ended(Option<String>),
}

/// Tells the agent the option the person chose, or none when the question
/// was cancelled. It writes to the agent before it returns, so an answer
/// given before the chat lets go of its agent is never lost.
pub(crate) type Respond = Box<dyn FnOnce(Option<String>) + Send>;

struct PendingPermission {
    option_ids: Vec<String>,
    answer: Respond,
    request: Value,
    at: u64,
    /// What the agent's backend needs to answer the agent with the choice.
    reply: Value,
}

/// A question still put to the person, as plain data, for a core that takes
/// the chat over to keep asking.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Asked {
    pub request_id: String,
    pub option_ids: Vec<String>,
    pub request: Value,
    pub at: u64,
    pub reply: Value,
}

/// Who started a chat, a paired device or another agent, and with which of
/// the app's launchers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Origin {
    pub started_by: Option<String>,
    pub launcher: Option<String>,
}

pub(crate) struct Chat {
    /// How the chat was first started. Its agent, provider and folder never
    /// change; the rest can, and [`Chat::current_launch`] has it as it is now.
    pub launch: ChatLaunch,
    current: Mutex<ChatLaunch>,
    pub origin: Origin,
    core: Weak<Core>,
    generation: u64,
    commands: mpsc::UnboundedSender<ChatCommand>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub feed: Arc<Feed>,
    permissions: Mutex<HashMap<String, PendingPermission>>,
    readiness: watch::Sender<Readiness>,
    /// A prompt this side sent is being answered.
    pub running: AtomicBool,
    /// Wakes whatever waits for the running turn to end.
    turn_over: tokio::sync::Notify,
    /// The agent is working on a turn nobody here prompted.
    pub unprompted: AtomicBool,
    turned: AtomicBool,
    /// A turn ended with subagents still at work, so it is not finished yet.
    held: AtomicBool,
    /// A turn of this chat was answered, so its provider has it saved.
    pub answered: AtomicBool,
    approving: AtomicBool,
    /// The chat's loop waits for its next command, so it can let go of its
    /// agent at once.
    pub at_rest: AtomicBool,
    /// The chat is letting go of its agent, so the person's answers wait for
    /// the core that takes it over.
    releasing: AtomicBool,
    /// A turn has started that the agent's lead has not yet heard the end of.
    unreported: AtomicBool,
    /// Why the turn in progress failed, said before the turn or the chat ends.
    failure: Mutex<Option<String>>,
    /// The last turn failed for want of the network, and its prompt goes
    /// again once the network is back.
    pub(crate) offline_failed: AtomicBool,
    edits: super::edits::Open,
    /// The version of the agent's CLI, once the agent has said it.
    cli_version: Mutex<Option<String>>,
    /// Writes the chat down as it changes, for a core that starts after this one.
    keeper: Keeper,
    /// The agent's process now.
    agent: Mutex<Option<KeptAgent>>,
    /// When the agent was lately started again after it stopped by itself.
    restarts: Mutex<std::collections::VecDeque<std::time::Instant>>,
    /// What every app was last told the chat is doing.
    published: Mutex<&'static str>,
    /// A new chat starts its agent only once the person writes in it or something is sent to it.
    unstarted: AtomicBool,
    wanted: tokio::sync::Notify,
    /// The saved history of the session it resumes is shown already, so the agent's own
    /// replay of it as it starts stays quiet.
    pub(crate) saved_shown: AtomicBool,
}

impl Chat {
    pub(crate) fn agent_id(&self) -> &str {
        &self.launch.agent_id
    }

    pub(crate) fn provider(&self) -> &str {
        &self.launch.provider
    }

    /// The launch the agent runs with now, which moves to another account or
    /// session when the agent is started again under the chat.
    pub(crate) fn current_launch(&self) -> ChatLaunch {
        self.current.locked().clone()
    }

    fn set_current(&self, launch: ChatLaunch) {
        let mut current = self.current.locked();
        *current = launch;
    }

    pub(crate) fn emit(&self, kind: ChatEventKind, payload: Value) {
        if kind == ChatEventKind::TurnStarted {
            self.turned.store(true, Ordering::Release);
            self.held.store(false, Ordering::Release);
            self.unreported.store(true, Ordering::Release);
            self.set_failure(None);
        }
        if let Some(core) = self.core.upgrade() {
            let activity = match kind {
                ChatEventKind::TurnStarted => Some(Activity::Started),
                ChatEventKind::TurnCompleted => {
                    let cancelled =
                        payload.get("stopReason").and_then(Value::as_str) == Some("cancelled");
                    if !cancelled && self.feed.running_subagents() > 0 {
                        self.held.store(true, Ordering::Release);
                        core.agents_changed();
                        None
                    } else {
                        Some(Activity::Finished { cancelled })
                    }
                }
                ChatEventKind::Error => Some(Activity::Problem(
                    payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )),
                _ => None,
            };
            let ended = match &activity {
                Some(Activity::Finished { cancelled: false }) => Some(match self.take_failure() {
                    Some(failure) => Ending::Failed(failure),
                    None => Ending::Finished,
                }),
                Some(Activity::Finished { cancelled: true }) => Some(Ending::Stopped),
                Some(Activity::Problem(message)) => self.turn_failed(message),
                _ => None,
            };
            if let Some(activity) = activity {
                core.note_activity(self.agent_id(), activity);
            }
            if let Some(ending) = ended {
                let carries_on = !matches!(ending, Ending::Stopped);
                self.feed.emit(kind, payload);
                self.report_turn(&core, ending);
                self.publish_state();
                if carries_on {
                    self.send_queued_later();
                }
                return;
            }
        }
        if kind == ChatEventKind::SessionUpdate {
            self.note_tool_call(&payload);
            self.note_session(&payload);
        }
        if kind == ChatEventKind::TurnStarted {
            self.note_resumable();
        }
        self.feed.emit(kind, payload);
        if kind == ChatEventKind::SessionUpdate {
            self.finish_held();
        } else {
            self.publish_state();
        }
    }

    /// Keeps on the agent's record what the session says of it: the title the
    /// agent gave it, and the account it moved to.
    /// Keeps on the agent's record the session a turn just ran in, which the
    /// provider saved, so the agent can be resumed on it.
    fn note_resumable(&self) {
        let (Some(core), Some(start)) = (self.core.upgrade(), self.feed.start()) else {
            return;
        };
        let known = core.workspaces.agent(self.agent_id());
        if known.is_none_or(|agent| agent.resume_id.as_deref() == Some(start.session_id.as_str())) {
            return;
        }
        let patch =
            serde_json::Map::from_iter([("resumeId".to_owned(), Value::from(start.session_id))]);
        if core.workspaces.update_agent(self.agent_id(), patch).is_ok() {
            core.view_changed();
        }
    }

    fn note_session(&self, payload: &Value) {
        let kind = payload
            .pointer("/update/sessionUpdate")
            .and_then(Value::as_str);
        if !matches!(kind, Some("session_info_update" | "account_switched")) {
            return;
        }
        let Some(core) = self.core.upgrade() else {
            return;
        };
        if let Some(account) = payload.pointer("/update/account").and_then(Value::as_str) {
            let patch =
                serde_json::Map::from_iter([("profileId".to_owned(), Value::from(account))]);
            if core.workspaces.update_agent(self.agent_id(), patch).is_ok() {
                core.view_changed();
            }
        }
        let title = feed::session_title(payload);
        super::workspace::retitle(&core, self.agent_id(), title, false);
    }

    /// What the chat is doing, in the words every app shows an agent by.
    pub(crate) fn state_now(&self) -> &'static str {
        if !self.permissions.locked().is_empty() {
            "blocked"
        } else if self.turn_running()
            || self.held.load(Ordering::Acquire)
            || self.feed.running_subagents() > 0
        {
            "working"
        } else if self.feed.start().is_some() {
            "idle"
        } else {
            "unknown"
        }
    }

    /// Tells every app what the chat is doing whenever that changes, so none
    /// has to work it out from the transcript a pane happens to hold.
    pub(crate) fn publish_state(&self) {
        self.publish(self.state_now());
    }

    fn publish(&self, state: &'static str) {
        let mut published = self.published.locked();
        if *published == state {
            return;
        }
        *published = state;
        drop(published);
        if let Some(core) = self.core.upgrade() {
            super::agent::publish_chat_state(&core, self.agent_id(), state);
        }
    }

    fn note_tool_call(&self, update: &Value) {
        if update["sessionUpdate"] != "tool_call" {
            return;
        }
        let Some(core) = self.core.upgrade() else {
            return;
        };
        let tool = update["name"]
            .as_str()
            .or_else(|| update["title"].as_str())
            .unwrap_or_default();
        super::messages::used_tool(&core, self.agent_id(), tool);
    }

    /// Finishes a held turn once its last subagent has ended, unless the agent
    /// starts a turn of its own to answer it first.
    pub(crate) fn finish_held(&self) {
        if !self.held.load(Ordering::Acquire)
            || self.feed.running_subagents() > 0
            || self.turn_running()
        {
            return;
        }
        let core = self.core.clone();
        let agent_id = self.agent_id().to_owned();
        let generation = self.generation;
        tokio::spawn(async move {
            tokio::time::sleep(SUBAGENT_WAKE_GRACE).await;
            let Some(core) = core.upgrade() else { return };
            let Some(chat) = core.chats.get(&agent_id) else {
                return;
            };
            if chat.generation != generation
                || chat.turn_running()
                || chat.feed.running_subagents() > 0
                || !chat.held.swap(false, Ordering::AcqRel)
            {
                return;
            }
            core.note_activity(&agent_id, Activity::Finished { cancelled: false });
            chat.report_turn(&core, Ending::Finished);
            chat.send_queued_later();
        });
    }

    /// An error that arrives once nothing is at work on the turn any more
    /// ended it.
    fn turn_failed(&self, message: &str) -> Option<Ending> {
        let over = !self.turn_running() && !self.held.load(Ordering::Acquire);
        (over && self.unreported.load(Ordering::Acquire))
            .then(|| Ending::Failed(message.to_owned()))
    }

    /// The turn in progress failed, which the lead hears in place of its end,
    /// or of the chat's when that comes first.
    pub(crate) fn turn_failing(&self, failure: &Value) {
        let message = failure
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.set_failure(Some(message.to_owned()));
    }

    fn set_failure(&self, failure: Option<String>) {
        let mut slot = self.failure.locked();
        *slot = failure;
    }

    fn take_failure(&self) -> Option<String> {
        self.failure.locked().take()
    }

    /// Tells the agent's lead how its turn ended, once per turn.
    fn report_turn(&self, core: &Arc<Core>, ending: Ending) {
        if self.unreported.swap(false, Ordering::AcqRel) {
            super::spawn::turn_ended(core, self.agent_id(), ending);
        }
    }

    /// Tells the agent's lead the chat has ended, when that cut a turn short
    /// or the agent went away by itself after it had started.
    fn report_end(&self, core: &Arc<Core>, result: &Result<SessionEnd, String>) {
        let mid_turn = self.unreported.swap(false, Ordering::AcqRel);
        let went_away = self.is_ready() && !matches!(result, Ok(SessionEnd::Requested));
        if !mid_turn && !went_away {
            return;
        }
        let reason = match result {
            Err(error) => Some(error.clone()),
            Ok(SessionEnd::Requested) => None,
            Ok(_) => self.take_failure(),
        };
        super::spawn::turn_ended(core, self.agent_id(), Ending::Gone { reason, mid_turn });
    }

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    pub(crate) fn note_cli_version(&self, version: &str) {
        let mut known = self.cli_version.locked();
        *known = Some(version.to_owned()).filter(|version| !version.is_empty());
    }

    /// How long to wait before starting the agent again after it stopped by
    /// itself, or none once it has stopped too often lately.
    pub(crate) fn restart_after(&self) -> Option<Duration> {
        let now = std::time::Instant::now();
        let mut restarts = self.restarts.locked();
        restarts.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
        if restarts.len() >= MAX_RESTARTS {
            return None;
        }
        let wait = RESTART_WAIT * (1 << restarts.len());
        restarts.push_back(now);
        Some(wait)
    }

    /// Ends in the transcript the turn the agent was cut off in, so the one
    /// that carries it on starts clean.
    pub(crate) fn end_cut_turn(&self) {
        if self.feed.in_turn() {
            self.feed.emit(
                ChatEventKind::TurnCompleted,
                serde_json::json!({ "stopReason": "cancelled" }),
            );
        }
    }

    pub(crate) fn note_agent(&self, pid: u32) {
        let mut agent = self.agent.locked();
        *agent = KeptAgent::of(pid);
    }

    /// Writes down where the chat stands, with how its loop says its turns go.
    pub(crate) fn keep(&self, progress: kept::Progress<'_>) {
        let running = self.running.load(Ordering::Acquire);
        self.keeper.keep(KeptChat {
            launch: self.current_now(),
            from_shell: Vec::new(),
            keys: Vec::new(),
            origin: self.origin.clone(),
            feed: self.feed.id().to_owned(),
            agent: *self.agent.locked(),
            working: self.turn_running() || self.held.load(Ordering::Acquire),
            prompt: progress.prompt.filter(|_| running).cloned(),
            offline: progress
                .failed
                .filter(|_| self.offline_failed.load(Ordering::Acquire))
                .cloned(),
            reloadable: progress.reloadable,
            unstarted: self.unstarted(),
        });
    }

    pub(crate) fn cli_version(&self) -> Option<String> {
        self.cli_version.locked().clone()
    }

    pub(crate) fn work_started(&self) {
        if !self.running.load(Ordering::Acquire) && !self.unprompted.swap(true, Ordering::AcqRel) {
            self.emit(ChatEventKind::TurnStarted, serde_json::json!({}));
        }
    }

    /// The work nobody here asked for is over.
    pub(crate) fn work_ended(&self, stop_reason: &str) {
        if self.unprompted.swap(false, Ordering::AcqRel) {
            self.emit(
                ChatEventKind::TurnCompleted,
                serde_json::json!({ "stopReason": stop_reason }),
            );
        }
    }

    /// The agent was asked to stop, with `sent` saying whether the ask got
    /// there. Turn `turn`, when it runs, has a while to end before `stalled`
    /// is told; work nobody here asked for is over at once.
    pub(crate) fn cancel_sent(
        &self,
        sent: Result<(), String>,
        turn: u64,
        stalled: &mpsc::UnboundedSender<u64>,
    ) {
        if let Err(error) = sent {
            self.emit(ChatEventKind::Error, rebind::error_message(error));
        }
        if self.running.load(Ordering::Acquire) {
            stall_later(turn, stalled);
        } else {
            self.work_ended("cancelled");
        }
    }

    /// Takes a question the agent asked with its CLI's own tool into
    /// Sikemux's, and says what the agent is told in its place.
    pub(crate) fn ask_question(&self, input: &Value) -> String {
        match self.core.upgrade() {
            Some(core) => super::questions::ask_from_cli(&core, self.agent_id(), input),
            None => "Sikemux is shutting down; ask again later.".to_owned(),
        }
    }

    pub(crate) fn letter_unread(&self, text: String) {
        if let Some(core) = self.core.upgrade() {
            core.harness.mailbox.returned(self.agent_id(), text);
        }
    }

    /// Records the files a tool call changed once `update` says it is done.
    /// Only while the agent works, its turn or the subagents and background
    /// tasks that outlive it, so history read back again is not counted twice.
    pub(crate) fn note_edits(&self, update: &Value) {
        let working = self.turn_running()
            || self.held.load(Ordering::Acquire)
            || self.feed.running_subagents() > 0
            || self.feed.running_tasks() > 0;
        if !working {
            return;
        }
        let Some(core) = self.core.upgrade() else {
            return;
        };
        if core
            .collisions
            .saw(self.agent_id(), &self.launch.cwd, update)
        {
            core.edits.command_began(self.agent_id());
        }
        if let Some((tool_call_id, changes)) = self.edits.note(update) {
            core.edits.record(self, tool_call_id, changes);
        }
    }

    /// The running turn is over. Answers whether one was running.
    pub(crate) fn stop_running(&self) -> bool {
        let was = self.running.swap(false, Ordering::AcqRel);
        self.turn_over.notify_waiters();
        was
    }

    /// Resolves once no turn is running.
    pub(crate) async fn turn_ended(&self) {
        loop {
            let over = self.turn_over.notified();
            if !self.running.load(Ordering::Acquire) {
                return;
            }
            over.await;
        }
    }

    pub(crate) fn turn_running(&self) -> bool {
        self.running.load(Ordering::Acquire) || self.unprompted.load(Ordering::Acquire)
    }

    pub(crate) fn approves(&self) -> bool {
        self.approving.load(Ordering::Acquire)
    }

    pub(crate) fn set_permission_mode(&self, mode: PermissionMode) {
        self.approving
            .store(mode == PermissionMode::Bypass, Ordering::Release);
        self.feed.set_permission_mode(mode.as_str());
        if let Some(core) = self.core.upgrade() {
            core.view_changed();
        }
    }

    pub(crate) fn mark_ready(&self, start: ChatStart) {
        if let Some(core) = self.core.upgrade() {
            if let Some(launcher) = &self.origin.launcher {
                core.workspaces
                    .note_config_options(launcher, &start.setup["configOptions"]);
            }
            core.names.note_session(self.agent_id(), &start.session_id);
        }
        self.feed.set_start(start.clone());
        self.tell_saved_usage(&start);
        self.readiness.send_replace(Readiness::Ready(start));
        if let Some(core) = self.core.upgrade() {
            core.agents_changed();
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(*self.readiness.borrow(), Readiness::Ready(_))
    }

    fn end(&self, error: Option<String>) {
        self.readiness.send_if_modified(|readiness| {
            if matches!(readiness, Readiness::Ended(_)) {
                return false;
            }
            *readiness = Readiness::Ended(error);
            true
        });
    }

    /// Resolves once the session is ready, with why it never got there
    /// otherwise.
    async fn until_ready(&self) -> Result<ChatStart, String> {
        let mut readiness = self.readiness.subscribe();
        let settled = readiness
            .wait_for(|readiness| !matches!(readiness, Readiness::Starting))
            .await
            .map(|readiness| readiness.clone());
        match settled {
            Ok(Readiness::Ready(start)) => Ok(start),
            Ok(Readiness::Ended(Some(error))) => Err(error),
            _ => Err("ACP session stopped before initialization completed".into()),
        }
    }

    /// Keeps the request until the person answers it. False when it could
    /// not be kept, in which case it was answered as cancelled.
    pub(crate) fn hold_permission(
        &self,
        request_id: String,
        option_ids: Vec<String>,
        answer: Respond,
        request: Value,
        reply: Value,
    ) -> bool {
        let mut permissions = self.permissions.locked();
        let pending = PendingPermission {
            option_ids,
            answer,
            request,
            at: epoch_ms(),
            reply,
        };
        let attention = self.attention(&request_id, &pending);
        permissions.insert(request_id, pending);
        drop(permissions);
        if let Some(core) = self.core.upgrade() {
            core.note_activity(self.agent_id(), Activity::NeedsYou);
        }
        self.announce(&Event::Attention { attention });
        self.publish_state();
        true
    }

    /// Keeps asking a question an earlier core put to the person, who has
    /// been shown it already.
    pub(crate) fn hold_again(&self, asked: Asked, answer: Respond) {
        let mut permissions = self.permissions.locked();
        permissions.insert(
            asked.request_id,
            PendingPermission {
                option_ids: asked.option_ids,
                answer,
                request: asked.request,
                at: asked.at,
                reply: asked.reply,
            },
        );
    }

    /// The questions still put to the person, as plain data.
    fn asked(&self) -> Vec<Asked> {
        let permissions = self.permissions.locked();
        let mut asked: Vec<Asked> = permissions
            .iter()
            .map(|(request_id, pending)| Asked {
                request_id: request_id.clone(),
                option_ids: pending.option_ids.clone(),
                request: pending.request.clone(),
                at: pending.at,
                reply: pending.reply.clone(),
            })
            .collect();
        asked.sort_by_key(|asked| asked.at);
        asked
    }

    fn attention(&self, request_id: &str, pending: &PendingPermission) -> Attention {
        Attention {
            id: request_id.to_owned(),
            kind: AttentionKind::Permission,
            agent_id: self.launch.agent_id.clone(),
            provider: self.launch.provider.clone(),
            cwd: self.launch.cwd.clone(),
            request: pending.request.clone(),
            at: pending.at,
            choices: permission::choices(&pending.request),
        }
    }

    fn attentions(&self) -> Vec<Attention> {
        self.permissions
            .locked()
            .iter()
            .map(|(request_id, pending)| self.attention(request_id, pending))
            .collect()
    }

    fn announce(&self, event: &Event) {
        if let Some(core) = self.core.upgrade() {
            core.broadcast_event(event);
        }
    }

    fn cleared(&self, request_id: &str) {
        self.announce(&Event::AttentionCleared {
            id: request_id.to_owned(),
            agent_id: self.launch.agent_id.clone(),
        });
        self.publish_state();
    }

    pub(crate) fn cancel_permissions(&self) {
        let pending: Vec<_> = self.permissions.locked().drain().collect();
        for (request_id, request) in pending {
            self.feed.forget_permission(&request_id);
            self.cleared(&request_id);
            (request.answer)(None);
        }
    }

    fn pending_permissions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.permissions.locked().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn reply_permission(&self, request_id: &str, option_id: Option<String>) -> CoreResult<()> {
        // Answered with the list held, so the answer is written to the agent
        // before the chat can let go of it.
        let mut permissions = self.permissions.locked();
        if self.releasing.load(Ordering::Acquire) {
            return Err(super::upgrade::UPDATING.into());
        }
        let request = {
            let Some(request) = permissions.get(request_id) else {
                return Err("ACP permission request is no longer pending".into());
            };
            if let Some(option_id) = &option_id {
                if !request.option_ids.contains(option_id) {
                    return Err("ACP permission option is invalid".into());
                }
            }
            permissions
                .remove(request_id)
                .ok_or("ACP permission request is no longer pending")?
        };
        (request.answer)(option_id);
        drop(permissions);
        self.feed.forget_permission(request_id);
        self.cleared(request_id);
        Ok(())
    }

    fn send(&self, command: ChatCommand) -> CoreResult<()> {
        if self.unstarted.load(Ordering::Acquire) {
            match command {
                ChatCommand::Cancel | ChatCommand::StopTask { .. } | ChatCommand::DiscardFailed => {
                    return Ok(())
                }
                ChatCommand::HandOver { reply } => {
                    let _ = reply.send(Err("it had not started".into()));
                    return Ok(());
                }
                _ => self.start_agent(),
            }
        }
        self.commands.send(command).map_err(|_| STOPPED.into())
    }

    /// Starts the agent of a chat that waited for the person, once; later calls do nothing.
    pub(crate) fn start_agent(&self) {
        if self.unstarted.swap(false, Ordering::AcqRel) {
            self.wanted.notify_one();
        }
    }

    /// The chat waits for the person before its agent starts.
    pub(crate) fn unstarted(&self) -> bool {
        self.unstarted.load(Ordering::Acquire)
    }

    /// Sends the prompt a failed turn stopped again, as the person asked.
    pub(crate) fn retry_failed(&self) -> CoreResult<()> {
        self.offline_failed.store(false, Ordering::Release);
        self.send(ChatCommand::RetryFailed)
    }

    pub(crate) fn discard_failed(&self) -> CoreResult<()> {
        self.offline_failed.store(false, Ordering::Release);
        self.send(ChatCommand::DiscardFailed)?;
        self.send_queued_later();
        Ok(())
    }

    pub(crate) fn info(&self) -> ChatInfo {
        let start = self.feed.start();
        ChatInfo {
            agent_id: self.launch.agent_id.clone(),
            name: self
                .core
                .upgrade()
                .and_then(|core| core.names.name(self.agent_id())),
            provider: self.launch.provider.clone(),
            title: self.feed.title(),
            cwd: self.launch.cwd.clone(),
            session_id: start.as_ref().map(|start| start.session_id.clone()),
            state: if start.is_some() {
                ChatState::Ready
            } else {
                ChatState::Starting
            },
            running: self.turn_running(),
            pending_permissions: self.pending_permissions(),
            subagents: self.feed.running_subagents(),
            tasks: self.feed.running_tasks(),
            reconnecting: self.feed.reconnecting(),
            started_by: self.origin.started_by.clone(),
            launcher: self.origin.launcher.clone(),
            permission_mode: self.feed.permission_mode(),
            model: self.launch.model.clone(),
            model_provider: self.launch.env.get(ROUTED_ENV).cloned(),
            effort: self.launch.effort.clone(),
            asleep: false,
            unread: false,
            active_at: None,
            group: Default::default(),
            status: Default::default(),
            attention_rank: 0,
        }
    }

    /// Whether the provider has a saved session to load: one a turn answered,
    /// or one the chat was resumed from.
    pub(crate) fn has_answered(&self) -> bool {
        self.answered.load(Ordering::Acquire) || self.launch.resume_id.is_some()
    }

    fn kept_by_provider(&self) -> bool {
        self.turned.load(Ordering::Acquire) || self.launch.resume_id.is_some()
    }

    /// The launch the agent runs with now, in the mode, model and effort
    /// chosen since it started.
    pub(crate) fn current_now(&self) -> ChatLaunch {
        let current = self.current_launch();
        self.relaunch(&current).unwrap_or_else(|| ChatLaunch {
            permission_mode: self.feed.permission_mode(),
            ..current
        })
    }

    /// `launch` brought up to where the chat is: its session, once a turn has
    /// made the provider keep it, and the mode, model and effort chosen since.
    fn relaunch(&self, launch: &ChatLaunch) -> Option<ChatLaunch> {
        let start = self.feed.start()?;
        let setup = &start.setup;
        let effort_id = acp_native::effort_config_id(setup).map(str::to_owned);
        let mut launch = launch.clone();
        launch.resume_id = self.kept_by_provider().then(|| start.session_id.clone());
        launch.permission_mode = self.feed.permission_mode();
        launch.model = current_choice(setup, "model").or(launch.model);
        launch.effort = effort_id
            .and_then(|id| current_choice(setup, &id))
            .or(launch.effort);
        Some(launch)
    }

    /// How to start this chat again on its provider session, once a turn has
    /// made the provider keep it.
    fn resumable(&self) -> Option<ChatRecord> {
        if !self.kept_by_provider() {
            return None;
        }
        Some(ChatRecord {
            launch: self.relaunch(&self.current_launch())?,
            origin: self.origin.clone(),
            notice: None,
            unstarted: false,
        })
    }

    /// How to start this chat again: on its provider session once it has
    /// one, else afresh as it was launched.
    fn restart_record(&self) -> ChatRecord {
        self.resumable().unwrap_or_else(|| ChatRecord {
            launch: self.current_launch(),
            origin: self.origin.clone(),
            notice: None,
            unstarted: self.unstarted(),
        })
    }

    /// Whether the chat can let go of its agent now. One that cannot, still
    /// starting or in the middle of a request, holds an update up. One whose
    /// agent never started has nothing to lose and waits again after it.
    fn handable(&self) -> bool {
        self.unstarted() || (self.is_ready() && self.at_rest.load(Ordering::Acquire))
    }

    fn flags(&self) -> Flags {
        Flags {
            running: self.running.load(Ordering::Acquire),
            unprompted: self.unprompted.load(Ordering::Acquire),
            turned: self.turned.load(Ordering::Acquire),
            held: self.held.load(Ordering::Acquire),
            answered: self.answered.load(Ordering::Acquire),
            approving: self.approving.load(Ordering::Acquire),
        }
    }

    /// Lets go of the agent, still running, for a core that replaces this
    /// one. Answers with how to start the chat again instead, and why, when
    /// it cannot.
    async fn hand_over(self: Arc<Self>) -> HandOver {
        let record = self.restart_record();
        if record.unstarted {
            Chats::halt(&self);
            return HandOver::Resume(Box::new(record));
        }
        let why = if self.handable() {
            match self.release().await {
                Ok(handed) => return HandOver::Live(Box::new(handed)),
                Err(why) => why,
            }
        } else if self.is_ready() {
            "it was busy with a request".to_owned()
        } else {
            "it was still starting".to_owned()
        };
        if let Some(task) = Chats::halt(&self) {
            let _ = tokio::time::timeout(STOP_SETTLE, task).await;
        }
        HandOver::Resume(Box::new(ChatRecord {
            notice: Some(why),
            ..record
        }))
    }

    async fn release(&self) -> Result<HandedChat, String> {
        {
            let _permissions = self.permissions.locked();
            self.releasing.store(true, Ordering::Release);
        }
        let (reply, answer) = oneshot::channel();
        self.send(ChatCommand::HandOver { reply })
            .map_err(|_| "it had stopped".to_owned())?;
        let handed = match tokio::time::timeout(RELEASE_WAIT, answer).await {
            Ok(Ok(handed)) => handed?,
            Ok(Err(_)) => return Err("it had stopped".into()),
            Err(_) => return Err("it did not let go of its agent in time".into()),
        };
        let task = self.task.locked().take();
        if let Some(task) = task {
            let _ = tokio::time::timeout(STOP_SETTLE, task).await;
        }
        let Some((feed, subscribers)) = self.feed.release() else {
            native::kill_agent(handed.released.pid);
            return Err("its transcript could not be kept".into());
        };
        Ok(HandedChat {
            live: LiveChat {
                record: self.restart_record(),
                launch: self.launch.clone(),
                current: self.current_launch(),
                origin: self.origin.clone(),
                flags: self.flags(),
                feed,
                asked: self.asked(),
                session: handed.session,
                backend: handed.backend,
                agent: handed.released.record(),
                cli_version: self.cli_version(),
            },
            released: handed.released,
            subscribers,
        })
    }
}

/// What said whether a chat was at work, carried to the core that takes it
/// over.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Flags {
    running: bool,
    unprompted: bool,
    turned: bool,
    held: bool,
    answered: bool,
    approving: bool,
}

/// A chat handed whole to a core that replaces this one: the agent goes on
/// running, and the new core reads on where this one stopped.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveChat {
    /// How to start the chat again should taking its agent up fail.
    pub record: ChatRecord,
    launch: ChatLaunch,
    current: ChatLaunch,
    origin: Origin,
    flags: Flags,
    feed: FeedState,
    asked: Vec<Asked>,
    session: native::SessionState,
    backend: Value,
    pub agent: native::AgentRecord,
    cli_version: Option<String>,
}

/// A [`LiveChat`] and the pipes to its agent, still open in this process.
pub(crate) struct HandedChat {
    pub live: LiveChat,
    pub released: native::Released,
    /// The clients that were hearing the chat, which hear it again if this
    /// core takes the chat back up after a failed update.
    pub subscribers: Vec<Arc<ClientConn>>,
}

/// What becomes of a chat when the core replaces itself.
pub(crate) enum HandOver {
    Live(Box<HandedChat>),
    /// Started again on its session, saying why.
    Resume(Box<ChatRecord>),
}

/// Stops a chat agent no core will take up.
pub(crate) fn kill_agent(pid: u32) {
    native::kill_agent(pid);
}

/// The notice a chat shows when an update started its agent again rather
/// than carrying it over.
pub(crate) fn update_notice(why: &str) -> Value {
    serde_json::json!({
        "sessionUpdate": "notice",
        "severity": "warning",
        "title": "Sikemux restarted this agent while it updated",
        "description": format!("The agent could not be carried over because {why}. Anything it was doing stopped."),
    })
}

/// Tells `stalled` of turn `turn` once it has had a while to stop.
pub(crate) fn stall_later(turn: u64, stalled: &mpsc::UnboundedSender<u64>) {
    let stalled = stalled.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CANCEL_GRACE).await;
        let _ = stalled.send(turn);
    });
}

/// A chat the core cannot hand over to its replacement now.
#[derive(Clone, Debug)]
pub(crate) struct Blocker {
    pub agent_id: String,
    pub provider: String,
    pub title: Option<String>,
    pub reason: String,
}

/// A chat handed to a core that replaces this one, which starts it again on
/// the same provider session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatRecord {
    pub launch: ChatLaunch,
    #[serde(flatten)]
    pub origin: Origin,
    /// Why the chat was started again rather than carried over, said in it
    /// once it is back.
    pub notice: Option<String>,
    /// Its agent never started, so it waits for the person again.
    #[serde(default)]
    pub unstarted: bool,
}

#[derive(Default)]
pub(crate) struct Chats {
    chats: Mutex<HashMap<String, Arc<Chat>>>,
    next_generation: AtomicU64,
    /// Where each chat keeps everything it said, for phones to page back.
    history: Option<PathBuf>,
    /// Where each chat is written down, for a core that starts after this one.
    kept: Option<PathBuf>,
    pub(crate) resets: reset::Resets,
    /// The feeds of chats asleep, their history kept on disk for clients to page and find
    /// through without waking anything.
    resting: Mutex<Vec<Resting>>,
}

/// A sleeping chat's feed, how much history it keeps, and when a client last read it.
struct Resting {
    agent_id: String,
    feed: Arc<Feed>,
    bytes: u64,
    read: std::time::Instant,
}

/// The most history kept for sleeping chats in all; past it, the one read least lately goes.
const RESTING_BYTES: u64 = 2 * 1024 * 1024 * 1024;

impl Chats {
    pub(crate) fn new(data_dir: Option<&std::path::Path>) -> Self {
        Self {
            history: data_dir.map(|dir| dir.join("chat-history")),
            kept: data_dir.map(|dir| dir.join("chats")),
            resets: reset::Resets::new(data_dir),
            ..Self::default()
        }
    }

    /// Removes the history an earlier core left, but for the chats this one
    /// took over.
    pub(crate) fn clear_history(&self) {
        let Some(dir) = self.history.as_deref() else {
            return;
        };
        let keep: Vec<String> = self
            .all()
            .iter()
            .map(|chat| chat.feed.id().to_owned())
            .collect();
        history::clear(dir, &keep);
    }

    /// Every chat whose last turn the network cut short tries it once more,
    /// now that the network is back.
    pub(crate) fn retry_offline_failures(&self) {
        for chat in self.all() {
            if chat.offline_failed.swap(false, Ordering::AcqRel) {
                let _ = chat.send(ChatCommand::RetryFailed);
            }
        }
    }

    pub(crate) fn get(&self, agent_id: &str) -> Option<Arc<Chat>> {
        self.chats.locked().get(agent_id).cloned()
    }

    /// The chats a client has taken up and hears the events of.
    pub(crate) fn followed_by(&self, client: ClientId) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|chat| chat.feed.has_subscriber(client))
            .map(|chat| chat.agent_id().to_owned())
            .collect()
    }

    pub(crate) fn ids(&self) -> Vec<String> {
        self.chats.locked().keys().cloned().collect()
    }

    fn all(&self) -> Vec<Arc<Chat>> {
        self.chats.locked().values().cloned().collect()
    }

    pub(crate) fn running(&self, agent_id: &str) -> CoreResult<Arc<Chat>> {
        self.get(agent_id).ok_or_else(|| NOT_RUNNING.into())
    }

    /// The feed a client reads the agent's chat from: the running chat's, else the one it
    /// kept when it went to sleep. Reading a sleeping chat starts nothing.
    pub(crate) fn readable(&self, agent_id: &str) -> CoreResult<Arc<Feed>> {
        if let Some(chat) = self.get(agent_id) {
            return Ok(chat.feed.clone());
        }
        let mut resting = self.resting.locked();
        let kept = resting
            .iter_mut()
            .find(|resting| resting.agent_id == agent_id);
        let kept = kept.ok_or_else(|| CoreError::from(NOT_RUNNING))?;
        kept.read = std::time::Instant::now();
        Ok(kept.feed.clone())
    }

    /// Keeps the feed of `chat`, which went to sleep, with only its history on disk, letting
    /// go of the history read least lately once all those kept pass their size.
    pub(crate) fn rest(&self, chat: &Chat) {
        let bytes = chat.feed.rest();
        let mut resting = self.resting.locked();
        resting.retain(|resting| resting.agent_id != chat.agent_id());
        resting.push(Resting {
            agent_id: chat.agent_id().to_owned(),
            feed: chat.feed.clone(),
            bytes,
            read: std::time::Instant::now(),
        });
        while resting.iter().map(|resting| resting.bytes).sum::<u64>() > RESTING_BYTES {
            let Some(oldest) = (0..resting.len()).min_by_key(|&at| resting[at].read) else {
                break;
            };
            resting.remove(oldest);
        }
    }

    /// The feed the agent's chat kept while it slept, no longer kept apart.
    fn take_rest(&self, agent_id: &str) -> Option<Arc<Feed>> {
        let mut resting = self.resting.locked();
        let at = resting
            .iter()
            .position(|resting| resting.agent_id == agent_id)?;
        Some(resting.remove(at).feed)
    }

    /// Lets go of what a chat that will not wake again kept while it slept.
    pub(crate) fn forget_rest(&self, agent_id: &str) {
        self.resting
            .locked()
            .retain(|resting| resting.agent_id != agent_id);
    }

    pub(crate) fn count(&self) -> usize {
        self.chats.locked().len()
    }

    /// Whether every chat can let go of its agent now.
    pub(crate) fn all_handable(&self) -> bool {
        self.all().iter().all(|chat| chat.handable())
    }

    /// How many chats are in a turn, waiting on the person or unable to hand
    /// their agent over, which an update nobody asked for waits on.
    pub(crate) fn in_turn(&self) -> usize {
        self.all()
            .iter()
            .filter(|chat| !chat.handable() || matches!(chat.state_now(), "working" | "blocked"))
            .count()
    }

    /// The chats at work that an update now would stop and start again
    /// rather than carry over.
    pub(crate) fn blockers(&self) -> Vec<Blocker> {
        let mut blockers: Vec<Blocker> = self
            .all()
            .iter()
            .filter(|chat| !chat.handable())
            .map(|chat| Blocker {
                agent_id: chat.agent_id().to_owned(),
                provider: chat.provider().to_owned(),
                title: chat.feed.title(),
                reason: if chat.is_ready() {
                    "busy".to_owned()
                } else {
                    "starting".to_owned()
                },
            })
            .collect();
        blockers.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        blockers
    }

    pub(crate) fn attentions(&self) -> Vec<Attention> {
        let mut attentions: Vec<Attention> = self
            .all()
            .iter()
            .flat_map(|chat| chat.attentions())
            .collect();
        attentions.sort_by_key(|attention| attention.at);
        attentions
    }

    pub(crate) fn list(&self) -> Vec<ChatInfo> {
        let mut chats: Vec<ChatInfo> = self.all().iter().map(|chat| chat.info()).collect();
        chats.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        chats
    }

    pub(crate) fn forget_client(&self, client: ClientId) {
        for chat in self.all() {
            chat.feed.unsubscribe(client);
        }
        for resting in self.resting.locked().iter() {
            resting.feed.unsubscribe(client);
        }
    }

    /// Whether `chat` was the agent's current one, which this removes.
    fn remove_if_current(&self, chat: &Chat) -> bool {
        let mut chats = self.chats.locked();
        let current = chats
            .get(chat.agent_id())
            .is_some_and(|current| current.generation == chat.generation);
        if current {
            chats.remove(chat.agent_id());
        }
        current
    }

    fn take(&self, agent_id: &str) -> Option<Arc<Chat>> {
        self.chats.locked().remove(agent_id)
    }

    fn take_all(&self) -> Vec<Arc<Chat>> {
        self.chats.locked().drain().map(|(_, chat)| chat).collect()
    }

    /// Ends the chat at once, killing the agent and everything it started.
    /// Returns the task that ends with it.
    fn halt(chat: &Chat) -> Option<tokio::task::JoinHandle<()>> {
        chat.keeper.forget();
        chat.feed.close();
        chat.cancel_permissions();
        chat.end(None);
        let task = chat.task.locked().take()?;
        task.abort();
        Some(task)
    }

    /// Whether the agent had a chat to stop.
    pub(crate) fn stop(&self, agent_id: &str) -> bool {
        let Some(chat) = self.take(agent_id) else {
            return false;
        };
        Self::halt(&chat);
        true
    }

    /// Stops the chat only when nothing in it is running: no turn, question,
    /// subagent or background task. Answers whether it stopped.
    pub(crate) fn stop_if_idle(&self, agent_id: &str) -> bool {
        self.get(agent_id).is_none() || (!self.at_work(agent_id) && self.stop(agent_id))
    }

    /// Whether the agent's chat is in a turn or has tasks running.
    pub(crate) fn at_work(&self, agent_id: &str) -> bool {
        self.get(agent_id).is_some_and(|chat| {
            matches!(chat.state_now(), "working" | "blocked") || chat.feed.running_tasks() > 0
        })
    }

    pub(crate) fn stop_all(&self) {
        for chat in self.take_all() {
            Self::halt(&chat);
        }
    }

    /// Lets go of every chat's agent for a core that replaces this one, and
    /// stops the chats that cannot let go, saying how to start them again.
    pub(crate) async fn hand_over(&self) -> Vec<HandOver> {
        let mut releasing = tokio::task::JoinSet::new();
        for chat in self.take_all() {
            releasing.spawn(chat.hand_over());
        }
        let mut handed = Vec::new();
        while let Some(done) = releasing.join_next().await {
            handed.extend(done.ok());
        }
        handed
    }

    fn insert(
        &self,
        core: &Arc<Core>,
        launch: ChatLaunch,
        origin: Origin,
        kept: Option<(Arc<Feed>, Flags)>,
    ) -> CoreResult<(Arc<Chat>, mpsc::UnboundedReceiver<ChatCommand>)> {
        let mut chats = self.chats.locked();
        if chats.contains_key(&launch.agent_id) {
            return Err("ACP session is already running".into());
        }
        let (commands, queue) = mpsc::unbounded_channel();
        // A chat waking from sleep goes on in the feed it slept with, so a client reading
        // that feed hears it wake, and the history its start replays is already there.
        let rested = kept
            .is_none()
            .then(|| self.take_rest(&launch.agent_id))
            .flatten();
        let history_shown = rested.is_some();
        let (feed, flags) = kept.unwrap_or_else(|| {
            let feed = rested.unwrap_or_else(|| {
                Feed::new(
                    launch.agent_id.clone(),
                    launch.permission_mode.clone(),
                    self.history.as_deref(),
                )
            });
            let flags = Flags {
                running: false,
                unprompted: false,
                turned: false,
                held: false,
                answered: false,
                approving: crate::acp::approves_for_user(&launch.permission_mode),
            };
            (feed, flags)
        });
        let offers_effort = crate::agent_catalog::agent(&launch.provider)
            .and_then(|agent| agent.chat.as_ref())
            .is_some_and(|driver| {
                matches!(
                    driver,
                    crate::agent_catalog::ChatDriver::Claude
                        | crate::agent_catalog::ChatDriver::Codex
                )
            });
        if offers_effort {
            feed.offer_effort();
        }
        feed.work_in(launch.cwd.clone());
        let keeper = Keeper::new(self.kept.as_deref(), feed.id());
        let chat = Arc::new(Chat {
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            commands,
            task: Mutex::new(None),
            feed,
            permissions: Mutex::new(HashMap::new()),
            readiness: watch::channel(Readiness::Starting).0,
            running: AtomicBool::new(flags.running),
            turn_over: tokio::sync::Notify::new(),
            unprompted: AtomicBool::new(flags.unprompted),
            turned: AtomicBool::new(flags.turned),
            held: AtomicBool::new(flags.held),
            answered: AtomicBool::new(flags.answered),
            approving: AtomicBool::new(flags.approving),
            at_rest: AtomicBool::new(false),
            releasing: AtomicBool::new(false),
            unreported: AtomicBool::new(flags.running || flags.unprompted || flags.held),
            failure: Mutex::new(None),
            offline_failed: AtomicBool::new(false),
            edits: Default::default(),
            cli_version: Mutex::new(None),
            keeper,
            agent: Mutex::new(None),
            restarts: Mutex::new(Default::default()),
            published: Mutex::new("unknown"),
            unstarted: AtomicBool::new(false),
            wanted: tokio::sync::Notify::new(),
            saved_shown: AtomicBool::new(history_shown),
            current: Mutex::new(launch.clone()),
            launch,
            origin,
            core: Arc::downgrade(core),
        });
        self.resting
            .locked()
            .retain(|resting| resting.agent_id != chat.agent_id());
        chats.insert(chat.agent_id().to_owned(), chat.clone());
        Ok((chat, queue))
    }
}

/// A chat that is listed but never started, for tests of what surrounds it.
#[cfg(test)]
pub(crate) fn idle(core: &Arc<Core>, agent_id: &str, provider: &str, cwd: &str) -> Arc<Chat> {
    let launch = ChatLaunch {
        agent_id: agent_id.into(),
        provider: provider.into(),
        cwd: cwd.into(),
        program: "/bin/false".into(),
        args: Vec::new(),
        env: Default::default(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::WorkspaceWrite.as_str().into(),
        model: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };
    let (chat, _) = core
        .chats
        .insert(core, launch, Origin::default(), None)
        .unwrap_or_else(|error| panic!("{error}"));
    chat
}

fn validate_account(account: &ChatAccount) -> CoreResult<()> {
    bounded_text("account id", &account.id, 256)?;
    bounded_text("account name", &account.label, 256)?;
    for (key, value) in &account.env {
        bounded_text("account variable", key, 128)?;
        if value.len() > 4_096 || value.contains('\0') {
            return Err("account variables must be bounded text".into());
        }
    }
    Ok(())
}

fn validate(launch: &ChatLaunch) -> CoreResult<()> {
    bounded_text("agent id", &launch.agent_id, MAX_AGENT_ID)?;
    bounded_text("provider", &launch.provider, 64)?;
    bounded_text("working directory", &launch.cwd.to_string_lossy(), 4_096)?;
    PermissionMode::parse(&launch.permission_mode)?;
    if !launch.cwd.is_absolute() {
        return Err("ACP working directory must be absolute".into());
    }
    if let Some(resume_id) = launch.resume_id.as_deref() {
        let agent = crate::agent_catalog::AgentKind::from_name(&launch.provider);
        if !crate::sessions::is_session_id(agent, resume_id) {
            return Err(format!("{resume_id:?} is not a {} session id", launch.provider).into());
        }
    }
    if launch.program.as_os_str().is_empty() {
        return Err("ACP agent program is missing".into());
    }
    for account in launch.account.iter().chain(&launch.fallbacks) {
        validate_account(account)?;
    }
    Ok(())
}

/// Starts the chat's connection. The task ends the chat however the
/// connection ends.
fn launch(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    mut queue: mpsc::UnboundedReceiver<ChatCommand>,
    begin: Begin,
) {
    let ending = chat.clone();
    let owner = core.clone();
    let task = tokio::spawn(async move {
        if ending.unstarted() {
            ending.wanted.notified().await;
            super::activity_log::chat_launched(&owner, &ending.launch);
            ending.keep(kept::Progress::STARTING);
        }
        // The agent can be started again under the chat, on another account
        // or after a sign-in, and the chat carries on through it.
        let mut begin = begin;
        let mut first = ending.current_launch();
        if super::launching::own_home(&owner, &mut first).await {
            ending.set_current(first);
        }
        let result = loop {
            let launch = ending.current_launch();
            let ran = native::run(ending.clone(), launch, &mut queue, begin).await;
            if !matches!(ran, Ok(rebind::Outcome::Released)) {
                ending.feed.agent_gone();
            }
            match ran {
                Ok(rebind::Outcome::Rebind(next)) => {
                    ending.set_current(next.launch.clone());
                    begin = Begin::Start {
                        rebind: Some(next),
                        notice: None,
                    };
                }
                Ok(rebind::Outcome::Released) => {
                    ending.keeper.leave();
                    return;
                }
                Ok(rebind::Outcome::Ended(end)) => break Ok(end),
                Err(error) => break Err(error),
            }
        };
        ending.keeper.forget();
        ending.feed.close();
        ending.report_end(&owner, &result);
        let started = ending.is_ready();
        ending.emit(
            ChatEventKind::Status,
            crate::acp::ended_status(&result, started),
        );
        if let Err(error) = &result {
            ending.emit(
                ChatEventKind::Error,
                serde_json::json!({ "message": error }),
            );
        }
        ending.end(result.err());
        if owner.chats.remove_if_current(&ending) {
            owner.note_agent_gone(ending.agent_id());
        }
        ending.cancel_permissions();
        ending.publish("stopped");
    });
    let mut slot = chat.task.locked();
    *slot = Some(task);
}

/// Starts a chat agent, with `subscriber` hearing it from its first event.
/// The chat is known to the core when this returns, so a stop asked for
/// next finds it. With `waits_for_person`, its agent starts only once the
/// person writes in the chat or something is sent to it.
pub(crate) fn begin(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    subscriber: Option<&Arc<ClientConn>>,
    launcher: Option<String>,
    waits_for_person: bool,
) -> CoreResult<Arc<Chat>> {
    let origin = Origin {
        started_by: subscriber.and_then(|client| client.peer.device_id()),
        launcher,
    };
    start(core, launch_spec, origin, subscriber, waits_for_person)
}

/// Starts a chat agent another agent asked for, with `lead` as who started it.
pub(crate) fn begin_worker(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    lead: &str,
    launcher: Option<String>,
) -> CoreResult<Arc<Chat>> {
    let origin = Origin {
        started_by: Some(lead.to_owned()),
        launcher,
    };
    start(core, launch_spec, origin, None, false)
}

fn start(
    core: &Arc<Core>,
    mut launch_spec: ChatLaunch,
    origin: Origin,
    subscriber: Option<&Arc<ClientConn>>,
    waits_for_person: bool,
) -> CoreResult<Arc<Chat>> {
    super::launching::own_tools(core, &mut launch_spec);
    rebind::name_accounts(core, &mut launch_spec);
    for (key, value) in super::plugins::agent_environment(core, &launch_spec.cwd) {
        launch_spec.env.entry(key).or_insert(value);
    }
    validate(&launch_spec)?;
    let resume_id = launch_spec.resume_id.clone();
    let waking = core.workspaces.agent(&launch_spec.agent_id).is_some();
    let (chat, queue) = core.chats.insert(core, launch_spec, origin, None)?;
    core.workspaces.note_begun(&chat);
    super::model_providers::chat_begun(core, &chat.launch);
    let naming = (
        core.clone(),
        chat.agent_id().to_owned(),
        chat.launch.cwd.clone(),
        chat.provider().to_owned(),
    );
    tokio::task::spawn_blocking(move || {
        let (core, agent_id, cwd, provider) = naming;
        core.name_agent(&agent_id, &cwd, Some(&provider), resume_id.as_deref());
    });
    if !waking {
        core.seen.active(chat.agent_id());
    }
    if !waits_for_person {
        super::activity_log::chat_launched(core, &chat.launch);
    }
    chat.unstarted.store(waits_for_person, Ordering::Release);
    chat.keep(kept::Progress::STARTING);
    if let Some(client) = subscriber {
        chat.feed.subscribe(client);
    }
    if waits_for_person {
        chat.feed.wait_for_person();
    }
    launch(
        core,
        &chat,
        queue,
        Begin::Start {
            rebind: None,
            notice: None,
        },
    );
    if waits_for_person {
        native::show_saved(&chat);
        offer_options(core, &chat);
    }
    Ok(chat)
}

/// Answers once the chat's session is ready, or says why it never got there.
pub(crate) async fn until_started(core: &Arc<Core>, chat: Arc<Chat>) -> CoreResult<ChatStart> {
    match tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await {
        Ok(result) => result.map_err(CoreError::from),
        Err(_) => {
            if core.chats.remove_if_current(&chat) {
                core.note_agent_gone(chat.agent_id());
            }
            Chats::halt(&chat);
            Err("ACP adapter did not become ready within 150 seconds".into())
        }
    }
}

/// Starts again a chat an earlier core handed over. Its events wait in its
/// replay for a client to attach.
pub(crate) fn resume(core: &Arc<Core>, record: ChatRecord) {
    let agent_id = record.launch.agent_id.clone();
    if let Err(error) = validate(&record.launch) {
        eprintln!("sikemux core: chat {agent_id} was not resumed after the update: {error}");
        return;
    }
    let notice = record.notice.as_deref().map(update_notice);
    match core.chats.insert(core, record.launch, record.origin, None) {
        Ok((chat, queue)) => {
            if record.unstarted {
                chat.feed.wait_for_person();
                chat.unstarted.store(true, Ordering::Release);
            }
            chat.keep(kept::Progress::STARTING);
            launch(
                core,
                &chat,
                queue,
                Begin::Start {
                    rebind: None,
                    notice,
                },
            );
            if chat.unstarted() {
                native::show_saved(&chat);
                offer_options(core, &chat);
            }
        }
        Err(error) => {
            eprintln!("sikemux core: chat {agent_id} was not resumed after the update: {error}")
        }
    }
}

/// What an agent whose turn was cut short is sent to carry it on, `what`
/// saying what cut it short.
pub(crate) fn carry_on_prompt(what: &str) -> rebind::Replay {
    rebind::Replay {
        announce: false,
        message_id: None,
        text: format!("{what}, so your last turn stopped part-way. Check what got done, since a step that was cut off may not have finished, then carry on where you left off."),
        paths: Vec::new(),
        context: Vec::new(),
    }
}

pub(crate) fn restart_notice(title: &str, description: &str) -> Value {
    serde_json::json!({
        "sessionUpdate": "notice",
        "severity": "warning",
        "title": title,
        "description": description,
    })
}

const RESTARTED: &str = "Sikemux restarted while this agent was working";

/// Takes up again every chat an earlier core was running when it stopped,
/// but for those this core already runs. Each is listed at once, and starts
/// on its saved session in the background.
pub(crate) fn restore(core: &Arc<Core>) {
    let Some(dir) = core.chats.kept.clone() else {
        return;
    };
    for kept in kept::read(&dir) {
        let agent_id = kept.launch.agent_id.clone();
        match core.chats.get(&agent_id) {
            Some(chat) if chat.feed.id() == kept.feed => {}
            Some(_) => kept::forget(&dir, &kept.feed),
            None if !kept.begun() => {
                if let Some(agent) = kept.agent {
                    agent.stop();
                }
                kept::forget(&dir, &kept.feed);
            }
            None => {
                if let Err(error) = take_up(core, &dir, kept) {
                    eprintln!("sikemux core: chat {agent_id} was not taken up again: {error}");
                }
            }
        }
    }
}

fn take_up(core: &Arc<Core>, dir: &std::path::Path, kept: KeptChat) -> CoreResult<()> {
    if let Err(error) = validate(&kept.launch) {
        kept::forget(dir, &kept.feed);
        return Err(error);
    }
    if let Some(agent) = kept.agent {
        agent.stop();
    }
    let launch_spec = &kept.launch;
    let feed = Feed::recovering(
        launch_spec.agent_id.clone(),
        launch_spec.permission_mode.clone(),
        core.chats.history.as_deref(),
        &kept.feed,
    );
    kept::moved(dir, &kept, feed.id());
    let saved = launch_spec.resume_id.is_some();
    let flags = Flags {
        running: false,
        unprompted: false,
        turned: saved,
        held: false,
        answered: saved,
        approving: crate::acp::approves_for_user(&launch_spec.permission_mode),
    };
    let (chat, queue) = core.chats.insert(
        core,
        launch_spec.clone(),
        kept.origin.clone(),
        Some((feed, flags)),
    )?;
    if kept.unstarted {
        chat.feed.wait_for_person();
        chat.unstarted.store(true, Ordering::Release);
    }
    let core = core.clone();
    tokio::spawn(async move {
        let reading = (core.clone(), chat.clone());
        let read = tokio::task::spawn_blocking(move || {
            let (core, chat) = reading;
            if let Some(history) = core.chats.history.as_deref() {
                chat.feed.recover(history);
            }
            kept.unsealed(&core).map(|launch| (launch, kept))
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|read| read);
        match read {
            Ok((launch_spec, _)) if chat.unstarted() => {
                chat.set_current(launch_spec);
                launch(
                    &core,
                    &chat,
                    queue,
                    Begin::Start {
                        rebind: None,
                        notice: None,
                    },
                );
            }
            Ok((launch_spec, kept)) => carry_on(&core, &chat, queue, launch_spec, &kept),
            Err(error) => {
                chat.end(Some(error.clone()));
                chat.feed
                    .emit(ChatEventKind::Error, rebind::error_message(&error));
                Chats::halt(&chat);
                if core.chats.remove_if_current(&chat) {
                    core.note_agent_gone(chat.agent_id());
                }
            }
        }
    });
    Ok(())
}

/// Starts a chat taken up after a restart on its saved session, ending the
/// turn the restart cut short and asking the agent to carry it on.
fn carry_on(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    queue: mpsc::UnboundedReceiver<ChatCommand>,
    mut launch_spec: ChatLaunch,
    kept: &KeptChat,
) {
    // The kept record is written after the feed, so it may still say a turn is at work
    // whose end the feed has on disk.
    let working = kept.working && !chat.feed.settled();
    chat.feed.agent_gone();
    chat.end_cut_turn();
    let resumes = kept.reloadable && launch_spec.resume_id.is_some();
    let (replay, notice) = match (working, resumes) {
        (true, true) => (
            Some(carry_on_prompt("Sikemux restarted while you were working")),
            Some(restart_notice(
                RESTARTED,
                "It was asked to check what got done and carry on.",
            )),
        ),
        (true, false) => (
            kept.prompt.clone(),
            Some(restart_notice(
                RESTARTED,
                "This agent cannot load its saved conversation, so it started a new one on your last message.",
            )),
        ),
        (false, _) => (kept.offline.clone(), None),
    };
    if !kept.reloadable {
        launch_spec.resume_id = None;
    }
    chat.set_current(launch_spec.clone());
    launch(
        core,
        chat,
        queue,
        Begin::Start {
            rebind: Some(Box::new(rebind::Rebind {
                launch: launch_spec,
                replay,
                exhausted: Vec::new(),
                signed_in_again: false,
                notice,
            })),
            notice: None,
        },
    );
}

/// Takes up a chat an earlier core handed over whole, its agent still
/// running on `pipes`. Answers with why it could not be.
pub(crate) fn adopt(
    core: &Arc<Core>,
    live: LiveChat,
    pipes: [native::Pipe; 3],
    subscribers: &[Arc<ClientConn>],
) -> Result<(), String> {
    validate(&live.current).map_err(|error| error.to_string())?;
    let feed = Feed::restored(live.launch.agent_id.clone(), live.feed);
    let (chat, queue) = core
        .chats
        .insert(core, live.launch, live.origin, Some((feed, live.flags)))
        .map_err(|error| error.to_string())?;
    chat.set_current(live.current);
    if let Some(version) = &live.cli_version {
        chat.note_cli_version(version);
    }
    for client in subscribers {
        chat.feed.subscribe(client);
    }
    if chat.turn_running() {
        core.seen.started(chat.agent_id());
    }
    if !live.asked.is_empty() {
        core.seen.wants_a_look(chat.agent_id());
    }
    launch(
        core,
        &chat,
        queue,
        Begin::Adopt(Box::new(native::Adopted {
            session: live.session,
            backend: live.backend,
            agent: live.agent,
            pipes,
            asked: live.asked,
        })),
    );
    Ok(())
}

/// The person began writing in a new chat, or a sleeping one, so its agent starts while
/// they write.
pub(crate) fn composing(core: &Arc<Core>, agent_id: &str) {
    if let Some(chat) = core.chats.get(agent_id) {
        chat.start_agent();
    } else if stopped(core, agent_id) {
        let (core, agent_id) = (core.clone(), agent_id.to_owned());
        tokio::spawn(async move {
            if let Err(error) = super::launching::wake_chat(&core, agent_id).await {
                eprintln!("sikemux core: a chat the person began writing in did not wake: {error}");
            }
        });
    }
}

/// Answers once the running chat is ready, or says why it never got there. A new chat
/// still waiting for the person answers at once.
pub(crate) async fn until_ready(core: &Core, agent_id: &str) -> CoreResult<()> {
    let chat = core.chats.running(agent_id)?;
    if chat.unstarted() {
        return Ok(());
    }
    chat.until_ready().await.map(drop).map_err(CoreError::from)
}

pub(crate) async fn attach(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    agent_id: &str,
    since: Option<ChatMark>,
) {
    let missing = || {
        client.respond(
            request_id,
            Ok(Response::ChatAttached {
                attachment: ChatAttachment::Missing,
            }),
        )
    };
    // A sleeping chat is read from the feed it kept, or else taken up to wait for the person,
    // its saved history shown when the core can read it; neither starts its agent.
    let chat = match core.chats.get(agent_id) {
        Some(chat) => chat,
        None => {
            if let Ok(feed) = core.chats.readable(agent_id) {
                let standing = Standing {
                    running: false,
                    turned: false,
                };
                feed.attach(client, request_id, standing, since);
                return;
            }
            let in_terminal = core
                .workspaces
                .agent(agent_id)
                .is_some_and(|agent| agent.terminal);
            let taken_up = stopped(core, agent_id)
                && !in_terminal
                && super::launching::start_saved(core, agent_id, true)
                    .await
                    .is_ok();
            match core.chats.get(agent_id).filter(|_| taken_up) {
                Some(chat) => chat,
                None => {
                    missing();
                    return;
                }
            }
        }
    };
    // A chat still starting, such as one resumed after an update, is taken up
    // once it is ready. One that never gets there is gone. A new chat waiting
    // for the person is taken up at once, and its start follows as changes.
    if !chat.unstarted() {
        let ready = tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await;
        if !matches!(ready, Ok(Ok(_))) {
            missing();
            return;
        }
    }
    chat.feed.attach(
        client,
        request_id,
        Standing {
            running: chat.turn_running(),
            turned: chat.turned.load(Ordering::Acquire),
        },
        since,
    );
}

pub(crate) fn history(
    core: &Core,
    agent_id: &str,
    feed: &str,
    before: u64,
    turns: u32,
) -> CoreResult<Response> {
    let page = core
        .chats
        .readable(agent_id)?
        .history(feed, before, turns as usize)?;
    Ok(Response::ChatHistory {
        page: serde_json::to_value(page).map_err(|error| CoreError::from(error.to_string()))?,
    })
}

/// The rows of a chat a client pages to: before or after an event, or around the row holding
/// one, whole and with their meta, under the window's size.
pub(crate) fn page(
    core: &Core,
    params: super::calls::ChatPageParams,
) -> CoreResult<sikemux_wire::chat::ChatPage> {
    use feed::Anchor;
    let anchor = match (params.before, params.after, params.around) {
        (Some(before), None, None) => Anchor::Before(before),
        (None, Some(after), None) => Anchor::After(after),
        (None, None, Some(around)) => Anchor::Around(around),
        _ => return Err("name one of before, after or around".into()),
    };
    let turns = params
        .turns
        .map_or(sikemux_fold::TAIL_TURNS, |turns| turns as usize);
    Ok(core.chats.readable(&params.agent_id)?.page(
        &params.feed,
        anchor,
        turns,
        params.session.as_deref(),
    )?)
}

/// One step of `client`'s find through a chat: what it found, and where to go on from.
pub(crate) fn find(
    core: &Core,
    client: ClientId,
    params: find::FindParams,
) -> CoreResult<find::FindAnswer> {
    Ok(core
        .chats
        .readable(&params.agent_id)?
        .find(client, &params)?)
}

/// The change a chat's tool call made and what it printed, for a client that
/// opened the call's row.
pub(crate) fn tool_detail(
    core: &Core,
    agent_id: &str,
    tool_call_id: &str,
) -> CoreResult<sikemux_wire::chat::ToolDetail> {
    core.chats
        .running(agent_id)?
        .feed
        .tool_detail(tool_call_id)
        .ok_or_else(|| CoreError::from("that call left nothing to show"))
}

pub(crate) fn detach(core: &Core, client: ClientId, agent_id: &str) {
    if let Some(chat) = core.chats.get(agent_id) {
        chat.feed.unsubscribe(client);
    }
    let resting = core.chats.resting.locked();
    if let Some(resting) = resting.iter().find(|resting| resting.agent_id == agent_id) {
        resting.feed.unsubscribe(client);
    }
}

/// Whether `agent_id` is an agent of the workspace whose chat isn't running, asleep or
/// stopped, so a message to it must start it first on its saved conversation.
pub(crate) fn stopped(core: &Core, agent_id: &str) -> bool {
    core.chats.get(agent_id).is_none() && core.workspaces.agent(agent_id).is_some()
}

/// Readies `agent_id`'s chat for a message from any client: starts it when it isn't
/// running, and opens again the earlier chats `context` names.
pub(crate) async fn wake_for_message(
    core: &Arc<Core>,
    agent_id: &str,
    context: &mut [ChatContext],
) -> CoreResult<()> {
    if stopped(core, agent_id) {
        super::launching::wake_chat(core, agent_id.to_owned()).await?;
    }
    mentions::wake(core, agent_id, context).await
}

pub(crate) fn prompt(
    core: &Core,
    agent_id: &str,
    message_id: Option<String>,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<()> {
    // A sent message is edited later by its id, so one the client did not name gets one here.
    let message_id = match message_id {
        Some(message_id) => {
            bounded_text("message id", &message_id, 256)?;
            message_id
        }
        None => uuid::Uuid::now_v7().to_string(),
    };
    mentions::fill(core, agent_id, &mut context);
    super::messages::worked(core, agent_id);
    let words = context.first().map(|context| context.title.as_str());
    let title = feed::title(if text.trim().is_empty() {
        words.unwrap_or_default()
    } else {
        &text
    });
    super::workspace::retitle(core, agent_id, title, true);
    core.chats.running(agent_id)?.send(ChatCommand::Prompt {
        message_id: Some(message_id),
        text,
        paths,
        context,
    })
}

pub(crate) async fn edit(
    core: &Core,
    agent_id: &str,
    message_id: String,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<()> {
    bounded_text("message id", &message_id, 256)?;
    mentions::fill(core, agent_id, &mut context);
    let (reply, answer) = oneshot::channel();
    core.chats.running(agent_id)?.send(ChatCommand::Edit {
        message_id,
        text,
        paths,
        context,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub(crate) async fn steer(
    core: &Core,
    agent_id: &str,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<String> {
    mentions::fill(core, agent_id, &mut context);
    let (reply, answer) = oneshot::channel();
    core.chats.running(agent_id)?.send(ChatCommand::Steer {
        text,
        paths,
        context,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub(crate) async fn letter(core: &Core, agent_id: &str, text: String) -> CoreResult<Delivery> {
    let (reply, answer) = oneshot::channel();
    core.chats
        .running(agent_id)?
        .send(ChatCommand::Letter { text, reply })?;
    answer.await.map_err(|_| CoreError::from(STOPPED))
}

/// Moves the agent's chat to `account`, a turn at work carrying on there. An
/// agent with no chat running starts on it next time.
pub(crate) fn switch_account(core: &Core, agent_id: &str, account: ChatAccount) -> CoreResult<()> {
    validate_account(&account)?;
    if let Some(chat) = core.chats.get(agent_id) {
        if !chat.unstarted() {
            return chat.send(ChatCommand::SwitchAccount { account });
        }
        // A chat waiting for the person starts on the account chosen, as on a model chosen.
        chat.set_current(rebind::on_account(
            chat.provider(),
            &chat.current_launch(),
            &account,
        ));
    }
    let patch = serde_json::Map::from_iter([("profileId".to_owned(), Value::from(account.id))]);
    core.workspaces.update_agent(agent_id, patch)?;
    core.view_changed();
    Ok(())
}

/// Stops the running turn. The messages waiting behind it go back to the
/// person rather than out.
pub(crate) fn cancel(core: &Core, agent_id: &str) -> CoreResult<()> {
    let chat = core.chats.running(agent_id)?;
    chat.feed.clear_queue();
    chat.send(ChatCommand::Cancel)
}

pub(crate) fn stop_task(core: &Core, agent_id: &str, task_id: String) -> CoreResult<()> {
    bounded_text("task id", &task_id, 256)?;
    core.chats
        .running(agent_id)?
        .send(ChatCommand::StopTask { task_id })
}

pub(crate) fn reply_permission(
    core: &Core,
    agent_id: &str,
    request_id: &str,
    option_id: Option<String>,
) -> CoreResult<()> {
    core.chats
        .get(agent_id)
        .ok_or("ACP permission request is no longer pending")?
        .reply_permission(request_id, option_id)
}

pub(crate) async fn set_permission_mode(
    core: &Core,
    agent_id: &str,
    mode: String,
) -> CoreResult<()> {
    let mode = PermissionMode::parse(&mode)?;
    let chat = core.chats.running(agent_id)?;
    let (reply, answer) = oneshot::channel();
    chat.send(ChatCommand::SetPermissionMode { mode, reply })?;
    chat.feed.set_changing_mode(true);
    let answered = tokio::time::timeout(REPLY_TIMEOUT, answer).await;
    chat.feed.set_changing_mode(false);
    answered
        .map_err(|_| CoreError::from("Permission update timed out"))?
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

/// Asks `agent_id`'s agent to sign in by `method_id`, one of the methods it
/// offered that it handles itself.
pub(crate) async fn authenticate(core: &Core, agent_id: &str, method_id: String) -> CoreResult<()> {
    bounded_text("sign-in method", &method_id, 256)?;
    let (reply, answer) = oneshot::channel();
    core.chats
        .running(agent_id)?
        .send(ChatCommand::Authenticate { method_id, reply })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub(crate) async fn set_config(
    core: &Core,
    agent_id: &str,
    config_id: String,
    value: String,
) -> CoreResult<Value> {
    bounded_text("config id", &config_id, 256)?;
    bounded_text("config value", &value, 4096)?;
    let chat = core.chats.running(agent_id)?;
    if chat.unstarted() {
        return choose_before_start(core, &chat, &config_id, value);
    }
    let (reply, answer) = oneshot::channel();
    chat.send(ChatCommand::SetConfig {
        config_id,
        value,
        reply,
    })?;
    let answer = answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)?;
    keep_model_choice(core, agent_id, &answer["configOptions"]);
    Ok(answer)
}

/// The options a chat that waits for the person offers: those its launcher's last session
/// offered, on the model and effort it will start with.
fn waiting_options(core: &Core, chat: &Chat) -> Option<Value> {
    let mut options = core
        .workspaces
        .launcher_options(chat.origin.launcher.as_deref()?)
        .filter(|options| options.as_array().is_some_and(|rows| !rows.is_empty()))?;
    let launch = chat.current_launch();
    for row in options.as_array_mut().into_iter().flatten() {
        let chosen = if row["id"] == "model" {
            launch.model.clone()
        } else if row["category"] == "thought_level" {
            launch.effort.clone()
        } else {
            None
        };
        if let Some(chosen) = chosen {
            row["currentValue"] = Value::String(chosen);
        }
    }
    Some(options)
}

/// Tells a chat that waits for the person what it offers to pick. A launcher that has never
/// offered anything has its agent's CLI asked once, with no session; after that only its own
/// sessions change what it offers.
fn offer_options(core: &Arc<Core>, chat: &Arc<Chat>) {
    let Some(launcher) = chat.origin.launcher.clone() else {
        return;
    };
    if core.workspaces.launcher_options(&launcher).is_some() {
        if let Some(options) = waiting_options(core, chat) {
            tell_options(chat, options);
        }
        return;
    }
    // Noted as offering nothing until the CLI answers, so no other chat asks it again.
    core.workspaces
        .note_config_options(&launcher, &Value::Array(Vec::new()));
    let (core, chat) = (core.clone(), chat.clone());
    tokio::spawn(async move {
        match native::read_options(&chat.current_launch()).await {
            Ok(options) => core.workspaces.note_config_options(&launcher, &options),
            Err(error) => {
                return eprintln!("sikemux core: {launcher} offered nothing to pick: {error}")
            }
        }
        if let Some(options) = waiting_options(&core, &chat).filter(|_| chat.unstarted()) {
            tell_options(&chat, options);
        }
    });
}

fn tell_options(chat: &Chat, options: Value) {
    let session = chat
        .launch
        .resume_id
        .clone()
        .unwrap_or_else(|| chat.agent_id().to_owned());
    chat.emit(
        ChatEventKind::SessionUpdate,
        serde_json::json!({
            "sessionId": session,
            "update": { "sessionUpdate": "config_option_update", "configOptions": options },
        }),
    );
}

/// Keeps a model or effort chosen before the chat's agent starts, which starts on it.
fn choose_before_start(
    core: &Core,
    chat: &Chat,
    config_id: &str,
    value: String,
) -> CoreResult<Value> {
    let options = waiting_options(core, chat).unwrap_or(Value::Null);
    let row = options
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == config_id);
    let mut launch = chat.current_launch();
    match row {
        Some(row) if row["id"] == "model" => launch.model = Some(value),
        Some(row) if row["category"] == "thought_level" => launch.effort = Some(value),
        _ => return Err(format!("{config_id} is chosen once the chat starts").into()),
    }
    chat.set_current(launch);
    let options = waiting_options(core, chat).unwrap_or(Value::Null);
    keep_model_choice(core, chat.agent_id(), &options);
    tell_options(chat, options.clone());
    Ok(serde_json::json!({ "configOptions": options }))
}

/// Keeps the model and effort a chat now runs on with its agent, so its next
/// session starts on them. An effort its CLI does not take is left out.
fn keep_model_choice(core: &Core, agent_id: &str, options: &Value) {
    let Some(agent) = core.workspaces.agent(agent_id) else {
        return;
    };
    let (model, effort) = crate::acp::pickers::pickers(options);
    let mut patch = serde_json::Map::new();
    if let Some(model) = model.filter(|model| !model.current.is_empty()) {
        patch.insert("model".into(), Value::String(model.current));
    }
    let takes = |effort: &str| {
        crate::agent_catalog::agent(&agent.provider)
            .is_some_and(|entry| entry.efforts.contains(&effort))
    };
    if let Some(effort) = effort.filter(|effort| takes(&effort.current)) {
        patch.insert("effort".into(), Value::String(effort.current));
    }
    if patch.is_empty() {
        return;
    }
    match core.workspaces.update_agent(agent_id, patch) {
        Ok(_) => core.view_changed(),
        Err(error) => eprintln!("sikemux core: the agent's model choice was not kept: {error}"),
    }
}
