//! What every list that picks with the keys shares: its actions and how they are bound, stepping
//! round its ends, a row chosen by hovering, and how a picker in a popover closes.

use std::rc::Rc;

use gpui_kit::component::popover::PopoverState;
use gpui_kit::{
    actions, App, Div, Entity, Hsla, KeyBinding, Stateful, StatefulInteractiveElement, Window,
};

use crate::theme::Tokens;

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
