//! How a slice of work stands, drawn the same wherever it shows: its mark, the colour its words
//! take, a strip of its steps, and the names of its gates.

use gpui_kit::{div, px, AnyElement, App, IntoElement, Length, ParentElement, Styled};
use sikemux_view::slice::{Gate, SliceState};

use crate::theme::Tokens;

pub(crate) fn gate_name(gate: Gate) -> &'static str {
    match gate {
        Gate::Built => "Built",
        Gate::LooksRight => "Looks right",
        Gate::Landed => "Landed",
    }
}

/// A slice's mark, the same everywhere: solid green done, a green ring moving, a dashed grey
/// ring waiting, amber when it needs the person and red when it failed.
pub(crate) fn state_mark(state: SliceState, cx: &App) -> AnyElement {
    super::mark_box(14., state_dot(state, 8., cx)).into_any_element()
}

/// The colour a state's mark and words take.
pub(crate) fn state_ink(state: SliceState, t: &Tokens) -> gpui_kit::Hsla {
    match state {
        SliceState::Done | SliceState::Moving => t.live,
        SliceState::Waiting => t.text_tertiary,
        SliceState::NeedsYou => t.warn,
        SliceState::Failed => t.danger,
    }
}

/// That mark on its own, at `size`.
pub(crate) fn state_dot(state: SliceState, size: f32, cx: &App) -> AnyElement {
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
    width: impl Into<Length>,
    cx: &App,
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
