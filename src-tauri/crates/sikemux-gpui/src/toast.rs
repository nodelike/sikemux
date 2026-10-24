//! Toasts as the Mac draws them: a mark and one line on the island at the bottom centre, as wide
//! as what they say, in GPUI Kit's notifications.

use std::collections::HashMap;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::theme::Theme as KitTheme;
use gpui_kit::component::{h_flex, Icon, IconName, WindowExt};
use gpui_kit::{
    canvas, div, px, relative, App, EntityId, Global, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, StyleRefinement, Styled, TextRun, Window,
};

use crate::theme::{self, Tokens};

const TEXT: f32 = 12.5;
const MARK: Pixels = px(14.);
const GAP: Pixels = px(9.);
const LEFT: Pixels = px(10.);
/// Room on the right for the × GPUI Kit shows over a toast's top right while it is pointed at.
const RIGHT: Pixels = px(24.);
const BORDER: Pixels = px(1.);
const WIDEST: Pixels = px(520.);
/// What a window keeps clear on each side of its toasts.
const MARGIN: Pixels = px(16.);

pub fn info(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Info, text.into())
}

pub fn success(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Success, text.into())
}

pub fn error(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Error, text.into())
}

#[derive(Clone, Copy)]
enum Kind {
    Info,
    Success,
    Error,
}

impl Kind {
    fn mark(self, t: &Tokens) -> Icon {
        let (name, tint) = match self {
            Kind::Info => (IconName::Info, t.accent),
            Kind::Success => (IconName::CircleCheck, t.live),
            Kind::Error => (IconName::CircleX, t.danger),
        };
        Icon::new(name).size(MARK).flex_none().text_color(tint)
    }
}

fn toast(kind: Kind, text: SharedString) -> Notification {
    Notification::new().content(move |note, window, cx| {
        let t = Tokens::get(cx);
        *note.style() = island(t);
        let size = theme::ui_text(TEXT).to_pixels(window.rem_size());
        let run = TextRun {
            len: text.len(),
            font: gpui_kit::font(theme::UI),
            color: t.text_primary,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let said = window
            .text_system()
            .shape_line(text.clone(), size, &[run], None)
            .width;
        let (id, laid) = (cx.entity_id(), KitTheme::global(cx).notification.width);
        h_flex()
            .relative()
            .gap(GAP)
            .text_color(t.text_primary)
            .child(kind.mark(t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .group_hover("", |this| this.whitespace_normal())
                    .text_size(size)
                    .line_height(relative(1.4))
                    .child(text.clone()),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        fit(
                            id,
                            laid - bounds.size.width,
                            MARK + GAP + said.ceil(),
                            window,
                            cx,
                        )
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    })
}

/// The Mac's toast: 34 points high at the least, rounder than a float, on the theme's darkest
/// ground with a faint hairline.
fn island(t: &Tokens) -> StyleRefinement {
    StyleRefinement::default()
        .min_h(px(34.))
        .py(px(6.))
        .pl(LEFT)
        .pr(RIGHT)
        .gap(GAP)
        .rounded(px(13.))
        .bg(t.surface_sunken)
        .border_color(t.surface_hover)
}

/// For each toast on screen, the room around its line and what the line needs, so the stack is as
/// wide as the widest.
#[derive(Default)]
struct Wanted(HashMap<EntityId, (Pixels, Pixels)>);

impl Global for Wanted {}

/// Records the room `around` toast `id`'s line, and the width its line `needs`, then makes every
/// toast as wide as the widest still shown wants.
fn fit(id: EntityId, around: Pixels, needs: Pixels, window: &mut Window, cx: &mut App) {
    let shown = window.notifications(cx);
    let wanted = &mut cx.default_global::<Wanted>().0;
    wanted.retain(|kept, _| shown.iter().any(|note| note.entity_id() == *kept));
    // A toast behind the front one is drawn narrower, so its least room is the room it has.
    let entry = wanted.entry(id).or_insert((around, needs));
    *entry = (entry.0.min(around), needs);
    let room = window.viewport_size().width - MARGIN * 2.;
    let widest = wanted
        .values()
        .map(|(around, needs)| *around + *needs)
        .fold(px(0.), Pixels::max);
    let width = widest
        .min(WIDEST)
        .min(room)
        .max(LEFT + MARK + RIGHT + BORDER * 2.);
    let toasts = &mut KitTheme::global_mut(cx).notification;
    if (toasts.width - width).abs() >= px(0.5) {
        toasts.width = width;
        window.refresh();
    }
}
