//! Stops a page that keeps asking for something, like a popup or a dialog, in
//! a loop. A few requests close together pass; past that the page is refused
//! until it has gone quiet for a while.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct Burst {
    limit: usize,
    window: Duration,
    recent: VecDeque<Instant>,
    refused_until: Option<Instant>,
}

impl Burst {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            limit,
            window,
            recent: VecDeque::with_capacity(limit),
            refused_until: None,
        }
    }

    /// Whether a request made at `now` may go ahead. Each refused request
    /// pushes the quiet period out again, so a loop stays refused while it runs.
    pub fn admit(&mut self, now: Instant) -> bool {
        if self.refused_until.is_some_and(|until| now < until) {
            self.refused_until = Some(now + self.window);
            return false;
        }
        self.refused_until = None;
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= self.window)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= self.limit {
            self.recent.clear();
            self.refused_until = Some(now + self.window);
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(10);

    #[test]
    fn a_few_requests_pass_and_a_loop_is_refused_until_it_stops() {
        let start = Instant::now();
        let mut burst = Burst::new(3, WINDOW);
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        assert!(burst.admit(at(0)));
        assert!(burst.admit(at(1)));
        assert!(burst.admit(at(2)));
        assert!(!burst.admit(at(3)));
        assert!(!burst.admit(at(12)));
        assert!(!burst.admit(at(21)));
        assert!(burst.admit(at(31)));
    }

    #[test]
    fn requests_spread_out_never_trip_it() {
        let start = Instant::now();
        let mut burst = Burst::new(3, WINDOW);
        for step in 0..20 {
            assert!(burst.admit(start + Duration::from_secs(step * 4)));
        }
    }
}
