//! A project's goals: first what needs the person and what is moving, then each goal, a card
//! with slices under it, the slices on their ladders of gates, and the evidence the core
//! collected for each. The person reads what a goal waits on,
//! looks at a slice's evidence and passes its gate or pins a note to what needs work, and moves a
//! project's rule into AGENTS.md. The core keeps the board and the rules and says where each
//! slice stands; the pane only draws and asks.

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
    BoardNoteParams, BoardParams, BoardPassParams, BoardView, CardView, MemoryEntryParams,
    MemoryParams,
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
mod rules;
mod slice;
mod start;

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
    /// The project's folder, which the core's board calls take.
    pub folder: String,
    bench: WeakEntity<Workbench>,
    board: Option<BoardView>,
    /// The rules and knowledge kept for the project and the folders above it.
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
        folder: String,
        bench: WeakEntity<Workbench>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            asks,
            folder,
            bench,
            board: None,
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

    /// Reads the board and the project's rules again, as after the core says either changed.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        let here = core::Target::here(&self.asks);
        let project = self.folder.clone();
        let board = here.request_typed::<call::Board>(BoardParams {
            project: project.clone(),
        });
        let digest = here.request("digest", serde_json::json!({ "project": project }));
        let memory = here.request_typed::<call::Memory>(MemoryParams { project });
        self._reading = cx.spawn(async move |pane, cx| {
            let (board, memory, digest) = (board.await, memory.await, digest.await);
            let _ = pane.update(cx, |pane, cx| {
                match board {
                    Ok(board) => pane.take(board),
                    Err(error) => pane.said(&format!("Couldn't read the goals: {error}"), cx),
                }
                match memory {
                    Ok(memory) => pane.memory = Some(memory),
                    Err(error) => pane.said(&format!("Couldn't read the rules: {error}"), cx),
                }
                match digest.and_then(|digest| {
                    serde_json::from_value(digest).map_err(|error| error.to_string())
                }) {
                    Ok(digest) => pane.digest = Some(digest),
                    Err(error) => pane.said(&format!("Couldn't read what happened: {error}"), cx),
                }
                cx.notify();
            });
        });
    }

    fn take(&mut self, board: BoardView) {
        self.board = Some(board);
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
                        pane.take(board);
                        then(pane);
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cards(&self) -> &[CardView] {
        self.board
            .as_ref()
            .map_or(&[], |board| board.cards.as_slice())
    }

    fn card(&self, id: u64) -> Option<&CardView> {
        self.cards().iter().find(|card| card.id == id)
    }

    /// The slices under card `goal`, in the board's order.
    fn slices(&self, goal: u64) -> Vec<(&CardView, &SliceView)> {
        self.cards()
            .iter()
            .filter(|card| card.parent == Some(goal))
            .filter_map(|card| Some((card, card.slice.as_ref()?)))
            .collect()
    }

    /// The cards on top of the board with slices under them.
    fn goals(&self) -> Vec<&CardView> {
        self.cards()
            .iter()
            .filter(|card| card.parent.is_none() && !self.slices(card.id).is_empty())
            .collect()
    }

    /// Shows the first screen, as opening the pane from the rail does.
    pub fn home(&mut self, cx: &mut Context<Self>) {
        self.go(At::Home, cx);
    }

    /// The project's id, as the core names it in the inbox.
    fn project(&self) -> Option<&str> {
        Some(self.board.as_ref()?.project.as_str())
    }

    /// The goal an inbox item's card belongs to, as the core says, when it is a goal here.
    fn goal_of(&self, card: &InboxCard) -> Option<&CardView> {
        let goal = card.goal.unwrap_or(card.id);
        self.goals().into_iter().find(|known| known.id == goal)
    }

    /// Opens what an inbox item is about: its slice or goal here, else its agent's chat.
    fn open(&mut self, item: &InboxItem, cx: &mut Context<Self>) {
        let on_board = item
            .card
            .as_ref()
            .filter(|card| Some(card.project.as_str()) == self.project())
            .and_then(|card| Some((self.card(card.id)?, card)));
        match on_board {
            Some((card, _)) if card.slice.is_some() => self.go(At::Slice(card.id), cx),
            Some((card, inbox)) if self.goal_of(inbox).is_some_and(|goal| goal.id == card.id) => {
                self.go(At::Goal(card.id), cx)
            }
            _ => {
                if let Some(agent) = item.agent_id.clone() {
                    let _ = self.bench.update(cx, |bench, _| bench.open_agent(&agent));
                }
            }
        }
    }

    fn go(&mut self, at: At, cx: &mut Context<Self>) {
        if self.at != at {
            self.picked = None;
            self.note = None;
        }
        self.at = at;
        cx.notify();
    }

    /// Passes the gate slice `card` waits at, as the person.
    fn pass(&mut self, card: u64, gate: Gate, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let params = BoardPassParams {
            project: self.folder.clone(),
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
        let params = BoardNoteParams {
            project: self.folder.clone(),
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
        let params = MemoryEntryParams {
            project: self.folder.clone(),
            id,
        };
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
        self.start_field(window, cx);
        self.assert_field(window, cx);
        let body = match (self.board.is_some(), self.at) {
            (false, _) => empty(None, "reading the goals…", cx),
            (true, At::Home) => home::home(self, cx),
            (true, At::Overnight) => home::overnight(self, cx),
            (true, At::Goal(id)) => goal::goal(self, id, cx),
            (true, At::Slice(id)) => slice::slice(self, id, cx),
        };
        div()
            .id("goals")
            .key_context("Goals")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(680.))
                    .mx_auto()
                    .px(crate::theme::SPACE_6)
                    .py(crate::theme::SPACE_5)
                    .child(body),
            )
    }
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
