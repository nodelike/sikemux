//! ⌃`: holding ⌃ and pressing ` steps through the projects and the SSH hosts
//! open beside them; letting go of ⌃ opens the one reached, and Esc stays put.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, Modifiers, ModifiersChangedEvent, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use serde_json::json;

use crate::chat::tilde;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::{Strip, Workbench};

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "session.next")]
pub struct NextPlace;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "session.previous")]
pub struct PreviousPlace;

/// The modifier the person holds while stepping, which opens the place when let go.
#[derive(Clone, Copy, PartialEq)]
enum Held {
    Control,
    Alt,
    Command,
}

impl Held {
    fn of(modifiers: &Modifiers) -> Option<Self> {
        if modifiers.control {
            Some(Held::Control)
        } else if modifiers.alt {
            Some(Held::Alt)
        } else if modifiers.platform {
            Some(Held::Command)
        } else {
            None
        }
    }

    fn held(self, modifiers: &Modifiers) -> bool {
        match self {
            Held::Control => modifiers.control,
            Held::Alt => modifiers.alt,
            Held::Command => modifiers.platform,
        }
    }

    fn glyph(self) -> &'static str {
        match (self, cfg!(target_os = "macos")) {
            (Held::Control, true) => "⌃",
            (Held::Alt, true) => "⌥",
            (Held::Command, true) => "⌘",
            (Held::Control, false) => "Ctrl",
            (Held::Alt, false) => "Alt",
            (Held::Command, false) => "Super",
        }
    }
}

pub struct Switcher {
    places: Vec<Strip>,
    at: usize,
    held: Held,
}

impl Workbench {
    /// Every project in the rail's order, then the SSH hosts with a strip open, then the
    /// terminals of no project when one is open.
    pub(crate) fn strips_in_order(&self) -> Vec<Strip> {
        let projects = self
            .projects()
            .iter()
            .map(|project| Strip::Project(project.id.clone()));
        let layout = self.view.as_ref().map(|view| &view.layout);
        let hosts = layout
            .map(|layout| layout.hosts.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let terminals = layout.is_some_and(|layout| !layout.terminals.tabs.is_empty());
        projects
            .chain(hosts.into_iter().map(Strip::Host))
            .chain(terminals.then_some(Strip::Terminals))
            .collect()
    }

    /// Steps `by` places on among those of the kind in front, projects among projects and
    /// hosts among hosts: in the switcher while a modifier is held, else at once.
    pub fn step_place(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(switcher) = self.switcher.as_mut() {
            let count = switcher.places.len() as isize;
            switcher.at = (switcher.at as isize + by).rem_euclid(count) as usize;
            return self.parts.redraw(&self.parts.finder, cx);
        }
        let front = self.strip();
        let places: Vec<Strip> = self
            .strips_in_order()
            .into_iter()
            .filter(|place| {
                front.as_ref().is_none_or(|front| {
                    std::mem::discriminant(place) == std::mem::discriminant(front)
                })
            })
            .collect();
        if places.len() < 2 {
            return;
        }
        let count = places.len() as isize;
        let here = self
            .strip()
            .and_then(|strip| places.iter().position(|place| *place == strip))
            .unwrap_or(0);
        let at = (here as isize + by).rem_euclid(count) as usize;
        match Held::of(&window.modifiers()) {
            Some(held) => {
                self.switcher = Some(Switcher { places, at, held });
                self.parts.redraw(&self.parts.finder, cx);
                cx.notify();
            }
            None => self.show_strip(&places[at], cx),
        }
    }

    pub fn modifiers_changed(&mut self, event: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        if self
            .switcher
            .as_ref()
            .is_some_and(|switcher| !switcher.held.held(&event.modifiers))
        {
            self.end_switch(true, cx);
        }
    }

    pub fn switcher_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.switcher.is_some() && event.keystroke.key == "escape" {
            self.end_switch(false, cx);
            cx.stop_propagation();
        }
    }

    fn end_switch(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        if let Some(place) = switcher.places.get(switcher.at).filter(|_| open) {
            self.show_strip(place, cx);
        }
        self.parts.redraw(&self.parts.finder, cx);
        cx.notify();
    }

    /// Brings a project, or an SSH host's strip, to the front.
    pub fn show_strip(&self, strip: &Strip, cx: &mut App) {
        match strip {
            Strip::Project(project) => self.activate_project(project),
            Strip::Host(alias) => {
                let open = self
                    .view
                    .as_ref()
                    .and_then(|view| view.layout.hosts.get(alias))
                    .and_then(|layout| layout.active.clone());
                match open {
                    Some(tab) => self.activate_tab(strip, &tab),
                    None => self.open_ssh_terminal(alias, cx),
                }
            }
            Strip::Terminals => self.call("showTerminals", json!({})),
        }
    }
}

pub fn overlay(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let switcher = bench.switcher.as_ref()?;
    let t = Tokens::get(cx).clone();
    let rows = switcher.places.iter().enumerate().map(|(place, strip)| {
        let (glyph, name, line): (Icon, SharedString, SharedString) = match strip {
            Strip::Project(id) => match bench.projects().iter().find(|project| &project.id == id) {
                Some(project) => (
                    Icon::Folder,
                    project.name.clone().into(),
                    tilde(&project.path).into(),
                ),
                None => (Icon::Folder, id.clone().into(), SharedString::default()),
            },
            Strip::Host(alias) => (Icon::Server, alias.clone().into(), "SSH".into()),
            Strip::Terminals => (Icon::Terminal, "Terminals".into(), "~".into()),
        };
        let chosen = place == switcher.at;
        div()
            .id(("switcher-place", place))
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(40.))
            .px(px(10.))
            .rounded(theme::RADIUS_2)
            .when(chosen, |row| row.bg(t.surface_chosen))
            .on_click(cx.listener(move |bench, _, _, cx| {
                if let Some(switcher) = bench.switcher.as_mut() {
                    switcher.at = place;
                }
                bench.end_switch(true, cx);
            }))
            .child(ui::icon(glyph, 16.).text_color(if chosen {
                t.text_primary
            } else {
                t.text_tertiary
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::theme::ui_text(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if chosen {
                                t.text_primary
                            } else {
                                t.text_secondary
                            })
                            .child(name),
                    )
                    .child(ui::meta(line, cx).truncate()),
            )
    });
    let held = switcher.held.glyph();
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(Hsla {
                a: 0.4,
                ..t.surface_sunken
            })
            .child(
                ui::float(cx)
                    .w(px(420.))
                    .max_w(gpui_kit::relative(0.86))
                    .max_h(gpui_kit::relative(0.7))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .px(px(14.))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(t.border)
                            .child(ui::meta("Switch project", cx))
                            .child(ui::meta(format!("release {held} to open"), cx)),
                    )
                    .child(
                        div()
                            .id("switcher-places")
                            .flex()
                            .flex_col()
                            .p(px(6.))
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .px(px(14.))
                            .py(px(8.))
                            .border_t_1()
                            .border_color(t.border)
                            .child(ui::kbd(format!("{held}`"), cx))
                            .child(ui::meta("next", cx))
                            .child(ui::kbd("Esc", cx))
                            .child(ui::meta("stay", cx)),
                    ),
            )
            .into_any_element(),
    )
}
