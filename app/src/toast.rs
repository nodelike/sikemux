//! Toasts as the Mac draws them: a ringed mark, one line, then the toast's button and its ×, on
//! a raised island at the bottom centre, as wide as what they say, in GPUI Kit's notifications.

use std::collections::HashMap;

use std::rc::Rc;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::theme::Theme as KitTheme;
use gpui_kit::component::{h_flex, WindowExt};
use gpui_kit::{
    canvas, div, px, relative, App, EntityId, Global, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, StyleRefinement, Styled, TextRun, WeakEntity, Window,
};

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, TEXT_XS};
use crate::ui::dialog::Answer;
use crate::ui::{self, Button, IconButton, Variant};
use crate::workbench::Workbench;

const TEXT: f32 = 12.5;
const MARK: f32 = 15.;
const GAP: Pixels = px(10.);
const LEFT: Pixels = px(12.);
const RIGHT: Pixels = px(6.);
const HEIGHT: Pixels = px(40.);
const CLOSE: f32 = 24.;
const BORDER: Pixels = px(1.);
/// Spare room after a measured line, so text that just fits is not cut short by rounding.
const SLACK: Pixels = px(4.);
const WIDEST: Pixels = px(540.);
const BUTTON_HEIGHT: f32 = 26.;
const BUTTON_PADDING: f32 = 11.;
/// What a window keeps clear on each side of its toasts.
const MARGIN: Pixels = px(16.);
/// The hover group of a toast's own row. The row takes the pointer from GPUI Kit's toast
/// beneath it, which keeps Kit's own × hidden in favour of the one drawn here.
const ROW: &str = "toast";

/// Puts `note` up from something that holds the workbench only weakly. It waits until the current
/// update ends, since callers may be running inside the workbench's own update.
pub fn show(bench: &WeakEntity<Workbench>, note: Notification, cx: &mut App) {
    let bench = bench.clone();
    cx.defer(move |cx| {
        let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
    });
}

pub fn info(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Info, text.into(), None)
}

pub fn success(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Success, text.into(), None)
}

pub fn error(text: impl Into<SharedString>) -> Notification {
    toast(Kind::Error, text.into(), None)
}

/// Says what was just done, with Undo to take it back.
pub fn undoable(
    text: impl Into<SharedString>,
    undo: impl Fn(&mut Window, &mut App) + 'static,
) -> Notification {
    let act = Action {
        label: t!("Undo"),
        variant: Variant::Outline,
        act: Rc::new(undo),
    };
    toast(Kind::Success, text.into(), Some(act)).autohide(true)
}

/// Says `text` with a filled button that does `act` and puts the toast away.
pub fn acting(
    text: impl Into<SharedString>,
    label: &'static str,
    act: impl Fn(&mut Window, &mut App) + 'static,
) -> Notification {
    let act = Action {
        label,
        variant: Variant::Primary,
        act: Rc::new(act),
    };
    toast(Kind::Info, text.into(), Some(act))
}

#[derive(Clone)]
struct Action {
    label: &'static str,
    variant: Variant,
    act: Answer,
}

#[derive(Clone, Copy)]
enum Kind {
    Info,
    Success,
    Error,
}

impl Kind {
    fn mark(self, t: &Tokens) -> impl IntoElement {
        let (glyph, tint) = match self {
            Kind::Info => (Icon::InfoCircle, t.accent),
            Kind::Success => (Icon::CheckCircle, t.live),
            Kind::Error => (Icon::AlertCircle, t.danger),
        };
        ui::icon(glyph, MARK).text_color(tint)
    }
}

/// Toasts saying the same words share one place, so a repeated one replaces the last.
struct Said;

fn toast(kind: Kind, text: SharedString, action: Option<Action>) -> Notification {
    let one_per_text = Notification::new().id1::<Said>(text.clone());
    one_per_text.content(move |note, window, cx| {
        let t = Tokens::get(cx);
        *note.style() = island(t);
        let rem = window.rem_size();
        let said = measure(&text, TEXT, rem, window, cx);
        let button = action.as_ref().map_or(px(0.), |action| {
            measure(&action.label.into(), TEXT_XS, rem, window, cx) + px(BUTTON_PADDING * 2.) + GAP
        });
        let (id, laid) = (cx.entity_id(), KitTheme::global(cx).notification.width);
        let close = cx.listener(|note, _, window, cx| note.dismiss(window, cx));
        h_flex()
            .id("toast")
            .occlude()
            .group(ROW)
            .relative()
            .w_full()
            .min_h(HEIGHT)
            .py(SPACE_1H)
            .pl(LEFT)
            .pr(RIGHT)
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
                    .group_hover(ROW, |this| this.whitespace_normal())
                    .text_size(theme::ui_text(TEXT).to_pixels(rem))
                    .line_height(relative(1.4))
                    .child(text.clone()),
            )
            .children(action.clone().map(|action| {
                let act = action.act.clone();
                Button::new("act", action.label, action.variant)
                    .sized(BUTTON_HEIGHT, BUTTON_PADDING, TEXT_XS)
                    .mouse_only(true)
                    .on_click(cx.listener(move |note, _, window, cx| {
                        act(window, cx);
                        note.dismiss(window, cx);
                    }))
            }))
            .child(div().w(BORDER).h(px(18.)).flex_none().bg(t.border))
            .child(
                IconButton::new("close", Icon::Close, t!("Dismiss"))
                    .height(CLOSE)
                    .width(CLOSE)
                    .glyph(12.)
                    .on_click(close),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let chrome = LEFT + px(MARK) + GAP * 3. + BORDER + px(CLOSE) + RIGHT;
                        fit(
                            id,
                            laid - bounds.size.width,
                            chrome + said.ceil() + SLACK + button,
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

fn measure(text: &SharedString, size: f32, rem: Pixels, window: &mut Window, cx: &App) -> Pixels {
    let run = TextRun {
        len: text.len(),
        font: gpui_kit::font(theme::UI),
        color: Tokens::get(cx).text_primary,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let size = theme::ui_text(size).to_pixels(rem);
    window
        .text_system()
        .shape_line(text.clone(), size, &[run], None)
        .width
}

/// The Mac's toast: a raised island with a hairline and a soft shadow. Its row draws all of
/// what is inside, so GPUI Kit's own padding is taken away.
fn island(t: &Tokens) -> StyleRefinement {
    StyleRefinement::default()
        .p_0()
        .gap(px(0.))
        .rounded(px(12.))
        .bg(t.surface_raised)
        .border_color(t.border)
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
        .max(LEFT + px(MARK) + RIGHT + BORDER * 2.);
    let toasts = &mut KitTheme::global_mut(cx).notification;
    if (toasts.width - width).abs() >= px(0.5) {
        toasts.width = width;
        window.refresh();
    }
}
