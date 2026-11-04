//! When a goal's work should be through, promised only from what the core saw
//! the same work take before, and why when that moves later. A promise lives
//! while the goal has work under way and ends once only the person is left, so
//! the time they take never counts against it.

use std::collections::HashMap;

use sikemux_wire::slice::{Gate, GoalEta, Slip, Wait, WaitOn};

use super::{Board, Card};

/// A gate's times kept from the board's slices, the latest.
const KEPT_TIMES: usize = 10;
/// A gate's estimate needs this many slices to have passed it.
const FEWEST_TIMES: usize = 3;
/// An ETA moved later by less than this, or a tenth of how far off it was when
/// promised if that is more, has not slipped.
const LEAST_SLIP_MS: u64 = 2 * 60_000;

/// The middle of `times`.
pub(super) fn middle(mut times: Vec<u64>) -> Option<u64> {
    times.sort_unstable();
    times.get(times.len() / 2).copied()
}

/// How long the board's slices took to pass each gate an agent or the core
/// passes, read from the slices themselves so it outlives the core.
#[derive(Default)]
pub(super) struct Took {
    built: Option<u64>,
    landed: Option<u64>,
}

impl Took {
    pub(super) fn of(board: &Board) -> Self {
        let took = |gate: Gate| {
            let mut times: Vec<(u64, u64)> = (board.cards.iter())
                .filter_map(|card| card.slice.as_ref()?.took(gate))
                .collect();
            times.sort_unstable_by_key(|(at, _)| std::cmp::Reverse(*at));
            times.truncate(KEPT_TIMES);
            (times.len() >= FEWEST_TIMES)
                .then(|| middle(times.into_iter().map(|(_, took)| took).collect()))
                .flatten()
        };
        Self {
            built: took(Gate::Built),
            landed: took(Gate::Landed),
        }
    }

    pub(super) fn of_gate(&self, gate: Gate) -> Option<u64> {
        match gate {
            Gate::Built => self.built,
            Gate::Landed => self.landed,
            Gate::LooksRight => None,
        }
    }
}

/// What the core promised of a goal, while it has work under way.
pub(super) struct Promise {
    /// When it first promised.
    made: u64,
    promised: u64,
    at: Option<u64>,
    then_you: bool,
    slip: Option<Slip>,
    /// The latest the person was told it would be: only later than this is news.
    told: u64,
}

impl Promise {
    /// None while it has nothing to say: no estimate, and nothing slipped.
    pub(super) fn view(&self) -> Option<GoalEta> {
        (self.at.is_some() || self.slip.is_some()).then_some(GoalEta {
            at: self.at,
            promised: self.promised,
            then_you: self.then_you,
            slip: self.slip,
        })
    }

    fn margin(&self) -> u64 {
        LEAST_SLIP_MS.max(self.promised.saturating_sub(self.made) / 10)
    }

    /// Takes what the core expects now: `at`, or no estimate and `floor` as the
    /// soonest it could still be through. A slip is new when the estimate is later
    /// than the person was last told, or the first time there is none and the
    /// moment passed.
    fn update(&mut self, at: Option<u64>, then_you: Option<bool>, floor: u64, why: Slip) {
        let margin = self.margin();
        let news = match at {
            Some(at) => at > self.told + margin,
            None => self.slip.is_none() && floor > self.told + margin,
        };
        let slipped = match at {
            Some(at) => at > self.promised + margin,
            None => self.slip.is_some() || news,
        };
        if news {
            self.told = at.unwrap_or(floor);
        }
        self.slip = slipped.then_some(why);
        self.at = at;
        self.then_you = then_you.unwrap_or(self.then_you);
    }
}

/// How a slice stands against the time.
#[derive(Clone, Copy)]
enum Reach {
    /// It should get as far as it can without the person by this moment, with
    /// their gate next when `then_you`.
    By {
        at: u64,
        then_you: bool,
    },
    /// Nothing is left of it but the person.
    AtPerson,
    Done,
    Unknown,
}

/// How far `card`'s slice gets. `behind` holds the cards waiting on it, so a loop
/// of cards waiting on each other has no answer rather than none ending.
fn reach(board: &Board, waits: &HashMap<u64, Wait>, card: &Card, behind: &mut Vec<u64>) -> Reach {
    let Some(slice) = card.slice.as_ref() else {
        return Reach::Unknown;
    };
    let Some(gate) = slice.waiting() else {
        return Reach::Done;
    };
    let landing = board.settings.landing;
    let (ahead, then_you) = slice.ahead(gate, landing);
    let left: Option<u64> = ahead.iter().map(|gate| board.took.of_gate(*gate)).sum();
    let Some(wait) = waits.get(&card.id) else {
        return Reach::Unknown;
    };
    let until = match &wait.on {
        WaitOn::Person { .. } => return Reach::AtPerson,
        WaitOn::Card { .. } => {
            behind.push(card.id);
            let first = (board.waits_on(card).iter())
                .map(|other| match board.card(*other) {
                    Ok(other) if !behind.contains(&other.id) => {
                        match reach(board, waits, other, behind) {
                            Reach::By { at, .. } => Some(at),
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect::<Option<Vec<u64>>>();
            behind.pop();
            first
                .and_then(|ends| ends.into_iter().max())
                .zip(board.took.of_gate(gate))
                .map(|(at, took)| at + took)
        }
        WaitOn::Check { .. } | WaitOn::Agent { .. } => wait.eta().filter(|_| !wait.overdue),
    };
    match until.zip(left) {
        Some((until, left)) => Reach::By {
            at: until + left,
            then_you,
        },
        None => Reach::Unknown,
    }
}

/// How a goal's work stands against the time.
enum Outlook {
    By {
        at: u64,
        then_you: bool,
    },
    /// Nothing is left of it but the person, or nothing at all.
    Through,
    Unknown,
}

fn outlook(board: &Board, goal: &Card, waits: &HashMap<u64, Wait>) -> Outlook {
    let mut latest: Option<u64> = None;
    let mut you = false;
    for card in board.held_by_goal(goal) {
        match reach(board, waits, card, &mut Vec::new()) {
            Reach::By { at, then_you } => {
                latest = Some(latest.map_or(at, |latest| latest.max(at)));
                you |= then_you;
            }
            Reach::AtPerson => you = true,
            Reach::Done => {}
            Reach::Unknown => return Outlook::Unknown,
        }
    }
    match latest {
        Some(at) => Outlook::By { at, then_you: you },
        None => Outlook::Through,
    }
}

/// Why a goal runs late, from what its slices say now: what went wrong with
/// one since the promise first, then what one waits on.
fn why(board: &Board, goal: &Card, waits: &HashMap<u64, Wait>, made: u64) -> Slip {
    let held = board.held_by_goal(goal);
    let slices = || (held.iter()).filter_map(|card| Some((*card, card.slice.as_ref()?)));
    let unfinished = || slices().filter(|(_, slice)| slice.waiting().is_some());
    if let Some(rerunning) = unfinished().find_map(|(_, slice)| slice.failed_since(made)) {
        return Slip::CheckFailed {
            ci: board.checking.in_ci(),
            rerunning,
        };
    }
    if unfinished().any(|(card, _)| card.at > made) {
        return Slip::NewSlice;
    }
    if unfinished().any(|(_, slice)| slice.reworked_since(made)) {
        return Slip::NewCommit;
    }
    let landing = board.settings.landing;
    for (card, _) in unfinished() {
        if let Some(WaitOn::Card { card: other }) = waits.get(&card.id).map(|wait| &wait.on) {
            let on_person = (board.card(*other).ok())
                .and_then(|other| other.slice.as_ref())
                .is_some_and(|other| other.waits_on_person(landing).is_some());
            return match on_person {
                true => Slip::OnPerson,
                false => Slip::OnCard { card: *other },
            };
        }
    }
    if unfinished().any(|(card, _)| card.owner.is_none()) {
        return Slip::NoOwner;
    }
    Slip::TakingLonger
}

/// What a board's promises came to.
#[derive(Default)]
pub(super) struct Promised {
    pub due: Option<u64>,
    /// Clients have something new to show.
    pub changed: bool,
}

/// Holds each of the board's goals to what the core promised of it.
pub(super) fn settle(board: &mut Board, waits: &HashMap<u64, Wait>, now: u64) -> Promised {
    let mut promised = Promised::default();
    let goals: Vec<u64> = (board.cards.iter())
        .filter(|goal| !board.held_by_goal(goal).is_empty())
        .map(|goal| goal.id)
        .collect();
    let before = board.promises.len();
    board.promises.retain(|goal, _| goals.contains(goal));
    promised.changed = board.promises.len() != before;
    for id in goals {
        let Ok(outlook) = board.card(id).map(|goal| outlook(board, goal, waits)) else {
            continue;
        };
        let (at, then_you) = match outlook {
            Outlook::Through => {
                promised.changed |= board.promises.remove(&id).is_some();
                continue;
            }
            Outlook::Unknown => (None, None),
            Outlook::By { at, then_you } => {
                board.promises.entry(id).or_insert(Promise {
                    made: now,
                    promised: at,
                    at: None,
                    then_you,
                    slip: None,
                    told: at,
                });
                (Some(at), Some(then_you))
            }
        };
        let Some(made) = board.promises.get(&id).map(|promise| promise.made) else {
            continue;
        };
        let Ok(reason) = board.card(id).map(|goal| why(board, goal, waits, made)) else {
            continue;
        };
        // A failed check has to run again, so it is late by at least that much.
        let again = matches!(reason, Slip::CheckFailed { .. })
            .then(|| board.checking.usual())
            .flatten();
        let Some(promise) = board.promises.get_mut(&id) else {
            continue;
        };
        let shown = promise.view();
        promise.update(at, then_you, now + again.unwrap_or(0), reason);
        promised.changed |= promise.view() != shown;
        if promise.at.is_none() && promise.slip.is_none() {
            let due = promise.told + promise.margin() + 1;
            promised.due = Some(promised.due.map_or(due, |next| next.min(due)));
        }
    }
    promised
}
