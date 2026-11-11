//! What every list that picks with the keys shares: its actions and how they are bound, stepping
//! round its ends, a row chosen by hovering, and how a picker in a popover closes.

use std::rc::Rc;

use gpui_kit::component::popover::PopoverState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, App, Div, ElementId, Entity, Hsla, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Stateful, StatefulInteractiveElement, Styled, Window,
};

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_2, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS_H};

actions!(
    list_picker,
    [Next, Previous, PageDown, PageUp, Choose, Close]
);

/// What Tab does in a list.
pub enum Tab {
    Steps,
    Chooses,
    Nothing,
}

/// Binds a list's keys while `context` has them, with Tab doing what `tab` says.
pub fn bind(context: &'static str, tab: Tab, cx: &mut App) {
    let context = Some(context);
    cx.bind_keys([
        KeyBinding::new("down", Next, context),
        KeyBinding::new("up", Previous, context),
        KeyBinding::new("enter", Choose, context),
        KeyBinding::new("escape", Close, context),
    ]);
    match tab {
        Tab::Steps => cx.bind_keys([
            KeyBinding::new("tab", Next, context),
            KeyBinding::new("shift-tab", Previous, context),
        ]),
        Tab::Chooses => cx.bind_keys([KeyBinding::new("tab", Choose, context)]),
        Tab::Nothing => {}
    }
}

/// The row `by` on from `at` in a list of `count`, round its ends.
pub fn stepped(at: usize, by: isize, count: usize) -> usize {
    (at as isize + by).rem_euclid(count as isize) as usize
}

/// Has `row`, at `at` in its list, become the chosen one when the pointer comes over it.
pub fn hover_chooses<V: 'static>(
    row: Stateful<Div>,
    at: usize,
    list: &Entity<V>,
    choose: fn(&mut V, usize),
) -> Stateful<Div> {
    let list = list.clone();
    row.on_hover(move |over: &bool, _, cx| {
        if *over {
            list.update(cx, |list, cx| {
                choose(list, at);
                cx.notify();
            });
        }
    })
}

/// Closes the popover a picker is in.
pub type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

/// How a popover's content closes it.
pub fn dismiss(popover: Entity<PopoverState>) -> Dismiss {
    Rc::new(move |window, cx| {
        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
    })
}

/// How dark the window goes under a picker over it.
pub const BACKDROP: f32 = 0.68;
/// How far down the window a picker over it opens, as a share of its height.
pub const BACKDROP_TOP: f32 = 0.13;

/// The ground under a picker over the window.
pub fn backdrop_color(t: &Tokens) -> Hsla {
    Hsla {
        a: BACKDROP,
        ..t.surface_sunken
    }
}

/// The floating panel a picker in a popover is drawn on, `width` wide.
pub fn popup(width: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .w(px(width))
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .p(px(crate::ui::PANEL_INSET))
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_3)
        .shadow(t.shadow_float())
}

/// The quiet name over a group of rows.
pub fn heading(words: impl IntoElement, cx: &App) -> Div {
    div()
        .px(SPACE_2)
        .pt(SPACE_2)
        .pb(SPACE_0)
        .text_size(theme::ui_text(TEXT_3XS_H))
        .text_color(Tokens::get(cx).text_tertiary)
        .child(words)
}

/// A row of a picker, `id`, at `at` in its `list`, lit while it is the `chosen` one, which the
/// pointer coming over it makes it.
pub fn row<V: 'static>(
    id: impl Into<ElementId>,
    at: usize,
    chosen: bool,
    list: &Entity<V>,
    choose: fn(&mut V, usize),
    cx: &App,
) -> Stateful<Div> {
    let t = Tokens::get(cx);
    let row = div()
        .id(id)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .h(px(28.))
        .px(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .when(chosen, |row| row.bg(t.surface_hover));
    hover_chooses(row, at, list, choose)
}

/// A row's name, brighter on the one in use now.
pub fn label(name: impl IntoElement, current: bool, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .text_size(theme::ui_text(TEXT_XS_H))
        .text_color(if current {
            t.text_primary
        } else {
            t.text_secondary
        })
        .child(name)
}

/// What a row says after its name, such as where it is, cut short to fit.
pub fn detail(words: impl IntoElement, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(theme::ui_text(TEXT_2XS_H))
        .text_color(Tokens::get(cx).text_tertiary)
        .child(words)
}

/// The tick at the end of the row in use now.
pub fn check(cx: &App) -> impl IntoElement {
    crate::ui::icon(Icon::Check, 12.).text_color(Tokens::get(cx).text_secondary)
}
