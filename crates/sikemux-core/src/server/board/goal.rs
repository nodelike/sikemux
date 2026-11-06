//! What a goal keeps on its card, so it goes when the goal goes: what it
//! decided, rules only its own agents follow, and assertions, what must hold
//! once its slices are done, agreed before they are cut. A goal is a card at
//! the top of the board, and one that keeps anything stays there. What it
//! keeps shares ids that are never given twice. Each is corrected, keeping
//! what it said before, or retired; a goal full of one kind drops its oldest
//! retired first, then its oldest. The person or the goal's lead writes its
//! assertions, and once a slice fulfilling one has started, only the person.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::memory::Learned;
use sikemux_view::slice::{Assertion, Collect, Decision, GoalRule};

use super::{goal_of, project_at, project_view, Board, BoardView, GoalRef};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::text::to_keep;

const MAX_EACH: usize = 64;
const MAX_TEXT_CHARS: usize = 1000;
const MAX_WORKFLOW_CHARS: usize = 200;

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
    #[serde(default)]
    assertions: Vec<Assertion>,
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

    pub(super) fn assertions(&self) -> &[Assertion] {
        &self.assertions
    }

    /// The assertion `id`, while it stands.
    pub(super) fn assertion(&self, id: u64) -> Option<&Assertion> {
        (self.assertions.iter()).find(|assertion| assertion.id == id && assertion.retired.is_none())
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

impl Kept for Assertion {
    const WORD: &'static str = "assertion";

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

#[derive(Clone, Copy)]
pub(crate) enum List {
    Decisions,
    Rules,
    Assertions,
}

pub(crate) enum Change {
    New {
        text: String,
        why: Option<String>,
        evidence: Option<Collect>,
    },
    Correct {
        id: u64,
        text: Option<String>,
        why: Option<String>,
        evidence: Option<Collect>,
    },
    Retire {
        id: u64,
    },
}

impl Change {
    /// Reads what is asked of a goal's decisions or rules; only a decision
    /// has a `why`.
    pub(crate) fn of(
        list: List,
        id: Option<u64>,
        text: Option<&str>,
        why: Option<&str>,
        retire: bool,
    ) -> Result<Self, String> {
        if !matches!(list, List::Decisions) && why.is_some() {
            return Err("Only a decision has a why".into());
        }
        Self::read(id, text, why, None, retire)
    }

    /// Reads what is asked of a goal's assertions; a new one names its evidence.
    pub(crate) fn of_assertion(
        id: Option<u64>,
        text: Option<&str>,
        evidence: Option<Collect>,
        retire: bool,
    ) -> Result<Self, String> {
        if id.is_none() && evidence.is_none() && !retire {
            return Err(
                "Say what evidence shows it: check, recording, screenshot or workflow".into(),
            );
        }
        Self::read(id, text, None, evidence, retire)
    }

    fn read(
        id: Option<u64>,
        text: Option<&str>,
        why: Option<&str>,
        evidence: Option<Collect>,
        retire: bool,
    ) -> Result<Self, String> {
        let text = text
            .map(|text| to_keep(text, "text", MAX_TEXT_CHARS))
            .transpose()?;
        let why = why
            .map(|why| to_keep(why, "why", MAX_TEXT_CHARS))
            .transpose()?;
        match (id, text, why, evidence, retire) {
            (None, _, _, _, true) => Err("Say which one to retire".into()),
            (None, Some(text), why, evidence, false) => Ok(Change::New {
                text,
                why,
                evidence,
            }),
            (None, None, _, _, false) => Err("Say it, as text".into()),
            (Some(id), None, None, None, true) => Ok(Change::Retire { id }),
            (Some(_), _, _, _, true) => Err("Retire it, or correct it, not both".into()),
            (Some(_), None, None, None, false) => {
                Err("Say what changes: its text, evidence or retire".into())
            }
            (Some(id), text, why, evidence, false) => Ok(Change::Correct {
                id,
                text,
                why,
                evidence,
            }),
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
/// oldest that no started slice is `bound` to; answers what was dropped, in
/// words. Refuses when every one binds a started slice.
fn make_room<T: Kept>(list: &mut Vec<T>, goal: u64, bound: &[u64]) -> Result<String, String> {
    if list.len() < MAX_EACH {
        return Ok(String::new());
    }
    let word = T::WORD;
    let oldest = (list.iter().position(Kept::retired))
        .or_else(|| list.iter().position(|kept| !bound.contains(&kept.id())))
        .ok_or_else(|| {
            format!("Goal #{goal} keeps {MAX_EACH} of each, and every {word} binds a slice that has started; retire one first")
        })?;
    let dropped = list.remove(oldest);
    Ok(format!(
        " Goal #{goal} keeps {MAX_EACH} of each, so {word} #{} was dropped.",
        dropped.id()
    ))
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
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{goal} on this project's board"))?;
    if let (List::Assertions, Some(agent)) = (list, by.agent.as_deref()) {
        may_assert(board, goal, agent, &change)?;
    }
    let bound = bound(board, goal);
    let card = board.card_mut(goal)?;
    if card.parent.is_some() {
        return Err(format!(
            "Card #{goal} sits under another card; a goal is a card at the top"
        ));
    }
    let kept = &mut card.goal;
    let said = match (list, change) {
        (List::Decisions, Change::New { text, why, .. }) => {
            let dropped = make_room(&mut kept.decisions, goal, &[])?;
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
            let dropped = make_room(&mut kept.rules, goal, &[])?;
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
        (List::Assertions, Change::New { text, evidence, .. }) => {
            let evidence = evidence
                .ok_or("Say what evidence shows it: check, recording, screenshot or workflow")?;
            let dropped = make_room(&mut kept.assertions, goal, &bound)?;
            kept.last += 1;
            let id = kept.last;
            kept.assertions.push(Assertion {
                id,
                text,
                evidence,
                learned: by,
                was: None,
                retired: None,
            });
            format!("Assertion #{id} is kept on goal #{goal}.{dropped}")
        }
        (List::Decisions, change) => apply(&mut kept.decisions, goal, change, by)?,
        (List::Rules, change) => apply(&mut kept.rules, goal, change, by)?,
        (List::Assertions, change) => {
            let evidence = match &change {
                Change::Correct { id, evidence, .. } => {
                    evidence.clone().map(|evidence| (*id, evidence))
                }
                _ => None,
            };
            let said = apply(&mut kept.assertions, goal, change, by)?;
            if let Some((id, evidence)) = evidence {
                if let Some(kept) = kept.assertions.iter_mut().find(|kept| kept.id == id) {
                    kept.evidence = evidence;
                }
            }
            said
        }
    };
    card.updated_at = epoch_ms();
    if matches!(list, List::Assertions) {
        super::slice::rebind(board);
    }
    core.boards.save(&mut state, project);
    Ok(said)
}

/// The assertions of goal `goal` that a slice under it that has started fulfils.
fn bound(board: &Board, goal: u64) -> Vec<u64> {
    (board.cards.iter())
        .filter(|card| board.goal_above(card).id == goal)
        .filter_map(|card| card.slice.as_ref().filter(|slice| slice.started))
        .flat_map(|slice| slice.asserts())
        .collect()
}

/// Whether `agent` may make `change` to goal `goal`'s assertions: it leads the
/// goal, and no slice fulfilling the assertion it changes has started.
fn may_assert(board: &Board, goal: u64, agent: &str, change: &Change) -> Result<(), String> {
    let card = board.card(goal)?;
    if card.owner_id() != Some(agent) && card.maker_id() != Some(agent) {
        return Err(format!(
            "Only the person or goal #{goal}'s lead writes its assertions; ask your lead with ask"
        ));
    }
    let (Change::Correct { id, .. } | Change::Retire { id }) = change else {
        return Ok(());
    };
    let started = board.cards.iter().find(|slice| {
        board.goal_above(slice).id == goal
            && (slice.slice.as_ref())
                .is_some_and(|slice| slice.started && slice.asserts().any(|named| named == *id))
    });
    match started {
        Some(slice) => Err(format!(
            "Slice #{} fulfils assertion #{id} and has started, so only the person changes it",
            slice.id
        )),
        None => Ok(()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolDecide {
    decision: Option<u64>,
    text: Option<String>,
    why: Option<String>,
    #[serde(default)]
    retire: bool,
    /// Not a decision of a goal: where the project deploys, for the person to accept.
    #[serde(default)]
    environments: Option<Vec<super::Environment>>,
}

/// `board_decide`: records, corrects or retires a decision of the goal the
/// agent's live card sits under, or proposes the project's environments.
pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "board_decide")?;
    let given: ToolDecide =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let core = core.clone();
    let project = request.project.clone();
    blocking(move || {
        if let Some(environments) = given.environments {
            let why = given.why.as_deref().unwrap_or_default();
            return Ok(super::propose_environments(
                &core,
                &project,
                &agent,
                environments,
                why,
            )?);
        }
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
    pub card: u64,
    /// The decision to correct or retire; none to record a new one.
    pub decision: Option<u64>,
    pub text: Option<String>,
    pub why: Option<String>,
    #[serde(default)]
    pub retire: bool,
}

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolAssert {
    goal: Option<u64>,
    assertion: Option<u64>,
    text: Option<String>,
    evidence: Option<String>,
    workflow: Option<String>,
    #[serde(default)]
    retire: bool,
}

/// The evidence the tool names: a kind, and the workflow's name for one a
/// workflow shows.
fn evidence_of(kind: Option<&str>, workflow: Option<String>) -> Result<Option<Collect>, String> {
    let workflow = (workflow.as_deref().map(str::trim))
        .filter(|name| !name.is_empty())
        .map(|name| to_keep(name, "workflow", MAX_WORKFLOW_CHARS))
        .transpose()?;
    match (kind, workflow) {
        (None, None) => Ok(None),
        (Some("check"), None) => Ok(Some(Collect::Check)),
        (Some("recording"), None) => Ok(Some(Collect::Recording)),
        (Some("screenshot"), None) => Ok(Some(Collect::Screenshot)),
        (Some("workflow") | None, Some(name)) => Ok(Some(Collect::Workflow { name })),
        (Some("workflow"), None) => Err("Name the workflow, as its CI calls it".into()),
        (Some(_), Some(_)) => Err("Only a workflow's evidence names a workflow".into()),
        (Some(kind), None) => Err(format!(
            "evidence is check, recording, screenshot or workflow, not {kind}"
        )),
    }
}

/// `board_assert`: writes, corrects or retires an assertion of a goal the agent
/// leads, the one its live card sits under unless it names one.
pub(crate) async fn tool_assert(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "board_assert")?;
    let given: ToolAssert =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let asserting = Change::of_assertion(
            given.assertion,
            given.text.as_deref(),
            evidence_of(given.evidence.as_deref(), given.workflow)?,
            given.retire,
        )?;
        let project = crate::server::caller::project_of(&core, &request);
        let goal = match given.goal {
            Some(goal) => goal,
            None => {
                goal_of(&core, &agent)
                    .ok_or("Name the goal's card with goal; you work on no card that is not done")?
                    .card
            }
        };
        let by = learned(&core, Some(&agent));
        let said = change(&core, &project, goal, List::Assertions, asserting, by)?;
        Ok(json!({ "goal": goal, "note": said }))
    })
    .await
    .map_err(|error| error.to_string())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertParams {
    pub project: String,
    pub card: u64,
    /// The assertion to correct or retire; none to write a new one.
    pub assertion: Option<u64>,
    pub text: Option<String>,
    pub evidence: Option<Collect>,
    #[serde(default)]
    pub retire: bool,
}

pub(crate) async fn person_assert(
    core: &Arc<Core>,
    params: AssertParams,
) -> Result<BoardView, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let asserting = Change::of_assertion(
            params.assertion,
            params.text.as_deref(),
            params.evidence,
            params.retire,
        )?;
        let by = learned(&core, None);
        change(
            &core,
            &project,
            params.card,
            List::Assertions,
            asserting,
            by,
        )?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}
