//! Rows the arrow keys walk while their list holds the keys, so a rail works without a pointer.
//! Enter or Space clicks the row the keys are on and Shift-F10 opens its menu, by pressing the
//! pointer where it was drawn, as the file tree's menu key does; Escape gives the keys back to
//! the view in front. Each row is named for screen readers, which follow the keyed one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    canvas, div, point, px, Action, AnyElement, App, Bounds, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement,
    Pixels, PlatformInput, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Window,
};

use crate::theme::{Tokens, RADIUS_2};

gpui::actions!(
    keyed_rows,
    [Previous, Next, First, Last, Open, ShowMenu, Leave]
);

pub const CONTEXT: &str = "KeyedRows";

pub fn install(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", Previous, context),
        KeyBinding::new("down", Next, context),
        KeyBinding::new("home", First, context),
        KeyBinding::new("end", Last, context),
        KeyBinding::new("enter", Open, context),
        KeyBinding::new("space", Open, context),
        KeyBinding::new("shift-f10", ShowMenu, context),
        KeyBinding::new("escape", Leave, context),
    ]);
}

pub struct KeyedRows {
    pub focus: FocusHandle,
    /// The scrolled list the rows sit in, which brings the keyed row into view.
    pub scroll: ScrollHandle,
    keyed: Option<SharedString>,
    /// The rows as last drawn, in order.
    order: RefCell<Vec<SharedString>>,
    /// The list held the keys when it was last drawn.
    holds: Cell<bool>,
    /// Where the keyed row was last drawn.
    keyed_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The keys moved since the keyed row was last drawn, so it is scrolled into view.
    moved: Rc<Cell<bool>>,
}

impl KeyedRows {
    pub fn new(focus: FocusHandle, scroll: ScrollHandle) -> Self {
        Self {
            focus,
            scroll,
            keyed: None,
            order: RefCell::default(),
            holds: Cell::new(false),
            keyed_at: Rc::default(),
            moved: Rc::default(),
        }
    }

    /// Starts drawing the list again, which names its rows as they are drawn.
    pub fn begin(&self, window: &Window) {
        self.order.borrow_mut().clear();
        self.holds.set(self.focus.is_focused(window));
    }

    /// Gives the list the keys, on the row they were last on while it is still drawn, else the
    /// first.
    pub fn take_keys(&mut self, window: &mut Window, cx: &mut App) {
        let order = self.order.borrow();
        if self
            .keyed
            .as_ref()
            .is_none_or(|keyed| !order.contains(keyed))
        {
            self.keyed = order.first().cloned();
        }
        drop(order);
        self.moved.set(true);
        window.focus(&self.focus, cx);
    }

    /// `row` as the list draws it: named `label` for screen readers, and ringed while the keys
    /// are on it.
    pub fn row(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        row: impl IntoElement,
        cx: &App,
    ) -> AnyElement {
        let id = id.into();
        let keyed = self.keyed.as_ref() == Some(&id);
        self.order.borrow_mut().push(id.clone());
        let ringed = keyed && self.holds.get();
        let wrapper = div()
            .id(SharedString::from(format!("keyed:{id}")))
            .role(Role::ListItem)
            .aria_label(label)
            .relative()
            .child(row);
        if !ringed {
            return wrapper.into_any_element();
        }
        let (at, moved, scroll) = (
            self.keyed_at.clone(),
            self.moved.clone(),
            self.scroll.clone(),
        );
        wrapper
            .aria_selected(true)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(Tokens::get(cx).border_selected),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        at.set(Some(bounds));
                        if moved.take() && reveal(&scroll, bounds) {
                            crate::ui::redraw_after_paint(window, cx);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }

    /// Acts on a key, and whether the list needs drawing again.
    pub fn key(&mut self, key: &dyn Action, window: &mut Window, cx: &mut App) -> bool {
        let order = self.order.borrow().clone();
        if order.is_empty() {
            return false;
        }
        let at = (self.keyed.as_ref()).and_then(|keyed| order.iter().position(|id| id == keyed));
        let last = order.len() - 1;
        let to = if key.as_any().is::<Previous>() {
            at.map_or(0, |at| at.saturating_sub(1))
        } else if key.as_any().is::<Next>() {
            at.map_or(0, |at| (at + 1).min(last))
        } else if key.as_any().is::<First>() {
            0
        } else if key.as_any().is::<Last>() {
            last
        } else {
            let button = if key.as_any().is::<ShowMenu>() {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            if at.is_some() {
                press(self.keyed_at.get(), button, window, cx);
            }
            return false;
        };
        self.keyed = Some(order[to].clone());
        self.moved.set(true);
        true
    }
}

/// Scrolls the list so `row` shows whole, and whether it had to.
fn reveal(scroll: &ScrollHandle, row: Bounds<Pixels>) -> bool {
    let view = scroll.bounds();
    let by = if row.top() < view.top() {
        view.top() - row.top()
    } else if row.bottom() > view.bottom() {
        view.bottom() - row.bottom()
    } else {
        return false;
    };
    let offset = scroll.offset();
    scroll.set_offset(point(offset.x, offset.y + by));
    true
}

/// Presses `button` once over the middle of `bounds`, after the key is handled.
fn press(bounds: Option<Bounds<Pixels>>, button: MouseButton, window: &mut Window, cx: &mut App) {
    let Some(bounds) = bounds else {
        return;
    };
    let position = point(
        bounds.center().x,
        bounds.top() + (bounds.size.height / 2.).min(px(14.)),
    );
    window.defer(cx, move |window, cx| {
        let modifiers = Modifiers::default();
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    });
}
