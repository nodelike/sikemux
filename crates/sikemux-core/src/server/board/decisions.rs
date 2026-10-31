//! What a goal decided, kept on the goal's card so it goes when the goal
//! goes. An agent decides for the goal its live card sits under. A decision
//! is corrected, keeping what it said before, or retired; a full goal drops
//! its oldest retired decisions first, then its oldest.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::memory::Learned;
use sikemux_wire::slice::Decision;

use super::{caller, goal_of, project_at, project_view, BoardView, Card, GoalRef};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::text::to_keep;

const MAX_DECISIONS: usize = 64;
const MAX_TEXT_CHARS: usize = 1000;

/// Who says something now, and in which chat turn; none for the person.
pub(crate) fn learned(core: &Core, agent: Option<&str>) -> Learned {
    let chat = agent.and_then(|agent| core.chats.get(agent));
    Learned {
        agent: agent.map(str::to_owned),
        name: agent.and_then(|agent| core.names.name(agent)),
        feed: chat.as_ref().map(|chat| chat.feed.id().to_owned()),
        turn: chat.as_ref().and_then(|chat| chat.feed.turn_start()),
        at: epoch_ms(),
    }
}

/// The goal's decisions that still stand, oldest first.
pub(crate) fn of_goal(core: &Core, goal: &GoalRef) -> Vec<Decision> {
    let state = core.boards.state.locked();
    let Some(card) = state
        .boards
        .get(&goal.project)
        .and_then(|board| board.card(goal.card).ok())
    else {
        return Vec::new();
    };
    card.decisions
        .iter()
        .filter(|decision| decision.retired.is_none())
        .cloned()
        .collect()
}

enum Deciding {
    New {
        text: String,
        why: Option<String>,
    },
    Correct {
        id: u64,
        text: Option<String>,
        why: Option<String>,
    },
    Retire {
        id: u64,
    },
}

impl Deciding {
    fn of(
        decision: Option<u64>,
        text: Option<&str>,
        why: Option<&str>,
        retire: bool,
    ) -> Result<Self, String> {
        let text = text
            .map(|text| to_keep(text, "text", MAX_TEXT_CHARS))
            .transpose()?;
        let why = why
            .map(|why| to_keep(why, "why", MAX_TEXT_CHARS))
            .transpose()?;
        match (decision, text, why, retire) {
            (None, _, _, true) => Err("Say which decision to retire".into()),
            (None, Some(text), why, false) => Ok(Deciding::New { text, why }),
            (None, None, _, false) => Err("Say what was decided, as text".into()),
            (Some(id), None, None, true) => Ok(Deciding::Retire { id }),
            (Some(_), _, _, true) => Err("Retire a decision, or correct it, not both".into()),
            (Some(_), None, None, false) => Err("Say what changes: text, why or retire".into()),
            (Some(id), text, why, false) => Ok(Deciding::Correct { id, text, why }),
        }
    }
}

fn decision_mut(card: &mut Card, goal: u64, id: u64) -> Result<&mut Decision, String> {
    card.decisions
        .iter_mut()
        .find(|decision| decision.id == id)
        .ok_or_else(|| format!("Goal #{goal} has no decision #{id}"))
}

/// Changes the decisions of goal card `goal` in `project`; answers what
/// happened, in words.
fn decide(
    core: &Core,
    project: &str,
    goal: u64,
    deciding: Deciding,
    by: Learned,
) -> Result<String, String> {
    let mut state = core.boards.state.locked();
    let card = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{goal} on this project's board"))?
        .card_mut(goal)?;
    let said = match deciding {
        Deciding::New { text, why } => {
            let mut said = String::new();
            if card.decisions.len() >= MAX_DECISIONS {
                let oldest = card
                    .decisions
                    .iter()
                    .position(|decision| decision.retired.is_some())
                    .unwrap_or(0);
                let dropped = card.decisions.remove(oldest);
                said = format!(
                    " Goal #{goal} keeps {MAX_DECISIONS} decisions, so #{} was dropped.",
                    dropped.id
                );
            }
            let id = card
                .decisions
                .iter()
                .map(|decision| decision.id)
                .max()
                .unwrap_or(0)
                + 1;
            card.decisions.push(Decision {
                id,
                text,
                why,
                learned: by,
                was: None,
                retired: None,
            });
            format!("Decision #{id} is kept on goal #{goal}.{said}")
        }
        Deciding::Correct { id, text, why } => {
            let decision = decision_mut(card, goal, id)?;
            if decision.retired.is_some() {
                return Err(format!("Decision #{id} was retired; decide again instead"));
            }
            if let Some(text) = text {
                decision.was = Some(std::mem::replace(&mut decision.text, text));
            }
            if why.is_some() {
                decision.why = why;
            }
            decision.learned = by;
            format!("Decision #{id} on goal #{goal} is corrected.")
        }
        Deciding::Retire { id } => {
            decision_mut(card, goal, id)?.retired = Some(by);
            format!("Decision #{id} on goal #{goal} is retired.")
        }
    };
    card.updated_at = epoch_ms();
    core.boards.save(&mut state, project);
    Ok(said)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolDecide {
    decision: Option<u64>,
    text: Option<String>,
    why: Option<String>,
    #[serde(default)]
    retire: bool,
}

/// `board_decide`: records, corrects or retires a decision of the goal the
/// agent's live card sits under.
pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "board_decide")?;
    let given: ToolDecide =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let core = core.clone();
    blocking(move || {
        let deciding = Deciding::of(
            given.decision,
            given.text.as_deref(),
            given.why.as_deref(),
            given.retire,
        )?;
        let goal = goal_of(&core, &agent).ok_or(
            "You work on no card that is not done, so there is no goal to decide for; claim your card first",
        )?;
        let said = decide(&core, &goal.project, goal.card, deciding, learned(&core, Some(&agent)))?;
        Ok(json!({ "goal": goal.card, "note": said }))
    })
    .await
    .map_err(|error| error.to_string())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideParams {
    pub project: String,
    /// The goal's card.
    pub card: u64,
    /// The decision to correct or retire; none to record a new one.
    pub decision: Option<u64>,
    pub text: Option<String>,
    pub why: Option<String>,
    #[serde(default)]
    pub retire: bool,
}

/// The person records, corrects or retires a goal's decision; answers with the board.
pub(crate) async fn person_decide(
    core: &Arc<Core>,
    params: DecideParams,
) -> Result<BoardView, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let deciding = Deciding::of(
            params.decision,
            params.text.as_deref(),
            params.why.as_deref(),
            params.retire,
        )?;
        decide(&core, &project, params.card, deciding, learned(&core, None))?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}
