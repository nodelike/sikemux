//! A label a band of light sweeps across while what it names is under way, as on the Mac.

use gpui::{
    div, AnyElement, Context, ElementId, HighlightStyle, Hsla, IntoElement, ParentElement, Render,
    SharedString, Styled, StyledText, Window,
};

use super::clock::{self, Slot};
use crate::theme::Tokens;

/// One sweep, from before the label's start to past its end.
const SWEEP_SECONDS: f32 = 2.2;
/// How far either side of its brightest point the band reaches, in widths of the label.
const REACH: f32 = 0.3;

/// `text` with the band sweeping across it, kept under `key` while it is drawn.
pub fn shimmer(key: impl Into<ElementId>, text: impl Into<SharedString>) -> AnyElement {
    Slot::new(key, Shimmer { text: text.into() }).into_any_element()
}

struct Shimmer {
    text: SharedString,
}

impl Render for Shimmer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        let (dim, lit) = (t.text_disabled, t.text_primary);
        let label = div().min_w_0().truncate().text_color(dim);
        if cx.reduce_motion() || !clock::tick(window, cx) {
            return label.child(self.text.clone());
        }
        let peak = 2. * (clock::seconds() / SWEEP_SECONDS).fract() - 0.5;
        let count = self.text.chars().count().max(1) as f32;
        let lights: Vec<_> = self
            .text
            .char_indices()
            .enumerate()
            .filter_map(|(ix, (at, letter))| {
                let reach = 1. - ((ix as f32 + 0.5) / count - peak).abs() / REACH;
                let light = Hsla {
                    a: lit.a * reach,
                    ..lit
                };
                let style = HighlightStyle {
                    color: Some(dim.blend(light)),
                    ..Default::default()
                };
                (reach > 0.).then_some((at..at + letter.len_utf8(), style))
            })
            .collect();
        label.child(StyledText::new(self.text.clone()).with_highlights(lights))
    }
}
