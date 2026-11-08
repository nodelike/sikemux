//! What a card's Auto run did and which of the board's switches were on for
//! it, so a run with a switch can be compared with one without.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;

use super::auto::BoardSettings;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct RunRecord {
    /// Each switch by name, as it stood when the run started.
    pub switches: BTreeMap<String, bool>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub cost_usd: Option<f64>,
    /// How many times its agent was sent back to change its work.
    pub rounds: u32,
    /// What reviewers asked it to change, added up over the rounds.
    pub findings: u32,
    pub broke_after_landing: bool,
    pub overruled: bool,
}

impl RunRecord {
    pub(super) fn begun(settings: &BoardSettings) -> Self {
        Self {
            switches: settings.switches(),
            started_at: epoch_ms(),
            ..Self::default()
        }
    }

    pub(super) fn finished(&mut self, rounds: u32) {
        self.finished_at = Some(epoch_ms());
        self.rounds = rounds;
    }
}
