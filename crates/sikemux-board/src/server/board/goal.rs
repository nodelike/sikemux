//! What a project decided, in one list, each decision tagged with the goal it was made on, and
//! the rules a goal keeps on its card for its own agents. A goal is a card at the top of the
//! board, and one that keeps anything stays there. Each is corrected, keeping what it said
//! before, or retired; a full list drops its oldest retired first, then its oldest.
//! What agents settle among themselves goes in the list too, where the person can overrule it.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_above::Overruled;
use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::{DecideParams, OverruleParams, UndoOverruleParams};
use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::SwarmDecision;
use sikemux_view::memory::Learned;
use sikemux_view::slice::{Decision, GoalRule};

use super::{goal_of, project_at, project_view, Board, GoalRef, ProjectBoard};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::text::{clipped, to_keep};

const MAX_RULES: usize = 64;
const MAX_DECISIONS: usize = 512;
const MAX_TEXT_CHARS: usize = 1000;

/// Who says something now, and in which chat turn; none for the person.
pub fn learned(core: &Core, agent: Option<&str>) -> Learned {
    let chat = agent.and_then(|agent| core.chats().get(agent));
    Learned {
        agent: agent.map(str::to_owned),
        name: agent.and_then(|agent| core.names().name(agent)),
        feed: chat.as_ref().map(|chat| chat.feed.id().to_owned()),
        turn: chat.as_ref().and_then(|chat| chat.feed.turn_start()),
        at: epoch_ms(),
        with: None,
    }
}

/// The rules a goal card keeps, with the last id it gave them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    last: u64,
    #[serde(default)]
    rules: Vec<GoalRule>,
}

impl Goal {
    pub fn is_none(&self) -> bool {
        self.last == 0
    }

    pub fn rules(&self) -> Vec<GoalRule> {
        self.rules.clone()
    }
}

/// Everything a project decided, oldest first, with the last id it gave.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decided {
    last: u64,
    #[serde(default)]
    decisions: Vec<Decision>,
}

impl Decided {
    pub fn is_none(&self) -> bool {
        self.last == 0
    }

    pub fn on_goal(&self, goal: u64) -> Vec<Decision> {
        (self.decisions.iter())
            .filter(|decision| decision.goal == Some(goal))
            .cloned()
            .collect()
    }

    pub fn standing(&self) -> impl Iterator<Item = &Decision> {
        (self.decisions.iter()).filter(|decision| decision.retired.is_none())
    }

    /// Decision `id`, made on `goal`. Decisions kept before the project had one list may share
    /// an id with another goal's, which the goal tells apart.
    fn find(&mut self, goal: Option<u64>, id: u64) -> Option<&mut Decision> {
        let at = (self.decisions.iter())
            .position(|decision| decision.id == id && decision.goal == goal)
            .or_else(|| {
                let mut same =
                    (self.decisions.iter().enumerate()).filter(|(_, decision)| decision.id == id);
                match (same.next(), same.next()) {
                    (Some((at, _)), None) => Some(at),
                    _ => None,
                }
            })?;
        self.decisions.get_mut(at)
    }

    fn add(&mut self, mut decision: Decision) -> (Decision, Option<u64>) {
        let dropped = make_room(&mut self.decisions, MAX_DECISIONS);
        self.last += 1;
        decision.id = self.last;
        self.decisions.push(decision.clone());
        (decision, dropped)
    }
}

/// The project's decisions still standing that were made since `since`, newest first, at
/// most `most`.
pub fn standing_since(core: &Core, project: &str, since: u64, most: usize) -> Vec<Decision> {
    let mut decisions = standing(core, project);
    decisions.retain(|decision| decision.learned.at >= since);
    decisions.reverse();
    decisions.truncate(most);
    decisions
}

/// The project's decisions that still stand, oldest first.
pub fn standing(core: &Core, project: &str) -> Vec<Decision> {
    let state = core.boards().state.locked();
    (state.boards.get(project))
        .map(|board| board.decided.standing().cloned().collect())
        .unwrap_or_default()
}

/// The rules goal `goal` keeps that still stand, oldest first.
pub fn rules(core: &Core, goal: &GoalRef) -> Vec<GoalRule> {
    let state = core.boards().state.locked();
    (state.boards.get(&goal.project))
        .and_then(|board| board.card(goal.card).ok())
        .map(|card| {
            (card.goal.rules.iter())
                .filter(|rule| rule.retired.is_none())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
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

#[derive(Clone, Copy)]
pub enum List {
    Decisions,
    Rules,
}

pub enum Change {
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
    fn id(&self) -> Option<u64> {
        match self {
            Change::Correct { id, .. } | Change::Retire { id } => Some(*id),
            Change::New { .. } => None,
        }
    }

    /// Reads what is asked of a goal's decisions or rules; only a decision
    /// has a `why`.
    pub fn of(
        list: List,
        id: Option<u64>,
        text: Option<&str>,
        why: Option<&str>,
        retire: bool,
    ) -> Result<Self, String> {
        if !matches!(list, List::Decisions) && why.is_some() {
            return Err("Only a decision has a why".into());
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
            (Some(_), None, None, false) => Err("Say what changes: its text or retire".into()),
            (Some(id), text, why, false) => Ok(Change::Correct { id, text, why }),
        }
    }
}

/// Applies a change other than a new one to `kept`, found as `whose` {word} #`id`; answers
/// what happened.
fn apply<T: Kept>(
    kept: Option<&mut T>,
    whose: &str,
    change: Change,
    by: Learned,
) -> Result<String, String> {
    let word = T::WORD;
    let (Change::Correct { id, .. } | Change::Retire { id }) = &change else {
        return Err(format!("A new {word} is added, not changed"));
    };
    let id = *id;
    let kept = kept.ok_or_else(|| format!("{whose} has no {word} #{id}"))?;
    if kept.retired() {
        return Err(format!(
            "{whose} {word} #{id} was retired; add it again instead"
        ));
    }
    match change {
        Change::Correct { text, why, .. } => {
            kept.correct(text, why, by);
            Ok(format!("{whose} {word} #{id} is corrected."))
        }
        _ => {
            kept.retire(by);
            Ok(format!("{whose} {word} #{id} is retired."))
        }
    }
}

/// Makes room for one more in `list` of at most `most`, dropping its oldest retired, else its
/// oldest; answers the id dropped.
fn make_room<T: Kept>(list: &mut Vec<T>, most: usize) -> Option<u64> {
    if list.len() < most {
        return None;
    }
    let oldest = list.iter().position(Kept::retired).unwrap_or(0);
    Some(list.remove(oldest).id())
}

fn dropped_note<T: Kept>(dropped: Option<u64>, whose: &str, most: usize) -> String {
    dropped.map_or_else(String::new, |id| {
        format!(" {whose} keeps {most}, so {} #{id} was dropped.", T::WORD)
    })
}

/// Changes a decision of `project`, made on goal card `goal` when given, or a rule of that goal;
/// answers what happened, in words.
pub fn change(
    core: &Core,
    project: &str,
    goal: Option<u64>,
    list: List,
    change: Change,
    by: Learned,
) -> Result<String, String> {
    let mut state = core.boards().state.locked();
    let board = (state.boards)
        .entry(project.to_owned())
        .or_insert_with(|| Board::empty(project));
    if let Some(goal) = goal {
        let card = board.card(goal)?;
        if card.parent.is_some() {
            return Err(format!(
                "Card #{goal} sits under another card; a goal is a card at the top"
            ));
        }
    }
    let mut decided = None;
    let said = match (list, goal) {
        (List::Decisions, _) => {
            let whose = goal.map_or("The project's".to_owned(), |goal| format!("Goal #{goal}'s"));
            match change {
                Change::New { text, why } => {
                    let (decision, dropped) = board.decided.add(Decision {
                        id: 0,
                        goal,
                        text,
                        why,
                        holder: by.agent.clone(),
                        learned: by,
                        was: None,
                        retired: None,
                        asked: None,
                    });
                    let id = decision.id;
                    decided = decision.holder.is_some().then_some(decision);
                    let on = goal.map_or("for the project".to_owned(), |goal| {
                        format!("on goal #{goal}")
                    });
                    let dropped = dropped_note::<Decision>(dropped, "The project", MAX_DECISIONS);
                    format!("Decision #{id} is kept {on}.{dropped}")
                }
                change => {
                    let decision = change.id().and_then(|id| board.decided.find(goal, id));
                    apply(decision, &whose, change, by)?
                }
            }
        }
        (List::Rules, None) => return Err("A rule is kept on a goal".into()),
        (List::Rules, Some(goal)) => {
            let card = board.card_mut(goal)?;
            card.updated_at = epoch_ms();
            let kept = &mut card.goal;
            let whose = format!("Goal #{goal}'s");
            match change {
                Change::New { text, .. } => {
                    let dropped = make_room(&mut kept.rules, MAX_RULES);
                    kept.last += 1;
                    let id = kept.last;
                    kept.rules.push(GoalRule {
                        id,
                        text,
                        learned: by,
                        was: None,
                        retired: None,
                    });
                    let dropped =
                        dropped_note::<GoalRule>(dropped, &format!("Goal #{goal}"), MAX_RULES);
                    format!("Rule #{id} is kept on goal #{goal}.{dropped}")
                }
                change => {
                    let id = change.id();
                    let rule = kept.rules.iter_mut().find(|rule| Some(rule.id) == id);
                    apply(rule, &whose, change, by)?
                }
            }
        }
    };
    core.boards().save(&mut state, project);
    drop(state);
    if let Some(decision) = decided {
        (core.board_above().chief_decided)(core, project, goal, &decision);
    }
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
    /// The person chose it, in their answer to the agent's question that started this turn.
    #[serde(default)]
    person: bool,
    /// Not a decision of a goal: where the project deploys, for the person to accept.
    #[serde(default)]
    environments: Option<Vec<Value>>,
}

/// `board_decide`: records, corrects or retires a decision of the project, tagged with the goal
/// the agent's live card sits under, or proposes the project's environments.
pub async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
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
        let goal = goal_of(&core, &agent);
        let answers = (core.chats().get(&agent))
            .and_then(|chat| chat.feed.turn_prompt())
            .map(|prompt| answers_in(&prompt))
            .unwrap_or_default();
        let decided = [given.text.as_deref(), given.why.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let mut by = learned(&core, Some(&agent));
        if (given.person && !answers.is_empty())
            || (answers.iter()).any(|(question, answer)| records_answer(&decided, question, answer))
        {
            by.agent = None;
            by.name = None;
        }
        let (project, goal) = match goal {
            Some(goal) => (goal.project, Some(goal.card)),
            None => (project, None),
        };
        let said = change(&core, &project, goal, List::Decisions, deciding, by)?;
        Ok(json!({ "goal": goal, "note": said }))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The answers in `prompt`, the message that started an agent's turn, that the person gave to
/// that agent's questions, each with the question it answers.
fn answers_in(prompt: &str) -> Vec<(String, String)> {
    (prompt.split("<question-answer ").skip(1))
        .filter_map(|block| {
            let (tag, rest) = block.split_once('>')?;
            let question = (tag.split_once("question=\""))
                .and_then(|(_, rest)| rest.split_once('"'))
                .map_or("", |(question, _)| question);
            let answer = rest.split("</question-answer>").next()?.trim();
            Some((unescaped(question), answer.to_owned()))
        })
        .collect()
}

fn unescaped(attribute: &str) -> String {
    attribute
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Whether a decision saying `decided` records what the person answered to `question`: it holds
/// most of the longer words of their answer, or of the question for a bare "Yes" or "No".
fn records_answer(decided: &str, question: &str, answer: &str) -> bool {
    let decided = longer_words(decided);
    let mut said = longer_words(answer);
    said.remove("recommended");
    if said.is_empty() {
        said = longer_words(question);
    }
    let held = said.iter().filter(|word| decided.contains(*word)).count();
    !said.is_empty() && held * 2 >= said.len()
}

fn longer_words(text: &str) -> HashSet<String> {
    (text.split(|c: char| !c.is_alphanumeric()))
        .filter(|word| word.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect()
}

pub async fn person_decide(core: &Arc<Core>, params: DecideParams) -> Result<ProjectBoard, String> {
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
        change(
            &core,
            &project,
            Some(params.card),
            List::Decisions,
            deciding,
            by,
        )?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Something agents settled without the person.
pub struct Settled {
    pub project: String,
    /// The agent that decided.
    pub by: String,
    /// The agent that goes by it.
    pub holder: String,
    /// The question it settles, when one did and the record may name it.
    pub asked: Option<String>,
    pub text: String,
    pub why: Option<String>,
}

/// Keeps what agents settled, tagged with the goal its holder works on.
pub fn keep_settled(core: &Core, settled: Settled) -> Option<(Option<u64>, u64)> {
    let goal = goal_of(core, &settled.holder);
    let learned = learned(core, Some(&settled.by));
    let project = goal
        .as_ref()
        .map_or(settled.project, |goal| goal.project.clone());
    let mut state = core.boards().state.locked();
    let board = (state.boards)
        .entry(project.clone())
        .or_insert_with(|| Board::empty(&project));
    let goal = goal.and_then(|goal| board.goal_log(goal.card).map(|card| card.id));
    let (decision, _) = board.decided.add(Decision {
        id: 0,
        goal,
        text: clipped(&settled.text, MAX_TEXT_CHARS),
        why: settled.why.map(|why| clipped(&why, MAX_TEXT_CHARS)),
        learned,
        was: None,
        retired: None,
        asked: (settled.asked).map(|asked| clipped(&asked, MAX_TEXT_CHARS)),
        holder: Some(settled.holder),
    });
    core.boards().save(&mut state, &project);
    drop(state);
    (core.board_above().chief_decided)(core, &project, goal, &decision);
    Some((goal, decision.id))
}

/// What agents decided that nobody overruled, oldest first, in `project` or in every project: on a
/// goal since `since`, as the goal's page lists the rest, and on a project's own log whenever, as
/// nowhere else lists it.
pub fn decided_since(core: &Core, since: Option<u64>, project: Option<&str>) -> Vec<SwarmDecision> {
    let state = core.boards().state.locked();
    let mut decided = Vec::new();
    for board in state.boards.values() {
        if project.is_some_and(|project| project != board.project) {
            continue;
        }
        let recent = board.decided.standing().filter(|decision| {
            let from = decision.goal.and(since).unwrap_or(0);
            decision.holder.is_some() && decision.learned.at >= from
        });
        decided.extend(recent.map(|decision| SwarmDecision {
            project: board.project.clone(),
            goal: decision.goal,
            decision: decision.clone(),
        }));
    }
    decided.sort_by_key(|swarm| swarm.decision.learned.at);
    decided
}

/// The person overrules what agents decided: it is retired, and the agent that goes by it hears,
/// as does the one that decided it. Asked again for a decision they already overruled, as a
/// second click sent before the first is answered, it changes nothing and answers the board.
pub async fn overrule(core: &Arc<Core>, params: OverruleParams) -> Result<ProjectBoard, String> {
    let goal = params.goal;
    let (project, overruled, view) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let by = learned(&core, None);
            let mut state = core.boards().state.locked();
            let board = (state.boards.get_mut(&project))
                .ok_or_else(|| "This project keeps no decisions".to_owned())?;
            let decision = (board.decided.find(params.goal, params.decision))
                .ok_or("That decision is not kept here")?;
            let already = (decision.retired.as_ref()).is_some_and(|by| by.agent.is_none());
            if decision.retired.is_some() && !already {
                return Err("That decision has been retired already".into());
            }
            let overruled = (!already).then(|| {
                decision.retired = Some(by);
                decision.clone()
            });
            if overruled.is_some() {
                core.boards().save(&mut state, &project);
            }
            drop(state);
            let view = project_view(&core, &project);
            let overruled = overruled.map(|decision| (decision, params.instead));
            Ok((project, overruled, view))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let Some((decision, instead)) = overruled else {
        return Ok(view);
    };
    let told: Vec<String> = (decision.holder.iter())
        .chain(
            decision
                .learned
                .agent
                .iter()
                .filter(|by| Some(*by) != decision.holder.as_ref()),
        )
        .cloned()
        .collect();
    for agent in &told {
        let name = core.names().name_or_id(agent);
        let text = instead
            .clone()
            .unwrap_or_else(|| "The person overruled this.".to_owned());
        let note = "The person overruled this decision of the team's. Stop going by it and undo what rests on it; if they have not said what they want instead, hold that work until they do.".to_owned();
        let overruled = Overruled {
            question: decision.says(),
            asker: name,
            asker_id: agent.clone(),
            cli: core
                .agent_now(agent)
                .cli()
                .unwrap_or_else(|| "agent".to_owned()),
            text,
            note,
        };
        if let Err(error) = (core.board_above().messages_overruled)(core, overruled).await {
            eprintln!(
                "sikemux core: an overruled decision in {project} did not reach its agent: {error}"
            );
        }
    }
    (core.board_above().chief_overruled)(core, &project, goal, (decision.id, &decision.text), told);
    Ok(view)
}

/// Undoes the person's Overrule: the decision stands again, the rule that brought its kind to
/// them is retired, and the agents the Overrule told and the chief hear it stands. Only a
/// decision the person retired by Overrule comes back.
/// Asked again for a decision that already stands, it changes nothing and answers the board.
pub async fn undo_overrule(
    core: &Arc<Core>,
    params: UndoOverruleParams,
) -> Result<ProjectBoard, String> {
    let (goal, id) = (params.goal, params.decision);
    let (project, undone, view) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let mut state = core.boards().state.locked();
            let board = (state.boards.get_mut(&project))
                .ok_or_else(|| "This project keeps no decisions".to_owned())?;
            let decision = (board.decided.find(params.goal, params.decision))
                .ok_or("That decision is not kept here")?;
            let stands = decision.retired.is_none();
            if !stands && !(decision.retired.as_ref()).is_some_and(|by| by.agent.is_none()) {
                return Err("That decision is not overruled".into());
            }
            let undone = match stands {
                true => None,
                false => {
                    let overrule = (core.board_above().chief_undoable)(
                        &core,
                        &project,
                        params.goal,
                        params.decision,
                    )?;
                    decision.retired = None;
                    Some((overrule, decision.text.clone()))
                }
            };
            if undone.is_some() {
                core.boards().save(&mut state, &project);
            }
            drop(state);
            let view = project_view(&core, &project);
            Ok((project, undone, view))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let Some((overrule, text)) = undone else {
        return Ok(view);
    };
    (core.board_above().chief_overrule_undone)(core, &project, goal, (id, &text), overrule).await;
    Ok(view)
}

/// How many decisions agents made in `project` from `from` to `to`, and how many of all its
/// decisions the person overruled then.
pub fn counted_since(core: &Core, project: &str, from: u64, to: u64) -> (usize, usize) {
    let state = core.boards().state.locked();
    let Some(board) = state.boards.get(project) else {
        return (0, 0);
    };
    let decisions = board.decided.decisions.iter();
    let (mut decided, mut overruled) = (0, 0);
    for decision in decisions {
        let within = |at: u64| (from..to).contains(&at);
        if decision.learned.agent.is_some() && within(decision.learned.at) {
            decided += 1;
        }
        if (decision.retired.as_ref()).is_some_and(|by| by.agent.is_none() && within(by.at)) {
            overruled += 1;
        }
    }
    (decided, overruled)
}

/// An Overrule: the rule it kept and the agents it told.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overrule {
    pub rule: Option<u64>,
    pub told: Vec<String>,
}
