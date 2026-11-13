use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::{CoreError, CoreResult};

/// Counts launching, live and retained sessions, so the PTY budget holds even
/// while a session is between being spawned and being published.
pub struct Capacity {
    pub active: AtomicUsize,
    pub limit: usize,
}

pub struct CapacityPermit(Arc<Capacity>);

impl Capacity {
    pub fn try_acquire(self: &Arc<Self>) -> CoreResult<CapacityPermit> {
        self.active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limit).then_some(active + 1)
            })
            .map(|_| CapacityPermit(self.clone()))
            .map_err(|_| CoreError::from("pty: PTY capacity reached"))
    }
}

impl Drop for CapacityPermit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}
