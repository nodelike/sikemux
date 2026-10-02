use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::increment_monotonic;
use super::store::global_observability;
use super::watchdog::WatchdogSignal;

/// Snapshot of a heartbeat shared with a watchdog thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatSnapshot {
    pub sequence: u64,
    pub last_beat_us: u64,
    pub armed: bool,
    pub visible: bool,
}

#[derive(Debug)]
struct HeartbeatInner {
    sequence: AtomicU64,
    last_beat_us: AtomicU64,
    armed: AtomicBool,
    visible: AtomicBool,
    /// The watchdog parks here while there is nothing to watch, so arming or
    /// showing the window has to tell it to come back.
    waker: Mutex<Option<Arc<WatchdogSignal>>>,
}

/// A cheap, monotonic heartbeat that may be updated from any thread.
///
/// UI code should call [`Heartbeat::beat`] after it proves forward progress,
/// for example after handling an input or presenting a frame. Arming and
/// visibility are separate so expected startup work or a hidden window does
/// not produce a false hang report.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    inner: Arc<HeartbeatInner>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

impl Heartbeat {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(HeartbeatInner {
                sequence: AtomicU64::new(0),
                last_beat_us: AtomicU64::new(global_observability().now_us()),
                armed: AtomicBool::new(true),
                visible: AtomicBool::new(true),
                waker: Mutex::new(None),
            }),
        }
    }

    pub(super) fn attach_waker(&self, signal: Arc<WatchdogSignal>) {
        match self.inner.waker.lock() {
            Ok(mut waker) => *waker = Some(signal),
            Err(poisoned) => *poisoned.into_inner() = Some(signal),
        }
    }

    fn wake_watcher(&self) {
        let waker = match self.inner.waker.lock() {
            Ok(waker) => waker.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(signal) = waker {
            signal.wake();
        }
    }

    /// Marks forward progress and returns the new monotonic heartbeat number.
    pub fn beat(&self) -> u64 {
        let now_us = global_observability().now_us();
        // Multiple producers may race. `fetch_max` prevents an older producer
        // from moving the observed heartbeat timestamp backwards.
        self.inner.last_beat_us.fetch_max(now_us, Ordering::Release);
        increment_monotonic(&self.inner.sequence)
    }

    pub fn set_armed(&self, armed: bool) {
        if armed {
            // Re-arming starts a fresh deadline instead of immediately
            // reporting time intentionally spent disarmed as a hang.
            self.inner
                .last_beat_us
                .fetch_max(global_observability().now_us(), Ordering::Release);
        }
        self.inner.armed.store(armed, Ordering::Release);
        if armed {
            self.wake_watcher();
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if visible {
            // Likewise, returning from a hidden state starts a fresh deadline.
            self.inner
                .last_beat_us
                .fetch_max(global_observability().now_us(), Ordering::Release);
        }
        self.inner.visible.store(visible, Ordering::Release);
        if visible {
            self.wake_watcher();
        }
    }

    pub fn snapshot(&self) -> HeartbeatSnapshot {
        HeartbeatSnapshot {
            sequence: self.inner.sequence.load(Ordering::Acquire),
            last_beat_us: self.inner.last_beat_us.load(Ordering::Acquire),
            armed: self.inner.armed.load(Ordering::Acquire),
            visible: self.inner.visible.load(Ordering::Acquire),
        }
    }
}
