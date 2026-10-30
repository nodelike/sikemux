//! Reads clients show, kept by the core: each is answered from what was read
//! lately, read again while a connection shows it, and clients are told when
//! one changed, so no client keeps time of its own. A read that fails is
//! tried again less and less often.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use super::connection::ClientId;
use super::plugins::Failure;
use super::Core;
use crate::backoff::Backoff;

/// A read no connection shows any more is kept this long after it was asked for.
const GRACE: Duration = Duration::from_secs(60);
/// The soonest and the latest the reading loop looks again.
const QUICKEST: Duration = Duration::from_secs(1);
const SLOWEST: Duration = Duration::from_secs(15);
const KEPT_AT_MOST: usize = 64;

/// What one kind of shown read is: how it is named, where clients show it,
/// how it is read again and who hears when it changed.
pub(crate) trait Shows: Sized + Send + Sync + 'static {
    type Key: Clone + Eq + Hash + Send + Sync + 'static;
    /// Where a connection shows a read, so two places keep one each.
    type Slot: Clone + Eq + Hash + Send + Sync + 'static;
    /// What reading a key again needs beside the key.
    type With: Clone + Send + Sync + 'static;
    /// The longest a failing read waits before it is tried again.
    const LONGEST_BACKOFF: Duration;

    fn cache(core: &Core) -> &ShownCache<Self>;

    /// How old a kept answer may be when a client asks for it, given how
    /// often it is read again.
    fn served_within(pace: Duration) -> Duration {
        pace
    }

    /// The answer, and how soon it is read again while shown.
    fn read(
        core: Arc<Core>,
        key: Self::Key,
        with: Self::With,
    ) -> impl Future<Output = Result<(Value, Duration), Failure>> + Send;

    /// The answers of `moved` changed, each with the slots that show it.
    fn moved(core: &Core, moved: Vec<(Self::Key, Vec<Self::Slot>)>);

    /// What the kind keeps beside its reads, let go of with the slots still shown.
    fn tidy(_core: &Core, _shown: &HashSet<Self::Slot>) {}
}

struct Kept<S: Shows> {
    answer: Value,
    with: S::With,
    read: Instant,
    /// How long after `read` it is read again while shown.
    fresh: Duration,
    /// What the last good read asked for, which `fresh` returns to after a failure.
    pace: Duration,
    failures: u32,
    asked: Instant,
}

struct State<S: Shows> {
    kept: HashMap<S::Key, Kept<S>>,
    /// The read each place of each connection last asked for, which it shows
    /// until it asks for another there or leaves.
    shown: HashMap<(ClientId, S::Slot), S::Key>,
}

pub(crate) struct ShownCache<S: Shows> {
    state: Mutex<State<S>>,
    ticking: AtomicBool,
    /// Wakes the reading loop when a read arrives that is read again sooner.
    wake: Arc<Notify>,
}

impl<S: Shows> Default for ShownCache<S> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                kept: HashMap::new(),
                shown: HashMap::new(),
            }),
            ticking: AtomicBool::new(false),
            wake: Arc::default(),
        }
    }
}

impl<S: Shows> ShownCache<S> {
    /// `client` shows `key` at `slot` now; what was read of it lately, when it
    /// is fresh enough to answer with.
    pub(crate) fn show(&self, client: ClientId, slot: S::Slot, key: &S::Key) -> Option<Value> {
        let mut state = self.state.locked();
        state.shown.insert((client, slot), key.clone());
        state.kept.get_mut(key).and_then(|kept| {
            kept.asked = Instant::now();
            (kept.read.elapsed() < S::served_within(kept.pace)).then(|| kept.answer.clone())
        })
    }

    /// `client` no longer shows the slots `gone` picks.
    pub(crate) fn away(&self, client: ClientId, gone: impl Fn(&S::Slot) -> bool) {
        self.state
            .locked()
            .shown
            .retain(|(shown_to, slot), _| *shown_to != client || !gone(slot));
    }

    /// The kept keys `pick` picks.
    pub(crate) fn keys(&self, pick: impl Fn(&S::Key) -> bool) -> Vec<S::Key> {
        self.state
            .locked()
            .kept
            .keys()
            .filter(|key| pick(key))
            .cloned()
            .collect()
    }

    /// Forgets the kept reads `pick` picks, and says which they were.
    pub(crate) fn forget(&self, pick: impl Fn(&S::Key) -> bool) -> Vec<S::Key> {
        let mut state = self.state.locked();
        let forgot: Vec<S::Key> = state.kept.keys().filter(|key| pick(key)).cloned().collect();
        state.kept.retain(|key, _| !pick(key));
        forgot
    }

    /// Every kept answer, with its key.
    pub(crate) fn each(&self, mut see: impl FnMut(&S::Key, &Value)) {
        for (key, kept) in &self.state.locked().kept {
            see(key, &kept.answer);
        }
    }
}

/// Keeps what was just read of `key`, read again after `pace` while shown.
pub(crate) fn keep<S: Shows>(
    core: &Arc<Core>,
    key: S::Key,
    answer: Value,
    pace: Duration,
    with: S::With,
) {
    let cache = S::cache(core);
    {
        let mut state = cache.state.locked();
        let now = Instant::now();
        let kept = Kept {
            answer,
            with,
            read: now,
            fresh: pace,
            pace,
            failures: 0,
            asked: now,
        };
        state.kept.insert(key, kept);
        while state.kept.len() > KEPT_AT_MOST {
            let Some(oldest) = state
                .kept
                .iter()
                .min_by_key(|(_, kept)| kept.asked)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            state.kept.remove(&oldest);
        }
    }
    cache.wake.notify_one();
    follow::<S>(core);
}

/// Reads what connections still show again as each goes stale, until none
/// shows one.
fn follow<S: Shows>(core: &Arc<Core>) {
    let cache = S::cache(core);
    if cache.ticking.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(core);
    let wake = cache.wake.clone();
    tokio::spawn(async move {
        let mut wait = QUICKEST;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wake.notified() => {}
            }
            let Some(core) = weak.upgrade() else { return };
            let connected: HashSet<ClientId> =
                core.clients().iter().map(|client| client.id).collect();
            let cache = S::cache(&core);
            let (due, slots) = {
                let mut state = cache.state.locked();
                state
                    .shown
                    .retain(|(client, _), _| connected.contains(client));
                let shown: HashSet<S::Key> = state.shown.values().cloned().collect();
                state
                    .kept
                    .retain(|key, kept| shown.contains(key) || kept.asked.elapsed() < GRACE);
                let slots: HashSet<S::Slot> =
                    state.shown.keys().map(|(_, slot)| slot.clone()).collect();
                if state.kept.is_empty() {
                    cache.ticking.store(false, Ordering::Release);
                    drop(state);
                    S::tidy(&core, &slots);
                    return;
                }
                let shown_kept = || state.kept.iter().filter(|(key, _)| shown.contains(*key));
                wait = shown_kept()
                    .map(|(_, kept)| kept.fresh.saturating_sub(kept.read.elapsed()))
                    .min()
                    .unwrap_or(SLOWEST)
                    .clamp(QUICKEST, SLOWEST);
                let due = shown_kept()
                    .filter(|(_, kept)| kept.read.elapsed() >= kept.fresh)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                (due, slots)
            };
            S::tidy(&core, &slots);
            if !due.is_empty() {
                refresh::<S>(&core, due).await;
                wait = QUICKEST;
            }
        }
    });
}

/// Reads `keys` again, telling clients about each that changed.
pub(crate) async fn refresh<S: Shows>(core: &Arc<Core>, keys: Vec<S::Key>) {
    let cache = S::cache(core);
    let mut reading = JoinSet::new();
    for key in keys {
        let Some(with) = cache
            .state
            .locked()
            .kept
            .get(&key)
            .map(|kept| kept.with.clone())
        else {
            continue;
        };
        let core = core.clone();
        reading.spawn(async move {
            let read = S::read(core, key.clone(), with).await;
            (key, read)
        });
    }
    let mut moved = Vec::new();
    while let Some(Ok((key, read))) = reading.join_next().await {
        let mut state = cache.state.locked();
        let Some(kept) = state.kept.get_mut(&key) else {
            continue;
        };
        kept.read = Instant::now();
        let Ok((answer, pace)) = read else {
            // A service that turned the read down, or limits how often it is asked, is asked less and less often.
            kept.failures = kept.failures.saturating_add(1);
            let backoff = Backoff {
                first: kept.pace,
                cap: S::LONGEST_BACKOFF.max(kept.pace),
            };
            kept.fresh = (kept.pace + backoff.after(kept.failures)).min(backoff.cap);
            continue;
        };
        kept.failures = 0;
        kept.pace = pace;
        kept.fresh = pace;
        if kept.answer == answer {
            continue;
        }
        kept.answer = answer;
        let slots = state
            .shown
            .iter()
            .filter(|(_, shown)| **shown == key)
            .map(|((_, slot), _)| slot.clone())
            .collect();
        moved.push((key, slots));
    }
    if !moved.is_empty() {
        S::moved(core, moved);
    }
}

/// Reads `keys` again at once, and again once the service has had a moment
/// to list a change it just took.
pub(crate) fn read_again_settled<S: Shows>(core: &Arc<Core>, keys: Vec<S::Key>) {
    /// A service lists a change a moment after it took it.
    const SETTLING: Duration = Duration::from_millis(1500);
    if keys.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        refresh::<S>(&core, keys.clone()).await;
        tokio::time::sleep(SETTLING).await;
        refresh::<S>(&core, keys).await;
    });
}

/// Reads `keys` again now.
pub(crate) fn read_again<S: Shows>(core: &Arc<Core>, keys: Vec<S::Key>) {
    if keys.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move { refresh::<S>(&core, keys).await });
}
