//! A project's goals: each goal a card with slices under it, the slices on their ladders of
//! gates, and the evidence the core collected for each. The person reads what a goal waits on,
//! looks at a slice's evidence and passes its gate or pins a note to what needs work. The core
//! keeps the board and says where each slice stands; the pane only draws and asks.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Task, WeakEntity, Window,
};
use serde::Serialize;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::{BoardNoteParams, BoardParams, BoardPassParams, BoardView, CardView};
use sikemux_wire::slice::{Gate, GateState, GateView, Need, Passer, SliceState, SliceView};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{self, Ask};
use crate::theme::{Tokens, SPACE_1H, TEXT_SM};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

mod evidence;
mod goal;
mod slice;

/// Where in the goals the pane is.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Goals,
    Goal(u64),
    Slice(u64),
}

pub struct GoalsPane {
    asks: UnboundedSender<Ask>,
    /// The project's folder, which the core's board calls take.
    pub folder: String,
    bench: WeakEntity<Workbench>,
    board: Option<BoardView>,
    at: At,
    /// The evidence of the open slice the person picked, in place of the one the gate leads with.
    picked: Option<u64>,
    /// The note being written on what needs work, while it is open.
    note: Option<(Entity<InputState>, Subscription)>,
    busy: bool,
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
            at: At::Goals,
            picked: None,
            note: None,
            busy: false,
            focus: cx.focus_handle(),
            _reading: Task::ready(()),
        };
        pane.changed(cx);
        pane
    }

    /// Reads the board again, as after the core says it changed.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        let read = core::Target::here(&self.asks).request_typed::<call::Board>(BoardParams {
            project: self.folder.clone(),
        });
        self._reading = cx.spawn(async move |pane, cx| {
            let board = read.await;
            let _ = pane.update(cx, |pane, cx| {
                match board {
                    Ok(board) => pane.take(board),
                    Err(error) => pane.said(&format!("Couldn't read the goals: {error}"), cx),
                }
                cx.notify();
            });
        });
    }

    fn take(&mut self, board: BoardView) {
        self.board = Some(board);
        let gone = match self.at {
            At::Goals => false,
            At::Goal(id) | At::Slice(id) => self.card(id).is_none(),
        };
        if gone {
            self.at = At::Goals;
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let goals = self.goals();
        let shown = match (self.at, goals.as_slice()) {
            (At::Goals, [only]) => At::Goal(only.id),
            (at, _) => at,
        };
        let body = match (self.board.is_some(), goals.is_empty(), shown) {
            (false, ..) => empty(None, "reading the goals…", cx),
            (true, true, _) => empty(
                Some("No goals yet"),
                "A goal is a card with slices under it, each with gates to pass.",
                cx,
            ),
            (true, false, At::Goals) => goal::list(self, cx),
            (true, false, At::Goal(id)) => goal::goal(self, id, goals.len() > 1, cx),
            (true, false, At::Slice(id)) => slice::slice(self, id, cx),
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

fn empty(title: Option<&'static str>, message: &'static str, cx: &gpui_kit::App) -> AnyElement {
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

/// What a slice waits on, in a few words: its gate's unmet needs, or who passes it.
fn waiting_words(card: &CardView, slice: &SliceView) -> Option<String> {
    let gate = waiting(slice)?;
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
