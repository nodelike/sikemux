//! Whether the core is handing itself over to a newer binary, and the work that waits on.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{watch, Notify};

/// Whether this core hands itself over to a newer binary, and the work in
/// progress a hand-over waits for.
pub struct Updating {
    /// True while the core hands itself over: it takes no new work and its
    /// readers stop at the next whole chunk.
    pub frozen: watch::Sender<bool>,
    pub started: AtomicBool,
    /// Readers that hold output not yet fed to their screen.
    pub pumping: Gauge,
    /// Sessions being spawned that are not in the table yet.
    pub launching: Gauge,
}

impl Default for Updating {
    fn default() -> Self {
        Self {
            frozen: watch::channel(false).0,
            started: AtomicBool::new(false),
            pumping: Gauge::default(),
            launching: Gauge::default(),
        }
    }
}

impl Updating {
    pub fn is_frozen(&self) -> bool {
        *self.frozen.borrow()
    }

    pub async fn until_frozen(&self) {
        let mut frozen = self.frozen.subscribe();
        let _ = frozen.wait_for(|frozen| *frozen).await;
    }

    /// Hears each time the core freezes or thaws.
    pub fn watch(&self) -> watch::Receiver<bool> {
        self.frozen.subscribe()
    }
}

/// Counts work in progress that a hand-over waits for, and wakes it when the
/// count drops.
#[derive(Default)]
pub struct Gauge {
    count: AtomicUsize,
    idle: Notify,
}

pub struct GaugeGuard<'a>(&'a Gauge);

impl Gauge {
    pub fn enter(&self) -> GaugeGuard<'_> {
        self.count.fetch_add(1, Ordering::AcqRel);
        GaugeGuard(self)
    }

    pub fn is_idle(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    /// False when the work did not finish in time.
    pub async fn settle(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.is_idle() {
                return true;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return self.is_idle();
            }
        }
    }
}

impl Drop for GaugeGuard<'_> {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
        self.0.idle.notify_waiters();
    }
}

/// What anything that asks the core for work hears while it hands itself over.
pub const UPDATING: &str = "Sikemux is updating its background process; try again in a moment";
