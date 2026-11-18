//! What the land queue keeps per base, and the one checkout its checks take turns in.

/// How many changes one check takes in.
pub const MAX_TOGETHER: usize = 8;

/// Waits for the queues' checkout, which one check uses at a time. A check of
/// commits bound for the remote goes before any check that only lands on a
/// local branch, such as a teammate's land into its lead's checkout.
pub async fn checkout_turn(to_remote: bool) -> tokio::sync::MutexGuard<'static, ()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CHECKOUT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static PUSHING: AtomicUsize = AtomicUsize::new(0);
    static PUSHED: tokio::sync::Notify = tokio::sync::Notify::const_new();
    if to_remote {
        PUSHING.fetch_add(1, Ordering::SeqCst);
        let turn = CHECKOUT.lock().await;
        PUSHING.fetch_sub(1, Ordering::SeqCst);
        PUSHED.notify_waiters();
        return turn;
    }
    loop {
        let pushed = PUSHED.notified();
        if PUSHING.load(Ordering::SeqCst) > 0 {
            pushed.await;
            continue;
        }
        let turn = CHECKOUT.lock().await;
        if PUSHING.load(Ordering::SeqCst) == 0 {
            return turn;
        }
    }
}

/// What the land queue keeps about one base while the core runs.
#[derive(Default)]
pub struct Lane {
    pub running: bool,
    /// A check of several failed, so each goes by itself until one lands.
    pub one_at_a_time: bool,
}

impl Lane {
    /// How many of `waiting` lands, from the front, the next check takes together.
    pub fn take(&self, waiting: usize) -> usize {
        match self.one_at_a_time {
            true => waiting.min(1),
            false => waiting.min(MAX_TOGETHER),
        }
    }
}
