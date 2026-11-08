//! Every chat this core runs, and the feeds of those at rest.

use super::*;

/// A chat the core cannot hand over to its replacement now.
#[derive(Clone, Debug)]
pub(crate) struct Blocker {
    pub agent_id: String,
    pub provider: String,
    pub title: Option<String>,
    pub reason: String,
}

#[derive(Default)]
pub(crate) struct Chats {
    pub(super) chats: Mutex<HashMap<String, Arc<Chat>>>,
    pub(super) next_generation: AtomicU64,
    /// Where each chat keeps everything it said, for phones to page back.
    pub(super) history: Option<PathBuf>,
    /// Where each chat is written down, for a core that starts after this one.
    pub(super) kept: Option<PathBuf>,
    pub(crate) resets: reset::Resets,
    /// The feeds of chats asleep, their history kept on disk for clients to page and find
    /// through without waking anything.
    pub(super) resting: Mutex<Vec<Resting>>,
}

/// A sleeping chat's feed, how much history it keeps, and when a client last read it.
pub(super) struct Resting {
    pub(super) agent_id: String,
    pub(super) feed: Arc<Feed>,
    pub(super) bytes: u64,
    pub(super) read: std::time::Instant,
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

    pub(crate) fn all(&self) -> Vec<Arc<Chat>> {
        self.chats.locked().values().cloned().collect()
    }

    pub(crate) fn running(&self, agent_id: &str) -> CoreResult<Arc<Chat>> {
        self.get(agent_id).ok_or_else(|| NOT_RUNNING.into())
    }

    /// The feed clients hear the agent's chat on, running or asleep, without counting as a read.
    pub(crate) fn heard(&self, agent_id: &str) -> Option<Arc<Feed>> {
        if let Some(chat) = self.get(agent_id) {
            return Some(chat.feed.clone());
        }
        let resting = self.resting.locked();
        resting
            .iter()
            .find(|resting| resting.agent_id == agent_id)
            .map(|resting| resting.feed.clone())
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
        chat.cut_turn();
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

    /// The feeds chats kept while they sleep, which clients still read, by agent.
    pub(crate) fn resting_feeds(&self) -> Vec<(String, Arc<Feed>)> {
        (self.resting.locked().iter())
            .map(|resting| (resting.agent_id.clone(), resting.feed.clone()))
            .collect()
    }

    /// The feed the agent's chat kept while it slept, no longer kept apart.
    pub(super) fn take_rest(&self, agent_id: &str) -> Option<Arc<Feed>> {
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
            .filter(|chat| !chat.handable() || chat.state_now().busy())
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
    pub(super) fn remove_if_current(&self, chat: &Chat) -> bool {
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
    pub(super) fn halt(chat: &Chat) -> Option<tokio::task::JoinHandle<()>> {
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
        self.get(agent_id)
            .is_some_and(|chat| chat.state_now().busy() || chat.feed.running_tasks() > 0)
    }

    /// Ends every chat's agent, a turn at work ending as stopped, and answers
    /// the chats it ended.
    pub(crate) fn stop_all(&self) -> Vec<Arc<Chat>> {
        let chats = self.take_all();
        for chat in &chats {
            chat.feed.agent_gone();
            chat.cut_turn();
            Self::halt(chat);
        }
        chats
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

    pub(super) fn insert(
        &self,
        core: &Arc<Core>,
        launch: ChatLaunch,
        origin: Origin,
        kept: Option<(Arc<Feed>, TurnState)>,
    ) -> CoreResult<(Arc<Chat>, mpsc::UnboundedReceiver<ChatCommand>)> {
        let mut chats = self.chats.locked();
        if chats.contains_key(&launch.agent_id) {
            return Err("This chat is already running".into());
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
                    launch.permission_mode,
                    self.history.as_deref(),
                )
            });
            (feed, TurnState::default())
        });
        let offers_effort = crate::agent_catalog::agent(&launch.provider)
            .and_then(|agent| agent.chat.as_ref())
            .is_some_and(crate::agent_catalog::ChatDriver::offers_effort);
        if offers_effort {
            feed.offer_effort();
        }
        feed.work_in(launch.cwd.clone());
        crate::server::triggers::seed(core, &feed, &launch.agent_id);
        let keeper = Keeper::new(self.kept.as_deref(), feed.id());
        let chat = Arc::new(Chat {
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            model_grant: Mutex::default(),
            commands,
            task: Mutex::new(None),
            feed,
            permissions: Mutex::new(HashMap::new()),
            readiness: watch::channel(Readiness::Starting).0,
            turn: Mutex::new(flags),
            turn_over: tokio::sync::Notify::new(),
            at_rest: AtomicBool::new(false),
            releasing: AtomicBool::new(false),
            offline_failed: AtomicBool::new(false),
            edits: Default::default(),
            cli_version: Mutex::new(None),
            keeper,
            agent: Mutex::new(None),
            restarts: Mutex::new(Default::default()),
            published: Mutex::new(AgentState::Unknown),
            unstarted: AtomicBool::new(false),
            brief: Mutex::new(None),
            wanted: tokio::sync::Notify::new(),
            saved_shown: AtomicBool::new(history_shown),
            saved_reading: watch::channel(false).0,
            current: Mutex::new(launch.clone()),
            launch,
            origin,
            core: Arc::downgrade(core),
        });
        let held = {
            let state = chat.turn.locked();
            (state.turn == Turn::Held).then_some(state.holds)
        };
        if let Some(hold) = held {
            chat.end_held_after(SUBAGENT_WAIT, hold, true);
        }
        self.resting
            .locked()
            .retain(|resting| resting.agent_id != chat.agent_id());
        chats.insert(chat.agent_id().to_owned(), chat.clone());
        // A goal that ended while it slept gets its line in the chat now.
        core.boards.wake.now();
        Ok((chat, queue))
    }
}

impl crate::server::part::Part for Chats {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        self.resets.arm_saved(core);
        Vec::new()
    }

    fn client_gone(&self, _core: &Arc<Core>, client: ClientId) {
        self.forget_client(client);
    }
}
