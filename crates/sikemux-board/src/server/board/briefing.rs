//! What the board adds to a new agent's brief, so nobody writes a handoff by hand: the goal it
//! works toward in the person's words. A teammate works toward its lead's goal until it holds a
//! card of its own.

use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use sikemux_process::sync::Locked;

use super::{open_work, Card, GoalRef};
use crate::server::Core;
use crate::text::{clipped, one_line};

const MAX_GOAL_CHARS: usize = 1500;

/// The goal `agent_id` works toward: its own card's, or else its lead's.
pub fn goal_for(core: &Core, agent_id: &str) -> Option<GoalRef> {
    super::goal_of(core, agent_id).or_else(|| super::goal_of(core, &lead(core, agent_id)?))
}

fn lead(core: &Core, agent_id: &str) -> Option<String> {
    let started_by = core
        .chats()
        .get(agent_id)
        .and_then(|chat| chat.origin.started_by.clone());
    started_by.or_else(|| (core.board_above().hosts_started_by)(core, agent_id))
}

/// The goal's own words, as a section of `agent_id`'s brief, unless the goal is its own card.
pub fn sections(core: &Core, agent_id: &str, goal: &GoalRef) -> Vec<String> {
    let state = core.boards().state.locked();
    let Some(board) = state.of(&goal.project) else {
        return Vec::new();
    };
    let Ok(goal_card) = board.card(goal.card) else {
        return Vec::new();
    };
    let own = open_work(&state, agent_id)
        .filter(|(project, _, _)| **project == goal.project)
        .map(|(_, _, card)| card);
    match own.is_none_or(|own| own.id != goal_card.id) {
        true => vec![goal_words(goal_card)],
        false => Vec::new(),
    }
}

fn goal_words(goal: &Card) -> String {
    let mut text = format!("The goal, card #{}: {}", goal.id, one_line(&goal.title));
    if !goal.details.trim().is_empty() {
        text.push_str(&format!("\n{}", clipped(&goal.details, MAX_GOAL_CHARS)));
    }
    if !goal.acceptance.trim().is_empty() {
        text.push_str(&format!(
            "\nDone when: {}",
            clipped(&goal.acceptance, MAX_GOAL_CHARS)
        ));
    }
    text
}
