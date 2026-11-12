//! The host picker a new chat's host opens: the hosts holding a copy of its project, which the
//! chat can move to, then the account's other hosts, which a copy of the project goes to first.

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window,
};

use crate::copies::{Folded, HostCopy};
use crate::hosts::{Copying, Place};
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, TEXT_2XS, TEXT_3XS_H};
use crate::ui;
use crate::ui::list_picker::{self, Choose, Close, Dismiss, Next, Previous};
use crate::ui::path::{middle, tilde};
use crate::workbench::Workbench;

const CONTEXT: &str = "HostPicker";
const PATH_MOST: usize = 30;

pub fn install(cx: &mut App) {
    list_picker::bind(CONTEXT, list_picker::Tab::Steps, cx);
}

#[derive(Clone)]
enum Choice {
    Has(HostCopy),
    CopyTo {
        host: String,
        name: SharedString,
        reached: bool,
    },
}

/// The open picker, for the chat `agent` in the copy `here` of `folded`.
pub struct HostPicker {
    bench: WeakEntity<Workbench>,
    agent: String,
    here: HostCopy,
    folded: Option<Folded>,
    chosen: usize,
    focus: FocusHandle,
    close: Dismiss,
}

impl Focusable for HostPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl HostPicker {
    fn choices(&self, cx: &App) -> Vec<Choice> {
        let Some(folded) = &self.folded else {
            return vec![Choice::Has(self.here.clone())];
        };
        let held = folded.copies.iter().cloned().map(Choice::Has);
        let targets = self
            .bench
            .upgrade()
            .map(|bench| bench.read(cx).copy_targets(folded))
            .unwrap_or_default();
        let targets = targets
            .into_iter()
            .map(|(host, name, reached)| Choice::CopyTo {
                host,
                name,
                reached,
            });
        held.chain(targets).collect()
    }

    fn pickable(&self, choice: &Choice) -> bool {
        match choice {
            Choice::Has(copy) => !copy.away && copy.place != self.here.place,
            Choice::CopyTo { reached, .. } => *reached,
        }
    }

    fn pick(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pickable(&choice) {
            return;
        }
        (self.close)(window, cx);
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let identity = self
            .folded
            .as_ref()
            .and_then(|folded| folded.identity.clone());
        let agent = self.agent.clone();
        bench.update(cx, |bench, cx| match choice {
            Choice::Has(copy) => bench.move_chat_to_copy(&agent, &copy, identity.as_deref(), cx),
            Choice::CopyTo { host, name, .. } => {
                let copying = Copying {
                    from: self.here.clone(),
                    identity,
                    host,
                    label: name,
                    uncommitted: !bench.rails.copy_leaves_uncommitted,
                    chat: Some(agent),
                };
                bench.copy_to(copying, cx);
            }
        });
    }

    /// How long the host at `place` has been out of reach: "away 2 h", or "away" before this
    /// window has seen it go.
    fn away(&self, place: &Place, cx: &App) -> SharedString {
        let since = match place {
            Place::Account(key) => self.bench.upgrade().and_then(|bench| {
                bench
                    .read(cx)
                    .feeds
                    .host(key)
                    .and_then(|feed| feed.away_since)
            }),
            Place::This => None,
        };
        since.map_or_else(
            || t!("away").into(),
            |since| t!("away {time}", time = crate::hosts::lasted(since)).into(),
        )
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.choices(cx).len();
        if count > 0 {
            self.chosen = list_picker::stepped(self.chosen, by, count);
        }
        cx.notify();
    }

    fn row(&self, at: usize, choice: &Choice, this: &Entity<Self>, cx: &App) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let home = |place: &Place| {
            self.bench
                .upgrade()
                .and_then(|bench| bench.read(cx).home_at(place))
        };
        let row = list_picker::row(
            ("host-choice", at),
            at,
            at == self.chosen && self.pickable(choice),
            this,
            |picker, at| picker.chosen = at,
            cx,
        )
        .when(!self.pickable(choice), |row| row.cursor_default());
        let row = match choice {
            Choice::Has(copy) => {
                let current = copy.place == self.here.place;
                let glyph = match copy.place {
                    Place::This => ui::icon(Icon::Laptop, 12.).text_color(t.text_tertiary),
                    Place::Account(_) => ui::icon(Icon::Server, 12.).text_color(t.host),
                };
                row.when(copy.away, |row| row.opacity(0.45))
                    .child(glyph)
                    .child(list_picker::label(copy.host.clone(), current, cx))
                    .child(list_picker::detail(
                        middle(
                            &tilde(&copy.path.to_string_lossy(), home(&copy.place).as_deref()),
                            PATH_MOST,
                        ),
                        cx,
                    ))
                    .when(copy.away, |row| {
                        row.child(quiet(self.away(&copy.place, cx), false, cx))
                    })
                    .when_some(
                        copy.git.as_ref().filter(|_| !copy.away && !current),
                        |row, git| {
                            row.child(
                                ui::mono(git.branch.clone(), TEXT_3XS_H, cx)
                                    .flex_none()
                                    .max_w(px(96.))
                                    .truncate()
                                    .text_color(t.text_tertiary),
                            )
                        },
                    )
                    .when(current, |row| row.child(list_picker::check(cx)))
            }
            Choice::CopyTo {
                host,
                name,
                reached,
            } => {
                let identity = self
                    .folded
                    .as_ref()
                    .and_then(|folded| folded.identity.clone());
                let remote = identity.filter(|identity| identity.contains('/'));
                let (from, verb) = match &remote {
                    Some(remote) => (
                        remote
                            .split_once('/')
                            .map_or(remote.as_str(), |(_, repo)| repo)
                            .to_owned(),
                        t!("Clone"),
                    ),
                    None => (t!("from {host}", host = self.here.host), t!("Copy")),
                };
                let lit = at == self.chosen;
                row.when(!reached, |row| row.opacity(0.45))
                    .child(ui::icon(Icon::Server, 12.).text_color(t.host))
                    .child(list_picker::label(name.clone(), false, cx))
                    .child(list_picker::detail(from, cx))
                    .child(match reached {
                        true => div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(SPACE_1)
                            .child(ui::icon(Icon::Download, 11.))
                            .child(quiet(verb.into(), lit, cx))
                            .text_color(if lit { t.accent } else { t.text_tertiary })
                            .into_any_element(),
                        false => quiet(self.away(&Place::Account(host.clone()), cx), false, cx)
                            .into_any_element(),
                    })
            }
        };
        let (this, picked) = (this.clone(), choice.clone());
        row.on_click(move |_, window, cx| {
            let picked = picked.clone();
            this.update(cx, |picker, cx| picker.pick(picked, window, cx))
        })
        .into_any_element()
    }
}

/// A short word at the end of a row, in the accent on the row the keys are on when it is what
/// picking does.
fn quiet(words: SharedString, lit: bool, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx);
    ui::text(words, TEXT_2XS, gpui_kit::FontWeight::NORMAL)
        .flex_none()
        .text_color(if lit { t.accent } else { t.text_tertiary })
}

impl Render for HostPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let choices = self.choices(cx);
        let this = cx.entity();
        let name = self
            .folded
            .as_ref()
            .map_or_else(String::new, |folded| folded.name.clone());
        let copies = choices
            .iter()
            .take_while(|choice| matches!(choice, Choice::Has(_)))
            .count();
        let held = choices[..copies]
            .iter()
            .enumerate()
            .map(|(at, choice)| self.row(at, choice, &this, cx));
        let targets = choices[copies..]
            .iter()
            .enumerate()
            .map(|(at, choice)| self.row(copies + at, choice, &this, cx));
        list_picker::popup(312., cx)
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|picker, _: &Previous, _, cx| picker.step(-1, cx)))
            .on_action(cx.listener(|picker, _: &Next, _, cx| picker.step(1, cx)))
            .on_action(cx.listener(|picker, _: &Choose, window, cx| {
                if let Some(choice) = picker.choices(cx).get(picker.chosen).cloned() {
                    picker.pick(choice, window, cx);
                }
            }))
            .on_action(cx.listener(|picker, _: &Close, window, cx| (picker.close)(window, cx)))
            .gap_0()
            .child(list_picker::heading(&t!("Has {name}", name = name), cx))
            .children(held)
            .when(copies < choices.len(), |popup| {
                popup
                    .child(list_picker::heading(t!("Copy to"), cx))
                    .children(targets)
            })
    }
}

/// Wraps `trigger`, the host over the new chat `agent`, in the picker that moves the chat to
/// another host's copy of its project, `project` on the host at `place`.
pub fn around(
    trigger: impl Selectable + IntoElement + 'static,
    agent: &str,
    here: HostCopy,
    this: &Entity<Workbench>,
) -> Popover {
    let key = SharedString::from(format!("host-picker-{agent}"));
    let (agent, this) = (agent.to_owned(), this.clone());
    Popover::new(key.clone())
        .anchor(Anchor::BottomLeft)
        .offset(px(6.))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let close = list_picker::dismiss(cx.entity());
            let (agent, here, bench) = (agent.clone(), here.clone(), this.clone());
            window.use_keyed_state(key.clone(), cx, move |window, cx| {
                let folded = (bench.read(cx).folded().into_iter()).find(|folded| {
                    (folded.copies.iter())
                        .any(|copy| copy.place == here.place && copy.project == here.project)
                });
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                HostPicker {
                    bench: bench.downgrade(),
                    agent,
                    here,
                    folded,
                    chosen: 0,
                    focus,
                    close,
                }
            })
        })
}
