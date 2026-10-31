//! What a goal keeps on its card, so it goes when the goal goes: what it
//! decided, and rules only its own agents follow. A goal is a card at the top
//! of the board, and one that keeps anything stays there. Its decisions and
//! rules share ids that are never given twice. Each is corrected, keeping
//! what it said before, or retired; a goal full of one kind drops its oldest
//! retired first, then its oldest.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::memory::Learned;
use sikemux_wire::slice::{Decision, GoalRule};

use super::{caller, goal_of, project_at, project_view, BoardView, GoalRef};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::text::to_keep;

const MAX_EACH: usize = 64;
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

/// What a goal card keeps, with the last id it gave.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Goal {
    last: u64,
    #[serde(default)]
    decisions: Vec<Decision>,
    #[serde(default)]
    rules: Vec<GoalRule>,
}

impl Goal {
    /// The card never kept anything as a goal.
    pub(super) fn is_none(&self) -> bool {
        self.last == 0
    }

    pub(super) fn decisions(&self) -> Vec<Decision> {
        self.decisions.clone()
    }

    pub(super) fn rules(&self) -> Vec<GoalRule> {
        self.rules.clone()
    }
}

/// What a goal keeps that still stands, oldest first.
#[derive(Default)]
pub(crate) struct Standing {
    pub decisions: Vec<Decision>,
    pub rules: Vec<GoalRule>,
}

pub(crate) fn standing(core: &Core, goal: &GoalRef) -> Standing {
    let state = core.boards.state.locked();
    let Some(card) = state
        .boards
        .get(&goal.project)
        .and_then(|board| board.card(goal.card).ok())
    else {
        return Standing::default();
    };
    Standing {
        decisions: (card.goal.decisions.iter())
            .filter(|decision| decision.retired.is_none())
            .cloned()
            .collect(),
        rules: (card.goal.rules.iter())
            .filter(|rule| rule.retired.is_none())
            .cloned()
            .collect(),
    }
}

/// A decision or a goal's rule, as one list of a goal's changes it.
trait Kept {
    const WORD: &'static str;
    fn id(&self) -> u64;
    fn retired(&self) -> bool;
    fn correct(&mut self, text: Option<String>, why: Option<String>, by: Learned);
    fn retire(&mut self, by: Learned);
}

impl Kept for Decision {
    const WORD: &'static str = "decision";

    fn id(&self) -> u64 {
        self.id
    }

    fn retired(&self) -> bool {
        self.retired.is_some()
    }

    fn correct(&mut self, text: Option<String>, why: Option<String>, by: Learned) {
        if let Some(text) = text {
            self.was = Some(std::mem::replace(&mut self.text, text));
        }
        if why.is_some() {
            self.why = why;
        }
        self.learned = by;
    }

    fn retire(&mut self, by: Learned) {
        self.retired = Some(by);
    }
}

impl Kept for GoalRule {
    const WORD: &'static str = "rule";

    fn id(&self) -> u64 {
        self.id
    }

    fn retired(&self) -> bool {
        self.retired.is_some()
    }

    fn correct(&mut self, text: Option<String>, _why: Option<String>, by: Learned) {
        if let Some(text) = text {
            self.was = Some(std::mem::replace(&mut self.text, text));
        }
        self.learned = by;
    }

    fn retire(&mut self, by: Learned) {
        self.retired = Some(by);
    }
}

/// Which of a goal's lists a change is to.
#[derive(Clone, Copy)]
pub(crate) enum List {
    Decisions,
    Rules,
}

pub(crate) enum Change {
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

impl Change {
    /// Reads what is asked of a goal's `list`; only a decision has a `why`.
    pub(crate) fn of(
        list: List,
        id: Option<u64>,
        text: Option<&str>,
        why: Option<&str>,
        retire: bool,
    ) -> Result<Self, String> {
        if matches!(list, List::Rules) && why.is_some() {
            return Err("A rule says what to do; only a decision has a why".into());
        }
        let text = text
            .map(|text| to_keep(text, "text", MAX_TEXT_CHARS))
            .transpose()?;
        let why = why
            .map(|why| to_keep(why, "why", MAX_TEXT_CHARS))
            .transpose()?;
        match (id, text, why, retire) {
            (None, _, _, true) => Err("Say which one to retire".into()),
            (None, Some(text), why, false) => Ok(Change::New { text, why }),
            (None, None, _, false) => Err("Say it, as text".into()),
            (Some(id), None, None, true) => Ok(Change::Retire { id }),
            (Some(_), _, _, true) => Err("Retire it, or correct it, not both".into()),
            (Some(_), None, None, false) => Err("Say what changes: text, why or retire".into()),
            (Some(id), text, why, false) => Ok(Change::Correct { id, text, why }),
        }
    }
}

/// Applies a change other than a new one to `list`; answers what happened.
fn apply<T: Kept>(
    list: &mut [T],
    goal: u64,
    change: Change,
    by: Learned,
) -> Result<String, String> {
    let word = T::WORD;
    let (Change::Correct { id, .. } | Change::Retire { id }) = &change else {
        return Err(format!("A new {word} is added, not changed"));
    };
    let id = *id;
    let kept = (list.iter_mut())
        .find(|kept| kept.id() == id)
        .ok_or_else(|| format!("Goal #{goal} has no {word} #{id}"))?;
    if kept.retired() {
        return Err(format!(
            "Goal #{goal}'s {word} #{id} was retired; add it again instead"
        ));
    }
    match change {
        Change::Correct { text, why, .. } => {
            kept.correct(text, why, by);
            Ok(format!("Goal #{goal}'s {word} #{id} is corrected."))
        }
        _ => {
            kept.retire(by);
            Ok(format!("Goal #{goal}'s {word} #{id} is retired."))
        }
    }
}

/// Makes room for one more in `list`, dropping its oldest retired, else its
/// oldest; answers what was dropped, in words.
fn make_room<T: Kept>(list: &mut Vec<T>, goal: u64) -> String {
    if list.len() < MAX_EACH {
        return String::new();
    }
    let oldest = list.iter().position(Kept::retired).unwrap_or(0);
    let dropped = list.remove(oldest);
    format!(
        " Goal #{goal} keeps {MAX_EACH} of each, so {} #{} was dropped.",
        T::WORD,
        dropped.id()
    )
}

/// Changes what goal card `goal` of `project` keeps; answers what happened,
/// in words.
pub(crate) fn change(
    core: &Core,
    project: &str,
    goal: u64,
    list: List,
    change: Change,
    by: Learned,
) -> Result<String, String> {
    let mut state = core.boards.state.locked();
    let card = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{goal} on this project's board"))?
        .card_mut(goal)?;
    if card.parent.is_some() {
        return Err(format!(
            "Card #{goal} sits under another card; a goal is a card at the top"
        ));
    }
    let kept = &mut card.goal;
    let said = match (list, change) {
        (List::Decisions, Change::New { text, why }) => {
            let dropped = make_room(&mut kept.decisions, goal);
            kept.last += 1;
            let id = kept.last;
            kept.decisions.push(Decision {
                id,
                text,
                why,
                learned: by,
                was: None,
                retired: None,
            });
            format!("Decision #{id} is kept on goal #{goal}.{dropped}")
        }
        (List::Rules, Change::New { text, .. }) => {
            let dropped = make_room(&mut kept.rules, goal);
            kept.last += 1;
            let id = kept.last;
            kept.rules.push(GoalRule {
                id,
                text,
                learned: by,
                was: None,
                retired: None,
            });
            format!("Rule #{id} is kept on goal #{goal}.{dropped}")
        }
        (List::Decisions, change) => apply(&mut kept.decisions, goal, change, by)?,
        (List::Rules, change) => apply(&mut kept.rules, goal, change, by)?,
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
        let deciding = Change::of(
            List::Decisions,
            given.decision,
            given.text.as_deref(),
            given.why.as_deref(),
            given.retire,
        )?;
        let goal = goal_of(&core, &agent).ok_or(
            "You work on no card that is not done, so there is no goal to decide for; claim your card first",
        )?;
        let by = learned(&core, Some(&agent));
        let said = change(&core, &goal.project, goal.card, List::Decisions, deciding, by)?;
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
        let deciding = Change::of(
            List::Decisions,
            params.decision,
            params.text.as_deref(),
            params.why.as_deref(),
            params.retire,
        )?;
        let by = learned(&core, None);
        change(&core, &project, params.card, List::Decisions, deciding, by)?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}
