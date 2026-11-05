//! The goals view, which takes the whole window: Home lists every goal and agent beside the row
//! chosen, and one column shows a goal, a card with slices under it, or a slice on its ladder of
//! gates with the evidence the
//! core collected for it. The person reads what a goal waits on, looks at a slice's evidence and
//! passes its gate or pins a note to what needs work, and moves a project's rule into AGENTS.md.
//! An agent's chat opens here too: the one its tab holds, with that tab in front behind the goals.
//! The core keeps the boards and the rules and says where each slice stands; the view only draws
//! and asks.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window,
};
use serde::Serialize;
use sikemux_core::server::board::goal::DecideParams as BoardDecideParams;
use sikemux_core::server::board::BoardView;
use sikemux_core::server::board::CardView;
use sikemux_core::server::board::Column;
use sikemux_core::server::board::NoteParams as BoardNoteParams;
use sikemux_core::server::board::PassParams as BoardPassParams;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::memory::KeepSeededParams;
use sikemux_core::server::memory::MemoryEntryParams;
use sikemux_core::server::memory::MemoryParams;
use sikemux_view::inbox::{
    InboxCard, InboxDigest, InboxItem, InboxKind, PROPOSED_ENVIRONMENTS_ITEM,
};
use sikemux_view::memory::{MemoryView, Seeding};
use sikemux_view::slice::{
    CheckRun, Gate, GateState, GateView, GoalEta, GoalSummaryParams, GoalSummaryView, Need, Passer,
    SavedChat, SliceState, SliceView, Wait, WaitOn,
};
use sikemux_wire::protocol::NoticeGoal;
use tokio::sync::mpsc::UnboundedSender;

use crate::agents::menu::AgentMenu;
use crate::chat::Chats;
use crate::core::{self, Ask};
use crate::theme::{Tokens, SPACE_1H, TEXT_SM};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

mod assertions;
mod environments;
mod evidence;
mod goal;
mod happened;
mod home;
mod panel;
mod rails;
mod room;
mod rules;
mod slice;
mod start;

/// How wide the back link from a chat grows before its name is cut short.
const CRUMB_MAX: f32 = 280.;

pub(crate) use evidence::thumb_face;
pub use rails::{goal_rows, places};
pub use start::start_composer;

/// The view moved to another place, or what it shows changed, which the window's rails show.
pub struct Moved;

impl gpui_kit::EventEmitter<Moved> for GoalsPane {}

actions!(
    goals,
    [LeaveChat, NextGoal, PreviousGoal, FindGoal, OpenChosen]
);

/// Binds the key that leaves a chat open in the goals. A chat with a turn to stop takes it first.
pub fn bind_keys(cx: &mut gpui_kit::App) {
    cx.bind_keys([
        KeyBinding::new("escape", LeaveChat, Some("Goals")),
        KeyBinding::new("j", NextGoal, Some("GoalsList")),
        KeyBinding::new("down", NextGoal, Some("GoalsList")),
        KeyBinding::new("k", PreviousGoal, Some("GoalsList")),
        KeyBinding::new("up", PreviousGoal, Some("GoalsList")),
        KeyBinding::new("/", FindGoal, Some("GoalsList")),
        KeyBinding::new("enter", OpenChosen, Some("GoalsList")),
    ]);
}

/// Where in the goals the pane is.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Home,
    /// What happened while the person was away.
    Overnight,
    /// Where the project in front deploys.
    Environments,
    Goal(u64),
    Slice(u64),
    /// The chat [`GoalsPane::chat`] names.
    Chat,
}

/// A chat opened from the goals: the agent's own, which draws here once its tab is in front.
struct OpenChat {
    agent: String,
    /// Where the back link returns to, and the project that was in front there.
    from: At,
    from_project: Option<String>,
    in_front: bool,
}

pub struct GoalsPane {
    asks: UnboundedSender<Ask>,
    bench: WeakEntity<Workbench>,
    /// Every project's board, once read.
    boards: Option<Vec<BoardView>>,
    /// The project in front of the workbench, which the view shows, as its board names it: its
    /// folder, which the core's board calls take.
    here: Option<String>,
    /// The project something was opened in from another, until the workbench has it in front.
    bound_for: Option<String>,
    /// The rules and knowledge kept for the project open and the folders above it.
    memory: Option<MemoryView>,
    /// The rules offered from past sessions the person unticked, to leave out when they keep.
    unticked: std::collections::HashSet<u64>,
    /// What finished, failed and waits since the person last left.
    digest: Option<InboxDigest>,
    at: At,
    chat: Option<OpenChat>,
    /// The evidence of the open slice the person picked, in place of the one the gate leads with.
    picked: Option<u64>,
    /// The note being written on what needs work, while it is open.
    note: Option<(Entity<InputState>, Subscription)>,
    busy: bool,
    /// Home's list of every goal and agent.
    room: room::Room,
    /// What takes the tab rail's place while Home shows.
    panel: Option<Entity<panel::PaneView>>,
    /// Home's numbers, tabs and rows.
    list: Option<Entity<panel::PaneView>>,
    /// The box that starts work, at the top of Home.
    start: start::Start,
    /// The line on a goal that writes a new assertion.
    asserting: Option<assertions::Asserting>,
    /// The project's environments while the person changes them.
    environments: Option<environments::Editing>,
    /// The summary of the goal open, as the core last answered, with the goal's card.
    summary: Option<(u64, GoalSummaryView)>,
    /// The sentences of a summary the person opened to see their sources.
    unfolded: std::collections::HashSet<String>,
    focus: FocusHandle,
    _reading: Task<()>,
    _reading_summary: Task<()>,
}

impl GoalsPane {
    pub fn new(
        asks: UnboundedSender<Ask>,
        bench: WeakEntity<Workbench>,
        here: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            asks,
            bench,
            boards: None,
            here,
            bound_for: None,
            memory: None,
            unticked: Default::default(),
            digest: None,
            at: At::Home,
            chat: None,
            picked: None,
            note: None,
            busy: false,
            room: room::Room::new(cx),
            panel: None,
            list: None,
            start: start::Start::default(),
            asserting: None,
            environments: None,
            summary: None,
            unfolded: Default::default(),
            focus: cx.focus_handle(),
            _reading: Task::ready(()),
            _reading_summary: Task::ready(()),
        };
        pane.changed(cx);
        pane.read_goals(cx);
        pane
    }

    /// What takes the tab rail's place while Home shows, made the first time it does.
    pub fn panel(&mut self, cx: &mut Context<Self>) -> Entity<panel::PaneView> {
        let pane = cx.entity();
        self.panel
            .get_or_insert_with(|| cx.new(|cx| panel::PaneView::new(&pane, panel::draw, cx)))
            .clone()
    }

    /// Home's numbers, tabs and rows, made the first time they show.
    fn list(&mut self, cx: &mut Context<Self>) -> Entity<panel::PaneView> {
        let pane = cx.entity();
        self.list
            .get_or_insert_with(|| cx.new(|cx| panel::PaneView::new(&pane, room::draw, cx)))
            .clone()
    }

    /// Whether the panel beside Home's list shows in place of the rail's goals.
    pub fn shows_panel(&self) -> bool {
        self.lit() == At::Home
    }

    /// Reads every board, what happened while the person was away and the open project's rules
    /// again, as after the core says one changed.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        let here = core::Target::here(&self.asks);
        let boards = here.request_typed::<call::Boards>(NoParams {});
        let digest = here.request("digest", serde_json::json!({}));
        let memory = self
            .here
            .clone()
            .map(|project| here.request_typed::<call::Memory>(MemoryParams { project }));
        self._reading = cx.spawn(async move |pane, cx| {
            let (boards, digest) = (boards.await, digest.await);
            let memory = match memory {
                Some(memory) => Some(memory.await),
                None => None,
            };
            let _ = pane.update(cx, |pane, cx| {
                match boards {
                    Ok(boards) => pane.take(boards),
                    Err(error) => pane.said(&format!("Couldn't read the goals: {error}"), cx),
                }
                match memory {
                    Some(Ok(memory)) => pane.memory = Some(memory),
                    Some(Err(error)) => pane.said(&format!("Couldn't read the rules: {error}"), cx),
                    None => pane.memory = None,
                }
                match digest.and_then(|digest| {
                    serde_json::from_value(digest).map_err(|error| error.to_string())
                }) {
                    Ok(digest) => pane.digest = Some(digest),
                    Err(error) => pane.said(&format!("Couldn't read what happened: {error}"), cx),
                }
                cx.emit(Moved);
                cx.notify();
            });
        });
    }

    fn take(&mut self, boards: Vec<BoardView>) {
        self.boards = Some(boards);
        let gone = match self.at {
            At::Home | At::Overnight | At::Environments | At::Chat => false,
            At::Goal(id) | At::Slice(id) => self.card(id).is_none(),
        };
        if gone {
            self.at = At::Home;
            self.picked = None;
            self.note = None;
        }
    }

    /// Takes one project's board as the core answered a change to it.
    fn take_one(&mut self, board: BoardView) {
        let mut boards = self.boards.take().unwrap_or_default();
        match boards.iter_mut().find(|kept| kept.project == board.project) {
            Some(kept) => *kept = board,
            None => boards.push(board),
        }
        self.take(boards);
    }

    fn said(&self, error: &str, cx: &mut Context<Self>) {
        let note = toast::error(error.to_owned()).autohide(false);
        let _ = self.bench.update(cx, |bench, cx| bench.notify(note, cx));
    }

    /// Asks the core to change the board, then shows the board it answers.
    fn ask<C: Call<Answer = BoardView> + 'static>(
        &mut self,
        params: C::Params,
        then: impl FnOnce(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) where
        C::Params: Serialize,
    {
        let asked = core::Target::here(&self.asks).request_typed::<C>(params);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                pane.busy = false;
                match answer {
                    Ok(board) => {
                        pane.take_one(board);
                        then(pane);
                        cx.emit(Moved);
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every project's board, as read last.
    fn boards(&self) -> &[BoardView] {
        self.boards.as_deref().unwrap_or_default()
    }

    fn board_of(&self, project: &str) -> Option<&BoardView> {
        self.boards().iter().find(|board| board.project == project)
    }

    /// The board of the project the view shows.
    fn board(&self) -> Option<&BoardView> {
        self.board_of(self.here.as_deref()?)
    }

    /// The cards of the project the view shows.
    fn cards(&self) -> &[CardView] {
        self.board().map_or(&[], |board| board.cards.as_slice())
    }

    /// Follows the project in front of the workbench: the view shows that project's goals, and
    /// a goal or slice of another goes back to Home.
    fn follow_project(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let front =
            (bench.read(cx).project()).map(|project| project.path.to_string_lossy().into_owned());
        if let Some(bound) = &self.bound_for {
            if front.as_ref() != Some(bound) {
                return;
            }
            self.bound_for = None;
        }
        if front == self.here {
            return;
        }
        self.here = front.clone();
        self.project_picked(front, cx);
        if matches!(self.at, At::Goal(_) | At::Slice(_)) {
            self.at = At::Home;
        }
        self.picked = None;
        self.note = None;
        self.memory = None;
        self.changed(cx);
    }

    fn card(&self, id: u64) -> Option<&CardView> {
        self.cards().iter().find(|card| card.id == id)
    }

    /// The slices under card `goal` of the project open, in the board's order.
    fn slices(&self, goal: u64) -> Vec<(&CardView, &SliceView)> {
        slices_of(self.cards(), goal)
    }

    /// Shows the first screen, as switching the window to the goals does.
    pub fn home(&mut self, cx: &mut Context<Self>) {
        self.room.picks_project = false;
        self.go(At::Home, cx);
    }

    /// Goes to `goal`, bringing its project to the front.
    pub fn open_goal(&mut self, goal: &NoticeGoal, cx: &mut Context<Self>) {
        if self.here.as_deref() != Some(goal.project.as_str()) {
            self.bring(goal.project.clone(), cx);
        }
        self.go(At::Goal(goal.card), cx);
    }

    /// The goal an inbox item's card belongs to, as the core says, when it is a goal.
    fn goal_of(&self, card: &InboxCard) -> Option<&CardView> {
        let goal = card.goal.unwrap_or(card.id);
        let board = self.board_of(&card.project)?;
        goals_of(&board.cards)
            .into_iter()
            .find(|known| known.id == goal)
    }

    /// Opens what an inbox item is about: an agent's question or assumption in its chat, a pull
    /// request's page, else its slice or goal here or its card on the project's board, else its
    /// agent's chat.
    fn open(&mut self, item: &InboxItem, cx: &mut Context<Self>) {
        let agent = item.agent_id.clone();
        let in_chat = matches!(
            item.kind,
            InboxKind::Question | InboxKind::Assumption | InboxKind::Permission | InboxKind::Input
        );
        match (item.kind, &item.card, &item.url) {
            (InboxKind::SeededRules, ..) => return self.go(At::Home, cx),
            (InboxKind::Held, None, _) if item.id.starts_with(PROPOSED_ENVIRONMENTS_ITEM) => {
                return self.go(At::Environments, cx)
            }
            (InboxKind::PullRequest, _, Some(url)) => return self.open_url(agent, url.clone(), cx),
            (_, Some(inbox), _) if !in_chat || agent.is_none() => {
                if let Some(card) = (self.board_of(&inbox.project))
                    .and_then(|board| board.cards.iter().find(|card| card.id == inbox.id))
                {
                    let at = match card.slice {
                        Some(_) => Some(At::Slice(card.id)),
                        None if self.goal_of(inbox).is_some_and(|goal| goal.id == card.id) => {
                            Some(At::Goal(card.id))
                        }
                        None => None,
                    };
                    let project = inbox.project.clone();
                    let card = card.id;
                    return match at {
                        Some(at) => {
                            if self.here.as_deref() != Some(project.as_str()) {
                                self.bring(project, cx);
                            }
                            self.go(at, cx)
                        }
                        None => {
                            let _ = (self.bench)
                                .update(cx, |bench, cx| bench.open_board_card(&project, card, cx));
                        }
                    };
                }
            }
            _ => {}
        }
        if let Some(agent) = agent {
            self.open_agent(&agent, cx);
        }
    }

    /// Opens a page on the desk of the agent it came from, else in the person's browser.
    fn open_url(&mut self, agent: Option<String>, url: String, cx: &mut Context<Self>) {
        let _ = self.bench.update(cx, |bench, cx| {
            let on_desk = agent.is_some_and(|agent| bench.open_on_desk(&agent, &url, cx));
            if !on_desk {
                cx.open_url(&url);
            }
        });
    }

    /// Shows the project at `folder`, bringing it to the front of the workbench too.
    fn bring(&mut self, folder: String, cx: &mut Context<Self>) {
        let _ = self.bench.update(cx, |bench, _| {
            let project = (bench.projects().iter())
                .find(|project| project.path.to_string_lossy() == folder)
                .map(|project| project.id.clone());
            if let Some(project) = project {
                bench.activate_project(&project);
            }
        });
        self.here = Some(folder.clone());
        self.bound_for = Some(folder);
        self.memory = None;
        self.changed(cx);
    }

    fn go(&mut self, at: At, cx: &mut Context<Self>) {
        if self.at != at {
            self.picked = None;
            self.note = None;
            if let At::Goal(card) = at {
                self.unfolded.clear();
                self.at = at;
                self.read_summary(card, true, false, cx);
            }
        }
        self.at = at;
        self.chat = None;
        if at == At::Home {
            self.read_goals_if_stale(cx);
        }
        cx.emit(Moved);
        cx.notify();
    }

    /// Reads what the core keeps as goal `card`'s summary, which never waits on a model. With
    /// `refresh` the core also looks at the goal again, and clients hear when that changes this;
    /// with `again`, it tries once more after an attempt that failed.
    fn read_summary(&mut self, card: u64, refresh: bool, again: bool, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = GoalSummaryParams {
            project,
            card,
            refresh,
            again,
        };
        let asked = core::Target::here(&self.asks).request_typed::<call::GoalSummary>(params);
        self._reading_summary = cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                match answer {
                    Ok(view) => pane.summary = Some((card, view)),
                    Err(error) => pane.said(&format!("Couldn't read the summary: {error}"), cx),
                }
                cx.notify();
            });
        });
    }

    /// The core says a goal's summary moved: shows the goal open as it stands, without looking
    /// at the goal again.
    pub fn summary_moved(&mut self, cx: &mut Context<Self>) {
        if let At::Goal(card) = self.at {
            self.read_summary(card, false, false, cx);
        }
    }

    fn summary_of(&self, card: u64) -> Option<&GoalSummaryView> {
        (self.summary.as_ref())
            .filter(|(known, _)| *known == card)
            .map(|(_, view)| view)
    }

    /// What waits on the person about goal `goal`: on a card of it, or from an agent holding one.
    fn needs_of(&self, goal: &CardView, cx: &Context<Self>) -> Vec<InboxItem> {
        let Some(bench) = self.bench.upgrade() else {
            return Vec::new();
        };
        let mut items = Self::needs(bench.read(cx));
        items.extend(Self::questions(bench.read(cx)));
        let cards: Vec<&CardView> = (self.cards().iter())
            .filter(|card| card.id == goal.id || card.parent == Some(goal.id))
            .collect();
        items.retain(|item| match &item.card {
            Some(card) => {
                (self.here.as_deref() == Some(card.project.as_str()))
                    && (card.goal.unwrap_or(card.id) == goal.id)
            }
            None => (item.agent_id.as_deref()).is_some_and(|agent| {
                cards
                    .iter()
                    .any(|card| card.owner.as_deref() == Some(agent))
            }),
        });
        items
    }

    /// The name of the agent holding goal `goal` while it is at work, for a goal with no slices
    /// to say what moves.
    fn working_on(&self, goal: &CardView, cx: &Context<Self>) -> Option<String> {
        if !goal.goal_slices.is_empty() {
            return None;
        }
        let bench = self.bench.upgrade()?;
        let owner = goal.owner.as_deref()?;
        let bench = bench.read(cx);
        let project = bench.project()?.id.clone();
        let working = bench.agents_of(&project).iter().any(|agent| {
            agent.id == owner
                && matches!(
                    agent.status,
                    sikemux_wire::protocol::AgentStatus::Working
                        | sikemux_wire::protocol::AgentStatus::Starting
                )
        });
        working.then(|| goal.owner_name.clone()).flatten()
    }

    /// Keeps a decision the summary found as one of the goal's own, as the person.
    fn keep_decision(&mut self, text: String, cx: &mut Context<Self>) {
        let (At::Goal(card), Some(project)) = (self.at, self.here.clone()) else {
            return;
        };
        let params = BoardDecideParams {
            project,
            card,
            decision: None,
            text: Some(text),
            why: None,
            retire: false,
        };
        self.ask::<call::BoardDecide>(params, |_| {}, cx);
    }

    /// Passes the gate slice `card` waits at, as the person.
    fn pass(&mut self, card: u64, gate: Gate, environment: Option<String>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = BoardPassParams {
            project,
            card,
            gate,
            environment,
        };
        self.ask::<call::BoardPass>(params, |_| {}, cx);
    }

    /// Opens a line to say what needs work, pinned to the evidence in front.
    fn write_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("What needs work, for its owner"));
        let sending = cx.subscribe_in(&input, window, |pane, input, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = input.read(cx).value().trim().to_owned();
                pane.send_note(text, cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.note = Some((input, sending));
        cx.notify();
    }

    fn send_note(&mut self, text: String, cx: &mut Context<Self>) {
        let At::Slice(card) = self.at else {
            return;
        };
        if text.is_empty() || self.busy {
            return;
        }
        let evidence = self
            .card(card)
            .and_then(|card| card.slice.as_ref())
            .and_then(|slice| self.shown_evidence(slice))
            .map(|evidence| evidence.id);
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = BoardNoteParams {
            project,
            card,
            text,
            evidence,
        };
        self.ask::<call::BoardNote>(params, |pane| pane.note = None, cx);
    }

    /// Moves rule `id` into the AGENTS.md of the repository it is kept for, as the person.
    fn promote(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(project) = self.here.clone() {
            self.ask_memory::<call::PromoteRule>(MemoryEntryParams { project, id }, cx);
        }
    }

    /// Ticks or unticks a rule offered from past sessions.
    fn tick_seeded(&mut self, id: u64, on: bool, cx: &mut Context<Self>) {
        match on {
            true => self.unticked.remove(&id),
            false => self.unticked.insert(id),
        };
        cx.notify();
    }

    /// Keeps the ticked rules offered from past sessions, or none when `keep` is false, which
    /// ends the offer either way.
    fn keep_seeded(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let offered = match self
            .memory
            .as_ref()
            .and_then(|memory| memory.seeding.as_ref())
        {
            Some(Seeding::Offered { rules, .. }) => rules,
            _ => return,
        };
        let keep = (offered.iter())
            .map(|rule| rule.id)
            .filter(|id| keep && !self.unticked.contains(id))
            .collect();
        self.unticked.clear();
        self.ask_memory::<call::KeepSeededRules>(KeepSeededParams { project, keep }, cx);
    }

    /// Looks through the project's past sessions for the rules the person repeats, again.
    fn seed_rules(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = self.here.clone() {
            self.ask_memory::<call::SeedRules>(MemoryParams { project }, cx);
        }
    }

    /// Asks the core to change what it keeps for the project, and shows what it keeps after.
    fn ask_memory<C: Call<Answer = MemoryView> + 'static>(
        &mut self,
        params: C::Params,
        cx: &mut Context<Self>,
    ) where
        C::Params: Serialize,
    {
        if self.busy {
            return;
        }
        let asked = core::Target::here(&self.asks).request_typed::<C>(params);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                pane.busy = false;
                match answer {
                    Ok(memory) => pane.memory = Some(memory),
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens agent `agent`'s chat here, bringing its tab to the front behind the goals. An agent
    /// that runs in a terminal has no chat, so its tab shows with the workbench in their place.
    fn open_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        if bench.read(cx).in_terminal(agent) {
            return bench.update(cx, |bench, cx| bench.open_agent(agent, cx));
        }
        bench.update(cx, |bench, _| bench.bring_agent(agent));
        self.expect_chat(agent.to_owned(), cx);
    }

    /// Shows agent `agent`'s chat as soon as its tab is in front.
    fn expect_chat(&mut self, agent: String, cx: &mut Context<Self>) {
        self.chat = Some(OpenChat {
            agent,
            from: self.at,
            from_project: self.here.clone(),
            in_front: false,
        });
        cx.notify();
    }

    /// Follows the agent in front of the workbench: the chat expected shows once its tab is there,
    /// taking the keys, and a chat showing gives way to Home when its tab no longer is.
    fn follow_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        let in_front = bench.read(cx).front_tab_agent() == Some(chat.agent.as_str());
        match (chat.in_front, in_front) {
            (false, true) => {
                chat.in_front = true;
                self.at = At::Chat;
                self.picked = None;
                self.note = None;
                cx.defer_in(window, |pane, window, cx| pane.take_keys(window, cx));
            }
            (true, false) => {
                self.at = At::Home;
                self.chat = None;
                cx.emit(Moved);
            }
            _ => {}
        }
    }

    /// Goes back from a chat to where it was opened from, its tab staying open behind the goals.
    pub fn leave_chat(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.take() else {
            return;
        };
        let known = |bench: &Workbench, folder: &str| {
            (bench.projects().iter()).any(|project| project.path.to_string_lossy() == folder)
        };
        let project = chat.from_project.filter(|folder| {
            self.here.as_ref() != Some(folder)
                && (self.bench.upgrade()).is_some_and(|bench| known(bench.read(cx), folder))
        });
        if let Some(project) = project {
            self.bring(project, cx);
        }
        let gone = match chat.from {
            At::Goal(id) | At::Slice(id) => self.card(id).is_none(),
            _ => false,
        };
        self.go(if gone { At::Home } else { chat.from }, cx);
        cx.defer_in(window, |pane, window, cx| pane.take_keys(window, cx));
    }

    /// Puts the keys where the place on show wants them, away from whatever held them before.
    fn take_keys(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bench) = self.bench.upgrade() {
            if let Some(keys) = self.keys_go_to(&bench.read(cx).chats, cx) {
                keys.focus(window, cx);
            }
        }
    }

    /// Whether agent `agent`'s chat shows here, or will once its tab is in front.
    pub fn wants_chat(&self, agent: &str) -> bool {
        self.chat.as_ref().is_some_and(|chat| chat.agent == agent)
    }

    /// The agent whose chat shows here.
    pub fn chat_shown(&self) -> Option<&str> {
        let chat = self.chat.as_ref().filter(|_| self.at == At::Chat)?;
        Some(chat.agent.as_str())
    }

    /// The place the rails light: where a chat was opened from, while it shows.
    fn lit(&self) -> At {
        match (self.at, &self.chat) {
            (At::Chat, Some(chat)) => chat.from,
            (at, _) => at,
        }
    }

    /// What the back link from `chat` calls the place it was opened from.
    fn place_name(&self, chat: &OpenChat) -> SharedString {
        let title = |id: u64| {
            let board = self.board_of(chat.from_project.as_deref()?)?;
            let card = board.cards.iter().find(|card| card.id == id)?;
            Some(SharedString::from(goal_name(card)))
        };
        match chat.from {
            At::Overnight => "Overnight".into(),
            At::Environments => "Environments".into(),
            At::Goal(id) | At::Slice(id) => title(id).unwrap_or_else(|| "Home".into()),
            At::Home | At::Chat => "Home".into(),
        }
    }

    /// The chat showing here, drawn by the workbench as its tab draws it, under its back link.
    fn chat_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let chat = self.chat.as_ref().filter(|_| self.at == At::Chat)?;
        let agent = chat.agent.clone();
        let back = goal::crumb("chat-back", self.place_name(chat), cx)
            .pb_0()
            .flex_none()
            .max_w(px(CRUMB_MAX))
            .on_click(cx.listener(|pane, _, window, cx| pane.leave_chat(window, cx)));
        (self.bench)
            .update(cx, |bench, cx| {
                crate::home::agent_view_in_goals(bench, &agent, back.into_any_element(), cx)
            })
            .ok()
    }

    /// Opens agent `agent`'s chat here, at a moment the core placed in it or at its latest rows.
    /// Once the agent has closed, its saved session opens again instead.
    fn open_chat_at(
        &mut self,
        agent: &str,
        feed: Option<&str>,
        index: Option<u64>,
        saved: Option<&SavedChat>,
        cx: &mut Context<Self>,
    ) {
        let open = self.bench.upgrade().is_some_and(|bench| {
            let bench = bench.read(cx);
            (bench.projects().iter()).any(|project| {
                bench
                    .agents_of(&project.id)
                    .iter()
                    .any(|row| row.id == agent)
            })
        });
        if !open {
            if let Some(saved) = saved {
                self.reopen(saved, cx);
            }
            return;
        }
        self.open_agent(agent, cx);
        if let (Some(feed), Some(around)) = (feed, index) {
            let _ = self.bench.update(cx, |bench, _| {
                bench.ask_chat(crate::core::ChatAsk::Around {
                    agent: agent.to_owned(),
                    feed: feed.to_owned(),
                    around,
                });
            });
        }
    }

    /// Opens a saved session again in the project the view shows, as a chat here.
    fn reopen(&mut self, saved: &SavedChat, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let target = core::Target::here(&self.asks);
        let taken = target.request(
            "resumeSession",
            serde_json::json!({
                "provider": saved.provider,
                "sessionId": saved.session,
                "project": project,
                "cwd": project,
                "title": "",
            }),
        );
        cx.spawn(async move |pane, cx| {
            let taken = taken.await;
            let _ = pane.update(cx, |pane, cx| match taken {
                Ok(taken) => {
                    let agent = taken["agentId"].as_str().unwrap_or_default().to_owned();
                    let tab = serde_json::json!({
                        "project": taken["project"],
                        "kind": "agent",
                        "target": agent,
                    });
                    target.call("openTab", tab);
                    pane.expect_chat(agent, cx);
                }
                Err(error) => pane.said(&format!("The chat did not open: {error}"), cx),
            });
        })
        .detach();
    }

    /// Opens the Git pane of the project the view shows at commit `sha`.
    fn open_commit(&self, sha: &str, cx: &mut Context<Self>) {
        let _ = self.bench.update(cx, |bench, cx| {
            bench.show_workbench(cx);
            bench.open_commit(sha, cx)
        });
    }

    /// Opens card `card` of the project the view shows beside its board, where all of it can
    /// be changed.
    fn open_on_board(&self, card: u64, cx: &mut Context<Self>) {
        if let Some(project) = self.here.clone() {
            let _ = (self.bench).update(cx, |bench, cx| bench.open_board_card(&project, card, cx));
        }
    }

    /// Opens the Git pane of the project the view shows.
    fn open_git(&self, cx: &mut Context<Self>) {
        let _ = self.bench.update(cx, |bench, cx| {
            bench.show_workbench(cx);
            bench.open_git(cx)
        });
    }

    /// Opens a file in its tab, with the workbench in place of the goals.
    fn open_file(&self, path: &str, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(path);
        let _ = self.bench.update(cx, |bench, cx| {
            bench.show_workbench(cx);
            bench.open_file(&path, cx)
        });
    }

    /// Opens a file of the project the view shows, named from its folder.
    fn open_project_file(&self, file: &str, cx: &mut Context<Self>) {
        if let Some(folder) = &self.here {
            let path = std::path::Path::new(folder).join(file);
            self.open_file(&path.to_string_lossy(), cx);
        }
    }

    /// Shows a picture full size over the goals.
    fn show_picture(&self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(path);
        let _ = (self.bench).update(cx, |bench, cx| bench.show_picture(path, window, cx));
    }
}

impl GoalsPane {
    /// Where the keys nothing else holds go: Home's list, the composer of the chat showing, else
    /// the pane, which a chat does not draw.
    pub fn keys_go_to(&self, chats: &Chats, cx: &gpui_kit::App) -> Option<FocusHandle> {
        match self.at {
            At::Home => Some(self.room.keys.clone()),
            At::Chat => {
                let agent = self.chat_shown()?;
                let composer = chats.open.get(agent)?.composer.as_ref()?;
                Some(composer.draft.read(cx).focus_handle(cx))
            }
            _ => Some(self.focus.clone()),
        }
    }
}

impl Focusable for GoalsPane {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GoalsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_chat(window, cx);
        self.follow_project(cx);
        self.room.picks_project = true;
        self.assert_field(window, cx);
        let chat = self.chat_view(cx);
        let column = |body: AnyElement| {
            div()
                .id("goals-column")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .max_w(px(crate::chat::composer::WIDTH) + crate::theme::SPACE_6 * 2.)
                        .mx_auto()
                        .px(crate::theme::SPACE_6)
                        .py(crate::theme::SPACE_5)
                        .child(body),
                )
        };
        let framed = |shown: AnyElement, cx: &mut Context<Self>| {
            ui::pane(cx)
                .flex_1()
                .min_w_0()
                .child(shown)
                .into_any_element()
        };
        let shown = match (self.boards.is_some(), self.at, chat) {
            (_, At::Chat, Some(chat)) => chat,
            (false, ..) => framed(
                column(empty(None, "reading the goals…", cx)).into_any_element(),
                cx,
            ),
            (true, At::Home | At::Chat, _) => framed(home::home(self, cx), cx),
            (true, At::Environments, _) => {
                framed(column(environments::page(self, cx)).into_any_element(), cx)
            }
            (true, At::Overnight, _) => {
                framed(column(home::overnight(self, cx)).into_any_element(), cx)
            }
            (true, At::Goal(id), _) => {
                framed(column(goal::goal(self, id, cx)).into_any_element(), cx)
            }
            (true, At::Slice(id), _) => {
                framed(column(slice::slice(self, id, cx)).into_any_element(), cx)
            }
        };
        div()
            .id("goals")
            .key_context("Goals")
            .when(self.at != At::Chat, |goals| goals.track_focus(&self.focus))
            .when(self.at == At::Chat, |goals| {
                goals.on_action(
                    cx.listener(|pane, _: &LeaveChat, window, cx| pane.leave_chat(window, cx)),
                )
            })
            .size_full()
            .flex()
            .child(shown)
    }
}

/// A line about an agent, with the agent's own menu on a right-click, as its tab has.
fn with_agent_menu(
    line: gpui_kit::Stateful<gpui_kit::Div>,
    agent: Option<String>,
    bench: &WeakEntity<Workbench>,
) -> AnyElement {
    let Some(agent) = agent else {
        return line.into_any_element();
    };
    let bench = bench.clone();
    line.context_menu(ui::menu(move |popup, _, cx| {
        let menu = (bench.update(cx, |bench, cx| AgentMenu::new(bench, &agent, cx)))
            .ok()
            .flatten();
        match menu {
            Some(menu) => menu.build(popup),
            None => popup,
        }
    }))
    .into_any_element()
}

/// A button that opens a goal or slice beside the agent board, where all of its card can be
/// changed.
fn on_board(card: u64, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    ui::IconButton::new(
        gpui_kit::SharedString::from(format!("on-board-{card}")),
        crate::icons::Icon::Board,
        "Open on the agent board",
    )
    .small()
    .on_click(cx.listener(move |pane, _, _, cx| pane.open_on_board(card, cx)))
}

/// The slices the core says goal `goal` holds among `cards`, in the board's order.
fn slices_of(cards: &[CardView], goal: u64) -> Vec<(&CardView, &SliceView)> {
    let Some(goal) = cards.iter().find(|card| card.id == goal) else {
        return Vec::new();
    };
    cards
        .iter()
        .filter(|card| goal.goal_slices.contains(&card.id))
        .filter_map(|card| Some((card, card.slice.as_ref()?)))
        .collect()
}

/// Where a goal stands: as its slices do together, or for one with none, as its card does.
fn goal_mark(goal: &CardView) -> SliceState {
    goal.goal_state.unwrap_or(match goal.column {
        Column::Done => SliceState::Done,
        Column::Needs => SliceState::NeedsYou,
        Column::Progress => SliceState::Moving,
        Column::Open => SliceState::Waiting,
    })
}

/// What a goal goes by: the short name its summary gave it, else its card's title.
fn goal_name(goal: &CardView) -> String {
    goal.display_title
        .clone()
        .unwrap_or_else(|| goal.title.clone())
}

/// The cards the core says are goals.
fn goals_of(cards: &[CardView]) -> Vec<&CardView> {
    cards.iter().filter(|card| card.is_goal).collect()
}

pub(super) fn empty(
    title: Option<&'static str>,
    message: &'static str,
    cx: &gpui_kit::App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .pt(px(120.))
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACE_1H)
        .when_some(title, |empty, title| {
            empty.child(ui::text(title, TEXT_SM, FontWeight::MEDIUM).text_color(t.text_secondary))
        })
        .child(ui::meta(message, cx))
        .into_any_element()
}

pub(crate) fn gate_name(gate: Gate) -> &'static str {
    match gate {
        Gate::Built => "Built",
        Gate::LooksRight => "Looks right",
        Gate::Landed => "Landed",
    }
}

/// What a step of a ladder is called: its environment's name, or its gate's.
pub(crate) fn step_name(gate: &GateView) -> &str {
    gate.environment.as_deref().unwrap_or(gate_name(gate.gate))
}

fn need_words(need: Need) -> &'static str {
    match need {
        Need::PassingCheck => "a passing check",
        Need::OnBranch => "its commits on the goal's branch",
        Need::Assertions => "evidence for its assertions",
    }
}

/// Who passed gate `gate` of slice `card`, or may pass it.
fn passer_name(gate: &GateView, card: &CardView) -> String {
    match gate.passer {
        Passer::Person => "you".into(),
        Passer::Core => "Sikemux".into(),
        Passer::Owner => gate
            .agent_name
            .clone()
            .or_else(|| card.owner_name.clone())
            .unwrap_or_else(|| "no owner yet".into()),
    }
}

/// The gate a slice is at now; none once it passed them all.
fn waiting(slice: &SliceView) -> Option<&GateView> {
    slice
        .gates
        .iter()
        .find(|gate| gate.state == GateState::Waiting)
}

/// What a slice waits on, in a few words: what is under way and roughly how long it takes,
/// else its gate's unmet needs, or who passes it.
fn waiting_words(card: &CardView, slice: &SliceView) -> Option<String> {
    let gate = waiting(slice)?;
    let owner = card.owner_name.as_deref();
    if let Some(wait) = slice.wait.as_ref().and_then(|wait| wait_words(wait, owner)) {
        return Some(format!("{} · {wait}", step_name(gate)));
    }
    Some(match gate.needs.as_slice() {
        [] if gate.passer == Passer::Person => {
            format!("{} · waiting for you", step_name(gate))
        }
        [] => format!("{} · {}", step_name(gate), passer_name(gate, card)),
        needs => format!(
            "{} · needs {}",
            step_name(gate),
            needs
                .iter()
                .map(|need| need_words(*need))
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    })
}

/// What is under way that a slice waits on, such as "check · ETA 16:40" or "waits for #4"; none
/// for the person, whom the gate already names.
fn wait_words(wait: &Wait, owner: Option<&str>) -> Option<String> {
    let what = match &wait.on {
        WaitOn::Person { .. } => return None,
        WaitOn::Card { card } => return Some(format!("waits for #{card}")),
        WaitOn::Agent { .. } => match owner {
            Some(owner) => format!("@{owner} at work"),
            None => "its owner at work".to_owned(),
        },
        WaitOn::Check {
            run: CheckRun::Queued,
            ..
        } => "check queued".to_owned(),
        WaitOn::Check {
            run: CheckRun::Local,
            ..
        } => "check".to_owned(),
        WaitOn::Check {
            run: CheckRun::Ci { workflow, .. },
            ..
        } => format!("CI {workflow}"),
    };
    Some(match wait.eta() {
        Some(at) if wait.overdue => format!("{what} · was due {}", ui::time::clock(at)),
        Some(at) => format!("{what} · ETA {}", ui::time::clock(at)),
        None => what,
    })
}

/// When a goal should be through, such as "ETA 16:40, then you", or when it was due once
/// the core has no estimate left.
fn eta_words(eta: &GoalEta) -> Option<String> {
    match (eta.at, eta.slip) {
        (Some(at), _) if eta.then_you => Some(format!("ETA {}, then you", ui::time::clock(at))),
        (Some(at), _) => Some(format!("ETA {}", ui::time::clock(at))),
        (None, Some(_)) => Some(format!("was due {}", ui::time::clock(eta.promised))),
        (None, None) => None,
    }
}

/// A slice's mark, the same everywhere: solid green done, a green ring moving, a dashed grey
/// ring waiting, amber when it needs the person and red when it failed.
pub(crate) fn state_mark(state: SliceState, cx: &gpui_kit::App) -> AnyElement {
    ui::mark_box(14., state_dot(state, 8., cx)).into_any_element()
}

/// That mark on its own, at `size`.
fn state_dot(state: SliceState, size: f32, cx: &gpui_kit::App) -> AnyElement {
    let t = Tokens::get(cx);
    let dot = div().size(px(size)).flex_none().rounded_full();
    match state {
        SliceState::Done => dot.bg(t.live),
        SliceState::Moving => dot.border(px(1.5)).border_color(t.live),
        SliceState::Waiting => dot.border_1().border_dashed().border_color(t.text_tertiary),
        SliceState::NeedsYou => dot.bg(t.warn),
        SliceState::Failed => dot.bg(t.danger),
    }
    .into_any_element()
}

/// A row of segments, one per state, the same wherever progress shows.
pub(crate) fn state_strip(
    states: impl IntoIterator<Item = SliceState>,
    width: impl Into<gpui_kit::Length>,
    cx: &gpui_kit::App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .w(width.into())
        .flex_none()
        .flex()
        .gap(px(3.))
        .children(states.into_iter().map(|state| {
            let segment = div().flex_1().h(px(4.)).rounded(px(2.));
            match state {
                SliceState::Done => segment.bg(t.live),
                SliceState::Moving => segment.bg(t.live.opacity(0.45)),
                SliceState::NeedsYou => segment.bg(t.warn),
                SliceState::Failed => segment.bg(t.danger),
                SliceState::Waiting => segment
                    .border_1()
                    .border_dashed()
                    .border_color(t.text_disabled),
            }
        }))
        .into_any_element()
}

/// How a goal counts `count` of its slices in `state`.
fn state_count(count: usize, state: SliceState) -> String {
    let words = match state {
        SliceState::Done => "done",
        SliceState::Moving => "moving",
        SliceState::Waiting => "waiting",
        SliceState::NeedsYou if count == 1 => "needs you",
        SliceState::NeedsYou => "need you",
        SliceState::Failed => "failed",
    };
    format!("{count} {words}")
}
