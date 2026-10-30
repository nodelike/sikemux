//! What every chat's replay holds in memory together, and letting go of the replays of
//! chats nobody watches once that is too much.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use sikemux_fold::Fold;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{Feed, Inner};

/// The most every chat's replay holds in memory together. Past it, the chats viewed least
/// lately that nobody watches and no turn runs in let go of theirs, which stays on disk.
const RESIDENT_BYTES: usize = 64 * 1024 * 1024;

/// How often the chats are looked over for one to let go of, while past [`RESIDENT_BYTES`].
const EVICT_EVERY_MS: u64 = 1000;

/// Every feed, and what their replays hold in memory together.
#[derive(Default)]
struct Resident {
    feeds: Mutex<Vec<Weak<Feed>>>,
    bytes: AtomicUsize,
    evicting: AtomicBool,
    last_pass: AtomicU64,
}

static RESIDENT: LazyLock<Resident> = LazyLock::new(Resident::default);

impl Resident {
    /// Lets go of replays, the feeds viewed least lately first, until they fit again.
    fn evict(&self) {
        let mut feeds: Vec<Arc<Feed>> = {
            let mut held = self.feeds.locked();
            held.retain(|feed| feed.strong_count() > 0);
            held.iter().filter_map(Weak::upgrade).collect()
        };
        feeds.sort_by_key(|feed| feed.viewed.load(Ordering::Relaxed));
        for feed in feeds {
            if self.bytes.load(Ordering::Relaxed) <= RESIDENT_BYTES {
                break;
            }
            feed.evict();
        }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        RESIDENT
            .bytes
            .fetch_sub(*self.held.get_mut(), Ordering::Relaxed);
    }
}

impl Feed {
    pub(super) fn counted(feed: Arc<Self>) -> Arc<Self> {
        let mut feeds = RESIDENT.feeds.locked();
        feeds.retain(|feed| feed.strong_count() > 0);
        feeds.push(Arc::downgrade(&feed));
        drop(feeds);
        feed
    }

    /// Counts what the replay holds now against the core's memory, and lets go of others'
    /// replays once all of them hold too much.
    pub(super) fn account(&self, inner: &Inner) {
        let now = inner.replay.bytes;
        let was = self.held.swap(now, Ordering::Relaxed);
        let total = if now >= was {
            RESIDENT.bytes.fetch_add(now - was, Ordering::Relaxed) + (now - was)
        } else {
            RESIDENT.bytes.fetch_sub(was - now, Ordering::Relaxed) - (was - now)
        };
        if total <= RESIDENT_BYTES
            || RESIDENT.last_pass.load(Ordering::Relaxed) + EVICT_EVERY_MS > epoch_ms()
            || RESIDENT.evicting.swap(true, Ordering::AcqRel)
        {
            return;
        }
        RESIDENT.last_pass.store(epoch_ms(), Ordering::Relaxed);
        self.runtime.spawn_blocking(|| {
            RESIDENT.evict();
            RESIDENT.evicting.store(false, Ordering::Release);
        });
    }

    /// A client read the chat just now.
    pub(super) fn viewed(&self) {
        self.viewed.store(epoch_ms(), Ordering::Relaxed);
    }

    /// Lets go of what the replay holds in memory, which its history keeps on disk, unless a
    /// client watches the chat or a turn runs in it.
    fn evict(&self) {
        let mut inner = self.inner.locked();
        let busy = !inner.subscribers.is_empty()
            || inner.replay.in_turn
            || inner.replay.awaiting
            || inner.holding
            || inner.fold.as_ref().is_some_and(Fold::is_dirty)
            || !inner.replay.keeps_history();
        if busy {
            return;
        }
        inner.fold = None;
        inner.replay.rest();
        self.account(&inner);
    }
}
