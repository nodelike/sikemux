//! ⌘P: every file in the project in front, ranked by name as the person types.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, uniform_list, AnyElement, App, AppContext, Context, Entity, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement,
    ScrollStrategy, StatefulInteractiveElement, Styled, Subscription, Task,
    UniformListScrollHandle, Window,
};
use serde_json::{json, Value};

use sikemux_wire::protocol::{Pane, PaneKind};

use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

actions!(finder, [Next, Previous, Choose, Close]);

/// Named by today's command id, so a shortcut the person set for it still finds it.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "palette.files")]
pub struct OpenFile;

const SHOWN: usize = 200;

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some("Finder > Input")),
        KeyBinding::new("tab", Next, Some("Finder > Input")),
        KeyBinding::new("up", Previous, Some("Finder > Input")),
        KeyBinding::new("shift-tab", Previous, Some("Finder > Input")),
        KeyBinding::new("enter", Choose, Some("Finder > Input")),
        KeyBinding::new("escape", Close, Some("Finder > Input")),
    ]);
}

/// Every file of one project as the core last listed it, relative and byte-sorted.
#[derive(Default)]
pub struct ProjectFiles {
    pub project: Option<String>,
    scan: Option<u64>,
    pub files: Arc<Vec<String>>,
    /// The project the answer on its way was asked for.
    asked: Option<String>,
}

impl ProjectFiles {
    /// Asks the core what changed since the scan held, or for the whole list of a new project.
    pub fn read(&mut self, project: Option<&str>, core: &Target) {
        let Some(project) = project else {
            *self = Self::default();
            return;
        };
        if self.project.as_deref() != Some(project) {
            *self = Self {
                project: Some(project.to_owned()),
                ..Self::default()
            };
        }
        self.asked = Some(project.to_owned());
        let params = json!({ "project": project, "since": self.scan });
        core.answer("projectFiles", "projectFiles", params);
    }

    /// Whether the core has listed the project's files, even as none.
    pub fn listed(&self) -> bool {
        self.scan.is_some()
    }

    /// Takes the core's answer in, telling whether the list changed.
    pub fn answered(&mut self, answer: &Value) -> bool {
        if self.asked.take() != self.project {
            return false;
        }
        let Some(scan) = answer["scanId"].as_u64() else {
            return false;
        };
        let paths =
            |key: &str| -> Option<Vec<String>> { serde_json::from_value(answer[key].clone()).ok() };
        self.scan = Some(scan);
        if let Some(files) = paths("files") {
            self.files = Arc::new(files);
        } else if let (Some(added), Some(removed)) = (paths("added"), paths("removed")) {
            self.files = Arc::new(merged(&self.files, &added, &removed));
        } else {
            return false;
        }
        true
    }
}

/// `files` without `removed` and with `added`, all three sorted.
fn merged(files: &[String], added: &[String], removed: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(files.len() + added.len());
    let (mut removed, mut added) = (removed.iter().peekable(), added.iter().peekable());
    for file in files {
        while removed.next_if(|path| *path < file).is_some() {}
        if removed.next_if(|path| *path == file).is_some() {
            continue;
        }
        while let Some(path) = added.next_if(|path| *path < file) {
            out.push(path.clone());
        }
        out.push(file.clone());
    }
    out.extend(added.cloned());
    out.dedup();
    out
}

/// The open finder: what the person typed, and the files it ranks first.
pub struct Finder {
    pub input: Entity<InputState>,
    query: String,
    /// Places in the project's files, best first.
    shown: Vec<usize>,
    /// The list the places point into.
    ranked: Arc<Vec<String>>,
    selected: usize,
    scroll: UniformListScrollHandle,
    ranking: Option<Task<()>>,
    /// What had focus before the finder took it, to hand it back.
    before: Option<FocusHandle>,
    _typing: Subscription,
}

impl Finder {
    pub fn open(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search files…"));
        let typing = cx.subscribe(&input, |bench, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = input.read(cx).value().to_string();
                if let Some(finder) = bench.finder.as_mut() {
                    finder.query = query;
                }
                bench.rank(cx);
            }
        });
        let before = window.focused(cx);
        input.update(cx, |state, cx| state.focus(window, cx));
        Self {
            input,
            query: String::new(),
            shown: Vec::new(),
            ranked: Arc::default(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            ranking: None,
            before,
            _typing: typing,
        }
    }

    fn select(&mut self, selected: usize) {
        self.selected = selected;
        self.scroll
            .scroll_to_item(selected, ScrollStrategy::Nearest);
    }

    fn step(&mut self, by: isize) {
        let count = self.shown.len() as isize;
        if count > 0 {
            self.select((self.selected as isize + by).rem_euclid(count) as usize);
        }
    }

    fn chosen(&self, place: usize) -> Option<&str> {
        self.shown
            .get(place)
            .and_then(|&file| self.ranked.get(file))
            .map(String::as_str)
    }
}

impl Workbench {
    pub fn toggle_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finder.is_some() {
            return self.close_finder(window, cx);
        }
        if let Some(palette) = self.plugin_palette_in_front() {
            return self.open_plugin_palette(palette, window, cx);
        }
        self.close_palette(window, cx);
        self.finder = Some(Finder::open(window, cx));
        let project = self
            .project()
            .map(|project| project.path.to_string_lossy().into_owned());
        self.project_files.read(project.as_deref(), &self.target());
        self.rank(cx);
    }

    /// Ranks the project's files for what the finder holds, off the main thread, and keeps
    /// the last ranking until the new one is in.
    pub fn rank(&mut self, cx: &mut Context<Self>) {
        let files = self.project_files.files.clone();
        let Some(finder) = self.finder.as_mut() else {
            return;
        };
        let query = finder.query.clone();
        let ranking = cx.background_spawn({
            let files = files.clone();
            async move { crate::fuzzy::rank_paths(&query, &files, SHOWN) }
        });
        finder.ranking = Some(cx.spawn(async move |this, cx| {
            let shown = ranking.await;
            let _ = this.update(cx, |bench, cx| {
                if let Some(finder) = bench.finder.as_mut() {
                    finder.shown = shown;
                    finder.ranked = files;
                    finder.select(0);
                    bench.parts.redraw(&bench.parts.finder, cx);
                }
            });
        }));
    }

    pub fn close_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(finder) = self.finder.take() else {
            return;
        };
        finder
            .before
            .as_ref()
            .unwrap_or(&self.focus)
            .focus(window, cx);
        cx.notify();
    }

    fn choose(&mut self, place: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(finder), Some(project)) = (self.finder.as_ref(), self.project()) else {
            return;
        };
        if let Some(path) = finder.chosen(place) {
            self.open_file(&project.path.join(path), cx);
            self.close_finder(window, cx);
        }
    }
}

pub fn overlay(
    bench: &Workbench,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let finder = bench.finder.as_ref()?;
    let t = Tokens::get(cx).clone();
    let project = bench.project().is_some();
    let empty = if !project {
        Some("open a project to search files")
    } else if finder.shown.is_empty() && bench.project_files.files.is_empty() {
        Some("indexing…")
    } else if finder.shown.is_empty() {
        Some("no matches")
    } else {
        None
    };
    let count = finder.shown.len();
    let close = cx.listener(|bench: &mut Workbench, _, window, cx| bench.close_finder(window, cx));
    Some(
        div()
            .id("finder")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(window.viewport_size().height * 0.13)
            .bg(Hsla {
                a: 0.68,
                ..t.surface_sunken
            })
            .occlude()
            .on_mouse_down(MouseButton::Left, close)
            .child(
                ui::float(cx)
                    .key_context("Finder Modal")
                    .w(px(560.))
                    .max_w(gpui_kit::relative(0.86))
                    .max_h(gpui_kit::relative(0.62))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_action(cx.listener(|bench, _: &Next, _, cx| {
                        if let Some(finder) = bench.finder.as_mut() {
                            finder.step(1);
                        }
                        bench.parts.redraw(&bench.parts.finder, cx);
                    }))
                    .on_action(cx.listener(|bench, _: &Previous, _, cx| {
                        if let Some(finder) = bench.finder.as_mut() {
                            finder.step(-1);
                        }
                        bench.parts.redraw(&bench.parts.finder, cx);
                    }))
                    .on_action(cx.listener(|bench, _: &Choose, window, cx| {
                        let selected = bench.finder.as_ref().map_or(0, |finder| finder.selected);
                        bench.choose(selected, window, cx);
                    }))
                    .on_action(
                        cx.listener(|bench, _: &Close, window, cx| bench.close_finder(window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(16.))
                            .py(px(14.))
                            .border_b_1()
                            .border_color(t.border)
                            .child(ui::icon(Icon::Search, 15.).text_color(t.text_tertiary))
                            .child(
                                Input::new(&finder.input)
                                    .appearance(false)
                                    .flex_1()
                                    .p_0()
                                    .text_size(crate::theme::ui_text(14.))
                                    .text_color(t.text_primary)
                                    .disabled(!project),
                            ),
                    )
                    .map(|panel| match empty {
                        Some(empty) => panel.child(
                            div()
                                .p(px(28.))
                                .text_center()
                                .text_color(t.text_tertiary)
                                .child(empty),
                        ),
                        None => panel.child(
                            uniform_list(
                                "finder-rows",
                                count,
                                cx.processor(
                                    |bench: &mut Workbench, range: Range<usize>, _window, cx| {
                                        range.map(|place| row(bench, place, cx)).collect::<Vec<_>>()
                                    },
                                ),
                            )
                            .track_scroll(&finder.scroll)
                            .h(px(30. * count as f32 + 8.))
                            .min_h_0()
                            .flex_shrink(1.)
                            .py(px(4.))
                            .px(px(6.)),
                        ),
                    }),
            )
            .into_any_element(),
    )
}

fn row(bench: &Workbench, place: usize, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(finder) = bench.finder.as_ref() else {
        return div().into_any_element();
    };
    let path = finder.chosen(place).unwrap_or_default();
    let (dir, name) = path.rsplit_once('/').unwrap_or((".", path));
    let selected = place == finder.selected;
    div()
        .id(("finder-row", place))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(30.))
        .px(px(6.))
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(13.))
        .text_color(if selected {
            t.text_primary
        } else {
            t.text_secondary
        })
        .when(selected, |row| row.bg(t.surface_chosen))
        .on_mouse_move(cx.listener(move |bench, _, _, cx| {
            if let Some(finder) = bench
                .finder
                .as_mut()
                .filter(|finder| finder.selected != place)
            {
                finder.selected = place;
                bench.parts.redraw(&bench.parts.finder, cx);
            }
        }))
        .on_click(cx.listener(move |bench, _, window, cx| bench.choose(place, window, cx)))
        .child(
            div()
                .w(px(22.))
                .flex_none()
                .flex()
                .justify_center()
                .children(
                    crate::file_icons::icon(name, cx).map(|icon| gpui_kit::img(icon).size(px(14.))),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_none()
                .pl(px(12.))
                .text_size(crate::theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child(dir.to_owned()),
        )
        .into_any_element()
}

impl Workbench {
    /// The first palette of the plugin whose pane is the tab in front, which ⌘P opens in place
    /// of the project's files.
    fn plugin_palette_in_front(&self) -> Option<&'static crate::plugin_pane::PluginPalette> {
        let Pane::View {
            kind: PaneKind::Plugin,
            target,
            ..
        } = &self.tab()?.pane
        else {
            return None;
        };
        crate::plugin_pane::palettes(crate::plugin_pane::parts(target).0).first()
    }
}
