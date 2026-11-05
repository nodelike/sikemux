//! Every goal and every agent on one screen: which agents work each goal, what is on their
//! desks, what comes next and by when. The window owns the browser and the editors, so it tells
//! the core what its desks hold.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use sikemux_process::sync::Locked;
use sikemux_view::goals::{
    AgentDesk, DeskCounts, DesksParams, GoalAgent, GoalClock, GoalNext, GoalRow, GoalSlice,
    GoalsView, LooseNeed,
};
use sikemux_view::inbox::{Inbox, InboxItem, InboxKind, InboxNeed};
use sikemux_view::slice::{Passer, SliceState, WaitOn};
use sikemux_wire::protocol::SessionId;

use super::board::{self, BoardView, CardView, Column};
use super::changed::Topic;
use super::connection::ClientId;
use super::{inbox, Core};

/// How many finished goals are listed, newest first.
const MAX_DONE: usize = 100;
/// How many things of one kind a desk tells the core of.
const MAX_ON_DESK: usize = 64;

#[derive(Default)]
pub(crate) struct Goals {
    /// Every desk the window holds, and which client that window is.
    desks: Mutex<Option<(ClientId, BTreeMap<String, AgentDesk>)>>,
    /// What the view last showed of its agents and needs.
    seen: Mutex<u64>,
}

/// Takes what the window's desks hold now. They close with the window, so they count only while
/// `client` stays connected.
pub(super) fn set_desks(core: &Core, client: ClientId, params: DesksParams) {
    let mut desks = params.desks;
    for desk in desks.values_mut() {
        desk.pages.truncate(MAX_ON_DESK);
        desk.files.truncate(MAX_ON_DESK);
        desk.terminals.truncate(MAX_ON_DESK);
    }
    desks.retain(|_, desk| *desk != AgentDesk::default());
    let held = Some((client, desks));
    let mut kept = core.goals.desks.locked();
    if *kept != held {
        *kept = held;
        drop(kept);
        core.changes.changed(Topic::Goals, None, None);
    }
}

/// Tells clients to read the goals again once the device view shows their agents or needs moved.
/// Hashing what they show costs far less than building the goals on every look at the view.
pub(super) fn view_moved(core: &Core, inbox: &Inbox) {
    let mut hasher = DefaultHasher::new();
    for agent in &inbox.agents {
        (&agent.agent_id, &agent.title, agent.status, agent.group).hash(&mut hasher);
        let face = agent.face.as_ref().map(|face| (&face.pack, &face.shape));
        (face, &agent.project).hash(&mut hasher);
    }
    for item in inbox.items.iter().filter(|item| item.need.is_some()) {
        (&item.id, &item.what, item.at).hash(&mut hasher);
    }
    if let Some((_, desks)) = &*core.goals.desks.locked() {
        for terminal in desks.values().flat_map(|desk| &desk.terminals) {
            (terminal.session, session_runs(core, terminal.session)).hash(&mut hasher);
        }
    }
    let moved = hasher.finish();
    let mut seen = core.goals.seen.locked();
    if *seen != moved {
        *seen = moved;
        drop(seen);
        core.changes.changed(Topic::Goals, None, None);
    }
}

pub(super) fn view(core: &Core) -> GoalsView {
    let (chats, sessions) = core.ranked_infos();
    let inbox = inbox::view(core, &chats, &sessions);
    let boards = board::boards(core);
    let desks = held_desks(core);
    let helpers = helpers_of(core);
    let open: HashSet<&str> = (inbox.agents.iter())
        .map(|agent| agent.agent_id.as_str())
        .collect();

    let mut rows = Vec::new();
    let mut tasks: HashMap<String, String> = HashMap::new();
    for board in &boards {
        for goal in board.cards.iter().filter(|card| card.is_goal) {
            let (agents, owned) = agents_of(board, goal, &helpers, &open);
            for (agent, slice) in owned {
                tasks.entry(agent).or_insert(slice);
            }
            let mut desk = DeskCounts::default();
            for agent in agents.iter().filter_map(|agent| desks.get(agent)) {
                desk.add(agent);
            }
            rows.push(Placed {
                row: GoalRow {
                    project: board.project.clone(),
                    card: goal.id,
                    title: goal
                        .display_title
                        .clone()
                        .unwrap_or_else(|| goal.title.clone()),
                    state: state_of(goal),
                    next: None,
                    clock: None,
                    slices: slices_of(board, goal),
                    agents,
                    needs: Vec::new(),
                    desk,
                    worker: goal.owner.clone().filter(|_| {
                        !goal.goal_stalled && (goal.slice.is_some() || goal.goal_slices.is_empty())
                    }),
                },
                goal,
                board,
            });
        }
    }

    let needing: Vec<&InboxItem> = (inbox.items.iter())
        .filter(|item| item.need.is_some())
        .collect();
    let loose_needs = place_needs(&mut rows, &needing);
    for placed in &mut rows {
        let first = (placed.row.needs.first())
            .and_then(|id| needing.iter().find(|item| item.id == *id).copied());
        placed.settle(first);
    }
    let order = |placed: &Placed| {
        let first = (placed.row.needs.first())
            .and_then(|id| needing.iter().position(|item| item.id == *id))
            .unwrap_or(usize::MAX);
        match placed.row.state {
            SliceState::NeedsYou => (0, first as u64),
            SliceState::Failed => (1, first as u64),
            SliceState::Moving => (2, u64::MAX - placed.goal.at),
            SliceState::Waiting => (3, u64::MAX - placed.goal.at),
            SliceState::Done => (4, u64::MAX - placed.goal.updated_at),
        }
    };
    rows.sort_by_key(order);
    let done = (rows.iter())
        .filter(|placed| placed.row.state == SliceState::Done)
        .count();
    let done_earlier = done.saturating_sub(MAX_DONE);
    rows.truncate(rows.len() - done_earlier);
    let rows: Vec<GoalRow> = rows.into_iter().map(|placed| placed.row).collect();

    let working: HashSet<&str> = (rows.iter())
        .filter(|row| row.state != SliceState::Done)
        .flat_map(|row| row.agents.iter().map(String::as_str))
        .collect();
    let no_goal = (inbox.agents.iter())
        .filter(|agent| !working.contains(agent.agent_id.as_str()))
        .map(|agent| agent.agent_id.clone())
        .collect();
    let names = core.workspaces.view();
    let names: HashMap<&str, Option<String>> = (names.agents.iter())
        .map(|agent| (agent.id.as_str(), core.names.name(&agent.id)))
        .collect();
    let agents = (inbox.agents.iter())
        .map(|agent| GoalAgent {
            id: agent.agent_id.clone(),
            name: names.get(agent.agent_id.as_str()).cloned().flatten(),
            cli: agent.cli.clone(),
            face: agent.face.clone(),
            status: agent.status,
            group: agent.group,
            task: (tasks.get(&agent.agent_id).cloned()).unwrap_or_else(|| agent.title.clone()),
            project: agent.project.clone(),
            desk: desks.get(&agent.agent_id).cloned().unwrap_or_default(),
        })
        .collect();
    GoalsView {
        goals: rows,
        done_earlier,
        agents,
        no_goal,
        loose_needs,
    }
}

/// A goal's row while the view is put together, with what it was made from.
struct Placed<'a> {
    row: GoalRow,
    goal: &'a CardView,
    board: &'a BoardView,
}

impl Placed<'_> {
    /// Says how it stands once its needs are known, what comes next and by when.
    fn settle(&mut self, first: Option<&InboxItem>) {
        let (goal, row) = (self.goal, &mut self.row);
        let need = first.and_then(|item| Some((item, item.need?)));
        row.state = match need {
            _ if row.state == SliceState::Failed => SliceState::Failed,
            Some((_, InboxNeed::Fix)) => SliceState::Failed,
            Some(_) => SliceState::NeedsYou,
            None if goal.goal_stalled => SliceState::NeedsYou,
            None => row.state,
        };
        let failed = (row.slices.iter()).find(|slice| slice.state == SliceState::Failed);
        let gate = goal.next.first();
        row.next = match (need, failed) {
            (Some((item, need)), _) => Some(GoalNext::Need {
                need,
                what: asks_on(item, &row.title),
            }),
            (None, Some(failed)) => Some(GoalNext::Failed {
                slice: failed.title.clone(),
            }),
            (None, None) if row.state == SliceState::Done => Some(GoalNext::Done),
            (None, None) => gate
                .and_then(|next| {
                    let card = self.board.cards.iter().find(|card| card.id == next.card)?;
                    // Whose gate it is: it says so of its owner, naming the slice only when the
                    // goal holds others, and an idle owner is all that holds it up.
                    let owners = next.passer == Passer::Owner;
                    let idle = owners && row.state == SliceState::Waiting && next.wait.is_none();
                    let at_work = owners
                        && card.title == row.title
                        && matches!(
                            next.wait.as_ref().map(|wait| &wait.on),
                            Some(WaitOn::Agent { .. })
                        );
                    Some(match (&card.owner_name, idle || at_work) {
                        (Some(agent), true) => GoalNext::Agent {
                            agent: agent.clone(),
                            idle,
                        },
                        (owner, _) => GoalNext::Gate {
                            gate: next.clone(),
                            slice: card.title.clone(),
                            owner: owner.clone(),
                        },
                    })
                })
                .or_else(|| {
                    let agent = goal.owner_name.clone()?;
                    let idle = row.state == SliceState::Waiting;
                    let shown = goal.goal_slices.is_empty()
                        && matches!(row.state, SliceState::Moving | SliceState::Waiting);
                    shown.then_some(GoalNext::Agent { agent, idle })
                }),
        };
        let eta = goal.eta.as_ref();
        row.clock = match (need, row.state) {
            (Some((item, _)), _) => Some(GoalClock::Since { at: item.at }),
            (None, SliceState::Failed) => Some(GoalClock::Since {
                at: goal.updated_at,
            }),
            (None, SliceState::Done) => Some(GoalClock::Finished {
                at: goal.updated_at,
            }),
            _ => match (eta.and_then(|eta| eta.at), eta) {
                (Some(at), _) => Some(GoalClock::Until { at }),
                (None, Some(eta)) if eta.slip.is_some() => {
                    Some(GoalClock::Late { at: eta.promised })
                }
                _ => (gate.and_then(|next| next.wait.as_ref()))
                    .and_then(|wait| wait.eta())
                    .map(|at| GoalClock::Until { at }),
            },
        };
    }
}

/// The desks that count now, by agent, each terminal saying whether it still runs.
fn held_desks(core: &Core) -> BTreeMap<String, AgentDesk> {
    let mut desks = match &*core.goals.desks.locked() {
        Some((client, desks)) if core.clients.locked().contains_key(client) => desks.clone(),
        _ => BTreeMap::new(),
    };
    for terminal in desks.values_mut().flat_map(|desk| &mut desk.terminals) {
        terminal.running = session_runs(core, terminal.session);
    }
    desks
}

fn session_runs(core: &Core, id: SessionId) -> bool {
    core.session(id).is_some_and(|session| session.is_running())
}

/// The agents each agent started, by the agent that did.
fn helpers_of(core: &Core) -> HashMap<String, Vec<String>> {
    let mut helpers: HashMap<String, Vec<String>> = HashMap::new();
    for agent in core.workspaces.view().agents {
        if let Some(lead) = agent.lead {
            helpers.entry(lead).or_default().push(agent.id);
        }
    }
    helpers
}

/// The open agents working `goal`, its lead first: who holds it or a card under it and whom they
/// started; and for each one that holds a slice, the slice's title.
fn agents_of(
    board: &BoardView,
    goal: &CardView,
    helpers: &HashMap<String, Vec<String>>,
    open: &HashSet<&str>,
) -> (Vec<String>, Vec<(String, String)>) {
    let parents: HashMap<u64, Option<u64>> = (board.cards.iter())
        .map(|card| (card.id, card.parent))
        .collect();
    let under_goal = |card: &CardView| {
        let mut at = Some(card.id);
        while let Some(id) = at {
            if id == goal.id {
                return true;
            }
            at = parents.get(&id).copied().flatten();
        }
        false
    };
    let mut agents: Vec<String> = goal.owner.iter().cloned().collect();
    let mut owned = Vec::new();
    for card in board.cards.iter().filter(|card| under_goal(card)) {
        let Some(owner) = &card.owner else {
            continue;
        };
        if !agents.contains(owner) {
            agents.push(owner.clone());
        }
        if card.slice.is_some() && card.column != Column::Done {
            owned.push((owner.clone(), card.title.clone()));
        }
    }
    let mut at = 0;
    while let Some(agent) = agents.get(at) {
        for helper in helpers.get(agent).into_iter().flatten() {
            if !agents.contains(helper) {
                agents.push(helper.clone());
            }
        }
        at += 1;
    }
    agents.retain(|agent| open.contains(agent.as_str()));
    (agents, owned)
}

/// How `goal` stands by its slices, or for one with none, by its card.
fn state_of(goal: &CardView) -> SliceState {
    goal.goal_state.unwrap_or(match goal.column {
        Column::Done => SliceState::Done,
        Column::Needs => SliceState::NeedsYou,
        Column::Progress => SliceState::Moving,
        Column::Open => SliceState::Waiting,
    })
}

fn slices_of(board: &BoardView, goal: &CardView) -> Vec<GoalSlice> {
    (board.cards.iter())
        .filter(|card| goal.goal_slices.contains(&card.id))
        .filter_map(|card| {
            Some(GoalSlice {
                card: card.id,
                title: card.title.clone(),
                state: card.slice.as_ref()?.state,
            })
        })
        .collect()
}

/// What an item says it needs, in a line: what an agent asked, else the core's words.
fn asks(item: &InboxItem) -> String {
    match (item.kind, &item.detail) {
        (InboxKind::Question, Some(asked)) => asked.clone(),
        _ => item.what.clone(),
    }
}

/// What an item says on the row of the goal `title`, which needn't name that goal again.
fn asks_on(item: &InboxItem, title: &str) -> String {
    let what = asks(item);
    match &item.name {
        Some(name) if item.stalled.is_some() && what.contains(title) => {
            format!("@{name} stalled")
        }
        _ => what,
    }
}

/// Puts each need on the goal it is about, by its card or else by the agent asking, and answers
/// those about no goal.
fn place_needs(rows: &mut [Placed], needing: &[&InboxItem]) -> Vec<LooseNeed> {
    let mut loose = Vec::new();
    for item in needing {
        let on_card = item.card.as_ref().and_then(|card| {
            let goal = card.goal.unwrap_or(card.id);
            (rows.iter())
                .position(|placed| placed.row.card == goal && placed.row.project == card.project)
        });
        let by_agent = || {
            let agent = item.agent_id.as_ref()?;
            let holds = |placed: &Placed| placed.row.agents.contains(agent);
            (rows.iter())
                .position(|placed| holds(placed) && placed.row.state != SliceState::Done)
                .or_else(|| rows.iter().position(holds))
        };
        match on_card.or_else(by_agent) {
            Some(at) => rows[at].row.needs.push(item.id.clone()),
            None => loose.extend(item.need.map(|need| {
                LooseNeed {
                    id: item.id.clone(),
                    need,
                    what: asks(item),
                    agent: item.agent_id.clone(),
                    name: item.name.clone(),
                    project: (item.card.as_ref())
                        .map(|card| card.project.clone())
                        .or_else(|| item.project.clone()),
                    at: item.at,
                }
            })),
        }
    }
    loose
}
