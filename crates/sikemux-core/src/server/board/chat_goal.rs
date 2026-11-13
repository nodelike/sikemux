//! A chat picking up a goal. A goal stays a unit of work a chat takes on, never the chat itself:
//! one chat can hold several over its life, and finishing one leaves the chat going. The core
//! counts what a chat does and, once it looks like real work, tells the chat's agent, which
//! decides whether to track it as a goal.

use crate::server::agent_now::AgentNowCore;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::ChatGoalParams;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{ChatGoal, GoalMarker, GoalMoment};
use sikemux_view::slice::{SliceState, Wait, WaitOn};

use super::{holder, lease, slice, waits, Board, Card, CardStatus};
use crate::server::chat::feed::Feed;
use crate::server::chat::Chat;
use crate::server::connection::blocking;
use crate::server::kept::Kept;
use crate::server::Core;
use sikemux_fold::update::goal_notice;
use sikemux_fold::ChatEventKind;
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

/// A turn this long is one nobody would call quick.
const LONG_TURN_MS: u64 = 5 * 60 * 1000;
/// What each thing a chat did counts towards a bar. A teammate counts most, since nobody starts
/// one for a small job.
const COMMIT_POINTS: u32 = 2;
const TEAMMATE_POINTS: u32 = 3;
const LONG_TURN_POINTS: u32 = 2;
/// A CLI starts subagents by itself for small questions, often several at once, so each counts
/// least and none waives the time a bar asks: the chat's clock runs while they do.
const SUBAGENT_POINTS: u32 = 1;

/// How much a chat must have done before its agent is told. Points alone are not enough, as a few
/// commits in five minutes are a quick chat; a teammate waives the time, as the chat's own
/// clock stops while its teammates work.
struct Bar {
    points: u32,
    worked_ms: u64,
}

/// Three commits, a teammate and two commits, or three long turns, over ten minutes of work.
const FIRST_BAR: Bar = Bar {
    points: 6,
    worked_ms: 10 * 60 * 1000,
};
/// After the agent was told, or the person took a goal back: twice as much again, counted from
/// that moment.
const AFTER_TOLD: Bar = Bar {
    points: 12,
    worked_ms: 20 * 60 * 1000,
};

/// Chats kept, past which those touched longest ago go.
const MAX_CHATS: usize = 512;
const FILE: &str = "chat-goals.json";

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tally {
    commits: u32,
    teammates: u32,
    subagents: u32,
    long_turns: u32,
    worked_ms: u64,
}

impl Tally {
    fn since(self, earlier: Tally) -> Tally {
        Tally {
            commits: self.commits.saturating_sub(earlier.commits),
            teammates: self.teammates.saturating_sub(earlier.teammates),
            subagents: self.subagents.saturating_sub(earlier.subagents),
            long_turns: self.long_turns.saturating_sub(earlier.long_turns),
            worked_ms: self.worked_ms.saturating_sub(earlier.worked_ms),
        }
    }

    fn clears(self, bar: &Bar) -> bool {
        let points = self.commits * COMMIT_POINTS
            + self.teammates * TEAMMATE_POINTS
            + self.subagents * SUBAGENT_POINTS
            + self.long_turns * LONG_TURN_POINTS;
        points >= bar.points && (self.worked_ms >= bar.worked_ms || self.teammates > 0)
    }

    /// Why this looks like real work, as the agent is told it.
    fn reason(self) -> String {
        let counted = |count: u64, one: &str| match count {
            0 => None,
            1 => Some(format!("1 {one}")),
            _ => Some(format!("{count} {one}s")),
        };
        let parts = [
            counted(self.commits.into(), "commit"),
            counted(self.teammates.into(), "teammate"),
            counted(self.subagents.into(), "subagent"),
            counted(self.worked_ms / 60_000, "minute").map(|minutes| format!("{minutes} of work")),
        ];
        parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attached {
    project: String,
    card: u64,
    name: String,
    /// When the chat took it on.
    #[serde(default)]
    began: u64,
    /// How many of its steps were done when the chat last said so.
    #[serde(default)]
    steps_done: usize,
    /// The agent only just added it, and the person can still take that back.
    undoable: bool,
    /// Its line in the chat waits until that has passed, so the chat never shows a goal that was
    /// taken back.
    unshown: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    tally: Tally,
    /// What an earlier goal already covered, so only what came after counts.
    #[serde(default)]
    covered: Tally,
    /// How far the chat had got when its agent was last told or the person took a goal back,
    /// which raises the bar for telling it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    told: Option<Tally>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    goal: Option<Attached>,
    /// What the agent is told with the person's next message, so it starts no turn of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<ChatContext>,
    #[serde(skip)]
    began: Option<u64>,
    #[serde(default)]
    at: u64,
}

impl Record {
    /// A goal ended: only work after this counts, towards the higher bar when it was taken away
    /// rather than finished.
    fn ended(&mut self, finished: bool) {
        self.covered = self.tally;
        self.told = (!finished).then_some(self.tally);
    }

    /// Tells the agent, with the person's next message, once its chat has done enough to look
    /// like real work.
    fn weigh(&mut self) {
        if self.goal.is_some() {
            return;
        }
        let (from, bar) = match self.told {
            Some(told) => (told, &AFTER_TOLD),
            None => (self.covered, &FIRST_BAR),
        };
        let done = self.tally.since(from);
        if !done.clears(bar) {
            return;
        }
        self.told = Some(self.tally);
        self.note = Some(ChatContext {
            uri: track_note_uri(),
            title: "Real work".to_owned(),
            text: format!(
                "This chat has grown into real work: {}. If it is one piece of work with an end, track it as a goal yourself, without asking the person: board_add with a title, a promise saying what is true once it is done, and claim: true, then carry on, and mark it done with board_update and a result when it is. If it is not, carry on as you were. Don't mention this note.",
                done.reason()
            ),
        });
    }
}

pub(super) struct ChatGoals {
    kept: Kept,
    chats: Mutex<BTreeMap<String, Record>>,
    /// How each card with an owner last stood, to tell clients when that changes.
    owned: Mutex<HashMap<(String, u64), SliceState>>,
}

impl ChatGoals {
    pub(super) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let chats = Mutex::new(kept.load());
        Self {
            kept,
            chats,
            owned: Mutex::default(),
        }
    }

    fn save(&self, chats: &mut BTreeMap<String, Record>) {
        while chats.len() > MAX_CHATS {
            let oldest = (chats.iter())
                .filter(|(_, record)| record.goal.is_none())
                .min_by_key(|(_, record)| record.at)
                .map(|(agent, _)| agent.clone());
            let Some(oldest) = oldest else { break };
            chats.remove(&oldest);
        }
        self.kept.save_soon(&*chats);
    }
}

/// Whether the core counts this agent's work: a chat the person started, not one another agent
/// did, whose work its lead answers for.
fn counted(core: &Core, agent: &str) -> bool {
    core.chats().get(agent).is_some()
        && (core.names().get(agent)).is_some_and(|named| named.parent.is_none())
}

fn count(core: &Core, agent: &str, add: impl FnOnce(&mut Tally)) {
    if !counted(core, agent) {
        return;
    }
    let goals = &core.boards().chat_goals;
    let mut chats = goals.chats.locked();
    let record = chats.entry(agent.to_owned()).or_default();
    add(&mut record.tally);
    record.at = epoch_ms();
    goals.save(&mut chats);
}

pub(crate) fn committed(core: &Core, agent: &str) {
    count(core, agent, |tally| tally.commits += 1);
}

/// The agent started a teammate.
pub(crate) fn hired(core: &Core, lead: &str) {
    count(core, lead, |tally| tally.teammates += 1);
}

pub(crate) fn subagent_started(core: &Core, agent: &str) {
    count(core, agent, |tally| tally.subagents += 1);
}

/// A turn began, which ends the moment a goal just tracked could be taken back.
pub(crate) fn turn_started(core: &Core, agent: &str) {
    if !counted(core, agent) {
        return;
    }
    let goals = &core.boards().chat_goals;
    let (attached, settled) = {
        let mut chats = goals.chats.locked();
        let record = chats.entry(agent.to_owned()).or_default();
        record.began = Some(epoch_ms());
        let settled = (record.goal.as_mut())
            .filter(|goal| goal.unshown)
            .map(|goal| {
                goal.undoable = false;
                goal.unshown = false;
                goal.clone()
            });
        let attached = record.goal.clone();
        if settled.is_some() {
            goals.save(&mut chats);
        }
        (attached, settled)
    };
    if let Some(goal) = attached {
        lease::back_at_work(core, &goal.project, goal.card, agent);
    }
    if let (Some(goal), Some(chat)) = (settled, core.chats().get(agent)) {
        goal_line(core, &chat, &goal, GoalMoment::Started);
        push_head(core, agent);
    }
}

/// A turn ended: it counts towards the chat's work, and one that ran its course can tell the
/// agent so.
pub(crate) fn turn_ended(core: &Core, agent: &str, ran_its_course: bool) {
    let goals = &core.boards().chat_goals;
    let mut chats = goals.chats.locked();
    let Some(record) = chats.get_mut(agent) else {
        return;
    };
    let Some(began) = record.began.take() else {
        return;
    };
    let took = epoch_ms().saturating_sub(began);
    record.tally.worked_ms += took;
    record.tally.long_turns += u32::from(took >= LONG_TURN_MS);
    record.at = epoch_ms();
    if ran_its_course {
        record.weigh();
    }
    goals.save(&mut chats);
}

/// The agent lost its slice to the lease, which ends the moment it could be taken back.
pub(super) fn lease_lapsed(core: &Core, agent: &str) {
    let goals = &core.boards().chat_goals;
    let mut chats = goals.chats.locked();
    let Some(goal) = chats.get_mut(agent).and_then(|record| record.goal.as_mut()) else {
        return;
    };
    if goal.undoable {
        goal.undoable = false;
        goals.save(&mut chats);
    }
}

/// What the agent is told with the person's next message, once. Being told its work looks like a
/// goal no longer applies once the chat has one.
pub(crate) fn take_note(core: &Core, agent: &str) -> Option<ChatContext> {
    let goals = &core.boards().chat_goals;
    let mut chats = goals.chats.locked();
    let record = chats.get_mut(agent)?;
    let note = record.note.take()?;
    let stale = record.goal.is_some() && note.uri == track_note_uri();
    goals.save(&mut chats);
    (!stale).then_some(note)
}

fn track_note_uri() -> String {
    format!("{FOR_AGENT_URI}track-goal")
}

/// Undo: the card goes as if it never was, and the chat stays quiet until clearly more work
/// has happened. The agent hears of it with the person's next message rather than in a turn of
/// its own.
pub(crate) async fn undo(core: &Arc<Core>, agent: String) -> Result<(), String> {
    let notices = {
        let (core, agent) = (core.clone(), agent.clone());
        blocking(move || {
            let goals = &core.boards().chat_goals;
            let goal = {
                let mut chats = goals.chats.locked();
                let record = chats
                    .get_mut(&agent)
                    .ok_or("This chat has no goal to take back")?;
                let goal = (record.goal.take_if(|goal| goal.undoable))
                    .ok_or("This chat's goal can no longer be taken back")?;
                record.ended(false);
                record.note = Some(ChatContext {
                    uri: format!("{FOR_AGENT_URI}goal-{}", goal.card),
                    title: format!("Goal #{}", goal.card),
                    text: format!(
                        "The person took goal #{} off the board, so this work is not tracked as a goal. Carry on as you were, and don't add it again.",
                        goal.card
                    ),
                });
                goals.save(&mut chats);
                goal
            };
            let mut notices = super::remove_card(&core, &goal.project, goal.card)?;
            notices.retain(|notice| notice.to != agent);
            Ok(notices)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    super::send_notices(core, notices);
    push_head(core, &agent);
    Ok(())
}

fn say(chat: &Chat, title: String, marker: GoalMarker) {
    let Some(start) = chat.feed.start() else {
        return;
    };
    chat.emit(
        ChatEventKind::SessionUpdate,
        json!({
            "sessionId": start.session_id,
            "update": goal_notice(&title, marker),
        }),
    );
}

/// The line in the chat that marks `moment` of the goal, as it stands now.
fn goal_line(core: &Core, chat: &Chat, goal: &Attached, moment: GoalMoment) {
    let (steps, of_gates, state) = {
        let boards = core.boards().state.locked();
        let found = (boards.boards.get(&goal.project))
            .and_then(|board| Some((board, board.card(goal.card).ok()?)));
        let (steps, of_gates) =
            (found.map(|(board, card)| slice::goal_steps(core, board, card))).unwrap_or_default();
        let state = found.and_then(|(board, card)| goal_state(core, board, card));
        (steps, of_gates, state)
    };
    let (title, state) = match moment {
        GoalMoment::Started => ("Goal started", SliceState::Moving),
        GoalMoment::Step => ("Goal step done", SliceState::Moving),
        GoalMoment::Done => ("Goal done", SliceState::Done),
        GoalMoment::Stopped => ("Goal stopped", state.unwrap_or(SliceState::Waiting)),
    };
    let took = (moment != GoalMoment::Started && goal.began > 0)
        .then(|| epoch_ms().saturating_sub(goal.began));
    say(
        chat,
        format!("{title}: {}", goal.name),
        GoalMarker {
            card: goal.card,
            moment,
            state,
            name: goal.name.clone(),
            steps,
            of_gates,
            took,
        },
    );
}

fn steps_done(goal: &ChatGoal) -> usize {
    (goal.steps.iter())
        .filter(|step| **step == SliceState::Done)
        .count()
}

/// How the goal `goal` stands for a chat: its state, and a step for each gate or slice.
fn chat_goal(core: &Core, project: &str, board: &Board, goal: &Card) -> ChatGoal {
    let (steps, of_gates) = slice::goal_steps(core, board, goal);
    ChatGoal {
        project: project.to_owned(),
        card: goal.id,
        name: super::display_title(core, board, goal).unwrap_or_else(|| goal.title.clone()),
        state: goal_state(core, board, goal).unwrap_or(SliceState::Moving),
        steps,
        of_gates,
        undoable: false,
    }
}

/// How a goal stands: as its slices do together, or for one with none, as its owner does.
pub(super) fn goal_state(core: &Core, board: &Board, goal: &Card) -> Option<SliceState> {
    slice::goal_state(core, board, goal).or_else(|| plain_state(core, board, goal))
}

/// A goal with an agent on it and no slice takes its state from that agent: moving only while it
/// works, needing the person once it closed or waits on them, as the lease skips such a card.
fn plain_state(core: &Core, board: &Board, goal: &Card) -> Option<SliceState> {
    let owner = goal.owner_id().filter(|_| goal.parent.is_none())?;
    Some(match goal.status {
        CardStatus::Done => SliceState::Done,
        _ if waits(core, board, goal).is_some() => SliceState::NeedsYou,
        _ if core.agent_now(owner).at_work() => SliceState::Moving,
        _ => SliceState::Waiting,
    })
}

/// Whether nothing moves the slice `card` on for now: its owner is not at work and no check it
/// asked for is coming. Auto's cards move by Auto, whatever their agent does between its steps.
pub(super) fn owner_idle(core: &Core, board: &Board, card: &Card) -> bool {
    let Some(owner) = card.owner_id().filter(|_| card.auto.is_none()) else {
        return false;
    };
    let checking = |wait: Wait| matches!(wait.on, WaitOn::Check { .. });
    !core.agent_now(owner).at_work()
        && !super::wait::of(core, board, card, epoch_ms()).is_some_and(checking)
}

/// The goal each open chat works on now: the goal above the card it holds, or the slice it lost
/// and that nobody took since.
fn goals_held<'a>(
    core: &Core,
    agents: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, ChatGoal> {
    let state = core.boards().state.locked();
    let mut found = HashMap::new();
    for agent in agents {
        let Some(project) = core.names().get(agent).map(|named| named.project) else {
            continue;
        };
        let Some(board) = state.boards.get(&project) else {
            continue;
        };
        let held = (board.cards.iter())
            .filter(|card| card.status != CardStatus::Done)
            .filter(|card| {
                card.owner_id() == Some(agent)
                    || (card.owner.is_none()
                        && (card.slice.as_ref())
                            .and_then(|slice| slice.stalled.as_ref())
                            .is_some_and(|stalled| stalled.agent == agent))
            })
            .max_by_key(|card| card.work_rank());
        let Some(goal) = held.map(|card| board.goal_above(card)) else {
            continue;
        };
        let shown = chat_goal(core, &project, board, goal);
        if goal.status != CardStatus::Done && shown.state != SliceState::Done {
            found.insert(agent.to_owned(), shown);
        }
    }
    found
}

/// Whether the goal the chat worked on finished, rather than being taken away from it.
fn finished(core: &Core, goal: &Attached) -> bool {
    let state = core.boards().state.locked();
    (state.boards.get(&goal.project))
        .and_then(|board| Some((board, board.card(goal.card).ok()?)))
        .is_some_and(|(board, card)| {
            card.status == CardStatus::Done
                || goal_state(core, board, card) == Some(SliceState::Done)
        })
}

/// Tells each chat of its goal and marks where goals began and ended. Runs when a board or the
/// agents change, so a goal's strip clears, and the line stays, as soon as it finishes.
pub(super) fn settle(core: &Arc<Core>) {
    tell_owner_changes(core);
    let open = core.chats().all();
    let resting = core.chats().resting_feeds();
    let held = goals_held(
        core,
        (open.iter().map(|chat| chat.agent_id())).chain(resting.iter().map(|(a, _)| a.as_str())),
    );
    let added_itself = added_itself(core, &held);
    let goals = &core.boards().chat_goals;
    let mut lines: Vec<(Arc<Chat>, Attached, GoalMoment)> = Vec::new();
    let mut earlier_work = Vec::new();
    {
        let mut chats = goals.chats.locked();
        let mut changed = false;
        for chat in &open {
            let agent = chat.agent_id();
            let now = held.get(agent);
            if now.is_none() && !chats.contains_key(agent) {
                continue;
            }
            let record = chats.entry(agent.to_owned()).or_default();
            let was = record.goal.clone();
            match (&was, now) {
                (Some(was), Some(now)) if was.card == now.card => {
                    let held = record.goal.as_mut().expect("just read");
                    changed |= was.name != now.name;
                    held.name.clone_from(&now.name);
                    // Undo has passed, by the lease lapsing, so its line can show now.
                    if held.unshown && !held.undoable {
                        held.unshown = false;
                        changed = true;
                        lines.push((chat.clone(), held.clone(), GoalMoment::Started));
                    }
                    let done_now = steps_done(now);
                    if done_now != held.steps_done {
                        if done_now > held.steps_done && !held.unshown {
                            lines.push((chat.clone(), held.clone(), GoalMoment::Step));
                        }
                        held.steps_done = done_now;
                        changed = true;
                    }
                    continue;
                }
                (None, None) => continue,
                _ => {}
            }
            changed = true;
            let done = was.as_ref().is_some_and(|was| finished(core, was));
            if was.is_some() {
                record.ended(done);
            }
            let undoable = added_itself.contains(agent);
            let started = now.map(|now| Attached {
                project: now.project.clone(),
                card: now.card,
                name: now.name.clone(),
                began: epoch_ms(),
                steps_done: steps_done(now),
                undoable,
                unshown: undoable,
            });
            record.goal.clone_from(&started);
            if let Some(started) = started.as_ref().filter(|started| started.undoable) {
                let commits = record.tally.since(record.covered).commits as usize;
                earlier_work.push((
                    agent.to_owned(),
                    started.project.clone(),
                    started.card,
                    commits,
                ));
            }
            if let Some(was) = was {
                match (done, was.unshown) {
                    // A goal still waiting out its Undo has shown no line yet, so its end shows
                    // both.
                    (true, true) => {
                        lines.push((chat.clone(), was.clone(), GoalMoment::Started));
                        lines.push((chat.clone(), was, GoalMoment::Done));
                    }
                    (true, false) => lines.push((chat.clone(), was, GoalMoment::Done)),
                    (false, true) => {}
                    (false, false) => lines.push((chat.clone(), was, GoalMoment::Stopped)),
                }
            }
            if let Some(started) = started.filter(|started| !started.unshown) {
                lines.push((chat.clone(), started, GoalMoment::Started));
            }
        }
        if changed {
            goals.save(&mut chats);
        }
    }
    if !earlier_work.is_empty() {
        let core = core.clone();
        tokio::task::spawn_blocking(move || {
            for (agent, project, card, commits) in earlier_work {
                take_earlier_work(&core, &agent, &project, card, commits);
            }
        });
    }
    for (chat, goal, moment) in lines {
        goal_line(core, &chat, &goal, moment);
    }
    for chat in &open {
        apply_head(
            core,
            chat.agent_id(),
            &chat.feed,
            held.get(chat.agent_id()).cloned(),
        );
    }
    for (agent, feed) in &resting {
        apply_head(core, agent, feed, held.get(agent).cloned());
    }
}

/// The chats whose own agent added the goal it now holds, which the person can take back while
/// it is new. Agents another agent started answer to their lead instead.
fn added_itself(core: &Core, held: &HashMap<String, ChatGoal>) -> HashSet<String> {
    let state = core.boards().state.locked();
    (held.iter())
        .filter(|(agent, goal)| {
            (state.boards.get(&goal.project))
                .and_then(|board| board.card(goal.card).ok())
                .is_some_and(|card| card.maker_id() == Some(agent.as_str()))
        })
        .map(|(agent, _)| agent.clone())
        .filter(|agent| counted(core, agent))
        .collect()
}

/// What the agent committed before it added the goal is the slice's work, so it can pass Built
/// without a new commit.
fn take_earlier_work(core: &Core, agent: &str, project: &str, card: u64, commits: usize) {
    if let Some(head) = slice::earlier_work(core, project, agent, commits) {
        core.boards()
            .edit(project, card, |held| slice::took_work(held, &head));
    }
}

/// Clients read a board again when a card's state changed with its owner, which changes nothing
/// on the card itself.
fn tell_owner_changes(core: &Core) {
    let goals = &core.boards().chat_goals;
    let mut state = core.boards().state.locked();
    let mut now = HashMap::new();
    let mut changed = Vec::new();
    {
        let seen = goals.owned.locked();
        for board in state.boards.values() {
            let owned = (board.cards.iter())
                .filter(|card| card.owner.is_some() && card.status != CardStatus::Done);
            for card in owned {
                let stands =
                    slice::state_of(core, board, card).or_else(|| plain_state(core, board, card));
                if let Some(stands) = stands {
                    let key = (board.project.clone(), card.id);
                    if seen.get(&key) != Some(&stands) {
                        changed.push(board.project.clone());
                    }
                    now.insert(key, stands);
                }
            }
        }
    }
    *goals.owned.locked() = now;
    state.dirty.extend(changed);
}

fn apply_head(core: &Core, agent: &str, feed: &Feed, goal: Option<ChatGoal>) {
    let chats = core.boards().chat_goals.chats.locked();
    let record = chats.get(agent);
    let goal = goal.map(|goal| ChatGoal {
        undoable: record
            .and_then(|record| record.goal.as_ref())
            .is_some_and(|held| held.card == goal.card && held.undoable),
        ..goal
    });
    drop(chats);
    feed.set_goal(goal);
}

fn push_head(core: &Core, agent: &str) {
    let Some(chat) = core.chats().get(agent) else {
        return;
    };
    let goal = goals_held(core, [agent]).remove(agent);
    apply_head(core, agent, &chat.feed, goal);
}

/// Whether the card's owner closed, for a card with no slice, which keeps its owner since a card
/// with nobody on it and no slice is no goal.
pub(super) fn plain_owner_closed(core: &Core, card: &Card) -> bool {
    card.slice.is_none()
        && card.parent.is_none()
        && card.status != CardStatus::Done
        && card.owner.is_some()
        && holder(core, card).is_none()
}

/// The owner a goal with no slice lost by closing, as a slice's lease would have kept it.
pub(super) fn plain_stalled(core: &Core, card: &Card) -> Option<lease::Stalled> {
    plain_owner_closed(core, card).then(|| lease::Stalled {
        agent: card.owner_id().unwrap_or_default().to_owned(),
        name: card.owner_name(core).unwrap_or_default(),
        why: lease::Why::Closed,
        stays: true,
    })
}

impl ChatGoals {
    pub(super) fn flush(&self) {
        self.kept.flush();
    }
}
