//! What a slice waits on now and roughly how long that takes, worked out here
//! so nobody has to ask: the project's check, another card, the person, at a
//! gate of theirs or to approve a command, or its owner mid-turn. A check's estimate comes from how long the project's last
//! few checks took, kept while the core runs; an owner's from how long the
//! board's slices took at that gate before. The board tells clients again
//! whenever a wait starts, ends, or runs past its estimate.

use std::collections::{HashMap, VecDeque};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::slice::{CheckRun, Verdict, Wait, WaitOn};

use super::{eta, Board, Card, CardStatus, State};
use crate::server::notify::GoalMoved;
use crate::server::Core;

/// Run times kept per project and kind of check.
const KEPT_TIMES: usize = 10;

/// A project's check as the board heard of it: the one running now, and how
/// long the last few took.
#[derive(Default)]
pub(super) struct Checking {
    running: Option<Running>,
    local: VecDeque<u64>,
    ci: VecDeque<u64>,
    /// Its latest check ran in CI.
    in_ci: bool,
}

struct Running {
    covered: Vec<String>,
    at: u64,
    run: CheckRun,
}

impl Checking {
    pub(super) fn started(&mut self, covered: Vec<String>, run: CheckRun) {
        self.in_ci = matches!(run, CheckRun::Ci { .. });
        self.running = Some(Running {
            covered,
            at: epoch_ms(),
            run,
        });
    }

    /// The check that took in `covered` ended; a run that gave a verdict
    /// counts toward how long the next one takes.
    pub(super) fn ended(&mut self, covered: &[String], verdict: Option<Verdict>) {
        let Some(running) = self
            .running
            .take_if(|running| running.covered.iter().any(|sha| covered.contains(sha)))
        else {
            return;
        };
        if verdict.is_none() {
            return;
        }
        let times = match running.run {
            CheckRun::Ci { .. } => &mut self.ci,
            CheckRun::Local | CheckRun::Queued => &mut self.local,
        };
        if times.len() >= KEPT_TIMES {
            times.pop_front();
        }
        times.push_back(epoch_ms().saturating_sub(running.at));
    }

    /// The middle of the last few run times of that kind.
    fn took(&self, run: &CheckRun) -> Option<u64> {
        let times = match run {
            CheckRun::Ci { .. } => &self.ci,
            CheckRun::Local | CheckRun::Queued => &self.local,
        };
        eta::middle(times.iter().copied().collect())
    }

    /// How long the check its latest run was of usually takes.
    pub(super) fn usual(&self) -> Option<u64> {
        match self.in_ci {
            true => self.took(&CheckRun::Ci {
                workflow: String::new(),
                url: None,
            }),
            false => self.took(&CheckRun::Local),
        }
    }

    pub(super) fn in_ci(&self) -> bool {
        self.in_ci
    }
}

fn wait(on: WaitOn, since: u64, ends: Option<u64>, now: u64) -> Wait {
    Wait {
        on,
        since,
        estimate_ms: ends.map(|ends| ends.saturating_sub(since)),
        overdue: ends.is_some_and(|ends| ends < now),
    }
}

pub(super) fn of(core: &Core, board: &Board, card: &Card, now: u64) -> Option<Wait> {
    if card.status == CardStatus::Done {
        return None;
    }
    let slice = card.slice.as_ref()?;
    let gate = slice.waiting()?;
    if let Some(other) = board.waits_on(card).first() {
        return Some(wait(
            WaitOn::Card { card: *other },
            card.updated_at,
            None,
            now,
        ));
    }
    if slice.waits_on_person(&board.settings).is_some() {
        return Some(wait(
            WaitOn::Person {
                gate: gate.gate(),
                environment: gate.environment().map(str::to_owned),
            },
            slice.waiting_since(),
            None,
            now,
        ));
    }
    if let Some(sha) = slice.awaits_check(&board.settings) {
        let running = (board.checking.running.as_ref())
            .filter(|running| running.covered.iter().any(|covered| covered == sha));
        let queued = core
            .checks
            .queued(&board.project, sha)
            .then(|| (CheckRun::Queued, slice.waiting_since()));
        let on_its_way = running
            .map(|running| (running.run.clone(), running.at))
            .or(queued);
        if let Some((run, since)) = on_its_way {
            let ends = board.checking.took(&run).map(|took| since + took);
            let on = WaitOn::Check {
                sha: sha.to_owned(),
                run,
            };
            return Some(wait(on, since, ends, now));
        }
    }
    if slice.awaits_approval(&board.settings) {
        return Some(wait(
            WaitOn::Person {
                gate: gate.gate(),
                environment: gate.environment().map(str::to_owned),
            },
            slice.waiting_since(),
            None,
            now,
        ));
    }
    let owner = card
        .owner_id()
        .filter(|owner| core.agent_now(owner).mid_turn())?;
    let since = (core.seen.started_at(owner))
        .or_else(|| core.seen.active_at(owner))
        .unwrap_or_else(|| slice.waiting_since());
    let ends = (slice.working_on_since(gate))
        .zip(board.took.of_gate(gate))
        .map(|(from, took)| from + took)
        .filter(|ends| *ends >= since);
    Some(wait(
        WaitOn::Agent {
            agent: owner.to_owned(),
        },
        since,
        ends,
        now,
    ))
}

/// Notes which boards' waits and goals' ETAs changed since clients were last
/// told. Answers when the next wait runs past its estimate or ETA comes due,
/// and how goals moved since the person last heard.
pub(super) fn settle(core: &Core, state: &mut State) -> (Option<u64>, Vec<GoalMoved>) {
    let now = epoch_ms();
    let mut next: Option<u64> = None;
    let mut moved_goals = Vec::new();
    let State { boards, dirty, .. } = state;
    for board in boards.values_mut() {
        board.place_cards();
        board.took = eta::Took::of(board);
        let waits: HashMap<u64, Wait> = board
            .cards
            .iter()
            .filter_map(|card| Some((card.id, of(core, board, card, now)?)))
            .collect();
        for wait in waits.values().filter(|wait| !wait.overdue) {
            if let Some(estimate) = wait.estimate_ms {
                let due = wait.since + estimate + 1;
                next = Some(next.map_or(due, |next| next.min(due)));
            }
        }
        let promised = eta::settle(board, &waits, now);
        if let Some(due) = promised.due {
            next = Some(next.map_or(due, |next| next.min(due)));
        }
        if promised.changed {
            dirty.insert(board.project.clone());
        }
        moved_goals.extend(eta::moved(board, promised.slipped));
        if moved(&board.told_waits, &waits) {
            board.told_waits = waits;
            dirty.insert(board.project.clone());
        }
        board.forget_places();
    }
    (next, moved_goals)
}

/// The owners of slices that wait on the project's check, each with when that wait began and how
/// long the check usually takes.
pub(crate) fn owners_on_check(core: &Core) -> HashMap<String, (u64, Option<u64>)> {
    let state = core.boards.state.locked();
    let mut owners = HashMap::new();
    for board in state.boards.values() {
        for (card, wait) in &board.told_waits {
            if !matches!(wait.on, WaitOn::Check { .. }) {
                continue;
            }
            let owner = (board.cards.iter())
                .find(|known| known.id == *card)
                .and_then(Card::owner_id);
            if let Some(owner) = owner {
                owners.insert(owner.to_owned(), (wait.since, wait.estimate_ms));
            }
        }
    }
    owners
}

/// Whether a wait began or ended, or what one is on, when it should end or
/// whether it is overdue changed.
fn moved(told: &HashMap<u64, Wait>, now: &HashMap<u64, Wait>) -> bool {
    told.len() != now.len()
        || now.iter().any(|(card, wait)| {
            told.get(card).is_none_or(|told| {
                told.on != wait.on || told.eta() != wait.eta() || told.overdue != wait.overdue
            })
        })
}
