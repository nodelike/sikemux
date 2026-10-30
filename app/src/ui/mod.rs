//! The shared component set. Screens are built from these; one a screen needs
//! that is missing belongs here, once.

pub mod clock;
mod controls;
pub(crate) mod dialog;
mod dock;
mod file_card;
pub mod find_bar;
mod floats;
pub mod grain;
mod lists;
pub mod moving_face;
mod pane;
pub mod path;
mod select;
pub mod selectable;
pub mod shimmer;
pub mod sign_in;
mod state_mark;
pub mod time;

pub use controls::{Button, Field, IconButton, Segmented, Toggle, Variant};
pub use dialog::{Cheatsheet, Choice, Choices, Confirm, Prompt, Section};
pub use dock::{shell as dock_shell, DockMenu, DockRow, DockScope};
pub use file_card::{file_head, Opens};
pub use find_bar::FindBar;
pub use floats::{
    card_tooltip, float, menu, menu_button, split_button, tooltip, MenuRow, SplitMenu, FLOAT_INSET,
    PANEL_INSET,
};
pub use lists::{
    close_button, gives_way, hover_actions, page_tab, page_tabs, Group, Row, Tab, ROW,
};
pub use pane::{pane, plain_pane, PaneHeader, AGENT_INSET};
pub use select::{marked_select, select, Pick};
pub use state_mark::status_mark;

use std::f32::consts::TAU;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, img, point, px, svg, AnyElement, App, Context, Div, ElementId, FontFallbacks,
    FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement, PathBuilder, Render,
    SharedString, Styled, StyledText, Svg, Window,
};

use sikemux_wire::protocol::{AgentFace, AgentGroup};

use crate::icons::{self, Icon};
use crate::theme::{self, Tokens, TEXT_2XS, TEXT_LG};

/// What a menu calls showing a file in the system's file manager.
pub const REVEAL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Show in Files"
};
pub const SHOW_IN_FILES: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in Files"
};

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

/// A run's mark as `status_of_run` draws it, except that a running one turns a quarter of its
/// ring round on the shared clock; `key` keeps each turning mark apart.
pub fn run_mark(key: impl Into<SharedString>, outcome: &str, size: f32, cx: &App) -> AnyElement {
    if outcome != "running" {
        return status_of_run(outcome, size, cx);
    }
    let t = Tokens::get(cx);
    let spin = Spin {
        size: size * 0.82,
        head: t.warn,
        track: t.warn.opacity(0.3),
    };
    mark_box(size, clock::Slot::new(ElementId::Name(key.into()), spin)).into_any_element()
}

struct Spin {
    size: f32,
    head: Hsla,
    track: Hsla,
}

impl Render for Spin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let turn = match !cx.reduce_motion() && clock::tick(window, cx) {
            true => (clock::seconds() / 0.9).fract(),
            false => 0.,
        };
        arcs(
            Some((turn, turn + 0.25, self.head)),
            self.size,
            1.5,
            self.track,
        )
    }
}

/// The face agent `id` wears, or the mark of its kind `provider` while it wears none.
pub fn agent_mark(
    id: &str,
    provider: &str,
    face: Option<&AgentFace>,
    group: AgentGroup,
    size: f32,
    cx: &App,
) -> AnyElement {
    let Some((face, image)) =
        face.and_then(|face| crate::faces::image(face, group, cx).map(|image| (face, image)))
    else {
        return mark(provider, size);
    };
    match moving_face::moves(group, cx) {
        true => moving_face::face(id, face, group, image, size),
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
    text(value, TEXT_2XS, FontWeight::NORMAL).text_color(Tokens::get(cx).text_tertiary)
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
    text(value.to_uppercase(), TEXT_2XS, FontWeight::MEDIUM)
        .letter_spacing(0.2)
        .line_height(crate::theme::ui_text(TEXT_LG))
        .text_color(Tokens::get(cx).text_tertiary)
}

/// `value` with the letters a search for `query` found in it in the primary ink at semibold,
/// as every list that narrows to what is typed shows why a row is there.
pub fn found(value: impl Into<SharedString>, query: &str, cx: &App) -> StyledText {
    found_with(value, query, found_mark(cx))
}

/// How [`found`] marks the letters a search found.
pub fn found_mark(cx: &App) -> HighlightStyle {
    HighlightStyle {
        color: Some(Tokens::get(cx).text_primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..HighlightStyle::default()
    }
}

/// As [`found`], with the mark already looked up.
pub fn found_with(value: impl Into<SharedString>, query: &str, mark: HighlightStyle) -> StyledText {
    let value = value.into();
    let marks: Vec<_> = crate::fuzzy::matched(query, &value)
        .into_iter()
        .map(|range| (range, mark))
        .collect();
    StyledText::new(value).with_highlights(marks)
}

/// A shortcut as the system draws key names.
pub fn kbd(keys: impl Into<SharedString>, cx: &App) -> Div {
    text(keys, TEXT_2XS, FontWeight::MEDIUM)
        .font_family(theme::KBD)
        .line_height(crate::theme::ui_text(TEXT_2XS))
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

/// A ring `size` across, filled clockwise from the top in its tint as far as the percent,
/// over a full track; only the track without a fill.
pub fn ring(filled: Option<(f64, Hsla)>, size: f32, stroke: f32, track: Hsla) -> impl IntoElement {
    let filled = filled.and_then(|(percent, tint)| {
        let turn = (percent / 100.).clamp(0., 0.999) as f32;
        (turn > 0.005).then_some((0., turn, tint))
    });
    arcs(filled, size, stroke, track)
}

/// A ring `size` across: a full track, with an arc over it in its tint from one turn to the
/// other, measured clockwise from the top.
fn arcs(over: Option<(f32, f32, Hsla)>, size: f32, stroke: f32, track: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let radius = (size - stroke) / 2.;
            let centre = bounds.center();
            let at = |turn: f32| {
                centre
                    + point(
                        px(radius * (turn * TAU).sin()),
                        px(-radius * (turn * TAU).cos()),
                    )
            };
            let arc = |from: f32, to: f32| {
                let mut path = PathBuilder::stroke(px(stroke));
                path.move_to(at(from));
                path.arc_to(
                    point(px(radius), px(radius)),
                    px(0.),
                    to - from > 0.5,
                    true,
                    at(to),
                );
                path.build().ok()
            };
            for (from, to) in [(0., 0.5), (0.5, 1.)] {
                if let Some(path) = arc(from, to) {
                    window.paint_path(path, track);
                }
            }
            if let Some((from, to, tint)) = over {
                if let Some(path) = arc(from, to) {
                    window.paint_path(path, tint);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
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
