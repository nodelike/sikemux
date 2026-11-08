//! What a card's Auto run did and which of the board's switches were on for
//! it, so a run with a switch can be compared with one without.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::BoardSettings;
use super::blind::Verdict;

const MAX_FOUND: usize = 100;
use crate::server::Core;

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
    /// How many times Auto brought its agent back without the person.
    pub recoveries: u32,
    /// What each of those answered.
    pub recovery_causes: Vec<String>,
    /// How many runs of the project's check took in its commits, and how long they took.
    pub check_runs: u32,
    pub check_ms: u64,
    /// Searches for the commit that broke the check that took in its commits.
    pub culprit_searches: u32,
    /// Of those searches, how many named one commit.
    pub culprits_found: u32,
    pub culprit_steps: u32,
}

/// One run of a project's check, for the run records of the agents it took in.
pub(crate) struct CheckRan<'a> {
    pub took_ms: u64,
    pub passed: bool,
    pub authors: Vec<&'a String>,
    pub culprit_agents: &'a [String],
    pub found: bool,
    pub searched: bool,
    pub steps: u32,
}

impl CheckRan<'_> {
    /// Whether `agent`'s work is what broke the check: the commit found was
    /// theirs, or nobody could be found and only their commits were in the run.
    fn broken_by(&self, agent: &str) -> bool {
        !self.passed
            && match self.found {
                true => self.culprit_agents.iter().any(|named| named == agent),
                false => self.authors.iter().all(|author| author.as_str() == agent),
            }
    }
}

/// Adds `ran` to the latest run record of each agent it took in.
pub(crate) fn record_check(core: &Core, project: &str, ran: &CheckRan<'_>) {
    let mut state = core.boards.state.locked();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let mut agents: Vec<&String> = ran.authors.clone();
    agents.sort_unstable();
    agents.dedup();
    let mut recorded = false;
    for agent in agents {
        let latest = board
            .cards
            .iter_mut()
            .filter(|card| card.owner_id() == Some(agent.as_str()))
            .filter_map(|card| card.record.as_mut())
            .max_by_key(|record| record.started_at);
        let Some(record) = latest else {
            continue;
        };
        record.check_runs += 1;
        record.check_ms += ran.took_ms;
        record.culprit_searches += u32::from(ran.searched);
        record.culprits_found += u32::from(ran.found);
        record.culprit_steps += ran.steps;
        record.broke_after_landing |= ran.broken_by(agent);
        recorded = true;
    }
    if recorded {
        core.boards.save(&mut state, project);
    }
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
