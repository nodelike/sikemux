//! Every goal and every agent on one screen: which agents work each goal, what is on their
//! desks and what comes next. The window owns the browser and the editors, so it tells
//! the core what its desks hold.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use sikemux_process::sync::Locked;
use sikemux_view::goals::{
    AgentDesk, DeskCounts, DesksParams, GoalAgent, GoalClock, GoalNext, GoalRow, GoalsView,
    LooseNeed,
};
use sikemux_view::inbox::{AgentShown, Inbox, InboxItem, InboxKind, InboxNeed};
use sikemux_view::slice::SliceState;
use sikemux_wire::protocol::{AgentGroup, SessionId};

use super::board::{self, BoardCard, ProjectBoard};
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
    let mut kept = core.goals().desks.locked();
    let before = match &*kept {
        Some((held_by, held)) if *held_by == client && *held == desks => return,
        Some((held_by, held)) if *held_by == client => held.clone(),
        _ => BTreeMap::new(),
    };
    *kept = Some((client, desks.clone()));
    drop(kept);
    super::now::asks::desks_moved(core, &before, &desks);
    core.changed(What::Read(Topic::Goals));
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
    if let Some((_, desks)) = &*core.goals().desks.locked() {
        for terminal in desks.values().flat_map(|desk| &desk.terminals) {
            (terminal.session, session_runs(core, terminal.session)).hash(&mut hasher);
        }
    }
    let moved = hasher.finish();
    let mut seen = core.goals().seen.locked();
    if *seen != moved {
        *seen = moved;
        drop(seen);
        core.changed(What::Read(Topic::Goals));
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

    let families: Vec<Family> = boards.iter().map(Family::of).collect();
    let mut rows = Vec::new();
    for (board, family) in boards.iter().zip(&families) {
        for goal in board.cards.iter().filter(|card| card.is_goal) {
            let agents = agents_of(family, goal, &helpers, &open);
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
                    agents,
                    needs: Vec::new(),
                    desk,
                    worker: goal.owner.clone().filter(|_| !goal.goal_stalled),
                },
                goal,
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
    let asking: HashSet<&str> = (loose_needs.iter())
        .filter_map(|need| need.agent.as_deref())
        .collect();
    let no_goal = (inbox.agents.iter())
        .map(|agent| agent.agent_id.as_str())
        .filter(|agent| !working.contains(agent) && !asking.contains(agent))
        .map(str::to_owned)
        .collect();
    let names = core.workspaces().view();
    let names: HashMap<&str, Option<String>> = (names.agents.iter())
        .map(|agent| (agent.id.as_str(), core.names().name(&agent.id)))
        .collect();
    let agents = (inbox.agents.iter())
        .map(|agent| {
            let left = inbox::left(core, &agent.agent_id);
            GoalAgent {
                state: agent_state(agent, left.ended_well),
                left: left.words,
                offer: left.offer,
                id: agent.agent_id.clone(),
                name: names.get(agent.agent_id.as_str()).cloned().flatten(),
                cli: agent.cli.clone(),
                face: agent.face.clone(),
                status: agent.status,
                group: agent.group,
                task: agent.title.clone(),
                project: agent.project.clone(),
                desk: desks.get(&agent.agent_id).cloned().unwrap_or_default(),
            }
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

/// How an agent stands in the marks goals wear: done once its turn ended well, whether or not the
/// person has seen it yet.
fn agent_state(agent: &AgentShown, ended_well: bool) -> SliceState {
    match agent.group {
        AgentGroup::Working => SliceState::Moving,
        AgentGroup::Blocked => SliceState::NeedsYou,
        AgentGroup::Done if !agent.failed => SliceState::Done,
        AgentGroup::Idle if ended_well => SliceState::Done,
        AgentGroup::Done | AgentGroup::Idle => SliceState::Waiting,
    }
}

/// A board's cards by the card at the top above each, which is the card itself at the top, so a
/// goal's row is put together without reading the whole board again.
struct Family<'a> {
    in_goal: HashMap<u64, Vec<&'a BoardCard>>,
}

impl<'a> Family<'a> {
    fn of(board: &'a ProjectBoard) -> Self {
        let cards: HashMap<u64, &BoardCard> =
            (board.cards.iter()).map(|card| (card.id, card)).collect();
        let mut in_goal: HashMap<u64, Vec<&BoardCard>> = HashMap::new();
        for card in &board.cards {
            let mut top = card.id;
            while let Some(parent) = cards.get(&top).and_then(|card| card.parent) {
                top = parent;
            }
            in_goal.entry(top).or_default().push(card);
        }
        Self { in_goal }
    }

    fn in_goal(&self, goal: u64) -> &[&'a BoardCard] {
        self.in_goal.get(&goal).map_or(&[], Vec::as_slice)
    }
}

/// A goal's row while the view is put together, with the card it was made from.
struct Placed<'a> {
    row: GoalRow,
    goal: &'a BoardCard,
}

impl Placed<'_> {
    /// Says how it stands once its needs are known, what comes next and since when.
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
        row.next = match need {
            Some((item, need)) => Some(GoalNext::Need {
                need,
                what: asks_on(item, &row.title),
            }),
            None if row.state == SliceState::Done => Some(GoalNext::Done),
            None => goal
                .owner_name
                .clone()
                .filter(|_| matches!(row.state, SliceState::Moving | SliceState::Waiting))
                .map(|agent| GoalNext::Agent {
                    agent,
                    idle: row.state == SliceState::Waiting,
                }),
        };
        row.clock = match (need, row.state) {
            (Some((item, _)), _) => Some(GoalClock::Since { at: item.at }),
            (None, SliceState::Failed) => Some(GoalClock::Since {
                at: goal.updated_at,
            }),
            (None, SliceState::Done) => Some(GoalClock::Finished {
                at: goal.updated_at,
            }),
            _ => None,
        };
    }
}

/// The desks that count now, by agent, each terminal saying whether it still runs.
fn held_desks(core: &Core) -> BTreeMap<String, AgentDesk> {
    let mut desks = match &*core.goals().desks.locked() {
        Some((client, desks)) if core.connections().locked().contains_key(client) => desks.clone(),
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
    for agent in core.workspaces().view().agents {
        if let Some(lead) = agent.lead {
            helpers.entry(lead).or_default().push(agent.id);
        }
    }
    helpers
}

/// The open agents working `goal`, its lead first: who holds it or a card under it and whom they
/// started.
fn agents_of(
    family: &Family,
    goal: &BoardCard,
    helpers: &HashMap<String, Vec<String>>,
    open: &HashSet<&str>,
) -> Vec<String> {
    let mut agents: Vec<String> = goal.owner.iter().cloned().collect();
    for owner in family
        .in_goal(goal.id)
        .iter()
        .filter_map(|card| card.owner.as_ref())
    {
        if !agents.contains(owner) {
            agents.push(owner.clone());
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
    agents
}

fn state_of(goal: &BoardCard) -> SliceState {
    goal.goal_state.unwrap_or(SliceState::Waiting)
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
            Some(at) => {
                if let Some(placed) = rows.get_mut(at) {
                    placed.row.needs.push(item.id.clone());
                }
            }
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
