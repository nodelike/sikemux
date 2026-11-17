//! The tabs the person has had in front, in order, across projects and hosts, for the bar's
//! arrows, ⌘[ and ⌘] and the mouse's back and forward buttons to go through as a browser does
//! through pages.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, AnyElement, Context, IntoElement, ParentElement, Styled};
use sikemux_calls::call;
use sikemux_calls::layout::TabParams;

use crate::core::Target;
use crate::hosts::{HostChoice, Place};
use crate::icons::Icon;
use crate::swipe::Ends;
use crate::tabs::strip_layout;
use crate::theme::{SPACE_0, SPACE_2H};
use crate::track::Around;
use crate::ui::IconButton;
use crate::workbench::{actions, Strip, Workbench};

/// The most visits kept on either side of the one in front.
const KEPT: usize = 50;
/// How long a step waits for its tab to come to the front before the tab in front counts as
/// visited again.
const STEP_ARRIVES: Duration = Duration::from_secs(3);

/// A tab the person had in front: which host's core holds it, its strip there, and its id.
#[derive(Clone, PartialEq)]
struct Visit {
    place: Place,
    strip: Strip,
    tab: String,
}

#[derive(Default)]
pub struct Visits {
    back: Vec<Visit>,
    now: Option<Visit>,
    forward: Vec<Visit>,
    /// When a step back or forward asked for `now` to come to the front.
    stepping: Option<Instant>,
    /// The ask Back closed, which Forward opens again while its agent's tab stays in front.
    pub(crate) ask_forward: Option<u64>,
    /// Whether going back, and forward, would reach a tab still open.
    can_go_back: bool,
    can_go_forward: bool,
}

impl Workbench {
    /// Notes the tab in front as visited, and whether the arrows still lead anywhere.
    pub(crate) fn note_visit(&mut self) {
        self.arrive();
        self.visits.can_go_back = self.opened_ask.is_some() || self.next_visit(-1).is_some();
        self.visits.can_go_forward =
            self.visits.ask_forward.is_some() || self.next_visit(1).is_some();
    }

    /// The visit a step back, or forward for `by` of 1, would bring forward.
    fn next_visit(&self, by: isize) -> Option<&Visit> {
        let from = match by < 0 {
            true => &self.visits.back,
            false => &self.visits.forward,
        };
        from.iter().rev().find(|visit| self.leads_to(visit))
    }

    /// What a swipe over the stage goes through: the visit a step back sits on the left, so
    /// fingers moving right go back as in a browser, and the visit a step forward on the right.
    pub(crate) fn around_visit(&self) -> Around<String> {
        let place = self.place();
        let strip = self.strip_ref();
        let [previous, next] = [-1, 1].map(|by| self.next_visit(by));
        let here = |visit: Option<&Visit>| {
            visit
                .filter(|visit| visit.place == place && strip == Some(visit.strip.borrowed()))
                .map(|visit| visit.tab.clone())
        };
        Around {
            ends: Ends {
                previous: previous.is_some(),
                next: next.is_some(),
            },
            previous: here(previous),
            next: here(next),
        }
    }

    fn arrive(&mut self) {
        let shown = (self.visits.now.as_ref()).is_some_and(|now| self.in_front(now));
        if let Some(asked) = self.visits.stepping {
            if !shown && asked.elapsed() < STEP_ARRIVES {
                return;
            }
            self.visits.stepping = None;
        }
        if shown {
            return;
        }
        let (Some(strip), Some(tab)) = (self.strip(), self.tab()) else {
            return;
        };
        let visit = Visit {
            place: self.place(),
            strip,
            tab: tab.id.clone(),
        };
        let visits = &mut self.visits;
        visits.ask_forward = None;
        if let Some(was) = visits.now.replace(visit) {
            visits.back.push(was);
            if visits.back.len() > KEPT {
                visits.back.remove(0);
            }
        }
        visits.forward.clear();
    }

    fn in_front(&self, visit: &Visit) -> bool {
        let chosen = self
            .this_view()
            .and_then(|view| view.layout.chosen_host.as_deref());
        let on_host = match &visit.place {
            Place::This => chosen.is_none(),
            Place::Account(host) => chosen == Some(host.as_str()),
        };
        on_host
            && self.strip_ref() == Some(visit.strip.borrowed())
            && self
                .front_tab(&visit.strip)
                .is_some_and(|tab| tab.id == visit.tab)
    }

    /// Whether stepping to `visit` would bring forward a tab still open, other than the one
    /// already in front.
    fn leads_to(&self, visit: &Visit) -> bool {
        self.visits.now.as_ref() != Some(visit) && self.is_open(visit)
    }

    fn is_open(&self, visit: &Visit) -> bool {
        self.view_at(&visit.place)
            .and_then(|view| strip_layout(&view.layout, &visit.strip))
            .is_some_and(|strip| strip.tabs.iter().any(|tab| tab.id == visit.tab))
            && !self.closing(&visit.tab)
    }

    /// Brings forward the tab visited before the one in front, or after it for `by` of 1,
    /// passing over tabs closed since.
    pub(crate) fn step_visit(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.step_ask(by, cx) {
            self.note_visit();
            return;
        }
        loop {
            let visits = &mut self.visits;
            let from = match by < 0 {
                true => &mut visits.back,
                false => &mut visits.forward,
            };
            let Some(visit) = from.pop() else {
                return;
            };
            if !self.leads_to(&visit) {
                continue;
            }
            if let Place::Account(host) = &visit.place {
                if self.place() != visit.place {
                    self.choose_host(HostChoice::Account(host.clone()), cx);
                }
            }
            Target::new(&self.asks, visit.place.clone()).call_typed::<call::ActivateTab>(
                TabParams::new(visit.strip.at(), visit.tab.as_str()),
            );
            let visits = &mut self.visits;
            let to = match by < 0 {
                true => &mut visits.forward,
                false => &mut visits.back,
            };
            to.extend(visits.now.replace(visit));
            visits.stepping = Some(Instant::now());
            self.note_visit();
            cx.notify();
            return;
        }
    }
}

/// The bar's back and forward buttons.
pub fn arrows(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let back = match crate::keymap::hint(actions::GoBack::ID, cx) {
        Some(keys) => t!("Back — {keys}", keys = keys),
        None => t!("Back").to_owned(),
    };
    let forward = match crate::keymap::hint(actions::GoForward::ID, cx) {
        Some(keys) => t!("Forward — {keys}", keys = keys),
        None => t!("Forward").to_owned(),
    };
    div()
        .flex()
        .flex_none()
        .gap(SPACE_0)
        .when(!cfg!(target_os = "macos"), |arrows| arrows.pl(SPACE_2H))
        .child(
            IconButton::new("go-back", Icon::Chevron, back)
                .glyph(11.)
                .quiet(false)
                .turned(std::f32::consts::PI)
                .disabled(!bench.visits.can_go_back)
                .on_click(|_, window, cx| crate::keymap::dispatch(actions::GoBack::ID, window, cx)),
        )
        .child(
            IconButton::new("go-forward", Icon::Chevron, forward)
                .glyph(11.)
                .quiet(false)
                .disabled(!bench.visits.can_go_forward)
                .on_click(|_, window, cx| {
                    crate::keymap::dispatch(actions::GoForward::ID, window, cx)
                }),
        )
        .into_any_element()
}
