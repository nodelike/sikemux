//! How a goal stands as its slices do together: its state, steps and next gates.

use super::*;

/// Whether every slice left in the goal lost its owner, so nothing in it moves until the person
/// or another agent takes one up.
pub fn goal_stalled(board: &Board, goal: &Card) -> bool {
    let mut left = (board.held_by_goal(goal).into_iter())
        .filter(|card| card.status != CardStatus::Done)
        .filter_map(|card| card.slice.as_ref())
        .peekable();
    left.peek().is_some() && left.all(|slice| slice.stalled.is_some())
}

/// Every slice in the goal is done, and the person looked at what they show if they had to.
pub fn goal_done(board: &Board, goal: &Card) -> bool {
    let mut slices = (board.held_by_goal(goal).into_iter())
        .filter_map(|card| card.slice.as_ref())
        .peekable();
    slices.peek().is_some()
        && slices.all(|slice| slice.waiting().is_none())
        && look(board, goal).settled()
}

/// Whether nothing moves the slice `card` on: nobody holds it, it waits on another card, or its
/// owner sits idle. The one place every state of a slice reads this from.
pub fn held_up_now(core: &Core, board: &Board, card: &Card) -> bool {
    board.held_up(card) || super::super::chat_goal::owner_idle(core, board, card)
}

/// How the slice `card` stands; none for a card that is no slice.
pub fn state_of(core: &Core, board: &Board, card: &Card) -> Option<SliceState> {
    let held_up = held_up_now(core, board, card);
    Some(card.slice.as_ref()?.state(held_up, &board.settings))
}

/// How a goal's slices stand together, marked as one: failed when one
/// failed, needing the person when one does or the goal waits for them to
/// look, moving when one moves, done when every one is and nothing is left to
/// look at, else waiting. None for a card that is no goal.
pub fn goal_state(core: &Core, board: &Board, goal: &Card) -> Option<SliceState> {
    let states: Vec<SliceState> = (board.held_by_goal(goal).into_iter())
        .filter_map(|card| state_of(core, board, card))
        .collect();
    let any = |state: SliceState| states.contains(&state);
    let look = look(board, goal);
    Some(match () {
        _ if states.is_empty() => return None,
        _ if any(SliceState::Failed) => SliceState::Failed,
        _ if any(SliceState::NeedsYou) || matches!(look, Look::Due(_)) => SliceState::NeedsYou,
        _ if any(SliceState::Moving) => SliceState::Moving,
        _ if states.iter().all(|state| *state == SliceState::Done) && look.settled() => {
            SliceState::Done
        }
        _ => SliceState::Waiting,
    })
}

/// How the look at a goal stands as one of its steps; none when it needs no look.
fn look_step(board: &Board, goal: &Card) -> Option<SliceState> {
    Some(match look(board, goal) {
        Look::Unneeded => return None,
        Look::Due(_) => SliceState::NeedsYou,
        Look::Passed => SliceState::Done,
        Look::Ahead | Look::SentBack => SliceState::Waiting,
    })
}

/// How a goal stands step by step for a chat's strip: one step per gate of a goal of one slice,
/// where each passed gate is done and the one it waits at stands as the slice does, else one per
/// slice, then one for the person's look when it needs one, and whether the steps are gates.
pub fn goal_steps(core: &Core, board: &Board, goal: &Card) -> (Vec<SliceState>, bool) {
    let cards: Vec<&Card> = (board.held_by_goal(goal).into_iter())
        .filter(|card| card.slice.is_some())
        .collect();
    let state_of = |card: &Card| state_of(core, board, card);
    if let [card] = cards.as_slice() {
        let Some(slice) = card.slice.as_ref() else {
            return (Vec::new(), false);
        };
        let steps = (slice.ladder.iter())
            .map(|gate| {
                if slice.pass_of(gate).is_some() {
                    SliceState::Done
                } else if slice.waiting() == Some(gate) {
                    state_of(card).unwrap_or(SliceState::Waiting)
                } else {
                    SliceState::Waiting
                }
            })
            .chain(look_step(board, goal))
            .collect();
        return (steps, true);
    }
    let steps = (cards.into_iter().filter_map(state_of))
        .chain(look_step(board, goal))
        .collect();
    (steps, false)
}

/// The gates a goal's slices wait at, in the order they will be reached: a
/// slice that waits on another comes after it, and the person's look at the
/// goal comes last while it is still to come. Each says what it waits on now.
pub fn next_gates(core: &Core, board: &Board, goal: &Card, now: u64) -> Vec<NextGate> {
    let under = board.held_by_goal(goal);
    let mut placed: Vec<u64> = Vec::new();
    let mut next = Vec::new();
    while placed.len() < under.len() {
        let ready: Vec<&Card> = under
            .iter()
            .copied()
            .filter(|card| !placed.contains(&card.id))
            .filter(|card| {
                card.depends_on.iter().all(|other| {
                    placed.contains(other) || !under.iter().any(|sibling| sibling.id == *other)
                })
            })
            .collect();
        if ready.is_empty() {
            break;
        }
        for card in ready {
            placed.push(card.id);
            if let Some(gate) = card.slice.as_ref().and_then(Slice::waiting) {
                next.push(NextGate {
                    card: card.id,
                    gate: gate.gate(),
                    environment: gate.environment().map(str::to_owned),
                    passer: passer(gate, &board.settings),
                    wait: super::super::wait::of(core, board, card, now),
                });
            }
        }
    }
    if look(board, goal).coming() {
        next.push(NextGate {
            card: goal.id,
            gate: Gate::LooksRight,
            passer: Passer::Person,
            wait: None,
            environment: None,
        });
    }
    next
}
