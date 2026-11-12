//! How long the waits agents end their turns on usually take, kept through a restart: the last
//! few waits on the same thing, such as CI on one repository, and from the third on an estimate
//! a wait's clock shows.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::kept::Kept;

const FILE: &str = "waits.json";
/// Waits kept per thing waited on.
const KEPT: usize = 10;
/// An estimate shows only once this many waits on the same thing are known.
const KNOWN_AFTER: usize = 3;
/// Things waited on that are kept, the least recently waited on going first.
const MAX_THINGS: usize = 256;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    /// How long each wait took, in milliseconds, oldest first, by what it was on.
    took: BTreeMap<String, VecDeque<u64>>,
    /// What was waited on, least recently first.
    order: VecDeque<String>,
}

pub(crate) struct Waits {
    kept: Kept,
    state: Mutex<Saved>,
}

impl Waits {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let state = Mutex::new(kept.load());
        Self { kept, state }
    }

    /// A wait on `on` ended after `ms`.
    pub(crate) fn took(&self, on: &str, ms: u64) {
        let mut state = self.state.locked();
        state.order.retain(|known| known != on);
        state.order.push_back(on.to_owned());
        while state.order.len() > MAX_THINGS {
            if let Some(oldest) = state.order.pop_front() {
                state.took.remove(&oldest);
            }
        }
        let times = state.took.entry(on.to_owned()).or_default();
        if times.len() >= KEPT {
            times.pop_front();
        }
        times.push_back(ms);
        self.kept.save_soon(&*state);
    }

    /// The middle of the last waits on `on`, once there were enough of them.
    pub(crate) fn expected(&self, on: &str) -> Option<u64> {
        let state = self.state.locked();
        let times = state
            .took
            .get(on)
            .filter(|times| times.len() >= KNOWN_AFTER)?;
        let mut sorted: Vec<u64> = times.iter().copied().collect();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied()
    }
}
