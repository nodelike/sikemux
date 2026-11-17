//! The board's tasks as rows of the person's one list of tasks.

use sikemux_view::task_list::{TaskDropped, TaskFact, TaskHolder, TaskOf, TaskRow, TaskSection};
use sikemux_view::workspace::PullChecks;

use super::*;

/// The listed tasks on the board at `folder`, in board order.
pub fn task_rows(core: &Core, folder: &str) -> Vec<TaskRow> {
    let state = core.boards().state.locked();
    let Some(board) = state.of(folder) else {
        return Vec::new();
    };
    (board.cards.iter())
        .filter(|card| card.listed())
        .map(|card| row(core, board, card))
        .collect()
}

fn row(core: &Core, board: &Board, card: &Card) -> TaskRow {
    let section = section(core, card);
    let asked = card.asked.as_ref();
    let title = (asked.and_then(|asked| asked.name.clone()))
        .or_else(|| display_title(core, board, card))
        .unwrap_or_else(|| card.title.clone());
    TaskRow {
        of: TaskOf::Task { id: card.id },
        section,
        title,
        fact: Some(fact(card, section)),
        holder: holder_of(core, card),
        since: since(card, section),
        dropped: card.dropped.as_ref().map(|dropped| TaskDropped {
            by: match &dropped.by {
                DroppedBy::Agent { id, name } => {
                    Some(core.names().name(id).unwrap_or(name.clone()))
                }
                DroppedBy::Person | DroppedBy::Board => None,
            },
            by_board: dropped.by == DroppedBy::Board,
            why: dropped.why.clone(),
            instead: (dropped.instead)
                .and_then(|id| board.card(id).ok())
                .map(|card| card.title.clone()),
        }),
        asked: asked.is_some(),
    }
}

fn section(core: &Core, card: &Card) -> TaskSection {
    if card.dropped.is_some() {
        TaskSection::Dropped
    } else if card.status == CardStatus::Done {
        match card.opened_at {
            Some(_) => TaskSection::Opened,
            None => TaskSection::Finished,
        }
    } else if card.status == CardStatus::Progress || holder(core, card).is_some() {
        TaskSection::Moving
    } else {
        TaskSection::Queued
    }
}

fn nobody_since(card: &Card) -> u64 {
    (card.asked.as_ref())
        .and_then(|asked| asked.nobody_since)
        .unwrap_or(card.updated_at)
}

fn since(card: &Card, section: TaskSection) -> u64 {
    match section {
        TaskSection::Dropped => card.ended_at(),
        TaskSection::Moving => card.acted_at.max(card.at),
        TaskSection::Queued => nobody_since(card),
        TaskSection::ForYou | TaskSection::Finished | TaskSection::Opened => card.updated_at,
    }
}

fn fact(card: &Card, section: TaskSection) -> TaskFact {
    if section == TaskSection::Queued {
        return TaskFact::NobodySince {
            at: nobody_since(card),
        };
    }
    let failing = !card.ci.is_empty();
    let checks_of = |sha: &str| {
        if card.checked.as_deref() == Some(sha) {
            Some(PullChecks::Passing)
        } else {
            failing.then_some(PullChecks::Failing)
        }
    };
    let landed = (card.commits.iter().rev())
        .find(|commit| commit.landed)
        .map(|commit| commit.sha.clone())
        .or_else(|| card.asked.as_ref().and_then(|asked| asked.commit.clone()));
    if let Some(sha) = landed {
        return TaskFact::Landed {
            checks: checks_of(&sha),
            sha,
        };
    }
    if let Some(pull) = (card.pulls.iter().rev()).find(|pull| pull.state == PullState::Open) {
        return TaskFact::Pull {
            number: pull.number,
            checks: pull.checks,
        };
    }
    match card.commits.len() {
        0 => TaskFact::NoCommits,
        count => TaskFact::Commits {
            count: u32::try_from(count).unwrap_or(u32::MAX),
            checks: failing.then_some(PullChecks::Failing),
        },
    }
}

fn holder_of(core: &Core, card: &Card) -> Option<TaskHolder> {
    let agent_id = (card.asked.as_ref())
        .and_then(|asked| asked.owners.last())
        .map(|owner| owner.agent.as_str())
        .or_else(|| card.owner_id())?;
    let name = match card.owner_id() == Some(agent_id) {
        true => card.owner_name(core),
        false => core.names().name(agent_id),
    };
    Some(TaskHolder {
        agent_id: agent_id.to_owned(),
        name: name.unwrap_or_else(|| agent_id.to_owned()),
    })
}
