//! What a chat agent says, on its way to the clients that show it. Streamed
//! updates leave in one batch per frame. Everything since the session started
//! or loaded is kept, so a client that attaches later rebuilds the chat from
//! the same events the others watched arrive.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::protocol::{ChatContext, ChatStart, PermissionMode};
use sikemux_process::clock::epoch_ms;
use sikemux_wire::chat::{
    ChatChange, ChatDraft, ChatPage, ChatTrigger, QueuedMessage, SentContext, ToolDetail,
};

use super::super::connection::{ClientConn, ClientId};
use super::history::{History, HistoryState, Page, MAX_HISTORY_BYTES};
use super::queue::{batch_len, Queued, MAX_QUEUED};
use super::states::Subagent;
use sikemux_fold::{ChatEventKind, Place};
use sikemux_fold::{Fold, PageOf, PAGE_BYTES as ROW_BYTES};

mod replay;
mod residency;
mod subscribers;
mod work;

use replay::{is_kept, stamp};
pub(crate) use replay::{session_title, title, Anchor, Excerpt, Replay, Said};
use subscribers::Subscriber;
use work::Work;

/// How long the newest event waits for more text to join it before it is
/// written to the history, so a crash loses at most this much of the chat.
const SETTLE: Duration = Duration::from_secs(1);
pub(crate) const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_REPLAY_EVENTS: usize = 20_000;
const MAX_PAGE_TURNS: usize = 100;
const PAGE_BYTES: u64 = 1024 * 1024;
/// Where a chat taken up on a session it never ran numbers its first event, leaving the
/// numbers below it for what it reads further back.
const EARLIER_ROOM: u64 = 1 << 40;

/// How far into its agent CLI's own record of a session a chat's history reaches, so a resume
/// replays only what the CLI added since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Reach {
    /// The length of a transcript file.
    Bytes(u64),
    /// The latest turn of a thread.
    Turn(String),
}

/// Reads further back into an agent CLI's own record of a session than the chat holds, a part
/// at a time: the updates it tells, oldest first, until nothing earlier is left.
pub(crate) type Earlier = Box<dyn FnMut() -> Result<Option<Vec<(String, Value)>>, String> + Send>;

/// What a client attaching now is told about the chat besides its rows.
pub(crate) struct Standing {
    pub running: bool,
    pub turned: bool,
    pub draft: Option<ChatDraft>,
}

struct Inner {
    subscribers: HashMap<ClientId, Subscriber>,
    replay: Replay,
    /// The chat folded into rows, while a client watches it or a turn runs.
    fold: Option<Fold>,
    /// The agent always offers an effort picker.
    offers_effort: bool,
    flush_scheduled: bool,
    settle_scheduled: bool,
    catch_up_scheduled: bool,
    start: Option<ChatStart>,
    permission_mode: PermissionMode,
    title: Option<String>,
    /// The session ended, so nobody new is let in to wait for events that
    /// will not come.
    closed: bool,
    /// Events are kept and folded but not sent while a session loads.
    holding: bool,
    /// What the person sent while the agent was at work, oldest first.
    queued: Vec<Queued>,
    triggers: Vec<ChatTrigger>,
    /// The folder the chat's agent works in, and where its edits sit in their files,
    /// for each fold made for it.
    work: Option<(PathBuf, Place)>,
    /// A new chat whose agent waits for the person: clients take it up before it starts.
    waits_for_person: bool,
    /// The session the history follows, and how far into its CLI's record it reaches.
    reach: Option<(String, Reach)>,
    /// The CLI's record of the session holds more before what the chat holds.
    has_earlier: bool,
}

/// A feed as plain data, for a core that takes the chat over. Clients that
/// held a mark of it catch up from where they were.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeedState {
    id: String,
    seq: u64,
    replay: Replay,
    history: Option<HistoryState>,
    start: Option<ChatStart>,
    permission_mode: PermissionMode,
    title: Option<String>,
    reach: Option<(String, Reach)>,
}

pub(crate) struct Feed {
    me: Weak<Feed>,
    agent_id: String,
    /// Names this run of the chat's agent in the marks clients hold.
    id: String,
    runtime: tokio::runtime::Handle,
    inner: Mutex<Inner>,
    /// Each client's latest find in this chat, by number, so a newer one stops the one before.
    finding: Mutex<HashMap<ClientId, u64>>,
    /// What the replay holds in memory, as counted against [`RESIDENT_BYTES`].
    held: AtomicUsize,
    /// When a client last read the chat, in epoch ms.
    viewed: AtomicU64,
    earlier: Mutex<Option<Earlier>>,
}

fn queued_view(queued: &[Queued]) -> Vec<QueuedMessage> {
    queued
        .iter()
        .map(|message| QueuedMessage {
            id: message.id.clone(),
            text: message.text.clone(),
            paths: message.paths.clone(),
            context: message
                .context
                .iter()
                .map(|item| SentContext {
                    uri: item.uri.clone(),
                    title: item.title.clone(),
                })
                .collect(),
        })
        .collect()
}

/// A fold for a chat, showing an effort picker from the start when its agent
/// always offers one.
fn new_fold(
    permission_mode: PermissionMode,
    offers_effort: bool,
    work: Option<&(PathBuf, Place)>,
) -> Fold {
    let mut fold = Fold::new(permission_mode);
    if offers_effort {
        fold.offer_effort();
    }
    if let Some((folder, place)) = work {
        fold.work_in(folder.clone(), place.clone());
    }
    fold
}

/// The largest file an edit is looked for in; a bigger one leaves the edit's lines unnumbered.
const MAX_PLACED_BYTES: u64 = 2 * 1024 * 1024;

/// The line `text` starts on in the file at `path`, when it is there exactly once.
fn line_of(path: &std::path::Path, text: &str) -> Option<u64> {
    if text.is_empty() || std::fs::metadata(path).ok()?.len() > MAX_PLACED_BYTES {
        return None;
    }
    let held = std::fs::read_to_string(path).ok()?;
    let at = held.find(text)?;
    if held[at + text.len()..].contains(text) {
        return None;
    }
    Some(held[..at].matches('\n').count() as u64 + 1)
}

impl Feed {
    /// With `history`, everything the chat says is also kept in a file there,
    /// for clients to page back through.
    pub(crate) fn new(
        agent_id: String,
        permission_mode: PermissionMode,
        history: Option<&std::path::Path>,
    ) -> Arc<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        let history = history.map(|dir| History::open(dir, &id, MAX_HISTORY_BYTES));
        Self::counted(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            agent_id,
            id,
            runtime: tokio::runtime::Handle::current(),
            finding: Mutex::default(),
            held: AtomicUsize::new(0),
            viewed: AtomicU64::new(epoch_ms()),
            earlier: Mutex::new(None),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                replay: Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, history),
                fold: Some(Fold::new(permission_mode)),
                offers_effort: false,
                flush_scheduled: false,
                settle_scheduled: false,
                catch_up_scheduled: false,
                start: None,
                permission_mode,
                title: None,
                closed: false,
                holding: false,
                queued: Vec::new(),
                triggers: Vec::new(),
                work: None,
                waits_for_person: false,
                reach: None,
                has_earlier: false,
            }),
        }))
    }

    /// A feed for a chat an earlier core left when it stopped, whose history
    /// file `from` in `dir` is taken over under this feed's name. Nothing in
    /// it is read until [`Feed::recover`].
    pub(crate) fn recovering(
        agent_id: String,
        permission_mode: PermissionMode,
        dir: Option<&std::path::Path>,
        from: &str,
    ) -> Arc<Self> {
        let feed = Self::new(agent_id, permission_mode, None);
        if let Some(dir) = dir {
            let path = |name: &str| dir.join(format!("{name}.jsonl"));
            if let Err(error) = std::fs::rename(path(from), path(&feed.id)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "sikemux core: a chat's history was not kept through the restart: {error}"
                    );
                }
            }
        }
        feed
    }

    /// Reads back the history [`Feed::recovering`] took over, and goes on
    /// writing it. Reads the disk, so it blocks.
    pub(crate) fn recover(&self, dir: &std::path::Path) {
        let replay = Replay::recovered(dir, &self.id);
        let title = replay
            .standing
            .iter()
            .rev()
            .find_map(|(_, _, event)| session_title(&event.payload));
        let mut inner = self.inner.locked();
        inner.replay = replay;
        inner.title = title;
        inner.fold = None;
        self.account(&inner);
    }

    /// Takes up a feed an earlier core let go of.
    pub(crate) fn restored(agent_id: String, state: FeedState) -> Arc<Self> {
        let mut replay = state.replay;
        replay.history = state.history.map(History::reopen);
        replay.seq = state.seq;
        replay.unsent = true;
        replay.unnumbered = state.seq + 1;
        let feed = Self::counted(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            agent_id,
            id: state.id,
            runtime: tokio::runtime::Handle::current(),
            finding: Mutex::default(),
            held: AtomicUsize::new(0),
            viewed: AtomicU64::new(epoch_ms()),
            earlier: Mutex::new(None),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                replay,
                fold: None,
                offers_effort: false,
                flush_scheduled: false,
                settle_scheduled: false,
                catch_up_scheduled: false,
                start: state.start,
                permission_mode: state.permission_mode,
                title: state.title,
                closed: false,
                holding: false,
                queued: Vec::new(),
                triggers: Vec::new(),
                work: None,
                waits_for_person: false,
                reach: state.reach,
                has_earlier: false,
            }),
        }));
        feed.account(&feed.inner.locked());
        feed
    }

    /// Lets go of the feed for a core that takes the chat over, once what is
    /// waiting to go out has gone, with the clients that were hearing it.
    /// Nothing said here after that is kept.
    pub(crate) fn release(&self) -> Option<(FeedState, Vec<Arc<ClientConn>>)> {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.closed = true;
        let subscribers = inner
            .subscribers
            .drain()
            .map(|(_, subscriber)| subscriber.client)
            .collect();
        let history = inner.replay.history.take().map(History::release);
        let replay = std::mem::replace(
            &mut inner.replay,
            Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None),
        );
        self.account(&inner);
        let state = FeedState {
            id: self.id.clone(),
            seq: replay.seq,
            replay,
            history,
            start: inner.start.clone(),
            permission_mode: inner.permission_mode,
            title: inner.title.clone(),
            reach: inner.reach.clone(),
        };
        Some((state, subscribers))
    }

    /// The name of the file the feed keeps its history in.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// The fold, built again from the chat's events when it was let go.
    fn fold<'a>(&self, inner: &'a mut Inner) -> &'a mut Fold {
        if inner.fold.is_none() {
            inner.replay.reload();
            self.account(inner);
            let mut fold = new_fold(
                inner.permission_mode,
                inner.offers_effort,
                inner.work.as_ref(),
            );
            if let Some(start) = &inner.start {
                fold.apply(
                    inner.replay.next_index,
                    ChatEventKind::Ready,
                    &json!(start),
                    0,
                );
            }
            for (index, seq, event) in inner.replay.fold_events() {
                let at = event
                    .payload
                    .get("at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                fold.number(seq);
                fold.apply(index, event.kind, &event.payload, at);
            }
            fold.set_queued(queued_view(&inner.queued));
            fold.set_triggers(inner.triggers.clone());
            if inner.replay.is_trimmed() && !inner.replay.keeps_history() {
                fold.set_earlier_lost();
            }
            fold.rebuilt();
            if inner.replay.unsent {
                inner.replay.delivered();
            }
            fold.number(inner.replay.seq + 1);
            inner.fold = Some(fold);
        }
        inner.fold.as_mut().expect("just built")
    }

    /// Writes the newest event to the history once it has had a while to
    /// take in more.
    fn settle_later(self: &Arc<Self>, inner: &mut Inner) {
        if inner.settle_scheduled || !inner.replay.keeps_history() {
            return;
        }
        inner.settle_scheduled = true;
        let feed = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(SETTLE).await;
            let mut inner = feed.inner.locked();
            inner.settle_scheduled = false;
            inner.replay.settle();
        });
    }

    fn fold_in(&self, inner: &mut Inner, index: u64, kind: ChatEventKind, payload: &Value) {
        let at = payload
            .get("at")
            .and_then(Value::as_u64)
            .unwrap_or_else(epoch_ms);
        if let Some(fold) = inner.fold.as_mut() {
            fold.apply(index, kind, payload, at);
        }
    }

    /// Streamed updates go out once a frame; anything else goes out at once,
    /// after what streamed before it.
    pub(crate) fn emit(self: &Arc<Self>, kind: ChatEventKind, mut payload: Value) {
        let mut inner = self.inner.locked();
        if is_kept(kind) {
            stamp(&mut payload);
        }
        let index = inner.replay.push(kind, &payload);
        self.account(&inner);
        self.settle_later(&mut inner);
        if let Some(title) = session_title(&payload) {
            inner.title = Some(title);
        }
        self.fold_in(&mut inner, index, kind, &payload);
        if kind == ChatEventKind::SessionUpdate {
            self.flush_later(&mut inner);
            return;
        }
        self.flush_locked(&mut inner);
        self.let_go(&mut inner);
    }

    /// What a person sent the agent, as every client and every later reader
    /// of the chat sees it.
    pub(crate) fn prompted(
        self: &Arc<Self>,
        message_id: Option<&str>,
        text: &str,
        paths: &[String],
        context: &[ChatContext],
    ) {
        let mut payload = json!({ "text": text, "paths": paths });
        if let Some(message_id) = message_id {
            payload["messageId"] = json!(message_id);
        }
        if !context.is_empty() {
            payload["context"] = json!(context
                .iter()
                .map(|item| json!({ "uri": item.uri, "title": item.title }))
                .collect::<Vec<_>>());
        }
        self.emit(ChatEventKind::Prompt, payload);
    }

    /// The person took the chat back to before their message `message_id`:
    /// it goes, and everything after it.
    pub(crate) fn rewound(&self, message_id: &str) {
        let mut inner = self.inner.locked();
        inner.replay.rewind(message_id);
        self.account(&inner);
        if let Some(fold) = inner.fold.as_mut() {
            fold.rewind(message_id);
        }
        self.flush_locked(&mut inner);
    }

    /// Keeps `message` until the running turn ends, unless as many already wait.
    pub(crate) fn enqueue(&self, message: Queued) -> bool {
        let mut inner = self.inner.locked();
        if inner.queued.len() >= MAX_QUEUED {
            return false;
        }
        inner.queued.push(message);
        self.queue_changed(&mut inner);
        true
    }

    /// Takes the waiting messages `ids`, or with none the next that go out together.
    pub(crate) fn take_queued(&self, ids: Option<&[String]>) -> Vec<Queued> {
        let mut inner = self.inner.locked();
        let taken: Vec<Queued> = match ids {
            None => {
                let count = batch_len(&inner.queued);
                inner.queued.drain(..count).collect()
            }
            Some(ids) => {
                let (taken, kept) = std::mem::take(&mut inner.queued)
                    .into_iter()
                    .partition(|message| ids.contains(&message.id));
                inner.queued = kept;
                taken
            }
        };
        if !taken.is_empty() {
            self.queue_changed(&mut inner);
        }
        taken
    }

    pub(crate) fn clear_queue(&self) {
        let mut inner = self.inner.locked();
        if !inner.queued.is_empty() {
            inner.queued.clear();
            self.queue_changed(&mut inner);
        }
    }

    /// The chat's triggers changed; clients already hearing the chat are told.
    pub(crate) fn set_triggers(&self, triggers: Vec<ChatTrigger>) {
        let mut inner = self.inner.locked();
        inner.triggers = triggers.clone();
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_triggers(triggers);
            self.flush_locked(&mut inner);
        }
    }

    /// The chat's unsent draft changed; clients already hearing the chat are told.
    pub(crate) fn set_draft(&self, draft: Option<ChatDraft>) {
        let mut inner = self.inner.locked();
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_draft(draft);
            self.flush_locked(&mut inner);
        }
    }

    fn queue_changed(&self, inner: &mut Inner) {
        let queued = queued_view(&inner.queued);
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_queued(queued);
        }
        self.flush_locked(inner);
    }

    pub(crate) fn close(&self) {
        let mut inner = self.inner.locked();
        inner.closed = true;
    }

    /// Lets go of everything the chat holds in memory once it sleeps, writing out what is not
    /// yet on disk, so clients can still attach, page and find through its history, and
    /// hear the chat when it wakes in this feed, as do the clients that heard it go to sleep.
    /// Answers how many bytes of history it keeps.
    pub(crate) fn rest(&self) -> u64 {
        let mut inner = self.inner.locked();
        inner.closed = false;
        inner.fold = None;
        let bytes = inner.replay.rest();
        self.account(&inner);
        bytes
    }

    /// Keeps what the agent says from the clients until [`Feed::let_out`]. A
    /// resumed session replays its whole saved history while it starts, more
    /// than a client could be sent a change at a time.
    pub(crate) fn hold(&self) {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.holding = true;
    }

    /// Sends every client the chat as it stands once the history is in.
    pub(crate) fn let_out(&self) {
        let mut inner = self.inner.locked();
        if !inner.holding {
            return;
        }
        self.flush_locked(&mut inner);
        inner.holding = false;
        if inner.replay.is_trimmed() && !inner.replay.keeps_history() {
            self.fold(&mut inner).set_earlier_lost();
        }
        self.fold(&mut inner).flush();
        let seq = inner.replay.delivered();
        self.fold(&mut inner).number(seq + 1);
        let older = Self::holds_older(&inner);
        let view = self.fold(&mut inner).view(older);
        let frame = self.frame(
            seq,
            &[ChatChange::View {
                view: Box::new(view),
            }],
        );
        if let Some(frame) = frame {
            inner
                .subscribers
                .retain(|_, subscriber| subscriber.client.send(frame.clone()));
        }
    }

    /// Shows an effort picker from the start, as agents that offer effort on
    /// every model do.
    /// Makes the paths the chat's agent names whole against `cwd`, and numbers the lines
    /// of its edits from where they sit in their files.
    pub(crate) fn work_in(&self, cwd: PathBuf) {
        let folder = cwd.clone();
        let place: Place =
            Arc::new(move |path: &str, text: &str| line_of(&folder.join(path), text));
        let mut inner = self.inner.locked();
        if let Some(fold) = inner.fold.as_mut() {
            fold.work_in(cwd.clone(), place.clone());
        }
        inner.work = Some((cwd, place));
    }

    pub(crate) fn offer_effort(&self) {
        let mut inner = self.inner.locked();
        inner.offers_effort = true;
        if let Some(fold) = inner.fold.as_mut() {
            fold.offer_effort();
        }
    }

    /// Lets clients take up a new chat whose agent has not started, as it waits for the person.
    pub(crate) fn wait_for_person(&self) {
        self.inner.locked().waits_for_person = true;
    }

    pub(crate) fn set_start(&self, start: ChatStart) {
        let mut inner = self.inner.locked();
        inner.start = Some(start);
    }

    pub(crate) fn set_setup(&self, setup: &Value) {
        let mut inner = self.inner.locked();
        if let Some(start) = inner.start.as_mut() {
            start.setup = setup.clone();
        }
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_setup(setup);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn set_permission_mode(&self, mode: PermissionMode) {
        let mut inner = self.inner.locked();
        inner.permission_mode = mode;
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_permission_mode(mode);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn set_changing_mode(&self, changing: bool) {
        let mut inner = self.inner.locked();
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_changing_mode(changing);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.inner.locked().permission_mode
    }

    /// Numbers what the chat takes in from high up, so what is read further back into the
    /// session can go before it. Only a chat that holds nothing yet does.
    pub(crate) fn leave_room(&self) {
        let mut inner = self.inner.locked();
        if inner.replay.next_index == 0 && inner.replay.entries.is_empty() {
            inner.replay.next_index = EARLIER_ROOM;
        }
    }

    /// The CLI's record of the session holds more before what the chat holds, which
    /// `earlier` reads once a client pages back that far.
    pub(crate) fn tell_earlier(&self, earlier: Earlier) {
        *self.earlier.locked() = Some(earlier);
        self.inner.locked().has_earlier = true;
    }

    /// Reads further back into the session until the history holds `turns` turns before
    /// `before`, or nothing earlier is left.
    fn reach_back(&self, before: u64, turns: usize) {
        let mut earlier = self.earlier.locked();
        loop {
            {
                let inner = self.inner.locked();
                let enough = !inner.has_earlier
                    || inner
                        .replay
                        .plan(PageOf::Before(before), turns)
                        .is_ok_and(|planned| planned.beyond);
                if enough {
                    return;
                }
            }
            let read = match earlier.as_mut() {
                Some(read) => read(),
                None => Ok(None),
            };
            let mut inner = self.inner.locked();
            let went_in = match read {
                Ok(Some(updates)) => inner.replay.prepend(updates),
                Ok(None) => false,
                Err(error) => {
                    eprintln!("sikemux core: the start of a chat could not be read: {error}");
                    false
                }
            };
            self.account(&inner);
            if !went_in {
                inner.has_earlier = false;
                *earlier = None;
                return;
            }
        }
    }

    /// The history now holds everything the CLI's record of `session` holds, up to `reach`.
    pub(crate) fn reached(&self, session: &str, reach: Reach) {
        self.inner.locked().reach = Some((session.to_owned(), reach));
    }

    /// How far into the CLI's record of `session` the history reaches, when it follows it.
    pub(crate) fn reach(&self, session: &str) -> Option<Reach> {
        let inner = self.inner.locked();
        let (followed, reach) = inner.reach.as_ref()?;
        (followed == session).then(|| reach.clone())
    }

    /// What the agent last called this session, as the host's rail shows it.
    pub(crate) fn title(&self) -> Option<String> {
        self.inner.locked().title.clone()
    }

    /// What the chat said from index `from` on, or over its last `turns`.
    pub(crate) fn excerpt(&self, from: Option<u64>, turns: usize, max_bytes: usize) -> Excerpt {
        self.viewed();
        let mut inner = self.inner.locked();
        inner.replay.reload();
        self.account(&inner);
        let (said, skipped, next) = inner.replay.excerpt(from, turns, max_bytes);
        Excerpt {
            feed: self.id.clone(),
            said,
            skipped,
            next,
        }
    }

    /// Where the turn in progress, or the last one, starts in the history.
    pub(crate) fn turn_start(&self) -> Option<u64> {
        let mut inner = self.inner.locked();
        inner.replay.reload();
        self.account(&inner);
        inner.replay.turn_start()
    }

    /// What the agent wrote last in its latest turn.
    pub(crate) fn last_answer(&self) -> Option<String> {
        let mut inner = self.inner.locked();
        inner.replay.reload();
        self.account(&inner);
        inner.replay.last_answer()
    }

    /// Whether the last turn it holds has not ended.
    pub(crate) fn in_turn(&self) -> bool {
        self.inner.locked().replay.in_turn
    }

    /// Whether every prompt the agent was sent has had its turn end, and no turn is open.
    pub(crate) fn settled(&self) -> bool {
        let inner = self.inner.locked();
        !inner.replay.in_turn && !inner.replay.awaiting
    }

    pub(crate) fn reconnecting(&self) -> Option<u32> {
        self.inner.locked().replay.reconnecting
    }

    pub(crate) fn running_subagents(&self) -> u32 {
        self.inner.locked().replay.work.running_subagents() as u32
    }

    pub(crate) fn running_tasks(&self) -> u32 {
        self.inner.locked().replay.work.running_tasks() as u32
    }

    /// The agent's process went away, and every subagent and background task
    /// it had going went with it. The chat is told each one ended.
    pub(crate) fn agent_gone(&self) {
        self.end_work(Work::endings);
    }

    /// The turn waited too long on its subagents, which the chat is told failed.
    pub(crate) fn give_up_subagents(&self) {
        self.end_work(|work| work.subagent_endings(Subagent::Failed).collect());
    }

    fn end_work(&self, endings: impl FnOnce(&Work) -> Vec<Value>) {
        let mut inner = self.inner.locked();
        for mut payload in endings(&inner.replay.work) {
            stamp(&mut payload);
            let index = inner.replay.push(ChatEventKind::SessionUpdate, &payload);
            self.fold_in(&mut inner, index, ChatEventKind::SessionUpdate, &payload);
        }
        self.account(&inner);
        self.flush_locked(&mut inner);
    }

    pub(crate) fn start(&self) -> Option<ChatStart> {
        self.inner.locked().start.clone()
    }

    pub(crate) fn forget_permission(&self, request_id: &str) {
        let mut inner = self.inner.locked();
        inner.replay.forget_permission(request_id);
        self.account(&inner);
        if let Some(fold) = inner.fold.as_mut() {
            fold.forget_permission(request_id);
        }
        self.flush_locked(&mut inner);
    }

    /// The rows before `before` in this run of the chat, which a client
    /// shown only the chat's end scrolls back to.
    pub(crate) fn history(
        &self,
        feed: &str,
        before: u64,
        turns: usize,
    ) -> Result<ChatPage, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        self.page(feed, Anchor::Before(before), turns, None)
    }

    /// The rows `anchor` asks for in this run of the chat, `turns` of them at most: those
    /// before or after an event, or around the row holding one.
    /// With `session`, the rows are that subagent's rather than the chat's own.
    pub(crate) fn page(
        &self,
        feed: &str,
        anchor: Anchor,
        turns: usize,
        session: Option<&str>,
    ) -> Result<ChatPage, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        let turns = turns.clamp(1, MAX_PAGE_TURNS);
        match anchor {
            Anchor::Before(before) => {
                self.page_of(PageOf::Before(before), turns, ROW_BYTES, session)
            }
            Anchor::After(after) => self.page_of(PageOf::After(after), turns, ROW_BYTES, session),
            Anchor::Around(event) => {
                let half = (turns / 2).max(1);
                let before = PageOf::Before(event.saturating_add(1));
                let mut page = self.page_of(before, half, ROW_BYTES / 2, session)?;
                let newer = self.page_of(PageOf::After(event), half, ROW_BYTES / 2, session)?;
                let held: HashSet<String> = page.rows.iter().map(|row| row.id.clone()).collect();
                page.rows
                    .extend(newer.rows.into_iter().filter(|row| !held.contains(&row.id)));
                page.subagents.extend(newer.subagents);
                page.newer_after = newer.newer_after;
                Ok(page)
            }
        }
    }

    /// One step of `client`'s find through this run of the chat.
    pub(crate) fn find(
        &self,
        client: ClientId,
        params: &super::find::FindParams,
    ) -> Result<super::find::FindAnswer, String> {
        if params.feed != self.id {
            return Err("the chat started again; open it again to find in it".into());
        }
        let number = {
            let mut finding = self.finding.locked();
            let number = finding.get(&client).map_or(0, |number| number + 1);
            finding.insert(client, number);
            number
        };
        let rows = FindRows {
            feed: self,
            client,
            number,
        };
        let answer = super::find::step(&rows, params);
        let mut finding = self.finding.locked();
        if finding.get(&client) == Some(&number) {
            finding.remove(&client);
        }
        answer
    }

    fn page_of(
        &self,
        side: PageOf,
        turns: usize,
        budget: usize,
        session: Option<&str>,
    ) -> Result<ChatPage, String> {
        self.viewed();
        if let PageOf::Before(before) = side {
            self.reach_back(before, turns);
        }
        for _ in 0..2 {
            let planned = self.inner.locked().replay.plan(side, turns)?;
            let mut events = match &planned.reader {
                Some(reader) => reader
                    .range(planned.lo, planned.hi)
                    .map_err(|error| error.to_string())?,
                None => Vec::new(),
            };
            if planned
                .reader
                .as_ref()
                .is_some_and(|reader| !reader.still_current())
            {
                continue;
            }
            let edge = planned.edge(side);
            events.extend(planned.held);
            let mut inner = self.inner.locked();
            let page = inner.replay.with_spawns(Page {
                events,
                older_before: None,
                first: planned.lo,
            });
            return Ok(self
                .fold(&mut inner)
                .page_of(&page.events, side, budget, edge, session));
        }
        Err("that part of the chat changed; ask again".into())
    }

    /// A tool call's change and output, line by line, for a client that opened its row.
    pub(crate) fn tool_detail(&self, tool_call_id: &str) -> Option<ToolDetail> {
        let mut inner = self.inner.locked();
        if let Some(detail) = self.fold(&mut inner).tool_detail(tool_call_id) {
            return Some(detail);
        }
        let inner = &*inner;
        inner.replay.tool_detail(tool_call_id, inner.work.as_ref())
    }
}

/// The rows a find reads: those the fold holds while the place it reads from is among them,
/// else pages of the chat's history.
struct FindRows<'a> {
    feed: &'a Feed,
    client: ClientId,
    number: u64,
}

impl super::find::Rows for FindRows<'_> {
    fn batch(
        &self,
        wanted: PageOf,
        budget: usize,
    ) -> Result<(Vec<sikemux_wire::chat::ChatMessage>, bool), String> {
        let held = {
            let mut inner = self.feed.inner.locked();
            let fold = self.feed.fold(&mut inner);
            let inside = fold.floor().is_some_and(|floor| match wanted {
                PageOf::Before(before) => before > floor,
                PageOf::After(after) => after >= floor,
                PageOf::From(from) => from >= floor,
            });
            inside.then(|| fold.held(wanted, budget))
        };
        if let Some((rows, all)) = held {
            let last = all && !matches!(wanted, PageOf::Before(_));
            return Ok((rows, last));
        }
        let page = self.feed.page_of(wanted, MAX_PAGE_TURNS, budget, None)?;
        let last = match wanted {
            PageOf::Before(_) => page.older_before.is_none(),
            PageOf::After(_) | PageOf::From(_) => page.newer_after.is_none(),
        };
        let mut rows = page.rows;
        if matches!(wanted, PageOf::Before(_)) {
            rows.reverse();
        }
        Ok((rows, last))
    }

    fn superseded(&self) -> bool {
        self.feed.finding.locked().get(&self.client) != Some(&self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::replay::tests::{asks, text, turn, update};
    use super::*;
    use crate::protocol::{ChatAttachment, ChatMark, Event, ServerMessage};
    use sikemux_fold::UpdateKind;

    #[test]
    fn a_chat_folded_from_its_end_carries_the_start_of_every_subagent_it_speaks_of() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 50 * 5 + 2, None);
        let spawned = |session: &str, id: &str| {
            update(
                session,
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": id }),
            )
        };
        let ended = |id: &str| {
            update(
                "s",
                json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": id, "state": "completed" }),
            )
        };
        replay.push(ChatEventKind::SessionUpdate, &spawned("s", "corpus"));
        replay.push(ChatEventKind::SessionUpdate, &spawned("corpus", "fixtures"));
        replay.push(ChatEventKind::SessionUpdate, &spawned("s", "forgotten"));
        replay.push(ChatEventKind::SessionUpdate, &ended("forgotten"));
        for n in 0..50 {
            turn(&mut replay, n);
        }
        replay.push(
            ChatEventKind::SessionUpdate,
            &update(
                "fixtures",
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Recorded" } }),
            ),
        );
        replay.push(ChatEventKind::SessionUpdate, &ended("corpus"));

        let starts: Vec<String> = replay
            .fold_events()
            .iter()
            .map(|(_, _, event)| event)
            .filter(|event| UpdateKind::within(&event.payload) == Some(UpdateKind::SubagentSpawned))
            .map(|event| {
                event.payload["update"]["subagentSessionId"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(starts, ["corpus", "fixtures"]);
    }

    #[tokio::test]
    async fn a_long_history_loaded_at_the_start_reaches_clients_as_its_last_turns() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::server::Core::new(Default::default(), None).unwrap();
        let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
        let feed = Feed::new(
            "agent".into(),
            PermissionMode::WorkspaceWrite,
            Some(dir.path()),
        );
        feed.subscribe(&client);

        feed.hold();
        for n in 0..60 {
            let (asked, answer) = (format!("ask {n}"), format!("m{n}"));
            feed.emit(
                ChatEventKind::SessionUpdate,
                text("user_message_chunk", None, &asked),
            );
            feed.emit(
                ChatEventKind::SessionUpdate,
                text("agent_message_chunk", Some(&answer), "done"),
            );
        }
        let mut heard = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            heard.push(frame);
        }
        assert!(heard.is_empty(), "nothing goes out while the history loads");

        feed.let_out();
        let mut views = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            let frame = crate::protocol::read_frame_sync(&mut &frame[..])
                .unwrap()
                .unwrap();
            let Ok(ServerMessage::Event {
                event: Event::Chat { changes, .. },
            }) = serde_json::from_slice(&frame.payload)
            else {
                continue;
            };
            let changes: Vec<ChatChange> = serde_json::from_value(changes).unwrap();
            views.extend(changes.into_iter().filter_map(|change| match change {
                ChatChange::View { view } => Some(view),
                _ => None,
            }));
        }
        let [view] = &views[..] else {
            panic!(
                "the history goes out as one view, not {} of them",
                views.len()
            );
        };
        let first = 60 - sikemux_fold::TAIL_TURNS;
        assert_eq!(prompts(&view.rows), asks(first..60));
        assert_eq!(view.older_before, Some(first as u64 * 2));
    }

    fn prompts(rows: &[sikemux_wire::chat::ChatMessage]) -> Vec<String> {
        rows.iter()
            .filter(|row| row.role == sikemux_wire::chat::ChatRole::User)
            .flat_map(|row| row.parts.iter())
            .filter_map(|part| match part {
                sikemux_wire::chat::ChatPart::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_long_chat_opens_at_its_last_turns_and_pages_back() {
        let dir = tempfile::tempdir().unwrap();
        let feed = Feed::new(
            "agent".into(),
            PermissionMode::WorkspaceWrite,
            Some(dir.path()),
        );
        feed.set_start(ChatStart {
            session_id: "s".into(),
            capabilities: json!({}),
            setup: json!({}),
        });
        let view = |inner: &mut Inner| {
            inner.fold = None;
            feed.fold(inner);
            let older = Feed::holds_older(inner);
            feed.fold(inner).view(older)
        };
        let mut inner = feed.inner.lock().unwrap();
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let opened = view(&mut inner);
        let first = 60 - sikemux_fold::TAIL_TURNS;
        assert_eq!(prompts(&opened.rows), asks(first..60));
        assert_eq!(opened.older_before, Some(first as u64 * 5));
        drop(inner);
        let page = feed.history(&feed.id, first as u64 * 5, 100).unwrap();
        assert_eq!(prompts(&page.rows), asks(0..first));
        assert_eq!(page.older_before, None);
        assert!(feed
            .history("an earlier run", first as u64 * 5, 100)
            .is_err());
        let mut inner = feed.inner.lock().unwrap();

        let history = History::open(dir.path(), "trimmed", MAX_HISTORY_BYTES);
        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, Some(history));
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let opened = view(&mut inner);
        assert_eq!(prompts(&opened.rows), asks(58..60));
        assert_eq!(opened.older_before, Some(58 * 5));
        assert!(!opened.head.earlier_lost);

        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, None);
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let opened = view(&mut inner);
        assert!(opened.head.earlier_lost);
        assert_eq!(prompts(&opened.rows), asks(58..60));
    }

    #[tokio::test]
    async fn a_sleeping_chat_opens_at_its_last_turns_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let feed = Feed::new(
            "agent".into(),
            PermissionMode::WorkspaceWrite,
            Some(dir.path()),
        );
        feed.set_start(ChatStart {
            session_id: "s".into(),
            capabilities: json!({}),
            setup: json!({}),
        });
        let mut inner = feed.inner.lock().unwrap();
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        drop(inner);
        assert!(feed.rest() > 0);
        assert!(feed.inner.lock().unwrap().replay.entries.is_empty());

        let mut inner = feed.inner.lock().unwrap();
        feed.fold(&mut inner);
        let older = Feed::holds_older(&inner);
        let opened = feed.fold(&mut inner).view(older);
        drop(inner);
        let first = 60 - sikemux_fold::TAIL_TURNS;
        assert_eq!(prompts(&opened.rows), asks(first..60));
        assert_eq!(opened.older_before, Some(first as u64 * 5));
        let page = feed.history(&feed.id, first as u64 * 5, 100).unwrap();
        assert_eq!(prompts(&page.rows), asks(0..first));
        assert!(!feed.excerpt(None, 3, usize::MAX).said.is_empty());
    }

    fn said_turn(feed: &Arc<Feed>, n: usize) {
        feed.prompted(None, &format!("ask {n}"), &[], &[]);
        feed.emit(ChatEventKind::TurnStarted, json!({}));
        for piece in ["work", "ing"] {
            feed.emit(
                ChatEventKind::SessionUpdate,
                text("agent_message_chunk", Some(&format!("m{n}")), piece),
            );
        }
        feed.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
    }

    fn attached(
        feed: &Arc<Feed>,
        client: &Arc<ClientConn>,
        frames: &mut tokio::sync::mpsc::UnboundedReceiver<Arc<[u8]>>,
        since: Option<ChatMark>,
    ) -> ChatAttachment {
        feed.attach(
            client,
            1,
            Standing {
                running: false,
                turned: true,
                draft: None,
            },
            since,
        );
        while let Ok(frame) = frames.try_recv() {
            let frame = crate::protocol::read_frame_sync(&mut &frame[..])
                .unwrap()
                .unwrap();
            if let Ok(ServerMessage::Response {
                response: crate::protocol::Response::ChatAttached { attachment },
                ..
            }) = serde_json::from_slice(&frame.payload)
            {
                return attachment;
            }
        }
        panic!("the attach was not answered");
    }

    fn rows_sent(attachment: &ChatAttachment) -> Vec<String> {
        let ChatAttachment::Resumed { changes, .. } = attachment else {
            panic!("the chat was sent whole rather than resumed");
        };
        let changes: Vec<ChatChange> = serde_json::from_value(changes.clone()).unwrap();
        changes
            .into_iter()
            .filter_map(|change| match change {
                ChatChange::Row { row, .. } => Some(
                    row.parts
                        .iter()
                        .filter_map(|part| match part {
                            sikemux_wire::chat::ChatPart::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    }

    fn mark_of(attachment: &ChatAttachment) -> ChatMark {
        match attachment {
            ChatAttachment::Live { mark, .. } | ChatAttachment::Resumed { mark, .. } => {
                mark.clone()
            }
            ChatAttachment::Missing => panic!("the chat is missing"),
        }
    }

    #[tokio::test]
    async fn a_client_back_after_the_chat_moved_on_unwatched_gets_just_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::server::Core::new(Default::default(), None).unwrap();
        let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
        let feed = Feed::new(
            "agent".into(),
            PermissionMode::WorkspaceWrite,
            Some(dir.path()),
        );
        feed.set_start(ChatStart {
            session_id: "s".into(),
            capabilities: json!({}),
            setup: json!({}),
        });
        said_turn(&feed, 0);
        let left_at = mark_of(&attached(&feed, &client, &mut frames, None));
        feed.unsubscribe(client.id);
        assert!(feed.inner.lock().unwrap().fold.is_none());

        said_turn(&feed, 1);
        said_turn(&feed, 2);
        let back = attached(&feed, &client, &mut frames, Some(left_at));
        assert_eq!(
            rows_sent(&back),
            ["ask 1", "working", "ask 2", "working"],
            "the two turns it missed, and nothing it held"
        );

        let caught_up = mark_of(&back);
        feed.unsubscribe(client.id);
        let again = attached(&feed, &client, &mut frames, Some(caught_up));
        assert!(
            rows_sent(&again).is_empty(),
            "a fold built again keeps every row's number"
        );
    }
}
