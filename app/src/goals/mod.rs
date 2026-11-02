//! The goals view, which takes the whole window: a rail of its own with Home, Overnight and
//! every project's goals, beside one column showing what needs the person and what is moving,
//! a goal, a card with slices under it, or a slice on its ladder of gates with the evidence the
//! core collected for it. The person reads what a goal waits on, looks at a slice's evidence and
//! passes its gate or pins a note to what needs work, and moves a project's rule into AGENTS.md.
//! The core keeps the boards and the rules and says where each slice stands; the view only draws
//! and asks.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Task, WeakEntity, Window,
};
use serde::Serialize;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::{
    BoardNoteParams, BoardPassParams, BoardView, CardView, MemoryEntryParams, MemoryParams,
    NoParams,
};
use sikemux_wire::inbox::{InboxCard, InboxDigest, InboxItem};
use sikemux_wire::memory::MemoryView;
use sikemux_wire::slice::{
    CheckRun, Gate, GateState, GateView, Need, Passer, SliceState, SliceView, Wait, WaitOn,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{self, Ask};
use crate::theme::{Tokens, SPACE_1H, TEXT_SM};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

mod assertions;
mod evidence;
mod goal;
mod home;
mod rails;
mod rules;
mod slice;
mod start;

pub use rails::{goal_rows, places};
pub use start::START_DRAFT;

/// The view moved to another place, or what it shows changed, which the window's rails show.
pub struct Moved;

impl gpui_kit::EventEmitter<Moved> for GoalsPane {}

/// Where in the goals the pane is.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Home,
    /// What happened while the person was away.
    Overnight,
    Goal(u64),
    Slice(u64),
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
    /// What finished, failed and waits since the person last left.
    digest: Option<InboxDigest>,
    at: At,
    /// The evidence of the open slice the person picked, in place of the one the gate leads with.
    picked: Option<u64>,
    /// The note being written on what needs work, while it is open.
    note: Option<(Entity<InputState>, Subscription)>,
    busy: bool,
    /// The box that starts work, at the top of Home.
    start: start::Start,
    /// The line on a goal that writes a new assertion.
    asserting: Option<assertions::Asserting>,
    focus: FocusHandle,
    _reading: Task<()>,
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
            digest: None,
            at: At::Home,
            picked: None,
            note: None,
            busy: false,
            start: start::Start::default(),
            asserting: None,
            focus: cx.focus_handle(),
            _reading: Task::ready(()),
        };
        pane.changed(cx);
        pane
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
            At::Home | At::Overnight => false,
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
        self.here = front;
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
        self.go(At::Home, cx);
    }

    /// The goal an inbox item's card belongs to, as the core says, when it is a goal.
    fn goal_of(&self, card: &InboxCard) -> Option<&CardView> {
        let goal = card.goal.unwrap_or(card.id);
        let board = self.board_of(&card.project)?;
        goals_of(&board.cards)
            .into_iter()
            .find(|known| known.id == goal)
    }

    /// Opens what an inbox item is about: its slice or goal, else its agent's chat.
    fn open(&mut self, item: &InboxItem, cx: &mut Context<Self>) {
        let opens = item.card.as_ref().and_then(|inbox| {
            let card = self
                .board_of(&inbox.project)?
                .cards
                .iter()
                .find(|card| card.id == inbox.id)?;
            let at = match card.slice {
                Some(_) => At::Slice(card.id),
                None if self.goal_of(inbox).is_some_and(|goal| goal.id == card.id) => {
                    At::Goal(card.id)
                }
                None => return None,
            };
            Some((inbox.project.clone(), at))
        });
        match opens {
            Some((project, at)) => {
                if self.here.as_deref() != Some(project.as_str()) {
                    self.bring(project, cx);
                }
                self.go(at, cx)
            }
            None => {
                if let Some(agent) = item.agent_id.clone() {
                    let _ = self.bench.update(cx, |bench, _| bench.open_agent(&agent));
                }
            }
        }
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
        }
        self.at = at;
        cx.emit(Moved);
        cx.notify();
    }

    /// Passes the gate slice `card` waits at, as the person.
    fn pass(&mut self, card: u64, gate: Gate, cx: &mut Context<Self>) {
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
        if self.busy {
            return;
        }
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = MemoryEntryParams { project, id };
        let asked = core::Target::here(&self.asks).request_typed::<call::PromoteRule>(params);
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

    fn open_file(&self, path: &str, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(path);
        let _ = self
            .bench
            .update(cx, |bench, cx| bench.open_file(&path, cx));
    }
}

impl Focusable for GoalsPane {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GoalsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_project(cx);
        self.start_field(window, cx);
        self.assert_field(window, cx);
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
        let shown = match (self.boards.is_some(), self.at) {
            (false, _) => column(empty(None, "reading the goals…", cx)).into_any_element(),
            (true, At::Home) => home::home(self, cx),
            (true, At::Overnight) => column(home::overnight(self, cx)).into_any_element(),
            (true, At::Goal(id)) => column(goal::goal(self, id, cx)).into_any_element(),
            (true, At::Slice(id)) => column(slice::slice(self, id, cx)).into_any_element(),
        };
        div()
            .id("goals")
            .key_context("Goals")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .child(ui::pane(cx).flex_1().min_w_0().child(shown))
    }
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

/// The cards the core says are goals.
fn goals_of(cards: &[CardView]) -> Vec<&CardView> {
    cards
        .iter()
        .filter(|card| !card.goal_slices.is_empty())
        .collect()
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

fn gate_name(gate: Gate) -> &'static str {
    match gate {
        Gate::Built => "Built",
        Gate::LooksRight => "Looks right",
        Gate::Landed => "Landed",
    }
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
    if let Some(wait) = slice.wait.as_ref().and_then(|wait| wait_words(wait, card)) {
        return Some(format!("{} · {wait}", gate_name(gate.gate)));
    }
    Some(match gate.needs.as_slice() {
        [] if gate.passer == Passer::Person => {
            format!("{} · waiting for you", gate_name(gate.gate))
        }
        [] => format!("{} · {}", gate_name(gate.gate), passer_name(gate, card)),
        needs => format!(
            "{} · needs {}",
            gate_name(gate.gate),
            needs
                .iter()
                .map(|need| need_words(*need))
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    })
}

/// What is under way that a slice waits on, such as "check · ~3 min" or "waits for #4"; none
/// for the person, whom the gate already names.
fn wait_words(wait: &Wait, card: &CardView) -> Option<String> {
    let what = match &wait.on {
        WaitOn::Person { .. } => return None,
        WaitOn::Card { card } => return Some(format!("waits for #{card}")),
        WaitOn::Agent { .. } => {
            return Some(format!(
                "{} at work",
                card.owner_name.as_deref().unwrap_or("its owner")
            ))
        }
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
    let minutes = |ms: u64| ms.div_ceil(60_000);
    Some(match (wait.estimate_ms, wait.overdue) {
        (Some(ms), false) => format!("{what} · ~{} min", minutes(ms)),
        (Some(ms), true) => format!("{what} · over {} min", minutes(ms)),
        (None, _) => what,
    })
}

/// A slice's mark, the same everywhere: solid green done, a green ring moving, a dashed grey
/// ring waiting, amber when it needs the person and red when it failed.
fn state_mark(state: SliceState, cx: &gpui_kit::App) -> AnyElement {
    let t = Tokens::get(cx);
    let dot = div().size(px(8.)).rounded_full();
    let mark = match state {
        SliceState::Done => dot.bg(t.live),
        SliceState::Moving => dot.border(px(1.5)).border_color(t.live),
        SliceState::Waiting => dot.border_1().border_dashed().border_color(t.text_tertiary),
        SliceState::NeedsYou => dot.bg(t.warn),
        SliceState::Failed => dot.bg(t.danger),
    };
    ui::mark_box(14., mark).into_any_element()
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
