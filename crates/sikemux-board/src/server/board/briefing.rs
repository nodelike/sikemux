//! What the board adds to a new agent's brief, so nobody writes a handoff by hand: the goal it
//! works toward in the person's words, the goal's slices and who holds each, and where its own
//! card stands with the person's notes on it. A teammate works toward its lead's goal until it
//! holds a card of its own.

use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use sikemux_process::sync::Locked;
use sikemux_view::slice::{Gate, GateState, SliceView};

use super::{card_view, open_work, Board, Card, GoalRef};
use crate::server::Core;
use crate::text::{clipped, one_line};

const MAX_GOAL_CHARS: usize = 1500;
const MAX_SLICES: usize = 12;
const MAX_NOTES: usize = 6;
const MAX_NOTE_CHARS: usize = 400;

/// The goal `agent_id` works toward: its own card's, or else its lead's.
pub fn goal_for(core: &Core, agent_id: &str) -> Option<GoalRef> {
    super::goal_of(core, agent_id).or_else(|| super::goal_of(core, &lead(core, agent_id)?))
}

fn lead(core: &Core, agent_id: &str) -> Option<String> {
    let started_by = core
        .chats()
        .get(agent_id)
        .and_then(|chat| chat.origin.started_by.clone());
    started_by.or_else(|| (core.board_above().foreign)(core, agent_id)?.parent)
}

/// The goal's own words, its slices, and `agent_id`'s card, as sections of its brief. A blind
/// reviewer gets only the goal's words, which are the person's.
pub fn sections(core: &Core, agent_id: &str, goal: &GoalRef, blind: bool) -> Vec<String> {
    let state = core.boards().state.locked();
    let Some(board) = state.boards.get(&goal.project) else {
        return Vec::new();
    };
    let Ok(goal_card) = board.card(goal.card) else {
        return Vec::new();
    };
    let own = open_work(&state, agent_id)
        .filter(|(project, _, _)| **project == goal.project)
        .map(|(_, _, card)| card);
    let mut sections = Vec::new();
    if own.is_none_or(|own| own.id != goal_card.id) {
        sections.push(goal_words(goal_card));
    }
    if blind {
        return sections;
    }
    if let Some(slices) = slices(core, board, goal_card, agent_id) {
        sections.push(slices);
    }
    if let Some(own) = own.filter(|own| own.slice.is_some()) {
        let view = card_view(core, board, own);
        if let Some(slice) = &view.slice {
            sections.push(own_card(own.id, slice));
        }
    }
    sections
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

/// The goal's slices and who holds each, so a new agent knows who does what.
fn slices(core: &Core, board: &Board, goal: &Card, agent_id: &str) -> Option<String> {
    let held = board.held_by_goal(goal);
    if held.len() < 2 {
        return None;
    }
    let lines: Vec<String> = (held.iter().take(MAX_SLICES))
        .map(|card| {
            let promise = (card.slice.as_ref()).map_or(card.title.as_str(), |slice| &slice.promise);
            let holder = match (card.owner_id(), card.owner_name(core)) {
                (Some(owner), _) if owner == agent_id => "you".to_owned(),
                (Some(owner), Some(name)) => {
                    format!("@{}", crate::server::names::called(&name, owner))
                }
                _ => "nobody yet".to_owned(),
            };
            format!(
                "- #{} {} ({holder}, {})",
                card.id,
                one_line(promise),
                card.status.words()
            )
        })
        .collect();
    let more = held.len().saturating_sub(MAX_SLICES);
    let more = match more {
        0 => String::new(),
        more => format!("\n- and {more} more; board_list shows them"),
    };
    Some(format!(
        "Slices of goal card #{}:\n{}{more}",
        goal.id,
        lines.join("\n")
    ))
}

fn own_card(id: u64, slice: &SliceView) -> String {
    let mut text = format!("Your card #{id} promises: {}", one_line(&slice.promise));
    let passed: Vec<String> = (slice.gates.iter())
        .filter(|gate| gate.state == GateState::Passed)
        .map(|gate| gate_name(gate.gate, gate.environment.as_deref()))
        .collect();
    if !passed.is_empty() {
        text.push_str(&format!("\nPassed: {}.", passed.join(", ")));
    }
    if let Some(waiting) = (slice.gates.iter()).find(|gate| gate.state == GateState::Waiting) {
        text.push_str(&format!(
            "\nIt waits at {}.",
            gate_name(waiting.gate, waiting.environment.as_deref())
        ));
    }
    let notes: Vec<String> = (slice.notes.iter())
        .filter(|note| note.by.is_none())
        .rev()
        .take(MAX_NOTES)
        .map(|note| format!("- {}", clipped(&one_line(&note.text), MAX_NOTE_CHARS)))
        .collect();
    if !notes.is_empty() {
        text.push_str(&format!(
            "\nThe person's notes on it, newest first:\n{}",
            notes.join("\n")
        ));
    }
    text.push_str(&format!(
        "\nboard_list with card {id} shows its evidence and checks."
    ));
    text
}

fn gate_name(gate: Gate, environment: Option<&str>) -> String {
    if let Some(environment) = environment {
        return environment.to_owned();
    }
    match gate {
        Gate::Built => "Built",
        Gate::LooksRight => "Looks right",
        Gate::Landed => "Landed",
    }
    .to_owned()
}
