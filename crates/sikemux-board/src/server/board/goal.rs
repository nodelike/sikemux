//! What a goal keeps on its card, so it goes when the goal goes: what it
//! decided, rules only its own agents follow, and assertions, what must hold
//! once its slices are done, agreed before they are cut. A goal is a card at
//! the top of the board, and one that keeps anything stays there. What it
//! keeps shares ids that are never given twice. Each is corrected, keeping
//! what it said before, or retired; a goal full of one kind drops its oldest
//! retired first, then its oldest. The person or the goal's lead writes its
//! assertions, and once a slice fulfilling one has started, only the person. A
//! command the lead writes as an assertion's evidence counts only once the
//! person approves exactly that command, and any command only while the
//! repository files it names are as they accepted them.
//! What agents settle among themselves goes in its goal's log, or the project's
//! when they work on no goal, where the person can overrule it.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_above::Overruled;
use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::{
    AnswerCommandParams, AssertParams, DecideParams, OverruleParams, UndoOverruleParams,
};
use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::SwarmDecision;
use sikemux_view::memory::Learned;
use sikemux_view::slice::{
    Assertion, ChangedFile, Collect, CommandFiles, Decision, GoalReport, GoalRule, NamedFile,
    ReportEvery,
};

use super::{goal_of, project_at, project_view, Board, GoalRef, ProjectBoard};
use crate::cli::protocol::HarnessRequest;
use crate::server::changed::What;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::text::{clipped, to_keep};

const MAX_EACH: usize = 64;
const MAX_TEXT_CHARS: usize = 1000;
const MAX_WORKFLOW_CHARS: usize = 200;
const MAX_COMMAND_CHARS: usize = 1000;

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

/// What a goal card keeps, with the last id it gave.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    last: u64,
    #[serde(default)]
    decisions: Vec<Decision>,
    #[serde(default)]
    rules: Vec<GoalRule>,
    #[serde(default)]
    assertions: Vec<Assertion>,
    /// How often its report is written; none writes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report_every: Option<ReportEvery>,
    /// Its latest reports, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reports: Vec<GoalReport>,
}

impl Goal {
    /// The card never kept anything as a goal.
    pub fn is_none(&self) -> bool {
        self.last == 0 && self.report_every.is_none() && self.reports.is_empty()
    }

    pub fn report_every(&self) -> Option<ReportEvery> {
        self.report_every
    }

    /// Writes its report as often as `every` says, or no more for none.
    pub fn report_on(&mut self, every: Option<ReportEvery>) {
        self.report_every = every;
    }

    pub fn reports(&self) -> &[GoalReport] {
        &self.reports
    }

    pub fn last_report(&self) -> Option<&GoalReport> {
        self.reports.last()
    }

    /// Keeps `report` as its latest, letting the oldest go past `keep`.
    pub fn reported(&mut self, report: GoalReport, keep: usize) {
        self.reports.push(report);
        let over = self.reports.len().saturating_sub(keep);
        self.reports.drain(..over);
    }

    pub fn decisions(&self) -> Vec<Decision> {
        self.decisions.clone()
    }

    pub fn rules(&self) -> Vec<GoalRule> {
        self.rules.clone()
    }

    pub fn assertions(&self) -> &[Assertion] {
        &self.assertions
    }

    /// The assertion `id`, while it stands.
    pub fn assertion(&self, id: u64) -> Option<&Assertion> {
        (self.assertions.iter()).find(|assertion| assertion.id == id && assertion.retired.is_none())
    }

    /// Those of `named`, as slice `card` holds them at `head`, that the person has not accepted
    /// for assertion `id`: what it now waits for them to answer, in place of what it waited for of
    /// that slice, and what they turned down.
    pub fn files_of_slice(
        &mut self,
        id: u64,
        card: u64,
        head: &str,
        named: &[NamedFile],
    ) -> (Vec<NamedFile>, Vec<NamedFile>) {
        let Some(kept) = (self.assertions.iter_mut()).find(|kept| kept.id == id) else {
            return Default::default();
        };
        let files = &mut kept.files;
        files.changed.retain(|changed| changed.card != card);
        files.refused.retain(|refused| refused.card != card);
        let (refused, asked): (Vec<NamedFile>, Vec<NamedFile>) = (named.iter())
            .filter(|file| match file.blob {
                Some(_) => !files.accepted.contains(file),
                None => {
                    !files.accepted.contains(file)
                        && (files.accepted.iter()).any(|known| known.path == file.path)
                }
            })
            .cloned()
            .partition(|file| files.turned_down.contains(file));
        files
            .changed
            .extend(asked.iter().cloned().map(|file| ChangedFile {
                card,
                head: head.to_owned(),
                file,
            }));
        files
            .refused
            .extend(refused.iter().cloned().map(|file| ChangedFile {
                card,
                head: head.to_owned(),
                file,
            }));
        (asked, refused)
    }
}

/// Forgets what waited for the person of a slice that moved past that commit, or is not at work.
/// Whether any went.
pub fn forget_moved_on(board: &mut Board) -> bool {
    let waiting = |card: &super::Card| {
        (card.goal.assertions.iter()).any(|assertion| {
            !assertion.files.changed.is_empty() || !assertion.files.refused.is_empty()
        })
    };
    if !board.cards.iter().any(waiting) {
        return false;
    }
    let heads: Vec<(u64, String)> = (board.cards.iter())
        .filter(|card| card.status == super::CardStatus::Progress)
        .filter_map(|card| Some((card.id, card.slice.as_ref()?.head()?.to_owned())))
        .collect();
    let mut forgot = false;
    for assertion in (board.cards.iter_mut()).flat_map(|card| card.goal.assertions.iter_mut()) {
        let files = &mut assertion.files;
        let before = files.changed.len() + files.refused.len();
        let current = |kept: &ChangedFile| {
            (heads.iter()).any(|(card, head)| *card == kept.card && *head == kept.head)
        };
        files.changed.retain(current);
        files.refused.retain(current);
        forgot |= files.changed.len() + files.refused.len() != before;
    }
    if forgot {
        super::slice::rebind(board);
    }
    forgot
}

/// What a goal keeps that still stands, oldest first.
#[derive(Default)]
pub struct Standing {
    pub decisions: Vec<Decision>,
    pub rules: Vec<GoalRule>,
}

/// The project's decisions still standing that were made since `since`, newest first, at
/// most `most`: every goal's, and those made on no goal.
pub fn standing_since(core: &Core, project: &str, since: u64, most: usize) -> Vec<Decision> {
    let state = core.boards().state.locked();
    let Some(board) = state.boards.get(project) else {
        return Vec::new();
    };
    let mut decisions: Vec<Decision> = (board.cards.iter())
        .map(|card| &card.goal)
        .chain([&board.decided])
        .flat_map(|goal| goal.decisions.iter())
        .filter(|decision| decision.retired.is_none() && decision.learned.at >= since)
        .cloned()
        .collect();
    decisions.sort_by_key(|decision| std::cmp::Reverse(decision.learned.at));
    decisions.truncate(most);
    decisions
}

pub fn standing(core: &Core, goal: &GoalRef) -> Standing {
    let state = core.boards().state.locked();
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
pub enum List {
    Decisions,
    Rules,
    Assertions,
}

pub enum Change {
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
        Self::read(id, text, why, None, retire)
    }

    /// Reads what is asked of a goal's assertions; a new one names its evidence.
    pub fn of_assertion(
        id: Option<u64>,
        text: Option<&str>,
        evidence: Option<Collect>,
        retire: bool,
    ) -> Result<Self, String> {
        if id.is_none() && evidence.is_none() && !retire {
            return Err(
                "Say what evidence shows it: check, recording, screenshot, workflow or command"
                    .into(),
            );
        }
        let evidence = evidence.map(checked_command).transpose()?;
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

/// A command to keep is one line, so the person sees all of what they approve.
fn checked_command(evidence: Collect) -> Result<Collect, String> {
    let Collect::Command { command } = evidence else {
        return Ok(evidence);
    };
    let command = to_keep(&command, "command", MAX_COMMAND_CHARS)?;
    if command.chars().any(char::is_control) {
        return Err("command is one line, with no control characters".into());
    }
    Ok(Collect::Command { command })
}

/// The command a person writing `evidence` approves by writing it.
fn written_by_person(evidence: &Collect, by: &Learned) -> Option<String> {
    match evidence {
        Collect::Command { command } if by.agent.is_none() => Some(command.clone()),
        _ => None,
    }
}

const NOT_UNTIL_APPROVED: &str = " Its command does not count, and does not run, until the person approves exactly that command, in Needs you.";

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
pub fn change(
    core: &Core,
    project: &str,
    goal: u64,
    list: List,
    change: Change,
    by: Learned,
) -> Result<String, String> {
    keep_change(core, project, goal, list, change, by, Vec::new())
}

/// Changes goal `goal`'s assertions, keeping how the project's branch holds the files a command
/// named in the change names, as what the person accepts along with it.
fn change_assertion(
    core: &Core,
    project: &str,
    goal: u64,
    asserting: Change,
    by: Learned,
) -> Result<String, String> {
    let command = match &asserting {
        Change::New {
            evidence: Some(Collect::Command { command }),
            ..
        }
        | Change::Correct {
            evidence: Some(Collect::Command { command }),
            ..
        } => Some(command.as_str()),
        _ => None,
    };
    let files = command
        .and_then(|command| {
            super::commands::named_at(std::path::Path::new(project), "HEAD", command).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|file| file.blob.is_some())
        .collect();
    keep_change(core, project, goal, List::Assertions, asserting, by, files)
}

fn keep_change(
    core: &Core,
    project: &str,
    goal: u64,
    list: List,
    change: Change,
    by: Learned,
    files: Vec<NamedFile>,
) -> Result<String, String> {
    let mut state = core.boards().state.locked();
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
                holder: by.agent.clone(),
                learned: by,
                was: None,
                retired: None,
                asked: None,
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
            let evidence = evidence.ok_or(
                "Say what evidence shows it: check, recording, screenshot, workflow or command",
            )?;
            let dropped = make_room(&mut kept.assertions, goal, &bound)?;
            kept.last += 1;
            let id = kept.last;
            let approved = written_by_person(&evidence, &by);
            let assertion = Assertion {
                id,
                text,
                evidence,
                learned: by,
                was: None,
                retired: None,
                files: match &approved {
                    Some(_) => CommandFiles {
                        accepted: files,
                        ..CommandFiles::default()
                    },
                    None => CommandFiles {
                        proposed: files,
                        ..CommandFiles::default()
                    },
                },
                approved,
            };
            let waits = match assertion.awaits_approval() {
                Some(_) => NOT_UNTIL_APPROVED,
                None => "",
            };
            kept.assertions.push(assertion);
            format!("Assertion #{id} is kept on goal #{goal}.{dropped}{waits}")
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
            let by_person = by.agent.is_none().then(|| by.clone());
            let files_of_proposal = files.clone();
            let mut said = apply(&mut kept.assertions, goal, change, by)?;
            if let Some((id, evidence)) = evidence {
                if let Some(kept) = kept.assertions.iter_mut().find(|kept| kept.id == id) {
                    kept.files.changed.clear();
                    kept.files.proposed.clear();
                    if let Some(person) = &by_person {
                        kept.approved = written_by_person(&evidence, person);
                        kept.files = CommandFiles {
                            accepted: files,
                            ..CommandFiles::default()
                        };
                    }
                    kept.evidence = evidence;
                    if kept.awaits_approval().is_some() {
                        kept.files.proposed = files_of_proposal;
                        said.push_str(NOT_UNTIL_APPROVED);
                    }
                }
            }
            said
        }
    };
    card.updated_at = epoch_ms();
    let asserting = matches!(list, List::Assertions);
    if asserting {
        super::slice::rebind(board);
    }
    core.boards().save(&mut state, project);
    drop(state);
    if asserting {
        core.changed(What::View);
    }
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
    if let Change::Correct {
        evidence: Some(Collect::Command { .. }),
        ..
    } = change
    {
        let kept = card.goal.assertion(*id);
        if !kept.is_some_and(|kept| matches!(kept.evidence, Collect::Command { .. })) {
            return Err(format!(
                "Assertion #{id} isn't shown by a command, so turning down yours would leave it with nothing; write a new assertion for the command"
            ));
        }
    }
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
    /// The person chose it, in their answer to the agent's question that started this turn.
    #[serde(default)]
    person: bool,
    /// Not a decision of a goal: where the project deploys, for the person to accept.
    #[serde(default)]
    environments: Option<Vec<Value>>,
}

/// `board_decide`: records, corrects or retires a decision of the goal the
/// agent's live card sits under, or proposes the project's environments.
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
        let goal = goal_of(&core, &agent).ok_or(
            "You work on no card that is not done, so there is no goal to decide for; claim your card first",
        )?;
        let answers = (core.chats().get(&agent))
            .and_then(|chat| chat.feed.turn_prompt())
            .map(|prompt| answers_in(&prompt))
            .unwrap_or_default();
        if given.person && answers.is_empty() {
            return Err("person: true is for what the person chose in their answer to your question, and no answer of theirs started this turn; record it without person".into());
        }
        let decided = [given.text.as_deref(), given.why.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let mut by = learned(&core, Some(&agent));
        if given.person
            || (answers.iter()).any(|(question, answer)| records_answer(&decided, question, answer))
        {
            by.agent = None;
            by.name = None;
        }
        let said = change(&core, &goal.project, goal.card, List::Decisions, deciding, by)?;
        Ok(json!({ "goal": goal.card, "note": said }))
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
        change(&core, &project, params.card, List::Decisions, deciding, by)?;
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
    pub asked: String,
    pub text: String,
    pub why: Option<String>,
}

/// Keeps what agents settled, on the goal its holder works on, or else on its project's board.
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
    let kept = match goal.and_then(|goal| board.card_mut(goal).ok()) {
        Some(card) => {
            card.updated_at = epoch_ms();
            &mut card.goal
        }
        None => &mut board.decided,
    };
    make_room(&mut kept.decisions, 0, &[]).ok()?;
    kept.last += 1;
    kept.decisions.push(Decision {
        id: kept.last,
        text: clipped(&settled.text, MAX_TEXT_CHARS),
        why: settled.why.map(|why| clipped(&why, MAX_TEXT_CHARS)),
        learned,
        was: None,
        retired: None,
        asked: Some(clipped(&settled.asked, MAX_TEXT_CHARS)),
        holder: Some(settled.holder),
    });
    let decision = kept.last;
    core.boards().save(&mut state, &project);
    Some((goal, decision))
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
        let logs = (board.cards.iter())
            .map(|card| (Some(card.id), &card.goal))
            .chain([(None, &board.decided)]);
        for (goal, kept) in logs {
            let from = goal.and(since).unwrap_or(0);
            let recent = (kept.decisions.iter()).filter(|decision| {
                decision.holder.is_some()
                    && decision.retired.is_none()
                    && decision.learned.at >= from
            });
            decided.extend(recent.map(|decision| SwarmDecision {
                project: board.project.clone(),
                goal,
                decision: decision.clone(),
            }));
        }
    }
    decided.sort_by_key(|swarm| swarm.decision.learned.at);
    decided
}

/// The person overrules what agents decided: it is retired, and the agent that goes by it hears,
/// as does the one that decided it. Asked again for a decision they already overruled, as a
/// second click sent before the first is answered, it changes nothing and answers the board.
pub async fn overrule(core: &Arc<Core>, params: OverruleParams) -> Result<ProjectBoard, String> {
    let goal = params.goal;
    let (project, overruled, stopped_auto, view) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let by = learned(&core, None);
            let mut state = core.boards().state.locked();
            let board = (state.boards.get_mut(&project))
                .ok_or_else(|| "This project keeps no decisions".to_owned())?;
            let kept = match params.goal {
                Some(goal) => &mut board.card_mut(goal)?.goal,
                None => &mut board.decided,
            };
            let decision = (kept.decisions.iter_mut())
                .find(|decision| decision.id == params.decision)
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
            let stopped_auto = overruled.is_some()
                && super::recover::overruled(&core, &project, params.goal, params.decision);
            let view = project_view(&core, &project);
            let overruled = overruled.map(|decision| (decision, params.instead));
            Ok((project, overruled, stopped_auto, view))
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
    (core.board_above().chief_overruled)(
        core,
        &project,
        goal,
        (decision.id, &decision.text),
        stopped_auto,
        told,
    );
    Ok(view)
}

/// Undoes the person's Overrule: the decision stands again, the rule that brought its kind to
/// them is retired, and the agents the Overrule told and the chief hear it stands. Only a
/// decision the person retired by Overrule comes back, and not one whose Overrule stopped Auto.
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
            let kept = match params.goal {
                Some(goal) => &mut board.card_mut(goal)?.goal,
                None => &mut board.decided,
            };
            let decision = (kept.decisions.iter_mut())
                .find(|decision| decision.id == params.decision)
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
    let decisions = (board.cards.iter())
        .map(|card| &card.goal)
        .chain([&board.decided])
        .flat_map(|goal| goal.decisions.iter());
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolAssert {
    goal: Option<u64>,
    assertion: Option<u64>,
    text: Option<String>,
    evidence: Option<String>,
    workflow: Option<String>,
    command: Option<String>,
    #[serde(default)]
    retire: bool,
}

/// The evidence the tool names: a kind, with the workflow's name for one a
/// workflow shows and the command for one a command shows.
fn evidence_of(
    kind: Option<&str>,
    workflow: Option<String>,
    command: Option<String>,
) -> Result<Option<Collect>, String> {
    match (kind, command) {
        (Some("command") | None, Some(command)) if workflow.is_none() => {
            return Ok(Some(Collect::Command { command }))
        }
        (Some("command"), None) => return Err("Say the command to run".into()),
        (_, Some(_)) => return Err("Only a command's evidence names a command".into()),
        (_, None) => {}
    }
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
            "evidence is check, recording, screenshot, workflow or command, not {kind}"
        )),
    }
}

/// `board_assert`: writes, corrects or retires an assertion of a goal the agent
/// leads, the one its live card sits under unless it names one.
pub async fn tool_assert(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "board_assert")?;
    let given: ToolAssert =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let asserting = Change::of_assertion(
            given.assertion,
            given.text.as_deref(),
            evidence_of(given.evidence.as_deref(), given.workflow, given.command)?,
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
        let said = change_assertion(&core, &project, goal, asserting, by)?;
        Ok(json!({ "goal": goal, "note": said }))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The person answers a command an agent wrote, or a change to a file a command names. Accepted,
/// that exact command or content counts. A command turned down goes back to the one they last
/// approved, or its assertion is retired with none. A content turned down keeps the command from
/// running on it. Only the person's own app reaches this, so an agent has no way to approve its
/// own.
pub async fn person_answer_command(
    core: &Arc<Core>,
    params: AnswerCommandParams,
) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let by = learned(&core, None);
        let (settled, told) = {
            let mut state = core.boards().state.locked();
            let board = (state.boards.get_mut(&project))
                .ok_or("This project's board has no such assertion")?;
            let card = board.card_mut(params.card)?;
            let kept = (card.goal.assertions.iter_mut())
                .find(|kept| kept.id == params.assertion && kept.retired.is_none())
                .ok_or("That assertion is gone")?;
            let waiting: Vec<&NamedFile> = (kept.files.changed.iter())
                .map(|changed| &changed.file)
                .collect();
            let as_seen = kept.command() == Some(params.command.as_str())
                && (kept.awaits_approval().is_some()
                    || (!waiting.is_empty()
                        && waiting.iter().all(|file| params.files.contains(file))
                        && params.files.iter().all(|file| waiting.contains(&file))));
            if !as_seen {
                return Err(
                    "That command or a file it names changed, or was answered already; look at it again".into(),
                );
            }
            let told = if kept.awaits_approval().is_some() {
                let then = match (params.accept, kept.approved.take()) {
                    (true, _) => {
                        kept.approved = Some(params.command.clone());
                        kept.files.accepted = std::mem::take(&mut kept.files.proposed);
                        "approved it, so it runs on each slice's latest commit".to_owned()
                    }
                    (false, Some(earlier)) => {
                        let words = format!("turned it down, so `{earlier}` stands");
                        kept.evidence = Collect::Command {
                            command: earlier.clone(),
                        };
                        kept.approved = Some(earlier);
                        kept.files.proposed.clear();
                        words
                    }
                    (false, None) => {
                        kept.retired = Some(by);
                        "turned it down, so the assertion is retired".to_owned()
                    }
                };
                kept.learned.agent.clone().map(|agent| super::Notice {
                    to: agent,
                    text: format!(
                        "Of the command `{}` you wrote for assertion #{} of goal #{}, the person {then}.",
                        params.command, params.assertion, params.card
                    ),
                })
            } else {
                let answered: Vec<ChangedFile> = std::mem::take(&mut kept.files.changed);
                match params.accept {
                    true => (kept.files.accepted).extend(answered.into_iter().map(|changed| changed.file)),
                    false => {
                        (kept.files.turned_down).extend(answered.iter().map(|changed| changed.file.clone()));
                        kept.files.refused.extend(answered);
                    }
                }
                None
            };
            card.updated_at = epoch_ms();
            super::slice::rebind(board);
            let settled = super::steps::settle_cards(&core, &mut state, &project);
            core.boards().save(&mut state, &project);
            (settled, told)
        };
        core.changed(What::View);
        settled.later.run(&core);
        let mut notices = settled.notices;
        notices.extend(told);
        super::send_notices(&core, notices);
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

pub async fn person_assert(core: &Arc<Core>, params: AssertParams) -> Result<ProjectBoard, String> {
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
        change_assertion(&core, &project, params.card, asserting, by)?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// An Overrule: the rule it kept, whether it stopped Auto's run on a slice, which cannot be put
/// back, and the agents it told.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overrule {
    pub rule: Option<u64>,
    pub stopped_auto: bool,
    pub told: Vec<String>,
}
