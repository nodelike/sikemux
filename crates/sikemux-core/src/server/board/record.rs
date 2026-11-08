//! What a card's Auto run did and which of the board's switches were on for
//! it, so a run with a switch can be compared with one without.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::BoardSettings;
use super::blind::Verdict;

const MAX_FOUND: usize = 100;
const MAX_TRIP_CAUSES: usize = 8;
const MAX_CAUSE_CHARS: usize = 300;
const MAX_APPROVAL_LINES: usize = 20;
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
    /// Its latest reviewer approved it, and what that reviewer said of it.
    pub approved: bool,
    pub approval_said: Vec<String>,
    pub broke_after_landing: bool,
    /// The folders the change it had reviewed touched.
    pub areas: Vec<String>,
    /// How much review its latest change was rated to need, and why.
    pub review_why: Option<String>,
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
    /// How many times the progress guard held it, and why each time.
    pub progress_trips: u32,
    pub progress_trip_causes: Vec<String>,
    /// How long it waited in the merge queue, over all its times in it.
    pub queue_ms: u64,
    /// How many times the queue sent it back to be fixed or reviewed again.
    pub queue_bounces: u32,
    /// The card held an item of the project's reservoir.
    pub from_reservoir: bool,
    /// How many runs of the project's reservoir its landing started, and how long they took.
    pub reservoir_runs: u32,
    pub reservoir_ms: u64,
    /// Of those runs, how many failed and left the board as it was.
    pub reservoir_failures: u32,
    /// The cards those runs added, and those they closed as no longer listed.
    pub reservoir_added: u32,
    pub reservoir_closed: u32,
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
    pub text: &'a str,
}

impl CheckRan<'_> {
    /// Whether `agent`'s work is what broke the check: the commit found was
    /// theirs, or nobody could be found and only their commits were in the run.
    pub(super) fn broken_by(&self, agent: &str) -> bool {
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
    let mut recorded = super::guard::checked(core, board, ran);
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

fn approval_lines(verdict: &Verdict) -> Vec<String> {
    let graded = (verdict.criteria.iter())
        .filter(|graded| !graded.note.is_empty())
        .map(|graded| format!("{}: {}", graded.id, graded.note));
    let used = (verdict.exercised.iter()).map(|used| format!("Ran {}: {}", used.what, used.saw));
    let nothing = (verdict.nothing_to_run.iter()).map(|why| format!("Nothing to run: {why}"));
    graded
        .chain(used)
        .chain(nothing)
        .take(MAX_APPROVAL_LINES)
        .map(|line| super::recover::clipped(&line, MAX_CAUSE_CHARS))
        .collect()
}

impl RunRecord {
    pub(super) fn begun(settings: &BoardSettings, from_reservoir: bool) -> Self {
        Self {
            switches: settings.switches(),
            started_at: epoch_ms(),
            from_reservoir,
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
        self.approved = verdict.approve;
        self.approval_said = if verdict.approve {
            approval_lines(verdict)
        } else {
            Vec::new()
        };
    }

    pub(super) fn tripped(&mut self, why: &str) {
        self.progress_trips += 1;
        if self.progress_trip_causes.len() < MAX_TRIP_CAUSES {
            self.progress_trip_causes
                .push(super::recover::clipped(why, MAX_CAUSE_CHARS));
        }
    }

    pub(super) fn finished(&mut self, rounds: u32, cost_usd: Option<f64>) {
        self.finished_at = Some(epoch_ms());
        self.rounds = rounds;
        self.cost_usd = cost_usd.or(self.cost_usd);
    }
}
