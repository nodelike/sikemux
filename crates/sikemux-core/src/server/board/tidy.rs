//! Keeps a board to the work still on it, with nobody asked: a card whose cards under it are all
//! done is done too, finished cards leave a few days after they finish, and an open card nobody
//! has touched for a week moves to Later until someone takes it up again.

use super::*;

/// How long an open card nobody holds sits untouched before it moves to Later.
const UNTOUCHED_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const CLOSED_RESULT: &str = "Every card under it is done.";

/// Tidies every board, and answers when the next card comes due to leave or move.
pub(super) fn settle(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let projects: Vec<String> = core.boards.state.locked().boards.keys().cloned().collect();
    let mut next = None;
    for project in projects {
        close_parents(core, &project);
        let mut state = core.boards.state.locked();
        let kept_until = archive::finished_long_ago(core, &mut state, &project, now);
        next = sooner(next, kept_until);
        next = sooner(next, put_aside(core, &mut state, &project, now));
    }
    next
}

/// Whether `card` is done because every card under it is: it is no slice or ticket of its own,
/// nobody works it, and a goal's slices wait on nothing more, such as the person looking.
fn finished_under(core: &Core, board: &Board, card: &Card, under: &[&Card]) -> bool {
    card.status != CardStatus::Done
        && card.slice.is_none()
        && card.ticket.is_none()
        && card.auto.is_none()
        && holder(core, card).is_none()
        && !under.is_empty()
        && under.iter().all(|card| card.status == CardStatus::Done)
        && (board.held_by_goal(card).is_empty() || slice::goal_done(board, card))
}

fn close_parents(core: &Arc<Core>, project: &str) {
    let mut state = core.boards.state.locked();
    let Some(board) = state.boards.get(project) else {
        return;
    };
    let mut under: HashMap<u64, Vec<&Card>> = HashMap::new();
    for card in &board.cards {
        if let Some(parent) = card.parent {
            under.entry(parent).or_default().push(card);
        }
    }
    let closing: Vec<u64> = (board.cards.iter())
        .filter(|card| {
            let under = under.get(&card.id).map_or(&[][..], Vec::as_slice);
            finished_under(core, board, card, under)
        })
        .map(|card| card.id)
        .collect();
    if closing.is_empty() {
        return;
    }
    let mut notices = Vec::new();
    for id in closing {
        let changes = Changes {
            status: Some(CardStatus::Done),
            result: Some(CLOSED_RESULT.to_owned()),
            ..Changes::default()
        };
        let by = By::Auto("Board");
        match change_in(
            core,
            &mut state,
            project,
            &by,
            id,
            changes,
            &slice::Seen::unread(),
        ) {
            Ok((_, sent, _)) => notices.extend(sent),
            Err(error) => eprintln!("sikemux core: the board did not close #{id}: {error}"),
        }
    }
    core.boards.save(&mut state, project);
    drop(state);
    send_notices(core, notices);
}

/// The cards that stay on the board though nobody holds them: what a card being worked waits
/// on, and a card with an unfinished card under it still on the board.
fn wanted(board: &Board) -> HashSet<u64> {
    let mut wanted = HashSet::new();
    for card in &board.cards {
        let worked = card.status == CardStatus::Progress
            || (card.status == CardStatus::Open && (card.owner.is_some() || card.auto.is_some()));
        if worked {
            wanted.extend(card.depends_on.iter().copied());
        }
        if card.status != CardStatus::Done && card.aside_at.is_none() {
            wanted.extend(card.parent);
        }
    }
    wanted
}

/// Moves the open cards nobody has touched for long to Later, and brings back each one that
/// was taken up, changed or is wanted again. Answers when the next card comes due to move.
fn put_aside(core: &Core, state: &mut State, project: &str, now: u64) -> Option<u64> {
    let board = state.boards.get(project)?;
    let wanted = wanted(board);
    let idle: HashSet<u64> = (board.cards.iter())
        .filter(|card| {
            card.status == CardStatus::Open
                && card.owner.is_none()
                && card.auto.is_none()
                && !wanted.contains(&card.id)
                && waits(core, board, card).is_none()
        })
        .map(|card| card.id)
        .collect();
    let board = state.boards.get_mut(project)?;
    let mut changed = false;
    let mut next = None;
    for card in &mut board.cards {
        let touched = card.updated_at.max(card.acted_at);
        let idle = idle.contains(&card.id);
        match card.aside_at {
            Some(at) if !idle || touched > at => {
                card.aside_at = None;
                card.updated_at = now;
                changed = true;
            }
            Some(_) => {}
            None if idle && touched + UNTOUCHED_MS <= now => {
                card.aside_at = Some(now);
                changed = true;
            }
            None if idle => next = sooner(next, Some(touched + UNTOUCHED_MS)),
            None => {}
        }
    }
    if changed {
        core.boards.save(state, project);
    }
    next
}
