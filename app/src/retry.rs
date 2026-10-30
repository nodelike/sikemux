use std::time::Duration;

use gpui_kit::{Context, Task};

/// Waits that double after each failure, from `first` up to `longest`, so a call that keeps
/// failing is never asked again in a tight loop.
pub struct Backoff {
    first: Duration,
    longest: Duration,
    tries: u32,
}

impl Backoff {
    pub const fn new(first: Duration, longest: Duration) -> Self {
        Self {
            first,
            longest,
            tries: 0,
        }
    }

    /// How long to wait before the next try, each one twice as long as the last.
    pub fn next(&mut self) -> Duration {
        let wait = self
            .first
            .saturating_mul(1 << self.tries.min(16))
            .min(self.longest);
        self.tries = self.tries.saturating_add(1);
        wait
    }

    /// It worked, so the next failure waits the shortest time again.
    pub fn reset(&mut self) {
        self.tries = 0;
    }
}

/// Asks a refused call again after a wait that doubles each time, from a second up to a minute.
pub struct Retry {
    backoff: Backoff,
    waiting: Option<Task<()>>,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            backoff: Backoff::new(Duration::from_secs(1), Duration::from_secs(60)),
            waiting: None,
        }
    }
}

impl Retry {
    /// The call was answered, so the next refusal waits the shortest time again.
    pub fn answered(&mut self) {
        self.backoff.reset();
    }

    /// Runs `again` once the wait is over, unless a retry is already waiting.
    pub fn refused<T: 'static>(
        &mut self,
        cx: &mut Context<T>,
        retry: fn(&mut T) -> &mut Retry,
        again: impl FnOnce(&mut T, &mut Context<T>) + 'static,
    ) {
        if self.waiting.is_some() {
            return;
        }
        let wait = self.backoff.next();
        self.waiting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                retry(this).waiting = None;
                again(this, cx);
            });
        }));
    }
}
