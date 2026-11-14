//! Every chat this core runs, and the feeds of those at rest.

use super::feed::Reach;
use super::*;
use crate::server::chat_state::ChatParts;

/// A chat the core cannot hand over to its replacement now.
#[derive(Clone, Debug)]
pub struct Blocker {
    pub agent_id: String,
    pub provider: String,
    pub title: Option<String>,
    pub reason: String,
}

#[derive(Default)]
pub struct Chats {
    pub chats: Mutex<HashMap<String, Arc<Chat>>>,
    pub next_generation: AtomicU64,
    /// Where each chat keeps everything it said, for phones to page back.
    pub history: Option<PathBuf>,
    /// Where each chat is written down, for a core that starts after this one.
    pub kept: Option<PathBuf>,
    pub resets: reset::LimitResumes,
    pub handoffs: handoff::Handoffs,
    /// The feeds of chats asleep, their history kept on disk for clients to page and find
    /// through without waking anything.
    pub resting: Mutex<Vec<Resting>>,
    /// Every chat asleep or closed that can be opened again, with the history it keeps.
    pub shelf: shelf::Shelf,
    unshelving: Mutex<()>,
}

/// A sleeping chat's feed.
pub struct Resting {
    pub agent_id: String,
    pub feed: Arc<Feed>,
    /// The chat's meter as it fell asleep, taken up again as it wakes.
    pub meter: super::cap::KeptMeter,
    pub claude: bool,
}

/// The most history kept for chats asleep or closed in all; past it, the one read least lately
/// goes.
const SHELF_BYTES: u64 = 2 * 1024 * 1024 * 1024;

impl Chats {
    /// Where each chat keeps everything it said.
    pub fn history_dir(&self) -> Option<&std::path::Path> {
        self.history.as_deref()
    }

    pub fn new(data_dir: Option<&std::path::Path>) -> Self {
        let history = data_dir.map(|dir| dir.join("chat-history"));
        Self {
            shelf: shelf::Shelf::load(history.as_deref()),
            history,
            kept: data_dir.map(|dir| dir.join("chats")),
            resets: reset::LimitResumes::new(data_dir),
            handoffs: handoff::Handoffs::new(data_dir),
            ..Self::default()
        }
    }

    /// Removes the history an earlier core left, but for the chats this one
    /// took over and those asleep or closed that can be opened again.
    pub fn clear_history(&self) {
        let Some(dir) = self.history.as_deref() else {
            return;
        };
        let mut keep: Vec<String> = self
            .all()
            .iter()
            .map(|chat| chat.feed.id().to_owned())
            .collect();
        keep.extend(self.shelf.feeds());
        keep.push("kept".to_owned());
        history::clear(dir, &keep);
    }

    /// Every chat whose last turn the network cut short tries it once more,
    /// now that the network is back.
    pub fn retry_offline_failures(&self) {
        for chat in self.all() {
            if chat.offline_failed.swap(false, Ordering::AcqRel) {
                let _ = chat.send(ChatCommand::RetryFailed);
            }
        }
    }

    pub fn get(&self, agent_id: &str) -> Option<Arc<Chat>> {
        self.chats.locked().get(agent_id).cloned()
    }

    /// The chats a client has taken up and hears the events of.
    pub fn followed_by(&self, client: ClientId) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|chat| chat.feed.has_subscriber(client))
            .map(|chat| chat.agent_id().to_owned())
            .collect()
    }

    pub fn ids(&self) -> Vec<String> {
        self.chats.locked().keys().cloned().collect()
    }

    pub fn all(&self) -> Vec<Arc<Chat>> {
        self.chats.locked().values().cloned().collect()
    }

    pub fn running(&self, agent_id: &str) -> CoreResult<Arc<Chat>> {
        self.get(agent_id).ok_or_else(|| NOT_RUNNING.into())
    }

    /// The feed clients hear the agent's chat on, running or asleep, without counting as a read.
    pub fn heard(&self, agent_id: &str) -> Option<Arc<Feed>> {
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
    pub fn readable(&self, agent_id: &str) -> CoreResult<Arc<Feed>> {
        if let Some(chat) = self.get(agent_id) {
            return Ok(chat.feed.clone());
        }
        let feed = (self.resting.locked().iter())
            .find(|resting| resting.agent_id == agent_id)
            .map(|resting| resting.feed.clone())
            .ok_or_else(|| CoreError::from(NOT_RUNNING))?;
        self.shelf.read(agent_id);
        Ok(feed)
    }

    /// Keeps the feed of `chat`, which went to sleep, with only its history on disk.
    pub fn rest(&self, chat: &Chat) {
        self.shelve(chat);
        let mut resting = self.resting.locked();
        resting.retain(|resting| resting.agent_id != chat.agent_id());
        resting.push(Resting {
            agent_id: chat.agent_id().to_owned(),
            feed: chat.feed.clone(),
            meter: chat.meter.kept(),
            claude: chat.provider() == "claude",
        });
    }

    /// Keeps the history of `chat`, which stopped, on disk to show when it is opened again on
    /// its session, letting go of the history read least lately once all those kept pass their
    /// size. A chat that never had a session keeps nothing.
    fn shelve(&self, chat: &Chat) {
        chat.cut_turn();
        let bytes = chat.feed.rest();
        let launch = chat.current_now();
        let start = chat.feed.start();
        let follows = chat.feed.followed();
        let session = (follows.as_ref().map(|(session, _)| session.clone()))
            .or_else(|| start.as_ref().map(|start| start.session_id.clone()))
            .or_else(|| launch.resume_id.clone());
        let Some(session) = session else {
            return;
        };
        let account = crate::agent_catalog::AgentKind::from_name(&launch.provider)
            .and_then(|kind| crate::sessions::roots::account_root(kind, None, &launch.env));
        let mut shelved = shelf::Shelved {
            feed: chat.feed.id().to_owned(),
            reach: follows
                .filter(|(followed, _)| *followed == session)
                .map(|(_, reach)| reach),
            session,
            provider: launch.provider.clone(),
            cwd: launch.cwd.clone(),
            account,
            permission_mode: chat.feed.permission_mode(),
            claude: chat.provider() == "claude",
            start,
            transcript: None,
            running: false,
            bytes,
            read_at: epoch_ms(),
        };
        shelved.transcript = shelved.find_transcript();
        // A turn cut short is in the history already, past where the reach last stood.
        if let Some(reach) = whole_record(&shelved) {
            chat.feed.reached(&shelved.session, reach.clone());
            shelved.reach = Some(reach);
        }
        chat.feed.outlive_chat();
        let feed = shelved.feed.clone();
        let before = self.shelf.put(chat.agent_id(), shelved);
        if let (Some(dir), Some(before)) = (self.history.as_deref(), before) {
            if before.feed != feed {
                let _ = std::fs::remove_file(dir.join(format!("{}.jsonl", before.feed)));
            }
        }
        while let Some(oldest) = self.shelf.over(SHELF_BYTES) {
            self.forget(&oldest);
        }
    }

    /// Takes up the history an earlier core kept for the agent's chat, asleep or closed, to
    /// read while it sleeps and to wake on, when it follows `session` if one is named.
    /// Answers the feed when it was taken up just now. Reads the disk, so it blocks.
    pub fn unshelve(&self, agent_id: &str, session: Option<&str>) -> Option<Arc<Feed>> {
        let _one_at_a_time = self.unshelving.locked();
        if self
            .resting
            .locked()
            .iter()
            .any(|resting| resting.agent_id == agent_id)
        {
            return None;
        }
        let (Some(dir), Some(mut shelved)) = (self.history.as_deref(), self.shelf.get(agent_id))
        else {
            return None;
        };
        if session.is_some_and(|session| session != shelved.session) {
            return None;
        }
        if !dir.join(format!("{}.jsonl", shelved.feed)).is_file() {
            self.shelf.take(agent_id);
            return None;
        }
        let feed = Feed::kept(
            agent_id.to_owned(),
            shelved.permission_mode,
            dir,
            &shelved.feed,
        );
        if let Some(start) = shelved.start.clone() {
            feed.set_start(start);
        }
        // A chat that ran when its core stopped wrote on past its reach, as far as its record.
        if shelved.running {
            shelved.reach = whole_record(&shelved).or(shelved.reach);
            shelved.running = false;
            self.shelf.put(agent_id, shelved.clone());
        }
        if let Some(reach) = shelved.reach.clone() {
            feed.reached(&shelved.session, reach);
        }
        feed.rest();
        feed.outlive_chat();
        self.resting.locked().push(Resting {
            agent_id: agent_id.to_owned(),
            claude: shelved.claude,
            feed: feed.clone(),
            meter: Default::default(),
        });
        Some(feed)
    }

    /// The feeds chats kept while they sleep, which clients still read, by agent.
    pub fn resting_feeds(&self) -> Vec<(String, Arc<Feed>)> {
        (self.resting.locked().iter())
            .map(|resting| (resting.agent_id.clone(), resting.feed.clone()))
            .collect()
    }

    /// The feed the agent's chat kept while it slept, no longer kept apart, when it follows
    /// `session` if one is named. One that follows another session is let go of here, and
    /// stays on disk.
    pub fn take_rest(
        &self,
        agent_id: &str,
        session: Option<&str>,
    ) -> Option<(Arc<Feed>, super::cap::KeptMeter)> {
        let mut resting = self.resting.locked();
        let at = resting
            .iter()
            .position(|resting| resting.agent_id == agent_id)?;
        let rested = resting.remove(at);
        let other = (self.shelf.get(agent_id))
            .is_some_and(|shelved| session.is_some_and(|session| session != shelved.session));
        if other {
            return None;
        }
        self.shelf.woke(agent_id);
        rested.feed.woke();
        Some((rested.feed, rested.meter))
    }

    /// The sleeping chats' feeds, each with its meter and whether it is Claude's.
    pub fn resting_meters(&self) -> Vec<(Arc<Feed>, super::cap::KeptMeter, bool)> {
        (self.resting.locked().iter())
            .map(|resting| (resting.feed.clone(), resting.meter, resting.claude))
            .collect()
    }

    /// The person's switch for a sleeping chat whose cache expired.
    pub fn resting_keeps_history(
        &self,
        agent_id: &str,
        keep: bool,
    ) -> Option<(Arc<Feed>, super::cap::KeptMeter, bool)> {
        let mut resting = self.resting.locked();
        let rested = resting
            .iter_mut()
            .find(|resting| resting.agent_id == agent_id)?;
        rested.meter.keep_history(keep);
        Some((rested.feed.clone(), rested.meter, rested.claude))
    }

    /// Lets go of what a chat that cannot be opened again kept, its history file with it.
    pub fn forget(&self, agent_id: &str) {
        self.resting
            .locked()
            .retain(|resting| resting.agent_id != agent_id);
        let (Some(dir), Some(shelved)) = (self.history.as_deref(), self.shelf.take(agent_id))
        else {
            return;
        };
        if self.get(agent_id).is_none() {
            let _ = std::fs::remove_file(dir.join(format!("{}.jsonl", shelved.feed)));
        }
    }

    /// Lets go of the kept history of each chat closed or asleep whose agent no longer keeps
    /// its session, so it is gone from Recent, and that `kept` does not still hold.
    pub fn forget_gone(&self, kept: impl Fn(&str) -> bool) {
        for agent_id in self.shelf.gone_from_recent() {
            if !kept(&agent_id) {
                self.forget(&agent_id);
            }
        }
    }

    pub fn count(&self) -> usize {
        self.chats.locked().len()
    }

    /// Whether every chat can let go of its agent now.
    pub fn all_handable(&self) -> bool {
        self.all().iter().all(|chat| chat.handable())
    }

    /// How many chats are in a turn, waiting on the person or unable to hand
    /// their agent over, which an update nobody asked for waits on.
    pub fn in_turn(&self) -> usize {
        self.all()
            .iter()
            .filter(|chat| !chat.handable() || chat.state_now().busy())
            .count()
    }

    /// The chats at work that an update now would stop and start again
    /// rather than carry over.
    pub fn blockers(&self) -> Vec<Blocker> {
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

    pub fn attentions(&self) -> Vec<Attention> {
        let mut attentions: Vec<Attention> = self
            .all()
            .iter()
            .flat_map(|chat| chat.attentions())
            .collect();
        attentions.sort_by_key(|attention| attention.at);
        attentions
    }

    pub fn list(&self) -> Vec<ChatInfo> {
        let mut chats: Vec<ChatInfo> = self.all().iter().map(|chat| chat.info()).collect();
        chats.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        chats
    }

    pub fn forget_client(&self, client: ClientId) {
        for chat in self.all() {
            chat.feed.unsubscribe(client);
        }
        for resting in self.resting.locked().iter() {
            resting.feed.unsubscribe(client);
        }
    }

    /// Whether `chat` was the agent's current one, which this removes.
    pub fn remove_if_current(&self, chat: &Chat) -> bool {
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
    pub fn halt(chat: &Chat) -> Option<tokio::task::JoinHandle<()>> {
        chat.keeper.forget();
        chat.feed.close();
        chat.cancel_permissions();
        chat.end(None);
        let task = chat.task.locked().take()?;
        task.abort();
        Some(task)
    }

    /// Ends the agent's chat, which the person closed, keeping its history to show when it
    /// is opened again.
    pub fn close(&self, agent_id: &str) {
        if let Some(chat) = self.take(agent_id) {
            Self::halt(&chat);
            self.shelve(&chat);
        }
        self.resting
            .locked()
            .retain(|resting| resting.agent_id != agent_id);
    }

    /// Whether the agent had a chat to stop.
    pub fn stop(&self, agent_id: &str) -> bool {
        let Some(chat) = self.take(agent_id) else {
            return false;
        };
        Self::halt(&chat);
        true
    }

    /// Stops the chat only when nothing in it is running: no turn, question,
    /// subagent or background task. Answers whether it stopped.
    pub fn stop_if_idle(&self, agent_id: &str) -> bool {
        self.get(agent_id).is_none() || (!self.at_work(agent_id) && self.stop(agent_id))
    }

    /// Whether the agent's chat is in a turn or has tasks running.
    pub fn at_work(&self, agent_id: &str) -> bool {
        self.get(agent_id).is_some_and(|chat| {
            chat.state_now().busy() || chat.feed.running_tasks() > 0 || chat.meter.compacting()
        })
    }

    /// Ends every chat's agent, a turn at work ending as stopped, and answers
    /// the chats it ended.
    pub fn stop_all(&self) -> Vec<Arc<Chat>> {
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
    pub async fn hand_over(&self) -> Vec<HandOver> {
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

    pub fn insert(
        &self,
        core: &Arc<Core>,
        launch: ChatLaunch,
        origin: Origin,
        kept: Option<(Arc<Feed>, TurnProgress)>,
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
            .then(|| self.take_rest(&launch.agent_id, launch.resume_id.as_deref()))
            .flatten();
        let history_shown = rested.is_some();
        let rested_meter = rested.as_ref().map(|(_, meter)| *meter);
        let (feed, flags) = kept.unwrap_or_else(|| {
            let feed = rested.map(|(feed, _)| feed).unwrap_or_else(|| {
                Feed::new(
                    launch.agent_id.clone(),
                    launch.permission_mode,
                    self.history.as_deref(),
                )
            });
            (feed, TurnProgress::default())
        });
        let offers_effort = crate::agent_catalog::agent(&launch.provider)
            .and_then(|agent| agent.chat.as_ref())
            .is_some_and(crate::agent_catalog::ChatDriver::offers_effort);
        if offers_effort {
            feed.offer_effort();
        }
        feed.work_in(launch.cwd.clone());
        feed.set_handoff(self.handoffs.shown(&launch.agent_id));
        (core.chat_above().triggers_seed)(core, &feed, &launch.agent_id);
        let chief = core.workspaces().is_chief(&launch.agent_id);
        if chief {
            feed.seat_chief();
            (core.chat_above().chief_chat_opened)(core, &launch.agent_id, &feed);
        }
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
            moving_on: AtomicBool::new(false),
            edits: (core.chat_above().new_edits)(),
            cli_version: Mutex::new(None),
            keeper,
            agent: Mutex::new(None),
            restarts: Mutex::new(Default::default()),
            published: Mutex::new(AgentState::Unknown),
            unstarted: AtomicBool::new(false),
            brief: Mutex::new(None),
            meter: Default::default(),
            capped: AtomicBool::new(chief),
            rotation: Default::default(),
            wanted: tokio::sync::Notify::new(),
            person_came: tokio::sync::Notify::new(),
            saved_shown: AtomicBool::new(history_shown),
            saved_reading: watch::channel(false).0,
            current: Mutex::new(launch.clone()),
            launch,
            origin,
            core: Arc::downgrade(core),
        });
        if let Some(meter) = rested_meter {
            chat.meter.take_up(meter);
        }
        if let Some(kept) = chief
            .then(|| (core.chat_above().chief_kept_session)(core, &chat.launch.agent_id))
            .flatten()
        {
            super::rotation::take_up(&chat, kept);
        }
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
        (core.chat_above().board_wake)(core);
        Ok((chat, queue))
    }
}

impl crate::server::part::Part for Chats {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        self.resets.arm_saved(core);
        vec![super::reset::wake_on_readings(core)]
    }

    fn flush(&self) {
        self.shelf.flush();
    }

    fn client_gone(&self, _core: &Arc<Core>, client: ClientId) {
        self.forget_client(client);
    }
}

/// How far the agent's own record of the session reaches now, for agents read by its length.
fn whole_record(shelved: &shelf::Shelved) -> Option<Reach> {
    if !shelved.claude && !matches!(shelved.reach, Some(Reach::Bytes(_))) {
        return None;
    }
    let length = std::fs::metadata(shelved.transcript.as_deref()?)
        .ok()?
        .len();
    Some(Reach::Bytes(length))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review of #239: an agent that ran two sessions keeps the history of the newer one, and
    /// taking up the older from Recent must not show it.
    #[tokio::test]
    async fn a_kept_history_is_taken_up_only_for_the_session_it_follows() {
        let data = tempfile::tempdir().unwrap();
        let chats = Chats::new(Some(data.path()));
        let history = data.path().join("chat-history");
        std::fs::create_dir_all(&history).unwrap();
        std::fs::write(history.join("newer-feed.jsonl"), b"").unwrap();
        chats.shelf.put(
            "agent",
            shelf::Shelved {
                feed: "newer-feed".into(),
                session: "newer".into(),
                provider: "claude".into(),
                cwd: data.path().to_path_buf(),
                account: None,
                permission_mode: PermissionMode::WorkspaceWrite,
                claude: true,
                start: None,
                reach: None,
                transcript: None,
                running: false,
                bytes: 0,
                read_at: 0,
            },
        );

        assert!(chats.unshelve("agent", Some("older")).is_none());
        assert!(chats.resting_feeds().is_empty());
        assert!(chats.unshelve("agent", None).is_some());
        assert!(chats.take_rest("agent", Some("older")).is_none());
        assert!(chats.shelf.holds("agent"));
        assert!(history.join("newer-feed.jsonl").is_file());
        assert!(chats.unshelve("agent", Some("newer")).is_some());
        assert!(chats.take_rest("agent", Some("newer")).is_some());
    }
}
