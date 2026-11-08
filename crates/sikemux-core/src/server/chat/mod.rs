//! Chat agents: processes the core starts and speaks each one's own protocol
//! with, so a turn keeps going while the window is closed and a permission
//! request waits for the person to come back.

mod actions;
pub mod calls;
pub(crate) mod feed;
pub mod find;
mod history;
mod kept;
pub(crate) mod mentions;
mod native;
mod offers;
pub(crate) mod permission;
pub mod prompt;
pub mod queue;
mod rebind;
mod registry;
pub(crate) mod reset;
mod start;
mod states;
mod turn;
mod usage;

use crate::server::changed::What;
use sikemux_fold::update::{notice, turn_answer};
use sikemux_fold::UpdateKind;
use sikemux_view::chat::StopReason;
use sikemux_view::questions::QuestionChoice;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::ErrorKind;
use tokio::sync::{mpsc, oneshot, watch};

use crate::acp::pickers::{self, current_choice};
use crate::launch::PermissionMode;
pub(crate) use native::Steered;
use native::{ended_status, SessionEnd};
use prompt::bounded_text;
use sikemux_wire::protocol::{
    ask, AgentState, AgentStatus, Attention, AttentionKind, ChatAccount, ChatAttachment,
    ChatContext, ChatInfo, ChatLaunch, ChatMark, ChatStart, ChatState, Event, RequestId,
};

use super::activity::Activity;
use super::connection::{ClientConn, ClientId};
use super::inbox::Fault;
use super::spawn::Ending;
use super::{Core, CoreError, CoreResult};
use feed::{Feed, FeedState, Standing};
use kept::{Keeper, KeptAgent, KeptChat};
pub(crate) use native::kill_and_reap;
use native::Begin;
pub(crate) use sikemux_fold::ChatEventKind;
use sikemux_process::clock::epoch_ms;
use turn::{Turn, TurnState};

pub(crate) use actions::*;
pub(crate) use offers::{models_of, AgentModelsParams};
pub(crate) use registry::Chats;
pub(crate) use start::*;

const MAX_AGENT_ID: usize = 200;
const START_TIMEOUT: Duration = Duration::from_secs(150);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const NOT_RUNNING: &str = "This chat is not running";
const STOPPED: &str = "This chat has stopped";

/// How long a stop waits for an agent's process to be gone.
const STOP_SETTLE: Duration = Duration::from_secs(2);
/// How long a stopped turn may keep running before the agent is killed. The
/// agent only reads a stop between steps, and a wedged tool never gets there.
const CANCEL_GRACE: Duration = Duration::from_secs(10);
/// How long after its last subagent ends a chat waits for the agent to take
/// up the subagent's report before it counts as finished.
const SUBAGENT_WAKE_GRACE: Duration = Duration::from_secs(3);
/// How long a turn that ended waits for the subagents it started.
const SUBAGENT_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a chat has to let go of its agent for an update before it is
/// stopped and started again on its session instead.
const RELEASE_WAIT: Duration = Duration::from_secs(5);
/// An agent that stops by itself is started again, after a wait that doubles
/// each time, until it has stopped this often within the window.
const RESTART_WAIT: Duration = Duration::from_secs(1);
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);

pub(crate) enum ChatCommand {
    /// The person answered a dialog the agent's process is blocked on, by typing
    /// `answer` or by picking `choice`.
    AnswerDialog {
        id: String,
        answer: String,
        choice: Option<usize>,
    },
    Prompt {
        /// Names the message for a later edit of it.
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        /// Its row was put in the chat as it was sent, while the agent was still starting.
        shown: bool,
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
        reply: oneshot::Sender<Result<native::Steered, String>>,
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
    turn: Mutex<TurnState>,
    /// Wakes whatever waits for the running turn to end.
    turn_over: tokio::sync::Notify,
    /// The chat's loop waits for its next command, so it can let go of its
    /// agent at once.
    pub at_rest: AtomicBool,
    /// The chat is letting go of its agent, so the person's answers wait for
    /// the core that takes it over.
    releasing: AtomicBool,
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
    published: Mutex<AgentState>,
    /// A new chat starts its agent only once the person writes in it or something is sent to it.
    unstarted: AtomicBool,
    wanted: tokio::sync::Notify,
    /// The saved history of the session it resumes is shown already, so the agent's own
    /// replay of it as it starts stays quiet.
    pub(crate) saved_shown: AtomicBool,
    /// The saved history is still being read into the chat.
    pub(crate) saved_reading: watch::Sender<bool>,
    /// What Sikemux keeps for the project, which a chat the person started
    /// is handed once: with its agent's instructions, or else with its first message.
    brief: Mutex<Option<String>>,
}

impl Chat {
    pub(crate) fn agent_id(&self) -> &str {
        &self.launch.agent_id
    }

    /// What Sikemux keeps for the project, as the chat began.
    pub(crate) fn brief(&self) -> Option<String> {
        self.brief.locked().clone()
    }

    /// The chat's brief the first time it is asked for, for its first message.
    pub(crate) fn take_brief(&self) -> Option<String> {
        self.brief.locked().take()
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

    /// Tells the feed something that neither starts nor ends a turn: those go through
    /// [`Self::begin_turn`], [`Self::finish_turn`], [`Self::fail_turn`] and [`Self::error`].
    pub(crate) fn emit(&self, kind: ChatEventKind, payload: Value) {
        if kind != ChatEventKind::SessionUpdate {
            self.feed.emit(kind, payload);
            self.publish_state();
            return;
        }
        match UpdateKind::within(&payload) {
            Some(UpdateKind::ToolCall) => self.note_tool_call(&payload),
            // A saved session read back says the same outside any turn.
            Some(UpdateKind::SubagentSpawned)
                if self.turn_running() && self.feed.starts_own_subagent(&payload) =>
            {
                if let Some(core) = self.core.upgrade() {
                    super::board::chat_goal::subagent_started(&core, self.agent_id());
                }
            }
            Some(UpdateKind::SessionInfoUpdate | UpdateKind::AccountSwitched) => {
                self.note_session(&payload)
            }
            _ => {}
        }
        self.feed.emit(kind, payload);
        self.finish_held();
    }

    /// A turn starts, whoever prompted it.
    pub(crate) fn begin_turn(&self) {
        {
            let mut state = self.turn.locked();
            state.turned = true;
            state.unreported = true;
            state.failure = None;
            if state.turn == Turn::Held {
                state.turn = Turn::Idle;
            }
        }
        if let Some(core) = self.core.upgrade() {
            core.note_activity(self.agent_id(), Activity::Started);
        }
        self.note_resumable();
        self.feed.emit(ChatEventKind::TurnStarted, json!({}));
        self.publish_state();
    }

    /// The turn at work ended with `answer`. The feed hears under the lock the turn goes idle
    /// under, so whoever reads the chat as idle finds in its transcript why it stopped.
    pub(crate) fn finish_turn(&self, answer: Value) {
        self.finish(answer, |_| true);
    }

    fn finish(&self, answer: Value, ends: impl FnOnce(Turn) -> bool) {
        let cancelled = sikemux_fold::update::stop_reason(&answer) == Some(StopReason::Cancelled);
        let holds = !cancelled && self.feed.running_subagents() > 0;
        let new_hold = {
            let mut state = self.turn.locked();
            if !ends(state.turn) {
                return;
            }
            self.feed.emit(ChatEventKind::TurnCompleted, answer);
            let was_held = state.turn == Turn::Held;
            state.turn = if holds { Turn::Held } else { Turn::Idle };
            if holds && !was_held {
                state.holds += 1;
            }
            (holds && !was_held).then_some(state.holds)
        };
        self.turn_over.notify_waiters();
        if let Some(hold) = new_hold {
            self.end_held_after(SUBAGENT_WAIT, hold, true);
        }
        if holds {
            if let Some(core) = self.core.upgrade() {
                core.changed(What::Agents);
            }
            self.publish_state();
            return;
        }
        let ending = if cancelled {
            Ending::Stopped
        } else {
            match self.take_failure() {
                Some(failure) => Ending::Failed(failure),
                None => Ending::Finished,
            }
        };
        self.tell_of_ending(Activity::Finished { cancelled }, Some(ending));
    }

    /// `failure` ended the turn at work, with no answer to it, in one step as
    /// [`Self::finish_turn`] does.
    pub(crate) fn fail_turn(&self, failure: Value) {
        self.tell_failure(failure, |state, feed, failure| {
            feed.emit(ChatEventKind::Error, failure);
            if state.running() {
                state.turn = Turn::Idle;
            }
        });
        self.turn_over.notify_waiters();
    }

    /// Says `failure`, which leaves whatever is at work going on.
    pub(crate) fn error(&self, failure: Value) {
        self.tell_failure(failure, |state, feed, mut failure| {
            if state.running() {
                failure[sikemux_fold::update::TURN_GOES_ON] = json!(true);
            }
            feed.emit(ChatEventKind::Error, failure);
        });
    }

    /// The chat ends on `failure`, and the turn at work with it, which the agent's lead hears
    /// as how the chat ended.
    pub(crate) fn fail_for_good(&self, failure: Value) {
        self.tell_failure(failure, |_, feed, failure| {
            feed.emit(ChatEventKind::Error, failure);
        });
    }

    fn tell_failure(&self, failure: Value, tell: impl FnOnce(&mut TurnState, &Arc<Feed>, Value)) {
        let message = failure
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        // An error that arrives once nothing is at work on the turn any more ended it.
        let ending = {
            let mut state = self.turn.locked();
            tell(&mut state, &self.feed, failure);
            (state.turn == Turn::Idle && state.unreported).then(|| Ending::Failed(message.clone()))
        };
        self.tell_of_ending(Activity::Problem(&message), ending);
    }

    /// What follows a turn's end or a failure: the core hears of it, then the lead of `ending`
    /// when there is one, and messages waiting for the turn to end go in unless it was stopped.
    fn tell_of_ending(&self, activity: Activity<'_>, ending: Option<Ending>) {
        let carries_on = ending
            .as_ref()
            .is_some_and(|ending| !matches!(ending, Ending::Stopped));
        if let Some(core) = self.core.upgrade() {
            if matches!(ending, Some(Ending::Finished)) {
                super::questions::asked_in_reply(&core, self.agent_id());
            }
            core.note_activity(self.agent_id(), activity);
            if let Some(ending) = ending {
                self.report_turn(&core, ending);
            }
        }
        self.publish_state();
        if carries_on {
            self.send_queued_later();
        }
    }

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
        let _ = core.workspaces.edit_agent(self.agent_id(), |agent| {
            agent.resume_id = Some(start.session_id)
        });
    }

    /// Keeps on the agent's record what the session says of it: the title the
    /// agent gave it, and the account it moved to.
    fn note_session(&self, payload: &Value) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        if let Some(account) = payload.pointer("/update/account").and_then(Value::as_str) {
            let _ = core.workspaces.edit_agent(self.agent_id(), |agent| {
                agent.profile_id = Some(account.to_owned())
            });
        }
        let title = feed::session_title(payload);
        super::workspace::retitle(&core, self.agent_id(), title, false);
    }

    /// What the chat is doing, in the words every app shows an agent by.
    pub(crate) fn state_now(&self) -> AgentState {
        if !self.permissions.locked().is_empty() {
            AgentState::Blocked
        } else if self.working() || self.feed.running_subagents() > 0 {
            AgentState::Working
        } else if self.feed.start().is_some() {
            AgentState::Idle
        } else {
            AgentState::Unknown
        }
    }

    /// Tells every app what the chat is doing whenever that changes, so none
    /// has to work it out from the transcript a pane happens to hold.
    pub(crate) fn publish_state(&self) {
        self.publish(self.state_now());
    }

    fn publish(&self, state: AgentState) {
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

    fn note_tool_call(&self, payload: &Value) {
        let update = &payload["update"];
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
        let hold = {
            let state = self.turn.locked();
            if state.turn != Turn::Held {
                return;
            }
            state.holds
        };
        if self.feed.running_subagents() == 0 {
            self.end_held_after(SUBAGENT_WAKE_GRACE, hold, false);
        }
    }

    /// Ends held turn `hold` as finished after `wait`, should it still be held then.
    /// It waits for its subagents unless `giving_up`, which ends them as failed.
    fn end_held_after(&self, wait: Duration, hold: u64, giving_up: bool) {
        let core = self.core.clone();
        let agent_id = self.agent_id().to_owned();
        let generation = self.generation;
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let Some(core) = core.upgrade() else { return };
            let Some(chat) = core.chats.get(&agent_id) else {
                return;
            };
            if chat.generation != generation || !giving_up && chat.feed.running_subagents() > 0 {
                return;
            }
            {
                let mut state = chat.turn.locked();
                if state.turn != Turn::Held || state.holds != hold {
                    return;
                }
                state.turn = Turn::Idle;
            }
            if giving_up {
                chat.feed.give_up_subagents();
            }
            chat.tell_of_ending(
                Activity::Finished { cancelled: false },
                Some(Ending::Finished),
            );
        });
    }

    /// The turn in progress failed, which the lead hears in place of its end,
    /// or of the chat's when that comes first, and the inbox lists as `fault`.
    pub(crate) fn turn_failing(&self, failure: &Value, fault: Fault) {
        let message = failure
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(core) = self.core.upgrade() {
            core.inbox.failed(self.agent_id(), message, fault);
        }
        self.turn.locked().failure = Some(message.to_owned());
    }

    fn take_failure(&self) -> Option<String> {
        self.turn.locked().failure.take()
    }

    /// Takes whether the lead has yet to hear how the turn that started went.
    fn take_unreported(&self) -> bool {
        std::mem::take(&mut self.turn.locked().unreported)
    }

    /// Tells the agent's lead how its turn ended, once per turn.
    fn report_turn(&self, core: &Arc<Core>, ending: Ending) {
        if self.take_unreported() {
            core.note_ending(self.agent_id(), ending);
        }
    }

    /// Tells the agent's lead the chat has ended, when that cut a turn short
    /// or the agent went away by itself after it had started.
    fn report_end(&self, core: &Arc<Core>, result: &Result<SessionEnd, String>) {
        let mid_turn = self.take_unreported();
        let went_away = self.is_ready() && !matches!(result, Ok(SessionEnd::Requested));
        if !mid_turn && !went_away {
            return;
        }
        let asked = matches!(result, Ok(SessionEnd::Requested));
        let reason = match result {
            Err(error) => Some(error.clone()),
            Ok(SessionEnd::Requested) => None,
            Ok(_) => self.take_failure(),
        };
        core.note_ending(
            self.agent_id(),
            Ending::Gone {
                reason,
                mid_turn,
                asked,
            },
        );
    }

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

    /// The agent was cut off in the turn at work. It ends in the transcript, so the one that
    /// carries it on starts clean, and its lead hears only of that one.
    pub(crate) fn cut_turn(&self) {
        {
            let mut state = self.turn.locked();
            if self.feed.in_turn() {
                self.feed.emit(
                    ChatEventKind::TurnCompleted,
                    turn_answer(StopReason::Cancelled),
                );
            }
            if state.running() {
                state.turn = Turn::Idle;
            }
        }
        self.turn_over.notify_waiters();
    }

    pub(crate) fn note_agent(&self, pid: u32) {
        let mut agent = self.agent.locked();
        *agent = KeptAgent::of(pid);
    }

    /// Writes down where the chat stands, with how its loop says its turns go.
    pub(crate) fn keep(&self, progress: kept::Progress<'_>) {
        let (running, working) = {
            let state = self.turn.locked();
            (state.turn == Turn::Prompted, state.turn != Turn::Idle)
        };
        self.keeper.keep(KeptChat {
            launch: self.current_now(),
            from_shell: Vec::new(),
            keys: Vec::new(),
            origin: self.origin.clone(),
            feed: self.feed.id().to_owned(),
            agent: *self.agent.locked(),
            working,
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

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    pub(crate) fn work_started(&self) {
        let starts = {
            let mut state = self.turn.locked();
            let starts = matches!(state.turn, Turn::Idle | Turn::Held);
            if starts {
                state.turn = Turn::Unprompted;
            }
            starts
        };
        if starts {
            self.begin_turn();
        }
    }

    /// The work nobody here asked for is over.
    pub(crate) fn work_ended(&self, stop_reason: StopReason) {
        self.finish(turn_answer(stop_reason), |turn| turn == Turn::Unprompted);
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
            self.error(rebind::error_message(error));
        }
        if self.prompted() {
            stall_later(turn, stalled);
        } else {
            self.work_ended(StopReason::Cancelled);
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

    pub(crate) fn ask_dialog(
        &self,
        dialog: String,
        header: &str,
        text: &str,
        choices: Vec<QuestionChoice>,
    ) -> Result<(), String> {
        let core = self.core.upgrade().ok_or("Sikemux is shutting down")?;
        super::questions::ask_for_dialog(&core, self.agent_id(), dialog, header, text, choices)
    }

    pub(crate) fn withdraw_dialogs(&self) {
        if let Some(core) = self.core.upgrade() {
            super::questions::withdraw_dialogs(&core, self.agent_id());
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
        let working =
            self.working() || self.feed.running_subagents() > 0 || self.feed.running_tasks() > 0;
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

    pub(crate) async fn turn_ended(&self) {
        loop {
            let over = self.turn_over.notified();
            if !self.prompted() {
                return;
            }
            over.await;
        }
    }

    pub(crate) fn prompted(&self) -> bool {
        self.turn.locked().turn == Turn::Prompted
    }

    /// A prompt this side sent is being answered from now. Answers whether one already was.
    pub(crate) fn start_prompted(&self) -> bool {
        let mut state = self.turn.locked();
        let was = state.turn == Turn::Prompted;
        state.turn = Turn::Prompted;
        was
    }

    pub(crate) fn unprompted(&self) -> bool {
        self.turn.locked().turn == Turn::Unprompted
    }

    /// The turn nobody here prompted is over. Answers whether one was running.
    pub(crate) fn end_unprompted(&self) -> bool {
        let mut state = self.turn.locked();
        let was = state.turn == Turn::Unprompted;
        if was {
            state.turn = Turn::Idle;
        }
        was
    }

    pub(crate) fn turn_running(&self) -> bool {
        self.turn.locked().running()
    }

    /// Whether the agent is at work on a turn, or on subagents a turn that ended left going.
    pub(crate) fn working(&self) -> bool {
        self.turn.locked().turn != Turn::Idle
    }

    /// A turn of this chat was answered, so its provider has it saved.
    pub(crate) fn note_answered(&self) {
        self.turn.locked().answered = true;
    }

    /// Every agent still asks before some actions in its most open mode, such as
    /// Claude Code for an `rm -rf` of a computed path, and some agents have no
    /// such mode at all, so under YOLO the host says yes for the person.
    pub(crate) fn approves(&self) -> bool {
        self.feed.permission_mode() == PermissionMode::Bypass
    }

    pub(crate) fn set_permission_mode(&self, mode: PermissionMode) {
        self.feed.set_permission_mode(mode);
        if let Some(core) = self.core.upgrade() {
            core.changed(What::View);
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
            core.changed(What::Agents);
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
            _ => Err("This chat stopped before it was ready".into()),
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
            core.changed(What::Event(event));
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

    pub(crate) fn pending_permissions(&self) -> Vec<String> {
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
                return Err(permission_gone());
            };
            if let Some(option_id) = &option_id {
                if !request.option_ids.contains(option_id) {
                    return Err("That permission request does not offer this answer".into());
                }
            }
            permissions.remove(request_id).ok_or_else(permission_gone)?
        };
        (request.answer)(option_id);
        drop(permissions);
        self.feed.forget_permission(request_id);
        self.cleared(request_id);
        Ok(())
    }

    pub(crate) fn send(&self, command: ChatCommand) -> CoreResult<()> {
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

    pub(crate) fn unstarted(&self) -> bool {
        self.unstarted.load(Ordering::Acquire)
    }

    /// Whether the agent's first start brings no history the chat doesn't show already.
    pub(crate) fn history_in_place(&self) -> bool {
        self.launch.resume_id.is_none()
            || (self.saved_shown.load(Ordering::Acquire) && !*self.saved_reading.borrow())
    }

    pub(crate) async fn until_saved_read(&self) {
        let _ = self
            .saved_reading
            .subscribe()
            .wait_for(|reading| !reading)
            .await;
    }

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
            model_provider: self.launch.model_provider.clone(),
            effort: self.launch.effort.clone(),
            unread: false,
            active_at: None,
            group: Default::default(),
            status: match self.state_now() {
                AgentState::Working => AgentStatus::Working,
                _ => AgentStatus::default(),
            },
            next_trigger: None,
            called_at: None,
            attention_rank: 0,
        }
    }

    /// Whether the provider has a saved session to load: one a turn answered,
    /// or one the chat was resumed from.
    pub(crate) fn has_answered(&self) -> bool {
        self.turn.locked().answered || self.launch.resume_id.is_some()
    }

    fn kept_by_provider(&self) -> bool {
        self.turn.locked().turned || self.launch.resume_id.is_some()
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
        let effort_id = pickers::effort_config_id(setup).map(str::to_owned);
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
            kill_and_reap(handed.released.pid);
            return Err("its transcript could not be kept".into());
        };
        Ok(HandedChat {
            live: LiveChat {
                record: self.restart_record(),
                launch: self.launch.clone(),
                current: self.current_launch(),
                origin: self.origin.clone(),
                flags: self.turn.locked().clone(),
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
    flags: TurnState,
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

pub(crate) enum HandOver {
    Live(Box<HandedChat>),
    /// Started again on its session, saying why.
    Resume(Box<ChatRecord>),
}

/// Tells `stalled` of turn `turn` once it has had a while to stop.
pub(crate) fn stall_later(turn: u64, stalled: &mpsc::UnboundedSender<u64>) {
    let stalled = stalled.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CANCEL_GRACE).await;
        let _ = stalled.send(turn);
    });
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

pub(super) fn permission_gone() -> CoreError {
    CoreError::of(
        ErrorKind::NotPending,
        "That permission request is no longer pending",
    )
}
