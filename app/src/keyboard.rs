//! What holds the keyboard: a view just opened takes it once it shows, and when no view holds
//! it the one in front gets it back.

use std::cell::Cell;
use std::path::PathBuf;

use gpui_kit::{App, Context, FocusHandle, Subscription, Window};
use sikemux_view::layout::PaneKind;
use sikemux_view::prefs::WindowView;

use crate::tabs;
use crate::workbench::Workbench;

/// A view that takes the keyboard once it shows, after the person opened it.
#[derive(Clone, PartialEq)]
pub enum TakesKeys {
    Terminal,
    Git,
    File(PathBuf),
    Board,
}

impl TakesKeys {
    fn kind(&self) -> PaneKind {
        match self {
            Self::Terminal => PaneKind::Terminal,
            Self::Git => PaneKind::Git,
            Self::File(_) => PaneKind::Files,
            Self::Board => PaneKind::Board,
        }
    }
}

#[derive(PartialEq)]
pub(crate) enum Keys {
    /// Nothing with keys of its own holds the keyboard.
    Loose,
    /// A view the last frame did not draw holds it: one gone, or one not drawn yet.
    Gone,
    Held,
}

/// Where the window's keys go when nothing drawn holds them.
pub struct Keyboard {
    /// Holds the keyboard while nothing in the window does, so the window's shortcuts reach it.
    pub(crate) focus: FocusHandle,
    /// What the person just opened, which takes the keyboard once it shows.
    pending: Cell<Option<PendingKeys>>,
    /// Give the view in front the keyboard whenever nothing else holds it: once a view in
    /// focus stops being drawn, the window comes forward or the workbench itself is given it.
    _settle: [Subscription; 3],
    /// Catches the keys pressed while nothing holds the keyboard, and the shortcuts Settings
    /// answers first.
    _before_shortcuts: Subscription,
}

impl Keyboard {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let focus = cx.focus_handle();
        Self {
            pending: Cell::new(None),
            _settle: [
                cx.on_focus_lost(window, |bench, window, cx| bench.settle_keys(window, cx)),
                cx.observe_window_activation(window, |bench, window, cx| {
                    if window.is_window_active() {
                        bench.settle_keys(window, cx)
                    }
                }),
                cx.on_focus(&focus, window, |bench, window, cx| {
                    bench.settle_keys(window, cx)
                }),
            ],
            _before_shortcuts: {
                let this = cx.weak_entity();
                cx.intercept_keystrokes(move |event, window, cx| {
                    let _ = this.update(cx, |bench, cx| {
                        if !bench.catch_loose_keys(event, window, cx) {
                            crate::settings::before_shortcut(bench, &event.keystroke, window, cx)
                        }
                    });
                })
            },
            focus,
        }
    }
}

/// A view waiting to take the keyboard, and the tab in front when it was asked for.
struct PendingKeys {
    wants: TakesKeys,
    from: Option<String>,
}

impl Workbench {
    /// Gives the keyboard to `wants` once it shows. Only what the person opens asks for this,
    /// never what an agent opens beside them.
    /// The stage draws again at once, so a view already in front takes the keys too.
    pub fn take_keys_when_shown(&self, wants: TakesKeys, cx: &mut App) {
        let from = self.tab().map(|tab| tab.id.clone());
        self.keys.pending.set(Some(PendingKeys { wants, from }));
        self.parts.redraw_stage(cx);
    }

    /// Whether `shown`, now drawn, is what waits for the keyboard. It stops waiting once claimed.
    pub fn claims_keys(&self, shown: TakesKeys) -> bool {
        match self.keys.pending.take() {
            Some(pending) if pending.wants == shown => true,
            other => {
                self.keys.pending.set(other);
                false
            }
        }
    }

    /// What holds the keyboard, as the last frame drawn tells.
    pub(crate) fn keys(&self, window: &Window, cx: &App) -> Keys {
        let Some(held) = window.focused(cx) else {
            return Keys::Loose;
        };
        if held == self.keys.focus {
            return Keys::Loose;
        }
        let stack = window.context_stack();
        match stack.last() {
            None => Keys::Gone,
            Some(own) if stack.len() == 1 || own.contains(crate::keymap::SHELL) => Keys::Loose,
            Some(_) => Keys::Held,
        }
    }

    /// Gives the keyboard to the view in front, its terminal or its chat's draft, when nothing
    /// with keys of its own holds it: nothing, a view no longer drawn, the workbench itself or a
    /// button of its chrome. The workbench keeps it over Settings, a palette or a question, and
    /// while a view waits to take it.
    pub(crate) fn settle_keys(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keys(window, cx) == Keys::Held {
            return;
        }
        let held = window.focused(cx);
        let kept = self.settings.read(cx).is_open()
            || self.overlays.take_keys()
            || self.front_asks(cx).is_some()
            || self.waits_for_keys();
        if kept {
            // What holds them may be a field not drawn yet, which takes them once it is.
            if held.is_none() {
                self.keys.focus.focus(window, cx);
            }
            return;
        }
        match (self.window_view, &self.goals) {
            (WindowView::Goals, Some(goals)) => {
                if let Some(keys) = goals.read(cx).keys_go_to(&self.chats, cx) {
                    keys.focus(window, cx);
                }
            }
            _ => {
                if let Some(view) = self.focused_view() {
                    self.focus_view(&view, window, cx);
                }
            }
        }
        if window.focused(cx) == held {
            self.keys.focus.focus(window, cx);
        }
    }

    /// Presses a key again on the workbench when nothing in it held the keyboard, such as a view
    /// no longer drawn, so the window's shortcuts work wherever the focus was.
    pub(crate) fn catch_loose_keys(
        &mut self,
        event: &gpui_kit::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Below the window's own root, anything in focus has a context of its own.
        if event.context_stack.len() > 1 {
            return false;
        }
        self.keys.focus.focus(window, cx);
        if !self.keys.focus.is_focused(window) {
            return false;
        }
        cx.stop_propagation();
        let keystroke = event.keystroke.clone();
        window.defer(cx, move |window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        });
        true
    }

    pub(crate) fn waits_for_keys(&self) -> bool {
        let pending = self.keys.pending.take();
        let waits = pending.is_some();
        self.keys.pending.set(pending);
        waits
    }

    /// Stops waiting to give the keyboard away once another kind of tab comes to the front,
    /// so what failed to open never takes the keys later.
    pub(crate) fn settle_takes_keys(&self) {
        let Some(pending) = self.keys.pending.take() else {
            return;
        };
        let front = self.tab();
        let moved = front.map(|tab| &tab.id) != pending.from.as_ref();
        let shows = front
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| pending.wants.kind() == kind);
        if !moved || shows {
            self.keys.pending.set(Some(pending));
        }
    }

    /// Gives the workbench the keyboard if `held`, which is going away, has it.
    pub(crate) fn release_keys(&self, held: FocusHandle, cx: &mut Context<Self>) {
        let (window, focus) = (self.window, self.keys.focus.clone());
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                if held.is_focused(window) {
                    window.focus(&focus, cx)
                }
            });
        });
    }
}
