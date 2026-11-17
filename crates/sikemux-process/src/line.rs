//! A first-come line of those waiting for something only so many use at once, each keeping its
//! place while it asks again within a while, and the lease a holder keeps while it uses it. The
//! simulators agents attach wait on it; the core's line of heavy commands and things keeps the
//! same rules.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Places {
    places: VecDeque<Place>,
}

pub struct Place {
    pub who: String,
    pub joined: Instant,
    pub seen: Instant,
}

/// Where one waiter stands after asking.
pub struct Taken {
    /// How many wait ahead of it.
    pub ahead: usize,
    pub joined: Instant,
    /// Whether anyone joined or left the line, so those watching it should hear.
    pub moved: bool,
}

impl Places {
    /// `who`'s place, joining the line or keeping its place; those that have not asked within
    /// `kept` lose theirs.
    pub fn take(&mut self, who: &str, kept: Duration) -> Taken {
        let now = Instant::now();
        let before = self.places.len();
        (self.places).retain(|place| place.who == who || place.seen.elapsed() < kept);
        let ahead = match self.places.iter().position(|place| place.who == who) {
            Some(ahead) => ahead,
            None => {
                self.places.push_back(Place {
                    who: who.to_owned(),
                    joined: now,
                    seen: now,
                });
                self.places.len() - 1
            }
        };
        let place = &mut self.places[ahead];
        place.seen = now;
        Taken {
            ahead,
            joined: place.joined,
            moved: self.places.len() != before,
        }
    }

    /// Takes `who` out of the line, saying whether it was in it.
    pub fn leave(&mut self, who: &str) -> bool {
        let before = self.places.len();
        self.places.retain(|place| place.who != who);
        self.places.len() != before
    }

    /// Those still waiting, first in line first, with how many wait ahead of each.
    pub fn waiting(&self, kept: Duration) -> impl Iterator<Item = (usize, &Place)> {
        (self.places.iter().enumerate()).filter(move |(_, place)| place.seen.elapsed() < kept)
    }
}

/// Whether a holder that last used what it holds at `used` has gone without it for `lease`, so
/// it gives it up to someone waiting.
pub fn lapsed(used: Instant, lease: Duration) -> bool {
    used.elapsed() >= lease
}
