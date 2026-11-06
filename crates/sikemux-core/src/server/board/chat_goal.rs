//! A chat picking up a goal. A goal stays a unit of work a chat takes on, never the chat itself:
//! one chat can hold several over its life, and finishing one leaves the chat going. The core
//! counts what a chat does and suggests tracking it; only the person turns that into a card.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{ChatGoal, GoalMarker};
use sikemux_view::slice::{SliceState, Wait, WaitOn};

use super::{
    add_card, holder, lease, slice, title_of, waits, Adding, Board, By, Card, CardStatus,
    MAX_TITLE_CHARS,
};
use crate::server::chat::feed::Feed;
use crate::server::chat::Chat;
use crate::server::connection::blocking;
use crate::server::goal_summary;
use crate::server::kept::Kept;
use crate::server::Core;
use crate::sessions::asked::may_hold_secret;
use crate::sessions::transcript::Said;
use crate::text::clipped;
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

/// How much a chat must have done for a suggestion. Points alone are not enough, as a few
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
/// After Not now or Undo: twice as much again, counted from that moment.
const AFTER_NOT_NOW: Bar = Bar {
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

    /// Why this looks like real work, as the person reads it.
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
    /// The person only just tracked it and can still take that back.
    undoable: bool,
    /// Its line in the chat waits until that has passed, so the chat never shows a goal that was
    /// taken back.
    unshown: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    tally: Tally,
    /// What an earlier answer or goal already covered, so only what came after counts.
    #[serde(default)]
    covered: Tally,
    /// The person said Not now or Undo since, which raises the bar.
    #[serde(default)]
    declined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    goal: Option<Attached>,
    /// What the agent is told with its next message, so tracking starts no turn of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<ChatContext>,
    #[serde(skip)]
    began: Option<u64>,
    #[serde(default)]
    at: u64,
}

impl Record {
    fn answered(&mut self, declined: bool) {
        self.covered = self.tally;
        self.declined = declined;
        self.offer = None;
    }

    fn weigh(&mut self) {
        let bar = if self.declined {
            &AFTER_NOT_NOW
        } else {
            &FIRST_BAR
        };
        let done = self.tally.since(self.covered);
        self.offer = (self.goal.is_none() && done.clears(bar)).then(|| done.reason());
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
    core.chats.get(agent).is_some()
        && (core.names.get(agent)).is_some_and(|named| named.parent.is_none())
}

fn count(core: &Core, agent: &str, add: impl FnOnce(&mut Tally)) {
    if !counted(core, agent) {
        return;
    }
    let goals = &core.boards.chat_goals;
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
    let goals = &core.boards.chat_goals;
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
    if let (Some(goal), Some(chat)) = (settled, core.chats.get(agent)) {
        started_line(&chat, &goal);
        push_head(core, agent);
    }
}

/// A turn ended: it counts towards the chat's work, and one that ran its course can bring the
/// suggestion, since that is when the person reads what the chat did.
pub(crate) fn turn_ended(core: &Core, agent: &str, ran_its_course: bool) {
    let goals = &core.boards.chat_goals;
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
    drop(chats);
    push_head(core, agent);
}

/// The person's answer to a suggestion: quiet until clearly more work has happened.
pub(crate) fn not_now(core: &Core, agent: &str) {
    let goals = &core.boards.chat_goals;
    let mut chats = goals.chats.locked();
    if let Some(record) = chats.get_mut(agent) {
        record.answered(true);
        goals.save(&mut chats);
    }
    drop(chats);
    push_head(core, agent);
}

/// The agent lost its slice to the lease, which ends the moment it could be taken back.
pub(super) fn lease_lapsed(core: &Core, agent: &str) {
    let goals = &core.boards.chat_goals;
    let mut chats = goals.chats.locked();
    let Some(goal) = chats.get_mut(agent).and_then(|record| record.goal.as_mut()) else {
        return;
    };
    if goal.undoable {
        goal.undoable = false;
        goals.save(&mut chats);
    }
}

/// What the agent is told with the person's next message, once.
pub(crate) fn take_note(core: &Core, agent: &str) -> Option<ChatContext> {
    let goals = &core.boards.chat_goals;
    let mut chats = goals.chats.locked();
    let note = chats.get_mut(agent)?.note.take()?;
    goals.save(&mut chats);
    Some(note)
}

/// Whether a chat's title can name a goal: short enough for a card, and not something pasted or
/// that may hold a secret.
fn usable_title(title: &str) -> bool {
    !title.contains("```") && title.chars().count() <= MAX_TITLE_CHARS && !may_hold_secret(title)
}

/// The first draft of a goal, there at once: the title the chat goes by, or else what the person
/// last asked, which is not always what the agent was last sent, as a teammate's report comes in
/// too.
fn first_guess(core: &Core, chat: &Chat) -> (String, String, String) {
    const READ_TURNS: usize = 6;
    const READ_BYTES: usize = 64 * 1024;
    let said = chat.feed.excerpt(None, READ_TURNS, READ_BYTES).said;
    let asked = (said.iter().rev())
        .find_map(|(_, _, said)| match said {
            Said::Asked(text) => goal_summary::person_said(text),
            _ => None,
        })
        .unwrap_or_default();
    let shown = (core.workspaces.title_given(chat.agent_id())).filter(|title| usable_title(title));
    let title = match shown {
        Some(title) => title,
        None if asked.is_empty() => "Work in this chat".to_owned(),
        None => clipped(&asked, MAX_TITLE_CHARS),
    };
    let promise = match asked.as_str() {
        "" => title.clone(),
        line => clipped(line, slice::MAX_PROMISE_CHARS),
    };
    (title, promise, asked)
}

/// Track as goal: a card the chat's agent holds, a slice from the first so its gates, Failed and
/// strip all work. A model's better draft replaces the first one when it arrives.
pub(crate) async fn track(core: &Arc<Core>, agent: String) -> Result<(), String> {
    let (project, card, guess) = {
        let (core, agent) = (core.clone(), agent.clone());
        blocking(move || Ok(track_now(&core, &agent)?))
            .await
            .map_err(|error| error.to_string())?
    };
    push_head(core, &agent);
    let core = core.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(draft) = goal_summary::draft_goal(&core, &agent) {
            redraft(&core, &project, card, &guess, draft);
        }
    });
    Ok(())
}

fn track_now(core: &Arc<Core>, agent: &str) -> Result<(String, u64, String), String> {
    let chat = core.chats.get(agent).ok_or("That chat isn't open")?;
    let project = (core.names.get(agent))
        .ok_or("That chat has no project yet")?
        .project;
    let (title, promise, asked) = first_guess(core, &chat);
    let goals = &core.boards.chat_goals;
    // Held across adding the card, so a settle that runs at once finds the chat already attached.
    let mut chats = goals.chats.locked();
    let record = chats.entry(agent.to_owned()).or_default();
    if record.goal.is_some() {
        return Err("This chat already has a goal".into());
    }
    let adding = Adding {
        title: title_of(&title)?,
        details: super::long_text("details", &asked)?,
        promise: Some(slice::promise_of(&promise)?),
        ..super::Adding::default()
    };
    let commits = record.tally.since(record.covered).commits as usize;
    let (card, _) = add_card(core, &project, &By::Person, adding, Some(agent))?;
    record.goal = Some(Attached {
        project: project.clone(),
        card,
        name: title.clone(),
        undoable: true,
        unshown: true,
    });
    record.offer = None;
    record.at = epoch_ms();
    record.note = Some(ChatContext {
        uri: format!("{FOR_AGENT_URI}goal-{card}"),
        title: format!("Goal #{card}"),
        text: format!(
            "The person is now tracking this work as goal #{card}, and you hold its card. Carry on as you were. When the work is done, mark the card done with board_update and a result; board_list with card {card} shows where it stands."
        ),
    });
    goals.save(&mut chats);
    drop(chats);
    // What it committed before now is the slice's work, so it can pass Built without a new commit.
    if let Some(head) = slice::earlier_work(core, &project, agent, commits) {
        core.boards
            .edit(&project, card, |held| slice::took_work(held, &head));
    }
    Ok((project, card, title))
}

/// Takes the model's draft, unless someone already changed the card.
fn redraft(core: &Core, project: &str, card: u64, guess: &str, draft: goal_summary::GoalDraft) {
    let (Ok(title), Ok(promise)) = (title_of(&draft.title), slice::promise_of(&draft.promise))
    else {
        return;
    };
    core.boards.edit_if(project, card, |held| {
        if held.title != guess {
            return None;
        }
        held.title = title;
        if let Some(slice) = &mut held.slice {
            slice.promise = promise;
        }
        Some(())
    });
}

/// Undo: the card goes as if it never was, and the chat stays quiet until clearly more work
/// has happened. The agent was never told of it, so it isn't told it went.
pub(crate) async fn undo(core: &Arc<Core>, agent: String) -> Result<(), String> {
    let notices = {
        let (core, agent) = (core.clone(), agent.clone());
        blocking(move || {
            let goals = &core.boards.chat_goals;
            let goal = {
                let mut chats = goals.chats.locked();
                let record = chats
                    .get_mut(&agent)
                    .ok_or("This chat has no goal to take back")?;
                let goal = (record.goal.take_if(|goal| goal.undoable))
                    .ok_or("This chat's goal can no longer be taken back")?;
                record.note = None;
                record.answered(true);
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

fn say(chat: &Chat, title: String, card: u64, state: SliceState) {
    let Some(start) = chat.feed.start() else {
        return;
    };
    chat.emit(
        ChatEventKind::SessionUpdate,
        json!({
            "sessionId": start.session_id,
            "update": goal_notice(&title, GoalMarker { card, state }),
        }),
    );
}

fn started_line(chat: &Chat, goal: &Attached) {
    say(
        chat,
        format!("Goal started: {}", goal.name),
        goal.card,
        SliceState::Moving,
    );
}

fn done_line(chat: &Chat, goal: &Attached) {
    say(
        chat,
        format!("Goal done: {}", goal.name),
        goal.card,
        SliceState::Done,
    );
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
        _ if waits(core, &board.settings, goal).is_some() => SliceState::NeedsYou,
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
    let state = core.boards.state.locked();
    let mut found = HashMap::new();
    for agent in agents {
        let Some(project) = core.names.get(agent).map(|named| named.project) else {
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
    let state = core.boards.state.locked();
    (state.boards.get(&goal.project))
        .and_then(|board| Some((board, board.card(goal.card).ok()?)))
        .is_some_and(|(board, card)| {
            card.status == CardStatus::Done
                || goal_state(core, board, card) == Some(SliceState::Done)
        })
}

/// Tells each chat of its goal and marks where goals began and ended. Runs when a board or the
/// agents change, so a goal's strip clears, and the line stays, as soon as it finishes.
pub(super) fn settle(core: &Core) {
    tell_owner_changes(core);
    let open = core.chats.all();
    let resting = core.chats.resting_feeds();
    let held = goals_held(
        core,
        (open.iter().map(|chat| chat.agent_id())).chain(resting.iter().map(|(a, _)| a.as_str())),
    );
    let goals = &core.boards.chat_goals;
    let mut lines: Vec<(Arc<Chat>, Option<Attached>, Option<Attached>)> = Vec::new();
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
                        lines.push((chat.clone(), Some(held.clone()), None));
                    }
                    continue;
                }
                (None, None) => continue,
                _ => {}
            }
            changed = true;
            let done = was.as_ref().filter(|was| finished(core, was)).cloned();
            if was.is_some() {
                record.answered(done.is_none());
            }
            let started = now.map(|now| Attached {
                project: now.project.clone(),
                card: now.card,
                name: now.name.clone(),
                undoable: false,
                unshown: false,
            });
            record.goal.clone_from(&started);
            record.offer = record.offer.take().filter(|_| started.is_none());
            // A goal still waiting out its Undo has shown no line yet, so its end shows both.
            let unshown = was.filter(|was| was.unshown && done.is_some());
            if let Some(unshown) = unshown {
                lines.push((chat.clone(), Some(unshown), None));
            }
            lines.push((chat.clone(), started, done));
        }
        if changed {
            goals.save(&mut chats);
        }
    }
    for (chat, started, done) in lines {
        if let Some(done) = &done {
            done_line(&chat, done);
        }
        if let Some(started) = &started {
            started_line(&chat, started);
        }
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

/// Clients read a board again when a card's state changed with its owner, which changes nothing
/// on the card itself.
fn tell_owner_changes(core: &Core) {
    let goals = &core.boards.chat_goals;
    let mut state = core.boards.state.locked();
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
    let chats = core.boards.chat_goals.chats.locked();
    let record = chats.get(agent);
    let goal = goal.map(|goal| ChatGoal {
        undoable: record
            .and_then(|record| record.goal.as_ref())
            .is_some_and(|held| held.card == goal.card && held.undoable),
        ..goal
    });
    let offer = record.and_then(|record| record.offer.clone());
    drop(chats);
    feed.set_goal(goal, offer);
}

fn push_head(core: &Core, agent: &str) {
    let Some(chat) = core.chats.get(agent) else {
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
    })
}

/// The chat a goal's button acts on.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGoalParams {
    pub agent_id: String,
}

impl ChatGoals {
    pub(super) fn flush(&self) {
        self.kept.flush();
    }
}
