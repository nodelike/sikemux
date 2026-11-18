use std::rc::Rc;

use gpui_kit::{
    div, px, AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window,
};

use super::IconButton;
use crate::icons::Icon;
use crate::theme::{self, Tokens, TEXT_XS};

const HEIGHT: f32 = 22.;
const BUTTON: f32 = 22.;
const VALUE: f32 = 24.;

/// − and + joined around a number in one hairline box, from 0 to `most`.
pub fn stepper(
    id: impl Into<SharedString>,
    value: u32,
    most: u32,
    set: impl Fn(u32, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let id = id.into();
    let set = Rc::new(set);
    let (less, more) = (set.clone(), set);
    let inner = HEIGHT - 2.;
    let button = |side: &str, glyph: Icon, label: SharedString| {
        IconButton::new(SharedString::from(format!("{id}-{side}")), glyph, label)
            .width(BUTTON)
            .height(inner)
            .glyph(11.)
            .quiet(false)
            .radius(px(0.))
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(HEIGHT))
        .border(theme::border())
        .border_color(t.border)
        .rounded(theme::radius_2())
        .bg(t.surface_sunken)
        .overflow_hidden()
        .child(
            button("less", Icon::Minus, t!("One less").into())
                .disabled(value == 0)
                .on_click(move |_, window, cx| less(value.saturating_sub(1), window, cx)),
        )
        .child(
            div()
                .w(px(VALUE))
                .h(px(inner))
                .flex()
                .items_center()
                .justify_center()
                .border_l(theme::border())
                .border_r(theme::border())
                .border_color(t.border)
                .text_size(theme::ui_text(TEXT_XS))
                .font_features(theme::tabular())
                .text_color(t.text_primary)
                .child(value.to_string()),
        )
        .child(
            button("more", Icon::Plus, t!("One more").into())
                .disabled(value >= most)
                .on_click(move |_, window, cx| more((value + 1).min(most), window, cx)),
        )
        .into_any_element()
}
