//! The project picker a new chat's project name and the top bar open: the projects open here,
//! then folders found under the person's project roots, searchable in both.

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, Anchor, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window,
};
use serde_json::json;

use crate::core;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_2, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui;
use crate::ui::path::tilde;
use crate::workbench::Workbench;

actions!(project_switcher, [Up, Down, Pick, Close]);

const CONTEXT: &str = "ProjectSwitcher";

/// Closes the popover the switcher is in.
type Dismiss = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// What picking a project does with it, by its id, once a folder found on disk is one.
pub type OnPick = std::rc::Rc<dyn Fn(String, &mut App)>;

pub fn install(cx: &mut App) {
    let context = Some("ProjectSwitcher > Input");
    cx.bind_keys([
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("enter", Pick, context),
        KeyBinding::new("escape", Close, context),
    ]);
}

/// A place the chat can move to.
#[derive(Clone)]
struct Choice {
    path: String,
    /// The project's id, when it is one already.
    project: Option<String>,
    /// The home folder of the host the folder is on.
    home: Option<String>,
}

/// The open switcher: its search, the row the keys are on, and the folders found on disk.
pub struct Switcher {
    bench: WeakEntity<Workbench>,
    on_pick: OnPick,
    cwd: String,
    query: Entity<InputState>,
    chosen: usize,
    found: Vec<String>,
    close: Dismiss,
    _typing: Subscription,
}

fn name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// The folder a project's folder sits in.
fn parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map_or(
            path,
            |(parent, _)| if parent.is_empty() { "/" } else { parent },
        )
}

impl Switcher {
    fn new(
        bench: &Entity<Workbench>,
        on_pick: OnPick,
        cwd: String,
        close: Dismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects…"));
        query.update(cx, |query, cx| query.focus(window, cx));
        let typing = cx.subscribe(&query, |switcher, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                switcher.chosen = 0;
                cx.notify();
            }
        });
        let (asks, roots) = {
            let bench = bench.read(cx);
            (
                bench.asks.clone(),
                bench.preferences().project_roots.clone(),
            )
        };
        if !roots.is_empty() {
            cx.spawn(async move |switcher, cx| {
                let Ok(found) =
                    core::request(&asks, "projectRoots", json!({ "roots": roots })).await
                else {
                    return;
                };
                let found: Vec<String> = found
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
                    .collect();
                let _ = switcher.update(cx, |switcher, cx| {
                    switcher.found = found;
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            bench: bench.downgrade(),
            on_pick,
            cwd,
            query,
            chosen: 0,
            found: Vec::new(),
            close,
            _typing: typing,
        }
    }

    /// The projects open here, then the folders found on disk, matched to the search.
    fn choices(&self, cx: &App) -> Vec<Choice> {
        let open: Vec<Choice> = self
            .bench
            .upgrade()
            .map(|bench| {
                let bench = bench.read(cx);
                let home = bench.home_at(&bench.place());
                bench
                    .projects()
                    .iter()
                    .map(|project| Choice {
                        path: project.path.to_string_lossy().into_owned(),
                        project: Some(project.id.clone()),
                        home: home.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let found: Vec<Choice> = self
            .found
            .iter()
            .filter(|path| !open.iter().any(|choice| choice.path == **path))
            .map(|path| Choice {
                path: path.clone(),
                project: None,
                home: crate::ui::path::this_home(),
            })
            .collect();
        let all: Vec<Choice> = open.into_iter().chain(found).collect();
        let query = self.query.read(cx).value().trim().to_owned();
        if query.is_empty() {
            return all;
        }
        let paths: Vec<String> = all
            .iter()
            .map(|choice| tilde(&choice.path, choice.home.as_deref()))
            .collect();
        crate::fuzzy::rank_paths(&query, &paths, all.len())
            .into_iter()
            .map(|at| all[at].clone())
            .collect()
    }

    /// Picks `choice`, making the folder a project first when it is not one.
    fn pick(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        (self.close)(window, cx);
        if choice.path == self.cwd {
            return;
        }
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let asks = bench.read(cx).asks.clone();
        let on_pick = self.on_pick.clone();
        cx.spawn(async move |_, cx| {
            let project = match choice.project {
                Some(project) => project,
                None => {
                    let added =
                        core::request(&asks, "addProject", json!({ "path": choice.path })).await;
                    match added
                        .ok()
                        .and_then(|project| project["id"].as_str().map(str::to_owned))
                    {
                        Some(project) => project,
                        None => return,
                    }
                }
            };
            cx.update(|cx| on_pick(project, cx));
        })
        .detach();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.choices(cx).len() as isize;
        if count > 0 {
            self.chosen = (self.chosen as isize + by).rem_euclid(count) as usize;
        }
        cx.notify();
    }
}

impl Render for Switcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let choices = self.choices(cx);
        let typed = self.query.read(cx).value().trim().to_owned();
        let grouped = typed.is_empty();
        let this = cx.entity();
        let rows = choices.iter().enumerate().map(|(at, choice)| {
            let heading = (grouped
                && (at == 0 || choices[at - 1].project.is_some() != choice.project.is_some()))
            .then(|| {
                div()
                    .px(SPACE_2)
                    .pt(SPACE_2)
                    .pb(SPACE_0)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_tertiary)
                    .child(if choice.project.is_some() {
                        "Open"
                    } else {
                        "Found on disk"
                    })
            });
            let here = choice.path == self.cwd;
            let (this, picked) = (this.clone(), choice.clone());
            div().children(heading).child(
                div()
                    .id(("project-choice", at))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .h(px(28.))
                    .px(SPACE_2)
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .when(at == self.chosen, |row| row.bg(t.surface_hover))
                    .on_hover({
                        let this = this.clone();
                        move |over: &bool, _, cx| {
                            if *over {
                                this.update(cx, |switcher, cx| {
                                    switcher.chosen = at;
                                    cx.notify();
                                });
                            }
                        }
                    })
                    .on_click(move |_, window, cx| {
                        let picked = picked.clone();
                        this.update(cx, |switcher, cx| switcher.pick(picked, window, cx))
                    })
                    .child(ui::icon(Icon::Folder, 12.).text_color(t.live))
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::theme::ui_text(TEXT_XS_H))
                            .text_color(if here {
                                t.text_primary
                            } else {
                                t.text_secondary
                            })
                            .child(ui::found(name(&choice.path), &typed, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(tilde(parent(&choice.path), choice.home.as_deref())),
                    )
                    .when(here, |row| {
                        row.child(ui::icon(Icon::Check, 12.).text_color(t.text_secondary))
                    }),
            )
        });
        let empty = choices.is_empty();
        div()
            .key_context(format!("{CONTEXT} Modal").as_str())
            .on_action(cx.listener(|switcher, _: &Up, _, cx| switcher.step(-1, cx)))
            .on_action(cx.listener(|switcher, _: &Down, _, cx| switcher.step(1, cx)))
            .on_action(cx.listener(|switcher, _: &Pick, window, cx| {
                if let Some(choice) = switcher.choices(cx).get(switcher.chosen).cloned() {
                    switcher.pick(choice, window, cx);
                }
            }))
            .on_action(cx.listener(|switcher, _: &Close, window, cx| (switcher.close)(window, cx)))
            .w(px(360.))
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .p(px(ui::PANEL_INSET))
            .bg(t.surface_overlay)
            .border_1()
            .border_color(t.border)
            .rounded(theme::RADIUS_3)
            .shadow(t.shadow_float())
            .child(
                div()
                    .mx(px(-ui::PANEL_INSET))
                    .mt(px(-ui::PANEL_INSET))
                    .mb(SPACE_0)
                    .child(ui::Field::new(&self.query).icon(Icon::Search).flush()),
            )
            .child(
                div()
                    .id("project-choices")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px(SPACE_2)
                                .py(SPACE_2)
                                .text_size(crate::theme::ui_text(TEXT_XS))
                                .text_color(t.text_tertiary)
                                .child("No matches"),
                        )
                    }),
            )
    }
}

/// Wraps `trigger`, the project's name over a new chat, in the switcher it opens, which moves
/// the chat `agent`, now in `cwd`, elsewhere.
pub fn around(
    trigger: impl Selectable + IntoElement + 'static,
    agent: &str,
    cwd: &str,
    this: &Entity<Workbench>,
) -> Popover {
    let (moved, bench) = (agent.to_owned(), this.downgrade());
    let on_pick: OnPick = std::rc::Rc::new(move |project, cx| {
        let Some(asks) = bench.upgrade().map(|bench| bench.read(cx).asks.clone()) else {
            return;
        };
        let agent = moved.clone();
        cx.background_spawn(async move {
            let moving = json!({ "agentId": agent, "project": project });
            if let Err(error) = core::request(&asks, "moveAgentToProject", moving).await {
                eprintln!("sikemux: the chat stayed where it was: {error}");
            }
        })
        .detach();
    });
    let key = SharedString::from(format!("project-switcher-{agent}"));
    picker(key, trigger, cwd, Anchor::BottomLeft, on_pick, this)
}

/// Wraps `trigger` in the project picker, keyed `key`, with `cwd` the project shown now.
pub fn picker(
    key: SharedString,
    trigger: impl Selectable + IntoElement + 'static,
    cwd: &str,
    anchor: Anchor,
    on_pick: OnPick,
    this: &Entity<Workbench>,
) -> Popover {
    let (folder, this) = (cwd.to_owned(), this.clone());
    Popover::new(key.clone())
        .anchor(anchor)
        .offset(px(6.))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let popover = cx.entity();
            let close: Dismiss = std::rc::Rc::new(move |window, cx| {
                popover.update(cx, |popover, cx| popover.dismiss(window, cx));
            });
            let (on_pick, folder) = (on_pick.clone(), folder.clone());
            window.use_keyed_state(key.clone(), cx, |window, cx| {
                Switcher::new(&this, on_pick, folder, close, window, cx)
            })
        })
}
