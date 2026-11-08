//! What a card's Auto run did and which of the board's switches were on for
//! it, so a run with a switch can be compared with one without.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;

use super::auto::BoardSettings;
use super::blind::Verdict;

const MAX_FOUND: usize = 100;

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
    /// How many of its builder's contracts a reviewer looked at.
    pub contract_rounds: u32,
    /// How its contract ended, in the core's words.
    pub contract_outcome: Option<String>,
    /// How its reviewer graded the criteria, at its latest verdict.
    pub criteria_passed: u32,
    pub criteria_failed: u32,
    /// How many things its latest reviewer ran or used.
    pub exercised: u32,
    /// What its reviewers found, oldest first.
    pub reviewer_found: Vec<String>,
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

    pub(super) fn reviewed(&mut self, verdict: &Verdict, found: &[String]) {
        let passed = verdict.criteria.iter().filter(|graded| graded.pass).count() as u32;
        self.criteria_passed = passed;
        self.criteria_failed = verdict.criteria.len() as u32 - passed;
        self.exercised = verdict.exercised.len() as u32;
        self.reviewer_found.extend(found.iter().cloned());
        let over = self.reviewer_found.len().saturating_sub(MAX_FOUND);
        self.reviewer_found.drain(..over);
    }

    pub(super) fn finished(&mut self, rounds: u32, cost_usd: Option<f64>) {
        self.finished_at = Some(epoch_ms());
        self.rounds = rounds;
        self.cost_usd = cost_usd.or(self.cost_usd);
    }
}
