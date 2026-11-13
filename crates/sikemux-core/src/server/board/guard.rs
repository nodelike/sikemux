//! Auto's progress guard. Money alone never stops a card: what trips it is money or agent time
//! spent since the run last got anywhere, or the same failure coming back. A trip holds only
//! that card and puts one fresh agent on it; if that agent gets no further, the card goes to
//! the person. Real progress is a check passing on new commits or a criterion newly met.
//! A landing ends the run, so nothing needs counting after one.

use crate::server::agent_now::AgentNowCore;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::{AutoRun, RunHold, RunStage};
use super::blind::Verdict;
use super::finish::{self, Step};
use super::record::CheckRan;
use super::recover;
use super::{Board, Card};
use crate::server::codehost::log::failure_lines;
use crate::server::inbox::Fault;
use crate::server::Core;

const START_LIMIT_USD: f64 = 50.0;
const LOWEST_LIMIT_USD: f64 = 20.0;
const HIGHEST_LIMIT_USD: f64 = 150.0;
const RECORDS_TO_SCALE: usize = 5;
const TIMES_MEDIAN_COST: f64 = 2.0;
/// Agent time without progress that trips a run whose CLI reports no cost.
const WORK_LIMIT_MS: u64 = 4 * 60 * 60 * 1000;
const LOOK_EVERY_MS: u64 = 10 * 60 * 1000;
/// A round is the agent being told what failed and answering with a fix.
const ROUNDS_THAT_TRIP: u32 = 2;
/// Commits are cheaper than rounds, and an agent often lands a few on the way to a fix.
const CHECKS_THAT_TRIP: u32 = 3;
const KEY_LINES: usize = 3;
const MAX_REASON_CHARS: usize = 1_500;
const CLI_THAT_REPORTS_COST: &str = "claude";

/// What a run has spent, and what it has to show for it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Burn {
    /// What each agent last said it had cost, since each one counts from nothing.
    reported: BTreeMap<String, f64>,
    total_usd: f64,
    /// Since its last progress.
    usd: f64,
    work_ms: u64,
    looked: u64,
    failure: Option<String>,
    repeats: u32,
    passed_head: Option<String>,
    /// The criteria a reviewer has passed so far.
    passing: BTreeSet<String>,
    fresh_tried: bool,
}

impl Burn {
    fn heard(&mut self, agent: &str, cost: f64) -> f64 {
        let seen = self.reported.insert(agent.to_owned(), cost).unwrap_or(0.0);
        let added = if cost >= seen { cost - seen } else { cost };
        self.total_usd += added;
        self.usd += added;
        added
    }

    pub(super) fn start_over(&mut self) {
        self.usd = 0.0;
        self.work_ms = 0;
        self.failure = None;
        self.repeats = 0;
        self.fresh_tried = false;
    }

    /// How many times in a row the failure known by `key` has come.
    fn failed(&mut self, key: String) -> u32 {
        if self.failure.as_deref() == Some(key.as_str()) {
            self.repeats += 1;
        } else {
            self.failure = Some(key);
            self.repeats = 1;
        }
        self.repeats
    }

    /// A fresh agent gets its own allowance, but the same failure still counts against it.
    fn handed_on(&mut self) {
        self.usd = 0.0;
        self.work_ms = 0;
        self.repeats = self.repeats.min(1);
        self.fresh_tried = true;
    }

    /// What its agents have reported costing, none when no CLI of theirs reports it.
    pub(super) fn spent(&self) -> Option<f64> {
        (!self.reported.is_empty()).then_some(self.total_usd)
    }

    fn over_money(&self, limit: f64) -> Option<String> {
        (self.usd >= limit).then(|| {
            format!(
                "It spent ${:.2} with no check passing and no criterion newly met, past the ${limit:.0} Auto allows in this project",
                self.usd
            )
        })
    }
}

/// What a run may spend without progress: a fixed sum until the project has finished enough
/// cards to say what one usually costs.
fn limit_usd(board: &Board) -> f64 {
    let mut costs: Vec<f64> = (board.cards.iter())
        .filter_map(|card| card.record.as_ref())
        .filter(|record| record.finished_at.is_some())
        .filter_map(|record| record.cost_usd)
        .collect();
    if costs.len() < RECORDS_TO_SCALE {
        return START_LIMIT_USD;
    }
    costs.sort_by(f64::total_cmp);
    (costs[costs.len() / 2] * TIMES_MEDIAN_COST).clamp(LOWEST_LIMIT_USD, HIGHEST_LIMIT_USD)
}

fn limit_of(core: &Core, project: &str) -> f64 {
    let state = core.boards().state.locked();
    state.boards.get(project).map_or(START_LIMIT_USD, limit_usd)
}

fn failure_key(what: &str) -> String {
    let (named, _) = failure_lines(what, KEY_LINES);
    match named.is_empty() {
        true => recover::sameness(what),
        false => recover::sameness(&named.join("\n")),
    }
}

fn trip(core: &Core, card: &mut Card, run: &mut AutoRun, why: String) {
    if let Some(record) = card.record.as_mut() {
        record.tripped(&why);
    }
    let why = recover::clipped(&why, MAX_REASON_CHARS);
    if run.burn.fresh_tried {
        let note = format!("Auto held it for you, as a fresh agent got no further. {why}");
        finish::mark_held(core, card, run, RunHold::Fix, note);
    } else {
        run.burn.handed_on();
        if run.stage != RunStage::Working {
            run.stage = RunStage::Fixing;
        }
        run.note = Some(format!("Auto held it and put a fresh agent on it. {why}"));
        recover::take_over(run, why);
    }
}

fn at_work_stage(run: &AutoRun) -> bool {
    run.held.is_none()
        && run.recovery.due.is_none()
        && matches!(run.stage, RunStage::Working | RunStage::Fixing)
}

/// Adds what the agent's CLI last reported to its run and to the project, and trips the run
/// when one of its workers has taken it past the limit. Its lead's turn ending is left alone,
/// since its work ships next and the check on it is what says whether it got anywhere.
pub(super) fn turn_ended(core: &Core, agent: &str) {
    if let Some(cost) = core
        .chats()
        .get(agent)
        .and_then(|chat| chat.feed.cost_usd())
    {
        heard_cost(core, agent, cost);
    }
}

/// What `agent`'s CLI last said it cost, here or on a linked host.
pub(super) fn heard_cost(core: &Core, agent: &str, cost: f64) {
    let lead = core.names().team_of(agent);
    let mut state = core.boards().state.locked();
    let found = state.boards.iter().find_map(|(project, board)| {
        let index = board.cards.iter().position(|card| {
            card.kept_run().is_some_and(|run| {
                run.agent == lead
                    || (run.reviewer.as_ref()).is_some_and(|reviewer| reviewer.agent == agent)
            })
        })?;
        Some((project.clone(), index))
    });
    let Some((project, index)) = found else {
        return;
    };
    let Some(board) = state.boards.get_mut(&project) else {
        return;
    };
    let limit = limit_usd(board);
    let card = &mut board.cards[index];
    let Some(mut run) = card.auto.take() else {
        return;
    };
    let added = run.burn.heard(agent, cost);
    let worker = agent != run.agent && lead == run.agent;
    let why = (worker && at_work_stage(&run))
        .then(|| run.burn.over_money(limit))
        .flatten();
    let tripped = why.is_some();
    if let Some(why) = why {
        trip(core, card, &mut run, why);
    }
    card.auto = Some(run);
    board.auto_spent_usd += added;
    if added > 0.0 || tripped {
        core.boards().save(&mut state, &project);
    }
}

/// A check passed on the run's pushed commit, which is progress when that commit is new.
pub(super) fn check_passed(core: &Core, step: &Step, head: &str) {
    finish::change_run(core, &step.project, step.id, step.agent(), |_, run| {
        if run.burn.passed_head.as_deref() != Some(head) {
            run.burn.passed_head = Some(head.to_owned());
            run.burn.start_over();
        }
    });
}

/// The checks on the run's pushed commit failed. True when that tripped it, so its agent is
/// not asked to fix them.
pub(super) fn check_failed(core: &Core, step: &Step, what: &str) -> bool {
    let limit = limit_of(core, &step.project);
    let mut tripped = false;
    finish::change_run(core, &step.project, step.id, step.agent(), |card, run| {
        let again = run.burn.failed(failure_key(what)) >= ROUNDS_THAT_TRIP;
        let why = match again {
            true => Some(format!(
                "The same failure came back after a fix:\n{}",
                crate::git::tail(what)
            )),
            false => run.burn.over_money(limit),
        };
        if let Some(why) = why {
            trip(core, card, run, why);
            tripped = true;
        }
    });
    tripped
}

/// A reviewer turned the change down. A criterion it newly passed is progress all the same.
/// True when the verdict tripped the run, so its agent is not asked for the changes.
pub(super) fn turned_down(
    core: &Core,
    step: &Step,
    verdict: &Verdict,
    findings: &[String],
) -> bool {
    let limit = limit_of(core, &step.project);
    let mut failing: Vec<&str> = (verdict.criteria.iter())
        .filter(|graded| !graded.pass)
        .map(|graded| graded.id.as_str())
        .collect();
    failing.sort_unstable();
    let mut tripped = false;
    finish::change_run(core, &step.project, step.id, step.agent(), |card, run| {
        let newly: Vec<String> = (verdict.criteria.iter())
            .filter(|graded| graded.pass && !run.burn.passing.contains(&graded.id))
            .map(|graded| graded.id.clone())
            .collect();
        if !newly.is_empty() {
            run.burn.passing.extend(newly);
            run.burn.start_over();
        }
        let again = !failing.is_empty() && run.burn.failed(failing.join(",")) >= ROUNDS_THAT_TRIP;
        let why = match again {
            true => Some(format!(
                "A reviewer failed the same criteria again:\n{}",
                findings.join("\n")
            )),
            false => run.burn.over_money(limit),
        };
        if let Some(why) = why {
            trip(core, card, run, why);
            tripped = true;
        }
    });
    tripped
}

/// The project's check ran on commits of `ran`'s authors: for each run they work for, a pass is
/// progress and a failure of theirs counts towards a trip. True when a run changed.
pub(super) fn checked(core: &Core, board: &mut Board, ran: &CheckRan<'_>) -> bool {
    let by_lead: Vec<(String, &String)> = (ran.authors.iter())
        .map(|author| (core.names().team_of(author), *author))
        .collect();
    let limit = limit_usd(board);
    let mut changed = false;
    for card in &mut board.cards {
        if card.run().is_none() {
            continue;
        }
        let Some(mut run) = card.auto.take() else {
            continue;
        };
        let mut authors = by_lead
            .iter()
            .filter(|(lead, _)| *lead == run.agent)
            .map(|(_, author)| author.as_str())
            .peekable();
        if authors.peek().is_some() {
            if ran.passed {
                run.burn.start_over();
                changed = true;
            } else if authors.any(|author| ran.broken_by(author)) && at_work_stage(&run) {
                let repeats = run.burn.failed(failure_key(ran.text));
                let why = match repeats >= CHECKS_THAT_TRIP {
                    true => Some(format!(
                        "The project's check failed the same way {repeats} times in a row:\n{}",
                        crate::git::tail(ran.text)
                    )),
                    false => run.burn.over_money(limit),
                };
                if let Some(why) = why {
                    trip(core, card, &mut run, why);
                }
                changed = true;
            }
        }
        card.auto = Some(run);
    }
    changed
}

fn prices_its_work(card: &Card, run: &AutoRun) -> bool {
    run.burn.spent().is_some()
        || (card.owner.as_ref()).and_then(|owner| owner.cli.as_deref())
            == Some(CLI_THAT_REPORTS_COST)
}

/// When agent time was last counted for the run, if it goes by time: its CLI reports no cost.
pub(super) fn timed_since(card: &Card, run: &AutoRun) -> Option<u64> {
    (at_work_stage(run) && !prices_its_work(card, run)).then_some(run.burn.looked)
}

/// Counts agent time for a run that goes by time, and trips it at the limit. Answers when to
/// look again.
pub(super) fn look(core: &Core, project: &str, id: u64, agent: &str, looked: u64) -> u64 {
    let now = epoch_ms();
    if looked + LOOK_EVERY_MS > now {
        return looked + LOOK_EVERY_MS;
    }
    let agent_now = core.agent_now(agent);
    let at_work = (agent_now.maybe_mid_turn() || agent_now.workers_busy(agent))
        && !agent_now.waits_on_something()
        && !matches!(core.inbox().fault(agent), Some(Fault::Retrying));
    finish::change_run(core, project, id, agent, |card, run| {
        if at_work && looked != 0 {
            run.burn.work_ms += now.saturating_sub(looked).min(2 * LOOK_EVERY_MS);
        }
        run.burn.looked = now;
        if run.burn.work_ms >= WORK_LIMIT_MS {
            let why = format!(
                "Its agents worked {} hours with no check passing and no criterion newly met; its CLI reports no cost, so Auto goes by time",
                run.burn.work_ms / (60 * 60 * 1000)
            );
            trip(core, card, run, why);
        }
    });
    now + LOOK_EVERY_MS
}

pub(super) fn ceiling_reached(board: &Board) -> bool {
    (board.settings.spend_ceiling).is_some_and(|ceiling| board.auto_spent_usd >= f64::from(ceiling))
}

/// Why Auto starts nothing new here, once the project's ceiling is reached.
pub(super) fn ceiling_note(board: &Board) -> Option<String> {
    ceiling_reached(board).then(|| {
        format!(
            "Auto has spent ${:.2} here, which reached this project's ${} ceiling, so it starts no new cards. Running ones finish, and raising the ceiling carries on",
            board.auto_spent_usd,
            board.settings.spend_ceiling.unwrap_or_default()
        )
    })
}
