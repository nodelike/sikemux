//! ⌘P: every file in the project in front, ranked by name as the person types.

use crate::ui::Clicks;
use sikemux_calls::call;
use sikemux_calls::project_files::ProjectFiles as ScannedFiles;
use sikemux_calls::project_files::ProjectFilesParams;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, ScrollStrategy, Styled,
    Subscription, Task, UniformListScrollHandle, Window,
};

use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_MD, TEXT_SM};
use crate::ui;
use crate::ui::list_picker::{self, Choose, Close, Next, Previous};
use crate::workbench::Workbench;

const SHOWN: usize = 200;

pub fn install(cx: &mut App) {
    list_picker::bind("Finder > Input", list_picker::Tab::Steps, cx);
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
        core.answer::<call::ProjectFiles>(
            crate::reads::Read::ProjectFiles,
            ProjectFilesParams {
                project: project.to_owned(),
                since: self.scan,
            },
        );
    }

    /// Whether the core has listed the project's files, even as none.
    pub fn listed(&self) -> bool {
        self.scan.is_some()
    }

    /// Takes the core's answer in, telling whether the list changed.
    pub fn answered(&mut self, answer: ScannedFiles) -> bool {
        if self.asked.take() != self.project {
            return false;
        }
        self.scan = Some(answer.scan_id);
        if let Some(files) = answer.files {
            self.files = Arc::new(files);
        } else if let (Some(added), Some(removed)) = (answer.added, answer.removed) {
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
    /// Set to stop the ranking under way, once a newer query or closing makes it moot.
    stop_ranking: Arc<AtomicBool>,
    /// What had focus before the finder took it, to hand it back.
    before: Option<FocusHandle>,
    _typing: Subscription,
}

impl Finder {
    pub fn open(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("search files…")));
        let typing = cx.subscribe(&input, |bench, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = input.read(cx).value().to_string();
                if let Some(finder) = bench.overlays.finder.as_mut() {
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
            stop_ranking: Arc::default(),
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
        let count = self.shown.len();
        if count > 0 {
            self.select(list_picker::stepped(self.selected, by, count));
        }
    }

    fn chosen(&self, place: usize) -> Option<&str> {
        self.shown
            .get(place)
            .and_then(|&file| self.ranked.get(file))
            .map(String::as_str)
    }
}

impl Drop for Finder {
    fn drop(&mut self) {
        self.stop_ranking.store(true, Ordering::Relaxed);
    }
}

impl Workbench {
    pub fn toggle_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.finder.is_some() {
            return self.close_finder(window, cx);
        }
        self.close_palette(window, cx);
        self.overlays.finder = Some(Finder::open(window, cx));
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
        let Some(finder) = self.overlays.finder.as_mut() else {
            return;
        };
        let query = finder.query.clone();
        finder.stop_ranking.store(true, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        finder.stop_ranking = stop.clone();
        let ranking = cx.background_spawn({
            let files = files.clone();
            async move { crate::fuzzy::rank_paths_until(&query, &files, SHOWN, &stop) }
        });
        finder.ranking = Some(cx.spawn(async move |this, cx| {
            let Some(shown) = ranking.await else {
                return;
            };
            let _ = this.update(cx, |bench, cx| {
                if let Some(finder) = bench.overlays.finder.as_mut() {
                    finder.shown = shown;
                    finder.ranked = files;
                    finder.select(0);
                    bench.parts.redraw(&bench.parts.finder, cx);
                }
            });
        }));
    }

    pub fn close_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(finder) = self.overlays.finder.take() else {
            return;
        };
        finder
            .before
            .as_ref()
            .unwrap_or(&self.keys.focus)
            .focus(window, cx);
        cx.notify();
    }

    fn choose(&mut self, place: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(finder), Some(project)) = (self.overlays.finder.as_ref(), self.project()) else {
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
    let finder = bench.overlays.finder.as_ref()?;
    let t = Tokens::get(cx).clone();
    let project = bench.project().is_some();
    let empty = if !project {
        Some(t!("open a project to search files"))
    } else if finder.shown.is_empty() && bench.project_files.files.is_empty() {
        Some(t!("indexing…"))
    } else if finder.shown.is_empty() {
        Some(t!("no matches"))
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
            .pt(window.viewport_size().height * list_picker::BACKDROP_TOP)
            .bg(list_picker::backdrop_color(&t))
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
                        if let Some(finder) = bench.overlays.finder.as_mut() {
                            finder.step(1);
                        }
                        bench.parts.redraw(&bench.parts.finder, cx);
                    }))
                    .on_action(cx.listener(|bench, _: &Previous, _, cx| {
                        if let Some(finder) = bench.overlays.finder.as_mut() {
                            finder.step(-1);
                        }
                        bench.parts.redraw(&bench.parts.finder, cx);
                    }))
                    .on_action(cx.listener(|bench, _: &Choose, window, cx| {
                        let selected = bench
                            .overlays
                            .finder
                            .as_ref()
                            .map_or(0, |finder| finder.selected);
                        bench.choose(selected, window, cx);
                    }))
                    .on_action(
                        cx.listener(|bench, _: &Close, window, cx| bench.close_finder(window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2H)
                            .h(px(crate::palette::HEAD_HEIGHT))
                            .px(px(2. * ui::FLOAT_INSET))
                            .border_b_1()
                            .border_color(t.border)
                            .child(ui::icon(Icon::Search, 15.).text_color(t.text_tertiary))
                            .child(
                                Input::new(&finder.input)
                                    .appearance(false)
                                    .flex_1()
                                    .p_0()
                                    .text_size(crate::theme::ui_text(TEXT_MD))
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
                                        range
                                            .map(|place| {
                                                div()
                                                    .px(px(ui::FLOAT_INSET))
                                                    .child(row(bench, place, cx))
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                ),
                            )
                            .track_scroll(&finder.scroll)
                            .h(px((30. * count as f32)
                                .min(crate::palette::list_room(window, false))
                                + 2. * ui::FLOAT_INSET))
                            .min_h_0()
                            .flex_shrink(1.)
                            .py(px(ui::FLOAT_INSET)),
                        ),
                    }),
            )
            .into_any_element(),
    )
}

fn row(bench: &Workbench, place: usize, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(finder) = bench.overlays.finder.as_ref() else {
        return div().into_any_element();
    };
    let path = finder.chosen(place).unwrap_or_default();
    let (dir, name) = path.rsplit_once('/').unwrap_or((".", path));
    let selected = place == finder.selected;
    let query = finder.query.as_str();
    let found_in_name = !crate::fuzzy::matched(query, name).is_empty();
    div()
        .id(("finder-row", place))
        .w_full()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .h(px(30.))
        .px(SPACE_2)
        .rounded(theme::radius_2())
        .text_size(crate::theme::ui_text(TEXT_SM))
        .text_color(if selected {
            t.text_primary
        } else {
            t.text_secondary
        })
        .when(selected, |row| row.bg(t.surface_chosen))
        .on_mouse_move(cx.listener(move |bench, _, _, cx| {
            if let Some(finder) = bench
                .overlays
                .finder
                .as_mut()
                .filter(|finder| finder.selected != place)
            {
                finder.selected = place;
                bench.parts.redraw(&bench.parts.finder, cx);
            }
        }))
        .clicks(cx.listener(move |bench, _, window, cx| bench.choose(place, window, cx)))
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
                .child(ui::found(name.to_owned(), query, cx)),
        )
        .child(
            div()
                .flex_none()
                .pl(SPACE_3)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(ui::found(
                    dir.to_owned(),
                    if found_in_name { "" } else { query },
                    cx,
                )),
        )
        .into_any_element()
}
