//! A project as the rail lists it, once for every host that holds a copy (ADR 0023): which copy
//! is in front, and how the person moves to another copy or makes one on a host without.

use std::path::PathBuf;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, Anchor, AnyElement, App, Context, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Transformation, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::hosts::chat_move::MoveChatToHostParams;
use sikemux_core::server::layout::ActivateProjectParams;
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::workspace::{ProjectGit, ProjectInfo};
use std::f32::consts::FRAC_PI_2;

use crate::core::Target;
use crate::hosts::{HostChoice, Place};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_1, SPACE_1H, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

/// One host's copy of a project.
#[derive(Clone, PartialEq)]
pub struct HostCopy {
    pub place: Place,
    /// The project's id on that host.
    pub project: String,
    pub host: SharedString,
    pub path: PathBuf,
    pub git: Option<ProjectGit>,
    /// That host is out of reach, so the copy is as it last said.
    pub away: bool,
}

impl HostCopy {
    pub fn choice(&self) -> HostChoice {
        match &self.place {
            Place::This => HostChoice::This,
            Place::Account(key) => HostChoice::Account(key.clone()),
        }
    }

    pub fn icon(&self) -> Icon {
        match self.place {
            Place::This => Icon::Laptop,
            Place::Account(_) => Icon::Server,
        }
    }
}

/// A project the rail lists once, with this host's copy first when it holds one.
#[derive(Clone)]
pub struct Folded {
    pub name: String,
    pub identity: Option<String>,
    /// This host's own project, when it holds a copy.
    pub own: Option<ProjectInfo>,
    pub copies: Vec<HostCopy>,
}

impl Folded {
    /// What tells its rows apart: this host's project, else the first copy's.
    pub fn key(&self) -> String {
        match (&self.own, self.copies.first()) {
            (Some(own), _) => own.id.clone(),
            (None, Some(copy)) => format!("{}:{}", copy.choice().key(), copy.project),
            (None, None) => self.name.clone(),
        }
    }
}

impl Workbench {
    /// Every project the rail lists: this host's in its order, then those only other hosts hold.
    pub fn folded(&self) -> Vec<Folded> {
        let Some(view) = self.this_view() else {
            return Vec::new();
        };
        let own = view.workspace.projects.iter().map(|project| {
            let here = HostCopy {
                place: Place::This,
                project: project.id.clone(),
                host: "This host".into(),
                path: project.path.clone(),
                git: project.git.clone(),
                away: false,
            };
            Folded {
                name: project.name.clone(),
                identity: project.identity.clone(),
                copies: std::iter::once(here)
                    .chain(project.copies.iter().map(copy_of))
                    .collect(),
                own: Some(project.clone()),
            }
        });
        let elsewhere = view.workspace.elsewhere.iter().map(|project| Folded {
            name: project.name.clone(),
            identity: project.identity.clone(),
            own: None,
            copies: project.copies.iter().map(copy_of).collect(),
        });
        own.chain(elsewhere).collect()
    }

    /// The copy of `folded` that is the project in front, if one is.
    pub fn copy_in_front<'a>(&self, folded: &'a Folded) -> Option<&'a HostCopy> {
        let place = self.place();
        let project = self.project()?;
        (folded.copies.iter()).find(|copy| copy.place == place && copy.project == project.id)
    }

    /// The copy of `folded` the person works on: the one in front, else the one they last
    /// chose, else this host's, else the first; one on a host out of reach only when no copy
    /// is in reach.
    pub fn front_copy<'a>(&self, folded: &'a Folded) -> Option<&'a HostCopy> {
        let chosen = (folded.identity.as_ref())
            .and_then(|identity| self.preferences().front_copies.get(identity));
        let reached = |copy: &&HostCopy| !copy.away;
        self.copy_in_front(folded)
            .or_else(|| {
                let chosen = chosen?;
                (folded.copies.iter())
                    .filter(reached)
                    .find(|copy| copy.choice().key().as_ref() == chosen.as_str())
            })
            .or_else(|| folded.copies.iter().find(reached))
            .or_else(|| folded.copies.first())
    }

    /// The project in front, as the rail lists it.
    pub fn folded_in_front(&self) -> Option<Folded> {
        (self.folded().into_iter()).find(|folded| self.copy_in_front(folded).is_some())
    }

    /// Puts `copy` of the project known as `identity` in front: its host, then the project there.
    pub fn go_to_copy(&mut self, copy: &HostCopy, identity: Option<&str>, cx: &mut Context<Self>) {
        self.choose_host(copy.choice(), cx);
        Target::new(&self.asks, copy.place.clone()).call_typed::<call::ActivateProject>(
            ActivateProjectParams {
                project: copy.project.clone(),
            },
        );
        self.remember_copy(identity, copy.choice());
    }

    /// Moves the new chat `agent` onto `copy`, on its host, and puts it in front there. The
    /// chat's own core hands it over, with its draft, agent, model and effort.
    pub fn move_chat_to_copy(
        &mut self,
        agent: &str,
        copy: &HostCopy,
        identity: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let host = match &copy.place {
            Place::Account(key) => key.clone(),
            Place::This => match &self.remote {
                Some(remote) => remote.core_id.clone(),
                None => return,
            },
        };
        self.keep_draft(agent, cx);
        let moving = self
            .chat_target(agent)
            .request_typed::<call::MoveChatToHost>(MoveChatToHostParams {
                agent_id: agent.to_owned(),
                host,
                project: copy.project.clone(),
            });
        let (copy, identity) = (copy.clone(), identity.map(str::to_owned));
        cx.spawn(async move |bench, cx| {
            let moved = moving.await;
            let _ = bench.update(cx, |bench, cx| match moved {
                Ok(_) => bench.go_to_copy(&copy, identity.as_deref(), cx),
                Err(error) => bench.notify(
                    crate::toast::error(format!("The chat stayed where it was: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Notes that the person works on the project known as `identity` on `choice`, so its row
    /// goes there next time.
    pub fn remember_copy(&self, identity: Option<&str>, choice: HostChoice) {
        let Some(identity) = identity else {
            return;
        };
        let key = choice.key().to_string();
        let mut fronts = self.preferences().front_copies.clone();
        if fronts.get(identity) == Some(&key) {
            return;
        }
        fronts.insert(identity.to_owned(), key);
        crate::settings::set(self, PreferenceChange::FrontCopies(fronts));
    }

    /// Opens the settings of `copy`'s project on its host.
    pub fn open_copy_settings(&self, copy: &HostCopy) {
        Target::new(&self.asks, copy.place.clone()).call_typed::<call::OpenTab>(OpenTab::new(
            At::project(&copy.project),
            PaneKind::ProjectSettings,
            copy.path.to_string_lossy(),
        ));
    }

    /// The account's hosts that hold no copy of `folded`, this one too: each one's key, its
    /// name, and whether this app reaches it.
    pub fn copy_targets(&self, folded: &Folded) -> Vec<(String, SharedString, bool)> {
        if folded.identity.is_none() {
            return Vec::new();
        }
        let this = (self.remote.as_ref())
            .filter(|_| !(folded.copies.iter()).any(|copy| copy.place == Place::This))
            .map(|remote| {
                (
                    remote.core_id.clone(),
                    SharedString::from("This host"),
                    true,
                )
            });
        let others = (self.host_choices().into_iter())
            .filter_map(|option| match option.choice {
                HostChoice::Account(key) => Some((key, option.label)),
                _ => None,
            })
            .filter(|(key, _)| {
                !(folded.copies.iter()).any(|copy| copy.place == Place::Account(key.clone()))
            })
            .map(|(key, label)| {
                let reached = self.feeds.host(&key).is_some_and(|feed| feed.reached);
                (key, label, reached)
            });
        this.into_iter().chain(others).collect()
    }

    /// Where the host `key` is, as this app reaches it: this host by its own key, else that
    /// account host.
    pub fn place_of(&self, key: &str) -> Place {
        match &self.remote {
            Some(remote) if remote.core_id == key => Place::This,
            _ => Place::Account(key.to_owned()),
        }
    }

    /// A project of the host at `place`, by its id there.
    pub fn project_on(&self, place: &Place, id: &str) -> Option<&ProjectInfo> {
        (self.view_at(place)?.workspace.projects.iter()).find(|project| project.id == id)
    }
}

fn copy_of(copy: &sikemux_view::workspace::ProjectCopy) -> HostCopy {
    HostCopy {
        place: Place::Account(copy.host.clone()),
        project: copy.project.clone(),
        host: copy.host_name.clone().into(),
        path: copy.path.clone(),
        git: copy.git.clone(),
        away: copy.away,
    }
}

/// What the copy switch offers: each copy, the one in front ticked, then a copy on each host
/// without one.
pub fn copy_menu(
    bench: &Workbench,
    folded: &Folded,
    cx: &Context<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let this = cx.weak_entity();
    let front = bench.front_copy(folded).cloned();
    let targets = bench.copy_targets(folded);
    let leaves = bench.rails.copy_leaves_uncommitted;
    let folded = folded.clone();
    let t = Tokens::get(cx).clone();
    move |mut popup, _, cx| {
        for copy in &folded.copies {
            let (this, chosen, identity) = (this.clone(), copy.clone(), folded.identity.clone());
            let git = copy.git.as_ref();
            let hint = match (git, copy.away) {
                (_, true) => "out of reach".to_owned(),
                (Some(git), _) if git.ahead > 0 => format!("{} ↑{}", git.branch, git.ahead),
                (Some(git), _) => git.branch.clone(),
                (None, _) => String::new(),
            };
            let glyph = copy.icon();
            let ink = t.host;
            popup = popup.item(
                ui::MenuRow::new(copy.host.clone())
                    .mark(move || ui::icon(glyph, 13.).text_color(ink))
                    .hint(hint)
                    .item(cx)
                    .checked(front.as_ref() == Some(copy))
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |bench, cx| {
                            bench.go_to_copy(&chosen, identity.as_deref(), cx)
                        });
                    }),
            );
        }
        if targets.is_empty() {
            return popup;
        }
        popup = popup.separator();
        for (key, label, reached) in &targets {
            let (this, from, identity) = (this.clone(), front.clone(), folded.identity.clone());
            let (key, label) = (key.clone(), label.clone());
            popup = popup.item(
                ui::MenuRow::new(format!("Copy to {label}"))
                    .icon(Icon::Copy)
                    .when(!reached, |row| row.hint("out of reach"))
                    .item(cx)
                    .disabled(!reached)
                    .on_click(move |_, _, cx| {
                        let Some(from) = from.clone() else {
                            return;
                        };
                        let (key, label, identity) = (key.clone(), label.clone(), identity.clone());
                        let copying = crate::hosts::Copying {
                            from,
                            identity,
                            host: key,
                            label,
                            uncommitted: !leaves,
                            chat: None,
                        };
                        let _ = this.update(cx, |bench, cx| bench.copy_to(copying, cx));
                    }),
            );
        }
        let this = this.clone();
        popup.item(
            ui::MenuRow::new("Bring uncommitted changes")
                .item(cx)
                .checked(!leaves)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |bench, cx| {
                        bench.rails.copy_leaves_uncommitted = !leaves;
                        cx.notify();
                    });
                }),
        )
    }
}

/// `on <host> ▾`, which opens the copy switch of `folded`.
pub fn copy_chip(
    id: &'static str,
    bench: &Workbench,
    folded: &Folded,
    cx: &Context<Workbench>,
) -> Option<AnyElement> {
    let front = bench.front_copy(folded)?;
    let t = Tokens::get(cx).clone();
    let chip = gpui_kit::base::Button::new(SharedString::from(format!("{id}-button")))
        .accessibility_label("Copy in front")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(18.))
        .px(SPACE_1H)
        .rounded(RADIUS_1)
        .bg(t.surface_hover)
        .text_color(t.host)
        .hover(move |style| style.bg(t.surface_active))
        .child(ui::icon(front.icon(), 11.).text_color(t.host))
        .child(ui::text(
            format!("on {}", front.host),
            TEXT_2XS,
            gpui_kit::FontWeight::MEDIUM,
        ))
        .child(
            ui::icon(Icon::Chevron, 8.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.host),
        );
    Some(
        ui::menu_button(id, Anchor::TopRight, chip, copy_menu(bench, folded, cx))
            .into_any_element(),
    )
}

/// The copy in front as the project's tree first shows it, opening the copy switch.
pub fn copy_row(bench: &Workbench, folded: &Folded, cx: &Context<Workbench>) -> Option<AnyElement> {
    let front = bench.front_copy(folded)?;
    let t = Tokens::get(cx).clone();
    let git = front.git.as_ref();
    let row = gpui_kit::base::Button::new("copy-row-button")
        .accessibility_label("Copy in front")
        .w_full()
        .h(px(25.))
        .flex()
        .items_center()
        .gap(crate::theme::SPACE_2)
        .text_color(t.text_primary)
        .child(div().w(px(9.)).flex_none())
        .child(ui::mark_box(
            16.,
            ui::icon(front.icon(), 13.).text_color(t.host),
        ))
        .child(
            ui::text(
                front.host.clone(),
                crate::theme::TEXT_XS_H,
                gpui_kit::FontWeight::MEDIUM,
            )
            .flex_1()
            .min_w_0()
            .overflow_hidden(),
        )
        .children(git.map(|git| {
            ui::mono(git.branch.clone(), crate::theme::TEXT_3XS_H, cx)
                .text_color(t.text_secondary)
                .max_w(px(96.))
                .overflow_hidden()
        }))
        .children(git.filter(|git| git.ahead > 0).map(|git| {
            ui::mono(format!("↑{}", git.ahead), crate::theme::TEXT_3XS_H, cx).text_color(t.live)
        }))
        .children(
            git.filter(|git| git.dirty)
                .map(|_| div().size(px(6.)).flex_none().rounded_full().bg(t.warn)),
        )
        .child(
            ui::icon(Icon::Chevron, 9.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_tertiary),
        );
    let spine = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(px(1.))
        .bg(t.tree_spine);
    let tick = div()
        .absolute()
        .left_0()
        .top(px(12.))
        .w(px(9.))
        .h(px(1.))
        .bg(t.tree_tick);
    Some(
        div()
            .id("copy-row")
            .relative()
            .pr(crate::theme::SPACE_2)
            .child(spine)
            .child(tick)
            .child(ui::menu_button(
                "copy-switch",
                Anchor::TopLeft,
                row,
                copy_menu(bench, folded, cx),
            ))
            .into_any_element(),
    )
}

/// How a copy the person asked for goes, kept on its project's row until they dismiss it.
#[derive(Clone)]
pub struct CopyNote {
    pub host: SharedString,
    pub state: CopyState,
    /// What happened, in full, for the note's tooltip.
    pub said: SharedString,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CopyState {
    Going,
    Done,
    Failed,
}

impl Workbench {
    /// Notes on `identity`'s row how its copy to `host` goes.
    pub fn note_copy(
        &mut self,
        identity: Option<&str>,
        host: SharedString,
        state: CopyState,
        said: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(identity) = identity else {
            return;
        };
        let note = CopyNote {
            host,
            state,
            said: said.into(),
        };
        self.rails.copy_notes.insert(identity.to_owned(), note);
        cx.notify();
    }
}

/// How the copy of `folded` the person asked for goes: a spinner while it runs, then whether
/// it was made, which a click dismisses.
pub fn copy_note(
    bench: &Workbench,
    folded: &Folded,
    cx: &Context<Workbench>,
) -> Option<AnyElement> {
    let identity = folded.identity.clone()?;
    let note = bench.rails.copy_notes.get(&identity)?.clone();
    let t = Tokens::get(cx).clone();
    let (mark, word, tone) = match note.state {
        CopyState::Going => (
            crate::ui::sign_in::spinner("copy-note", cx),
            format!("to {}", note.host),
            t.text_secondary,
        ),
        CopyState::Done => (
            ui::icon(Icon::Check, 11.)
                .text_color(t.live)
                .into_any_element(),
            format!("on {}", note.host),
            t.live,
        ),
        CopyState::Failed => (
            ui::icon(Icon::Warning, 11.)
                .text_color(t.danger)
                .into_any_element(),
            "copy failed".to_owned(),
            t.danger,
        ),
    };
    let settled = note.state != CopyState::Going;
    let this = cx.weak_entity();
    Some(
        div()
            .id(SharedString::from(format!("copy-note-{identity}")))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .text_color(tone)
            .child(mark)
            .child(ui::text(word, TEXT_2XS, gpui_kit::FontWeight::MEDIUM))
            .tooltip(ui::tooltip(match settled {
                true => format!("{} Click to dismiss.", note.said),
                false => note.said.to_string(),
            }))
            .when(settled, |chip| {
                chip.cursor_pointer().on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = this.update(cx, |bench, cx| {
                        bench.rails.copy_notes.remove(&identity);
                        cx.notify();
                    });
                })
            })
            .into_any_element(),
    )
}

/// The host and how many hold `folded`, at the end of its row: the count of hosts when more than
/// one holds it, the host's name when only another one does, else nothing.
pub fn where_held(folded: &Folded, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx);
    let label: SharedString = match folded.copies.as_slice() {
        [] => return None,
        [only] if only.place == Place::This => return None,
        [only] => only.host.clone(),
        copies => copies.len().to_string().into(),
    };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .text_color(t.host)
            .child(ui::icon(Icon::Server, 11.).text_color(t.host))
            .child(ui::text(label, TEXT_2XS, gpui_kit::FontWeight::MEDIUM))
            .into_any_element(),
    )
}
