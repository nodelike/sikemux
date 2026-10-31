//! What a slice waits on now and roughly how long that takes, worked out here
//! so nobody has to ask: the project's check, another card, the person, or its
//! owner mid-turn. A check's estimate comes from how long the project's last
//! few checks took, kept while the core runs. The board tells clients again
//! whenever a wait starts, ends, or runs past its estimate.

use std::collections::{HashMap, VecDeque};

use sikemux_process::clock::epoch_ms;
use sikemux_wire::slice::{CheckRun, Verdict, Wait, WaitOn};

use super::auto::mid_turn;
use super::{Board, Card, State, Status};
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
}

struct Running {
    covered: Vec<String>,
    at: u64,
    run: CheckRun,
}

impl Checking {
    pub(super) fn started(&mut self, covered: Vec<String>, run: CheckRun) {
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
        let mut sorted: Vec<u64> = times.iter().copied().collect();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied()
    }
}

fn wait(on: WaitOn, since: u64, estimate_ms: Option<u64>, now: u64) -> Wait {
    Wait {
        on,
        since,
        estimate_ms,
        overdue: estimate_ms.is_some_and(|estimate| since + estimate < now),
    }
}

/// What `card`'s slice waits on now, if anything.
pub(super) fn of(core: &Core, board: &Board, card: &Card, now: u64) -> Option<Wait> {
    if card.status == Status::Done {
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
    if slice.waits_on_person().is_some() {
        return Some(wait(
            WaitOn::Person { gate },
            slice.waiting_since(),
            None,
            now,
        ));
    }
    if let Some(sha) = slice.awaits_check() {
        let running = (board.checking.running.as_ref())
            .filter(|running| running.covered.iter().any(|covered| covered == sha));
        let (run, since) = match running {
            Some(running) => (running.run.clone(), running.at),
            None => (CheckRun::Queued, slice.waiting_since()),
        };
        let estimate = board.checking.took(&run);
        let on = WaitOn::Check {
            sha: sha.to_owned(),
            run,
        };
        return Some(wait(on, since, estimate, now));
    }
    let owner = card.owner_id().filter(|owner| mid_turn(core, owner))?;
    let since = (core.seen.started_at(owner))
        .or_else(|| core.seen.active_at(owner))
        .unwrap_or_else(|| slice.waiting_since());
    Some(wait(
        WaitOn::Agent {
            agent: owner.to_owned(),
        },
        since,
        None,
        now,
    ))
}

/// Notes which boards' waits changed since clients were last told, and
/// answers when the next one runs past its estimate.
pub(super) fn settle(core: &Core, state: &mut State) -> Option<u64> {
    let now = epoch_ms();
    let mut next: Option<u64> = None;
    let State { boards, dirty, .. } = state;
    for board in boards.values_mut() {
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
        if moved(&board.told_waits, &waits) {
            board.told_waits = waits;
            dirty.insert(board.project.clone());
        }
    }
    next
}

/// Whether a wait began or ended, or what one is on, its estimate or whether
/// it is overdue changed; when one started alone never tells clients again.
fn moved(told: &HashMap<u64, Wait>, now: &HashMap<u64, Wait>) -> bool {
    told.len() != now.len()
        || now.iter().any(|(card, wait)| {
            told.get(card).is_none_or(|told| {
                told.on != wait.on
                    || told.estimate_ms != wait.estimate_ms
                    || told.overdue != wait.overdue
            })
        })
}
