//! A chat holding a goal. A goal stays a unit of work a chat takes on, never the chat itself:
//! one chat can hold several over its life, and finishing one leaves the chat going.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
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
use sikemux_view::slice::SliceState;

use super::{holder, state_of, waits_on_person, Board, Card, CardStatus};
use crate::server::chat::feed::Feed;
use crate::server::chat::Chat;
use crate::server::connection::blocking;
use crate::server::kept::Kept;
use crate::server::Core;
use sikemux_fold::update::goal_notice;
use sikemux_fold::ChatEventKind;
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

/// Chats kept, past which those touched longest ago go.
const MAX_CHATS: usize = 512;
const FILE: &str = "chat-goals.json";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    goal: Option<Attached>,
    /// What the agent is told with the person's next message, so it starts no turn of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<ChatContext>,
    #[serde(default)]
    at: u64,
}

pub struct ChatGoals {
    kept: Kept,
    chats: Mutex<BTreeMap<String, Record>>,
    /// How each card with an owner last stood, to tell clients when that changes.
    owned: Mutex<HashMap<(String, u64), SliceState>>,
}

impl ChatGoals {
    pub fn new(data_dir: Option<&Path>) -> Self {
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

/// A turn began, which ends the moment a goal just tracked could be taken back.
pub fn turn_started(core: &Core, agent: &str) {
    if !counted(core, agent) {
        return;
    }
    let goals = &core.boards().chat_goals;
    let settled = {
        let mut chats = goals.chats.locked();
        let record = chats.entry(agent.to_owned()).or_default();
        record.at = epoch_ms();
        let settled = (record.goal.as_mut())
            .filter(|goal| goal.unshown)
            .map(|goal| {
                goal.undoable = false;
                goal.unshown = false;
                goal.clone()
            });
        if settled.is_some() {
            goals.save(&mut chats);
        }
        settled
    };
    if let (Some(goal), Some(chat)) = (settled, core.chats().get(agent)) {
        goal_line(core, &chat, &goal, GoalMoment::Started);
        push_head(core, agent);
    }
}

/// What the agent is told with the person's next message, once.
pub fn take_note(core: &Core, agent: &str) -> Option<ChatContext> {
    let goals = &core.boards().chat_goals;
    let mut chats = goals.chats.locked();
    let note = chats.get_mut(agent)?.note.take()?;
    goals.save(&mut chats);
    Some(note)
}

/// Undo: the card goes as if it never was. The agent hears of it with the person's next message
/// rather than in a turn of its own.
pub async fn undo(core: &Arc<Core>, agent: String) -> Result<(), String> {
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
            let removed = (core.board_above().records_act)(
                &core,
                &goal.project,
                "boardRemove",
                json!({ "id": goal.card }),
            );
            let mut notices = match removed {
                Some(removed) => removed.map(|_| Vec::new())?,
                None => super::remove_card(&core, &goal.project, goal.card)?,
            };
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
        let found =
            (boards.of(&goal.project)).and_then(|board| Some((board, board.card(goal.card).ok()?)));
        let steps = (found.map(|(board, card)| goal_steps(core, board, card))).unwrap_or_default();
        let state = found.and_then(|(board, card)| goal_state(core, board, card));
        (steps, false, state)
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

/// How each card right under the goal stands, in order.
fn goal_steps(core: &Core, board: &Board, goal: &Card) -> Vec<SliceState> {
    (board.under(goal.id).into_iter())
        .map(|card| state_of(core, card))
        .collect()
}

/// How the goal `goal` stands for a chat: its state, and a step for each card under it.
fn chat_goal(core: &Core, project: &str, board: &Board, goal: &Card) -> ChatGoal {
    ChatGoal {
        project: project.to_owned(),
        card: goal.id,
        name: super::display_title(core, board, goal).unwrap_or_else(|| goal.title.clone()),
        state: goal_state(core, board, goal).unwrap_or(SliceState::Moving),
        steps: goal_steps(core, board, goal),
        of_gates: false,
        undoable: false,
    }
}

/// How a goal stands: done once done, needing the person while its agent waits on them, and
/// moving while an agent on it or on a card under it works.
pub fn goal_state(core: &Core, board: &Board, goal: &Card) -> Option<SliceState> {
    if !board.is_goal(goal) {
        return None;
    }
    let working = std::iter::once(goal)
        .chain(board.under(goal.id))
        .filter(|card| card.status != CardStatus::Done)
        .filter_map(Card::owner_id)
        .any(|owner| core.agent_now(owner).at_work());
    Some(match goal.status {
        CardStatus::Done => SliceState::Done,
        _ if waits_on_person(core, goal) => SliceState::NeedsYou,
        _ if working => SliceState::Moving,
        _ => SliceState::Waiting,
    })
}

/// The goal each open chat works on now: the goal above the card it holds.
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
        let Some(board) = state.of(&project) else {
            continue;
        };
        let held = (board.cards.iter())
            .filter(|card| card.status != CardStatus::Done)
            .filter(|card| card.owner_id() == Some(agent))
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
    (state.of(&goal.project))
        .and_then(|board| Some((board, board.card(goal.card).ok()?)))
        .is_some_and(|(board, card)| {
            card.status == CardStatus::Done
                || goal_state(core, board, card) == Some(SliceState::Done)
        })
}

/// Tells each chat of its goal and marks where goals began and ended. Runs when a board or the
/// agents change, so a goal's strip clears, and the line stays, as soon as it finishes.
pub fn settle(core: &Arc<Core>) {
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
            (state.of(&goal.project))
                .and_then(|board| board.card(goal.card).ok())
                .is_some_and(|card| card.maker_id() == Some(agent.as_str()))
        })
        .map(|(agent, _)| agent.clone())
        .filter(|agent| counted(core, agent))
        .collect()
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
                let stands = state_of(core, card);
                let key = (board.folder.clone(), card.id);
                if seen.get(&key) != Some(&stands) {
                    changed.push(board.folder.clone());
                }
                now.insert(key, stands);
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

/// Whether the goal's owner closed while the goal was not done.
pub fn plain_owner_closed(core: &Core, card: &Card) -> bool {
    card.parent.is_none()
        && card.status != CardStatus::Done
        && card.owner.is_some()
        && holder(core, card).is_none()
}

impl ChatGoals {
    pub fn flush(&self) {
        self.kept.flush();
    }
}
