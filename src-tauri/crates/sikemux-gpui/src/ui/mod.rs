//! The shared component set. Screens are built from these; one a screen needs
//! that is missing belongs here, once.

pub mod clock;
mod controls;
pub(crate) mod dialog;
mod dock;
pub mod find_bar;
mod floats;
pub mod grain;
mod lists;
pub mod moving_face;
mod pane;
mod select;
pub mod selectable;
pub mod shimmer;
mod state_mark;

pub use controls::{Button, Field, IconButton, OnClick, Segmented, Toggle, Variant};
pub use dialog::{Cheatsheet, Choice, Choices, Confirm, Prompt, Section};
pub use dock::{DockMenu, DockRow, DockScope};
pub use find_bar::FindBar;
pub use floats::{danger_popup_item, float, tooltip, Menu, MenuItem};
pub use lists::{close_button, Group, Row, Tab, ROW};
pub use pane::{pane, PaneHeader, AGENT_INSET};
pub use select::{select, Pick};
pub use state_mark::status_mark;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, svg, AnyElement, App, Div, FontFallbacks, FontWeight, Hsla, IntoElement,
    ParentElement, SharedString, Styled, Svg,
};

use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};
use crate::workbench::AgentRow;

pub fn icon(icon: Icon, size: f32) -> Svg {
    svg().path(icon.path()).size(px(size)).flex_none()
}

/// A mark at `size`: a picture as it is drawn, or a glyph in `tint`.
pub fn picture(glyph: Icon, size: f32, tint: Hsla) -> AnyElement {
    if glyph.is_picture() {
        img(glyph.path())
            .size(px(size))
            .flex_none()
            .into_any_element()
    } else {
        icon(glyph, size).text_color(tint).into_any_element()
    }
}

/// An agent's mark at `size`, in its brand colour.
pub fn mark(provider: &str, size: f32) -> AnyElement {
    picture(icons::agent(provider), size, theme::brand(provider))
}

/// How a CI run, job or check went, as GitHub draws it: a filled circle with its
/// tick, cross or slash once it ends, an amber dot while queued, an amber ring
/// while it runs, and an empty ring when nobody knows.
pub fn status_of_run(outcome: &str, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let ended = |color: Hsla, inner: Icon| {
        div()
            .size(px(size * 0.9))
            .rounded_full()
            .bg(color)
            .flex()
            .items_center()
            .justify_center()
            .child(icon(inner, size).text_color(t.on_accent))
    };
    let mark = match outcome {
        "success" => ended(t.live, Icon::MarkSuccess),
        "failure" => ended(t.danger, Icon::MarkFailure),
        "cancelled" => ended(t.text_tertiary, Icon::MarkCancelled),
        "skipped" => ended(t.text_tertiary, Icon::MarkSkipped),
        "queued" => div().size(px(size * 0.55)).rounded_full().bg(t.warn),
        "running" | "blocked" => div()
            .size(px(size * 0.82))
            .rounded_full()
            .border(px(1.5))
            .border_color(t.warn),
        _ => div()
            .size(px(size * 0.8))
            .rounded_full()
            .border(px(size * 1.3 / 16.))
            .border_color(t.text_tertiary),
    };
    mark_box(size, mark).into_any_element()
}

/// An agent's face, or its agent's mark while it wears none.
pub fn agent_mark(agent: &AgentRow, size: f32, cx: &App) -> AnyElement {
    let Some((face, image)) = agent
        .face
        .as_ref()
        .and_then(|face| crate::faces::image(face, agent.group, cx).map(|image| (face, image)))
    else {
        return mark(&agent.provider, size);
    };
    match moving_face::moves(agent.group, cx) {
        true => moving_face::face(&agent.id, face, agent.group, image, size),
        false => gpui_kit::img(image)
            .size(px(size))
            .flex_none()
            .into_any_element(),
    }
}

/// Someone's initials on a colour of their own, picked by `key` when there is one, else
/// their name, as today's history and code host draw people without a picture.
pub fn initials(name: &str, key: &str, size: f32, cx: &App) -> Div {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let initials: String = match parts.as_slice() {
        [] => "?".into(),
        [only] => only.chars().take(2).collect::<String>().to_uppercase(),
        [first, .., last] => first
            .chars()
            .take(1)
            .chain(last.chars().take(1))
            .collect::<String>()
            .to_uppercase(),
    };
    let key = if key.is_empty() { name } else { key };
    let hue = key.chars().fold(0u32, |hash, character| {
        hash.wrapping_mul(31).wrapping_add(character as u32)
    }) % 360;
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(gpui_kit::hsla(hue as f32 / 360., 0.62, 0.64, 1.))
        .text_color(Tokens::get(cx).on_accent)
        .text_size(px(size * 0.47))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials)
}

/// A square that centres what it holds, so marks of any drawing line up.
pub fn mark_box(size: f32, child: impl IntoElement) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
}

/// One line of UI text that never wraps.
pub fn text(value: impl Into<SharedString>, size: f32, weight: FontWeight) -> Div {
    div()
        .text_size(crate::theme::ui_text(size))
        .font_weight(weight)
        .whitespace_nowrap()
        .child(value.into())
}

/// What a row says about itself after its title: 11px in the tertiary ink.
pub fn meta(value: impl Into<SharedString>, cx: &App) -> Div {
    text(value, 11., FontWeight::NORMAL).text_color(Tokens::get(cx).text_tertiary)
}

/// What the machine says: the mono face, with tabular figures and its icon glyphs.
pub fn mono(value: impl Into<SharedString>, size: f32, cx: &App) -> Div {
    let mut mono = text(value, size, FontWeight::NORMAL)
        .font_family(theme::MONO)
        .font_features(theme::tabular())
        .text_color(Tokens::get(cx).text_tertiary);
    mono.text_style().font_fallbacks =
        Some(FontFallbacks::from_fonts(vec![theme::MONO_ICONS.into()]));
    mono
}

/// YOLO's letters, each in the next colour of the rainbow the app paints it with.
pub fn rainbow(word: &str, size: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let colors = [t.accent, t.cmd, t.danger, t.warn, t.live];
    div().flex().children(
        word.chars()
            .zip(colors.iter().cycle())
            .map(|(letter, color)| {
                text(letter.to_string(), size, FontWeight::SEMIBOLD)
                    .letter_spacing(0.08)
                    .text_color(*color)
            }),
    )
}

/// A group's spaced capitals.
pub fn caps(value: &str, cx: &App) -> Div {
    text(value.to_uppercase(), 11., FontWeight::MEDIUM)
        .letter_spacing(0.2)
        .line_height(crate::theme::ui_text(16.))
        .text_color(Tokens::get(cx).text_tertiary)
}

/// A shortcut as the system draws key names.
pub fn kbd(keys: impl Into<SharedString>, cx: &App) -> Div {
    text(keys, 11., FontWeight::MEDIUM)
        .font_family(theme::KBD)
        .line_height(crate::theme::ui_text(11.))
        .text_color(Tokens::get(cx).text_tertiary)
}

/// A rounded frame over the shell, as the rails and panes are.
pub fn panel(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .when(!crate::see_through::clear(cx), |panel| panel.bg(t.surface))
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_3)
}

/// How something reachable stands, as its 7px dot says.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Pending,
    /// A hollow ring: there, but not connected or not signed in.
    Idle,
    Failed,
}

pub fn dot(tone: Tone, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let dot = div().size(px(7.)).rounded_full();
    match tone {
        Tone::Ok => dot.bg(t.live),
        Tone::Pending => dot.bg(t.warn),
        Tone::Idle => {
            let mut ring = dot.border_color(t.text_tertiary);
            let width = Some(px(1.5).into());
            ring.style().border_widths = gpui_kit::EdgesRefinement {
                top: width,
                right: width,
                bottom: width,
                left: width,
            };
            ring
        }
        Tone::Failed => dot.bg(t.danger),
    }
}

/// Forgets the driver's spots named from `prefix`, before a view draws them again; nothing in
/// other builds.
pub fn forget_spots(_prefix: impl FnOnce() -> String) {
    #[cfg(feature = "drive")]
    crate::drive::forget_spots(&_prefix());
}

/// Where the test bundle's driver can find this element to click it; nothing in other builds.
pub fn spot(_name: impl FnOnce() -> String) -> Option<AnyElement> {
    #[cfg(feature = "drive")]
    return Some(crate::drive::spot(_name()));
    #[cfg(not(feature = "drive"))]
    None
}
