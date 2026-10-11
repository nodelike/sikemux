//! Chat agents: processes the core starts and speaks the Agent Client
//! Protocol with, so a turn keeps going while the window is closed and a
//! permission request waits for the person to come back.

mod connection;
pub(crate) mod feed;
mod history;
mod native;
mod rebind;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::acp::{bounded_text, current_choice, native as acp_native};
use crate::protocol::{
    Attention, AttentionKind, ChatAccount, ChatAttachment, ChatContext, ChatEventKind, ChatInfo,
    ChatLaunch, ChatMark, ChatStart, ChatState, Event, RequestId, Response,
};

use super::connection::{ClientConn, ClientId};
use super::notify::Signal;
use super::remote::unix_ms;
use super::{Core, CoreError, CoreResult};
use feed::{Feed, Standing};

const MAX_AGENT_ID: usize = 200;
const START_TIMEOUT: Duration = Duration::from_secs(150);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const NOT_RUNNING: &str = "ACP session is not running";
const STOPPED: &str = "ACP session stopped";
/// How long a stop waits for an agent's process to be gone.
const STOP_SETTLE: Duration = Duration::from_secs(2);

pub(crate) enum ChatCommand {
    Prompt {
        from: ClientId,
        /// Names the message for a later edit of it.
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
    SetPermissionMode {
        mode: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetConfig {
        config_id: String,
        value: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Steer {
        from: ClientId,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        reply: oneshot::Sender<Result<String, String>>,
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
        from: ClientId,
        message_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        /// Puts the files back as they were before that message too.
        restore_files: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Cancel,
}

#[derive(Clone)]
enum Readiness {
    Starting,
    Ready(ChatStart),
    Ended(Option<String>),
}

struct PendingPermission {
    option_ids: Vec<String>,
    /// Takes the option chosen, or none when the request was cancelled.
    answer: oneshot::Sender<Option<String>>,
    request: Value,
    at: u64,
}

/// Who started a chat, and with which of the app's launchers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// The agent is working on a turn nobody here prompted.
    pub unprompted: AtomicBool,
    turned: AtomicBool,
    /// A turn of this chat was answered, so its provider has it saved.
    pub answered: AtomicBool,
    approving: AtomicBool,
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
        self.current
            .lock()
            .map(|launch| launch.clone())
            .unwrap_or_else(|_| self.launch.clone())
    }

    fn set_current(&self, launch: ChatLaunch) {
        if let Ok(mut current) = self.current.lock() {
            *current = launch;
        }
    }

    pub(crate) fn emit(&self, kind: ChatEventKind, payload: Value) {
        if kind == ChatEventKind::TurnStarted {
            self.turned.store(true, Ordering::Release);
        }
        if let Some(core) = self.core.upgrade() {
            let agent_id = self.agent_id().to_owned();
            match kind {
                ChatEventKind::TurnStarted => {
                    core.seen.working(&agent_id);
                    core.notify.send(Signal::TurnStarted { agent_id });
                }
                ChatEventKind::TurnCompleted => {
                    core.seen.wants_a_look(&agent_id);
                    let cancelled =
                        payload.get("stopReason").and_then(Value::as_str) == Some("cancelled");
                    core.notify.send(Signal::TurnCompleted {
                        agent_id,
                        cancelled,
                    });
                }
                ChatEventKind::Error => {
                    let message = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    core.notify.send(Signal::ChatError { agent_id, message });
                }
                _ => {}
            }
        }
        self.feed.emit(kind, payload);
    }

    pub(crate) fn turn_running(&self) -> bool {
        self.running.load(Ordering::Acquire) || self.unprompted.load(Ordering::Acquire)
    }

    pub(crate) fn approves(&self) -> bool {
        self.approving.load(Ordering::Acquire)
    }

    pub(crate) fn set_permission_mode(&self, mode: &str) {
        self.approving
            .store(crate::acp::approves_for_user(mode), Ordering::Release);
        self.feed.set_permission_mode(mode);
    }

    pub(crate) fn mark_ready(&self, start: ChatStart) {
        if let Some(core) = self.core.upgrade() {
            core.workspaces
                .note_config_options(self.provider(), &start.setup);
        }
        self.feed.set_start(start.clone());
        self.readiness.send_replace(Readiness::Ready(start));
    }

    fn is_ready(&self) -> bool {
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
        answer: oneshot::Sender<Option<String>>,
        request: Value,
    ) -> bool {
        let Ok(mut permissions) = self.permissions.lock() else {
            let _ = answer.send(None);
            return false;
        };
        let pending = PendingPermission {
            option_ids,
            answer,
            request,
            at: unix_ms(),
        };
        let attention = self.attention(&request_id, &pending);
        permissions.insert(request_id, pending);
        drop(permissions);
        if let Some(core) = self.core.upgrade() {
            core.seen.wants_a_look(self.agent_id());
        }
        self.announce(&Event::Attention { attention });
        true
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
        }
    }

    fn attentions(&self) -> Vec<Attention> {
        self.permissions
            .lock()
            .map(|permissions| {
                permissions
                    .iter()
                    .map(|(request_id, pending)| self.attention(request_id, pending))
                    .collect()
            })
            .unwrap_or_default()
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
    }

    pub(crate) fn cancel_permissions(&self) {
        let pending: Vec<_> = match self.permissions.lock() {
            Ok(mut permissions) => permissions.drain().collect(),
            Err(_) => return,
        };
        for (request_id, request) in pending {
            self.feed.forget_permission(&request_id);
            self.cleared(&request_id);
            let _ = request.answer.send(None);
        }
    }

    fn pending_permissions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .permissions
            .lock()
            .map(|permissions| permissions.keys().cloned().collect())
            .unwrap_or_default();
        ids.sort();
        ids
    }

    fn reply_permission(&self, request_id: &str, option_id: Option<String>) -> CoreResult<()> {
        let request = {
            let mut permissions = self
                .permissions
                .lock()
                .map_err(|_| "ACP permission state is unavailable")?;
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
        self.feed.forget_permission(request_id);
        self.cleared(request_id);
        request
            .answer
            .send(option_id)
            .map_err(|_| CoreError::from(STOPPED))
    }

    fn send(&self, command: ChatCommand) -> CoreResult<()> {
        self.commands.send(command).map_err(|_| STOPPED.into())
    }

    pub(crate) fn info(&self) -> ChatInfo {
        let start = self.feed.start();
        ChatInfo {
            agent_id: self.launch.agent_id.clone(),
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
            started_by: self.origin.started_by.clone(),
            launcher: self.origin.launcher.clone(),
            permission_mode: self.feed.permission_mode(),
            model: self.launch.model.clone(),
            effort: self.launch.effort.clone(),
            asleep: false,
            unread: false,
            active_at: None,
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
        })
    }
}

/// A chat handed to a core that replaces this one, which starts it again on
/// the same provider session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatRecord {
    pub launch: ChatLaunch,
    #[serde(flatten)]
    pub origin: Origin,
}

#[derive(Default)]
pub(crate) struct Chats {
    chats: Mutex<HashMap<String, Arc<Chat>>>,
    next_generation: AtomicU64,
    /// Where each chat keeps everything it said, for phones to page back.
    history: Option<PathBuf>,
}

impl Chats {
    pub(crate) fn new(history: Option<PathBuf>) -> Self {
        if let Some(dir) = history.as_deref() {
            history::clear(dir);
        }
        Self {
            history,
            ..Self::default()
        }
    }

    pub(crate) fn get(&self, agent_id: &str) -> Option<Arc<Chat>> {
        self.chats.lock().ok()?.get(agent_id).cloned()
    }

    /// The chats a client has taken up and hears the events of.
    pub(crate) fn followed_by(&self, client: ClientId) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|chat| chat.feed.has_subscriber(client))
            .map(|chat| chat.agent_id().to_owned())
            .collect()
    }

    fn all(&self) -> Vec<Arc<Chat>> {
        self.chats
            .lock()
            .map(|chats| chats.values().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn running(&self, agent_id: &str) -> CoreResult<Arc<Chat>> {
        self.get(agent_id).ok_or_else(|| NOT_RUNNING.into())
    }

    pub(crate) fn count(&self) -> usize {
        self.chats.lock().map(|chats| chats.len()).unwrap_or(0)
    }

    pub(crate) fn any_turn_running(&self) -> bool {
        self.all().iter().any(|chat| chat.turn_running())
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
    }

    fn remove_if_current(&self, chat: &Chat) {
        if let Ok(mut chats) = self.chats.lock() {
            if chats
                .get(chat.agent_id())
                .is_some_and(|current| current.generation == chat.generation)
            {
                chats.remove(chat.agent_id());
            }
        }
    }

    fn take(&self, agent_id: &str) -> Option<Arc<Chat>> {
        self.chats.lock().ok()?.remove(agent_id)
    }

    fn take_all(&self) -> Vec<Arc<Chat>> {
        self.chats
            .lock()
            .map(|mut chats| chats.drain().map(|(_, chat)| chat).collect())
            .unwrap_or_default()
    }

    /// Ends the chat at once, killing the agent and everything it started.
    /// Returns the task that ends with it.
    fn halt(chat: &Chat) -> Option<tokio::task::JoinHandle<()>> {
        chat.feed.close();
        chat.cancel_permissions();
        chat.end(None);
        let task = chat.task.lock().ok()?.take()?;
        task.abort();
        Some(task)
    }

    pub(crate) fn stop(&self, agent_id: &str) {
        if let Some(chat) = self.take(agent_id) {
            Self::halt(&chat);
        }
    }

    pub(crate) fn stop_all(&self) {
        for chat in self.take_all() {
            Self::halt(&chat);
        }
    }

    /// Stops every chat, waiting a moment for their processes to go, and says
    /// how to start again the ones a provider keeps.
    pub(crate) async fn hand_over(&self) -> Vec<ChatRecord> {
        let chats = self.take_all();
        let records = chats.iter().filter_map(|chat| chat.resumable()).collect();
        let tasks: Vec<_> = chats.iter().filter_map(|chat| Self::halt(chat)).collect();
        let _ = tokio::time::timeout(STOP_SETTLE, async {
            for task in tasks {
                let _ = task.await;
            }
        })
        .await;
        records
    }

    fn insert(
        &self,
        core: &Arc<Core>,
        launch: ChatLaunch,
        origin: Origin,
    ) -> CoreResult<(Arc<Chat>, mpsc::UnboundedReceiver<ChatCommand>)> {
        let mut chats = self.chats.lock().map_err(CoreError::poisoned)?;
        if chats.contains_key(&launch.agent_id) {
            return Err("ACP session is already running".into());
        }
        let (commands, queue) = mpsc::unbounded_channel();
        let chat = Arc::new(Chat {
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            commands,
            task: Mutex::new(None),
            feed: Feed::new(
                launch.agent_id.clone(),
                launch.permission_mode.clone(),
                self.history.as_deref(),
            ),
            permissions: Mutex::new(HashMap::new()),
            readiness: watch::channel(Readiness::Starting).0,
            running: AtomicBool::new(false),
            unprompted: AtomicBool::new(false),
            turned: AtomicBool::new(false),
            answered: AtomicBool::new(false),
            approving: AtomicBool::new(crate::acp::approves_for_user(&launch.permission_mode)),
            current: Mutex::new(launch.clone()),
            launch,
            origin,
            core: Arc::downgrade(core),
        });
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
        permission_mode: "default".into(),
        model: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };
    let (chat, _) = core
        .chats
        .insert(core, launch, Origin::default())
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
    if !launch.cwd.is_absolute() {
        return Err("ACP working directory must be absolute".into());
    }
    if let Some(resume_id) = launch.resume_id.as_deref() {
        bounded_text("session id", resume_id, 4_096)?;
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
fn launch(core: &Arc<Core>, chat: &Arc<Chat>, mut queue: mpsc::UnboundedReceiver<ChatCommand>) {
    let ending = chat.clone();
    let owner = core.clone();
    let task = tokio::spawn(async move {
        // The agent can be started again under the chat, on another account
        // or after a sign-in, and the chat carries on through it.
        let mut rebind = None;
        let result = loop {
            let launch = ending.current_launch();
            let ran = match native::Kind::of(ending.provider()) {
                Some(kind) => {
                    native::run(kind, ending.clone(), launch, &mut queue, rebind.take()).await
                }
                None => connection::run(ending.clone(), launch, &mut queue, rebind.take()).await,
            };
            match ran {
                Ok(rebind::Outcome::Rebind(next)) => {
                    ending.set_current(next.launch.clone());
                    rebind = Some(next);
                }
                Ok(rebind::Outcome::Ended(end)) => break Ok(end),
                Err(error) => break Err(error),
            }
        };
        ending.feed.close();
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
        owner.chats.remove_if_current(&ending);
        ending.cancel_permissions();
    });
    if let Ok(mut slot) = chat.task.lock() {
        *slot = Some(task);
    }
}

/// Starts a chat agent, with `subscriber` hearing it from its first event.
/// The chat is known to the core when this returns, so a stop asked for
/// next finds it.
pub(crate) fn begin(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    subscriber: Option<&Arc<ClientConn>>,
    launcher: Option<String>,
) -> CoreResult<Arc<Chat>> {
    validate(&launch_spec)?;
    let origin = Origin {
        started_by: subscriber.and_then(|client| client.peer.device_id()),
        launcher,
    };
    let (chat, queue) = core.chats.insert(core, launch_spec, origin)?;
    core.seen.opened(chat.agent_id());
    if let Some(client) = subscriber {
        chat.feed.subscribe(client);
    }
    launch(core, &chat, queue);
    Ok(chat)
}

/// Answers once the chat's session is ready, or says why it never got there.
pub(crate) async fn until_started(core: &Arc<Core>, chat: Arc<Chat>) -> CoreResult<ChatStart> {
    match tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await {
        Ok(result) => result.map_err(CoreError::from),
        Err(_) => {
            core.chats.remove_if_current(&chat);
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
    match core.chats.insert(core, record.launch, record.origin) {
        Ok((chat, queue)) => launch(core, &chat, queue),
        Err(error) => {
            eprintln!("sikemux core: chat {agent_id} was not resumed after the update: {error}")
        }
    }
}

/// Answers once the running chat is ready, or says why it never got there.
pub(crate) async fn until_ready(core: &Core, agent_id: &str) -> CoreResult<()> {
    let chat = core.chats.running(agent_id)?;
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
    let Some(chat) = core.chats.get(agent_id) else {
        missing();
        return;
    };
    // A chat still starting, such as one resumed after an update, is taken up
    // once it is ready. One that never gets there is gone.
    let ready = tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await;
    if !matches!(ready, Ok(Ok(_))) {
        missing();
        return;
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
        .running(agent_id)?
        .feed
        .history(feed, before, turns as usize)?;
    Ok(Response::ChatHistory {
        events: page.events,
        older_before: page.older_before,
    })
}

pub(crate) fn detach(core: &Core, client: ClientId, agent_id: &str) {
    if let Some(chat) = core.chats.get(agent_id) {
        chat.feed.unsubscribe(client);
    }
}

pub(crate) fn prompt(
    core: &Core,
    from: ClientId,
    agent_id: &str,
    message_id: Option<String>,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> CoreResult<()> {
    if let Some(message_id) = message_id.as_deref() {
        bounded_text("message id", message_id, 256)?;
    }
    core.chats.running(agent_id)?.send(ChatCommand::Prompt {
        from,
        message_id,
        text,
        paths,
        context,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn edit(
    core: &Core,
    from: ClientId,
    agent_id: &str,
    message_id: String,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
    restore_files: bool,
) -> CoreResult<()> {
    bounded_text("message id", &message_id, 256)?;
    let (reply, answer) = oneshot::channel();
    core.chats.running(agent_id)?.send(ChatCommand::Edit {
        from,
        message_id,
        text,
        paths,
        context,
        restore_files,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub(crate) async fn steer(
    core: &Core,
    from: ClientId,
    agent_id: &str,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> CoreResult<String> {
    let (reply, answer) = oneshot::channel();
    core.chats.running(agent_id)?.send(ChatCommand::Steer {
        from,
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

pub(crate) fn switch_account(core: &Core, agent_id: &str, account: ChatAccount) -> CoreResult<()> {
    validate_account(&account)?;
    core.chats
        .running(agent_id)?
        .send(ChatCommand::SwitchAccount { account })
}

pub(crate) fn cancel(core: &Core, agent_id: &str) -> CoreResult<()> {
    core.chats.running(agent_id)?.send(ChatCommand::Cancel)
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
    let (reply, answer) = oneshot::channel();
    core.chats
        .running(agent_id)?
        .send(ChatCommand::SetPermissionMode { mode, reply })?;
    tokio::time::timeout(REPLY_TIMEOUT, answer)
        .await
        .map_err(|_| CoreError::from("Permission update timed out"))?
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
    let (reply, answer) = oneshot::channel();
    core.chats.running(agent_id)?.send(ChatCommand::SetConfig {
        config_id,
        value,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}
