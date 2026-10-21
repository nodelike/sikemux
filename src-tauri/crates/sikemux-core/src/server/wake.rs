//! Work the core does when something it depends on changes, or when a moment
//! it waits for comes, in place of looking every so often.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::Core;

pub(crate) struct Wake {
    notify: Notify,
    due: Mutex<Option<Instant>>,
}

impl Default for Wake {
    /// Runs its work once when the core starts, for what an earlier core left.
    fn default() -> Self {
        let wake = Self {
            notify: Notify::new(),
            due: Mutex::new(None),
        };
        wake.now();
        wake
    }
}

impl Wake {
    /// Something the work reads changed.
    pub(crate) fn now(&self) {
        self.notify.notify_one();
    }

    /// The work runs again by `at`, in Unix milliseconds, unless it runs
    /// sooner.
    pub(crate) fn at(&self, at: u64) {
        let at = Instant::now() + Duration::from_millis(at.saturating_sub(epoch_ms()));
        let mut due = self.due.locked();
        if due.is_none_or(|due| at < due) {
            *due = Some(at);
            drop(due);
            self.notify.notify_one();
        }
    }

    async fn woken(&self) {
        let due = *self.due.locked();
        match due {
            Some(due) => {
                tokio::select! {
                    () = self.notify.notified() => {}
                    () = tokio::time::sleep_until(due) => {}
                }
            }
            None => self.notify.notified().await,
        }
        let mut due = self.due.locked();
        if due.is_some_and(|due| due <= Instant::now()) {
            *due = None;
        }
    }
}

/// Runs `work` each time the wake `of` the core is woken, for as long as the
/// core runs. The work says when it next wants to run with [`Wake::at`].
pub(crate) async fn serve(core: Arc<Core>, of: fn(&Core) -> &Wake, work: fn(&Arc<Core>)) {
    loop {
        of(&core).woken().await;
        work(&core);
    }
}

/// The earlier of two moments, either of which may be none.
pub(crate) fn sooner(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
