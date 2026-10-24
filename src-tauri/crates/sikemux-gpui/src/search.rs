//! The Search pane: a project's text searched as the core walks it, each file's matches
//! listed as they arrive, the chosen one previewed in place, and every match replaced
//! once the person has seen how many.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, img, list, px, relative, AnyElement, App, AppContext, Context, Div, Entity,
    FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, ListAlignment, ListState, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window,
};
use serde_json::{json, Value};
use sikemux_core::server::search::{
    FileWindow, ReplaceResults, SearchFile, SearchHit, SearchOptions, SearchSummary,
};

use crate::core::{Streaming, Target};
use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::syntax::{self, LineRuns};
use crate::theme::{self, Tokens, RADIUS_2};
use crate::toast;
use crate::ui;

/// Brings the project's Search tab forward with its query in hand, opening one when it
/// has none.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "search.global")]
pub struct FocusSearch;

/// The same, from the shortcut that picks the Search tab among the project's surfaces.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "window.search")]
pub struct FocusSearchWindow;

actions!(search, [ShowReplace]);

/// Lets Tab in the find field open the replace field rather than leave the pane.
pub fn install(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "tab",
        ShowReplace,
        Some("SearchPane > Input"),
    )]);
}

/// How long typing rests before a search starts.
const PAUSE: Duration = Duration::from_millis(250);
const HEADER_HEIGHT: f32 = 24.;
const HIT_HEIGHT: f32 = 20.;
const BEFORE: u32 = 40;
const AFTER: u32 = 80;
/// How many files' previews are kept.
const WINDOWS_KEPT: usize = 4;
const PREVIEW_LINE: f32 = 19.8;

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Idle,
    Searching,
    Done,
    Failed,
}

#[derive(Clone, PartialEq)]
enum Row {
    Header { file: usize },
    Hit { file: usize, hit: usize },
}

/// The fields the person types into.
struct Fields {
    find: Entity<InputState>,
    replace: Entity<InputState>,
    include: Entity<InputState>,
    exclude: Entity<InputState>,
}

/// The lines around a match, as `fileWindow` answers, coloured once read.
struct Shown {
    path: String,
    window: FileWindow,
    colours: Option<Arc<[LineRuns]>>,
    /// The theme the colours are of, as `syntax::version` counts.
    coloured_in: u64,
}

pub struct SearchPane {
    core: Target,
    pub project: String,
    fields: Option<Fields>,
    options: SearchOptions,
    replace_open: bool,
    scope_open: bool,
    query: String,
    replace: String,
    files: Vec<SearchFile>,
    summary: Option<SearchSummary>,
    status: Status,
    error: Option<String>,
    /// Files changed on disk since the results were found.
    stale: bool,
    replacing: bool,
    /// What a replace would change, shown before it does.
    previewed: Option<ReplaceResults>,
    rows: Vec<Row>,
    list: ListState,
    collapsed: HashSet<String>,
    /// The chosen match: its file's path and its place among the file's matches.
    chosen: Option<(String, usize)>,
    windows: VecDeque<Shown>,
    focus: FocusHandle,
    _searching: Task<()>,
    _reading: Task<()>,
    _typing: Vec<Subscription>,
}

impl SearchPane {
    /// The host whose core the pane asks.
    pub fn place(&self) -> &crate::hosts::Place {
        self.core.place()
    }

    pub fn new(core: Target, project: String, cx: &mut Context<Self>) -> Self {
        Self {
            core,
            project,
            fields: None,
            options: SearchOptions::default(),
            replace_open: false,
            scope_open: false,
            query: String::new(),
            replace: String::new(),
            files: Vec::new(),
            summary: None,
            status: Status::Idle,
            error: None,
            stale: false,
            replacing: false,
            previewed: None,
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            collapsed: HashSet::new(),
            chosen: None,
            windows: VecDeque::new(),
            focus: cx.focus_handle(),
            _searching: Task::ready(()),
            _reading: Task::ready(()),
            _typing: Vec::new(),
        }
    }

    /// Makes the fields the first time the pane draws, which needs the window.
    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
            };
            let fields = Fields {
                find: field("find in project", window, cx),
                replace: field("replace with…", window, cx),
                include: field("src/**/*.ts", window, cx),
                exclude: field("**/*.test.ts", window, cx),
            };
            let typed =
                |field: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>| {
                    cx.subscribe_in(field, window, |pane, _, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => pane.typed(cx),
                            InputEvent::PressEnter {
                                secondary: true, ..
                            } if pane.replace_open => pane.replace_all(window, cx),
                            InputEvent::PressEnter { .. } => pane.open(window, cx),
                            _ => {}
                        }
                    })
                };
            self._typing = vec![
                typed(&fields.find, window, cx),
                typed(&fields.replace, window, cx),
                typed(&fields.include, window, cx),
                typed(&fields.exclude, window, cx),
            ];
            fields.find.update(cx, |find, cx| find.focus(window, cx));
            self.fields = Some(fields);
        }
        self.fields.as_ref().expect("made above")
    }

    /// Puts the keyboard in the find field, searching for `seed` when there is one, as ⌘⇧F does.
    pub fn focus_find(
        &mut self,
        seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let find = self.fields(window, cx).find.clone();
        find.update(cx, |find, cx| {
            if let Some(seed) = &seed {
                find.set_value(seed.clone(), window, cx);
            }
            find.focus(window, cx);
        });
        if seed.is_some() {
            self.typed(cx);
        }
    }

    /// Reads what was typed, searching again once typing pauses when the search changed.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.fields else {
            return;
        };
        let read = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let (query, include, exclude) = (
            read(&fields.find),
            read(&fields.include),
            read(&fields.exclude),
        );
        self.replace = read(&fields.replace);
        self.previewed = None;
        let options = SearchOptions {
            include,
            exclude,
            ..self.options.clone()
        };
        if query != self.query || options != self.options {
            self.query = query;
            self.options = options;
            self.search_soon(cx);
        }
        cx.notify();
    }

    fn search_soon(&mut self, cx: &mut Context<Self>) {
        self.previewed = None;
        if self.query.trim().is_empty() {
            self._searching = Task::ready(());
            self.files.clear();
            self.summary = None;
            self.status = Status::Idle;
            self.error = None;
            self.relist();
            return;
        }
        self.status = Status::Searching;
        self._searching = cx.spawn(async move |pane, cx| {
            cx.background_executor().timer(PAUSE).await;
            let Ok(streaming) = pane.update(cx, |pane, cx| pane.start(cx)) else {
                return;
            };
            listen(pane, streaming, cx).await;
        });
    }

    /// Starts the search, dropping the one before, which stops it in the core.
    fn start(&mut self, cx: &mut Context<Self>) -> Streaming {
        self.files.clear();
        self.summary = None;
        self.error = None;
        self.stale = false;
        self.status = Status::Searching;
        self.relist();
        cx.notify();
        let params =
            json!({ "project": self.project, "query": self.query, "options": self.options });
        self.core.stream("searchStream", params)
    }

    fn found(&mut self, items: Vec<Value>, cx: &mut Context<Self>) {
        for item in items {
            if let Some(file) = item.get("file") {
                if let Ok(file) = serde_json::from_value::<SearchFile>(file.clone()) {
                    self.files.push(file);
                }
            } else if let Some(summary) = item.get("summary") {
                self.summary = serde_json::from_value(summary.clone()).ok();
            }
        }
        self.relist();
        if self.chosen.is_none() {
            self.chosen = self.files.first().map(|file| (file.path.clone(), 0));
            self.read_window(cx);
        }
        cx.notify();
    }

    fn relist(&mut self) {
        let mut rows = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            rows.push(Row::Header { file: index });
            if !self.collapsed.contains(&file.path) {
                rows.extend((0..file.matches.len()).map(|hit| Row::Hit { file: index, hit }));
            }
        }
        crate::git::relist(&self.list, &self.rows, &rows);
        self.rows = rows;
        if let Some((path, hit)) = &self.chosen {
            let valid = self
                .files
                .iter()
                .any(|file| &file.path == path && *hit < file.matches.len());
            if !valid {
                self.chosen = None;
            }
        }
    }

    fn chosen_hit(&self) -> Option<(&SearchFile, &SearchHit, usize)> {
        let (path, at) = self.chosen.as_ref()?;
        let file = self.files.iter().find(|file| &file.path == path)?;
        Some((file, file.matches.get(*at)?, *at))
    }

    fn choose(&mut self, path: String, hit: usize, cx: &mut Context<Self>) {
        self.chosen = Some((path, hit));
        self.read_window(cx);
        cx.notify();
    }

    /// Moves the chosen match by `step` among the listed ones.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let hits: Vec<(usize, usize, usize)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, entry)| match entry {
                Row::Hit { file, hit } => Some((row, *file, *hit)),
                Row::Header { .. } => None,
            })
            .collect();
        if hits.is_empty() {
            return;
        }
        let now = self.chosen.as_ref().and_then(|(path, at)| {
            hits.iter()
                .position(|(_, file, hit)| &self.files[*file].path == path && hit == at)
        });
        let next = match now {
            Some(now) => (now as isize + step).clamp(0, hits.len() as isize - 1) as usize,
            None => 0,
        };
        let (row, file, hit) = hits[next];
        self.list.scroll_to_reveal_item(row);
        self.choose(self.files[file].path.clone(), hit, cx);
    }

    /// Reads the lines around the chosen match, unless a kept preview already holds them.
    fn read_window(&mut self, cx: &mut Context<Self>) {
        let Some((file, hit, _)) = self.chosen_hit() else {
            return;
        };
        let (path, line) = (file.path.clone(), hit.line);
        if let Some(at) = self.windows.iter().position(|shown| {
            shown.path == path
                && line >= shown.window.start_line
                && line < shown.window.start_line + shown.window.doc.lines().count().max(1) as u32
        }) {
            let shown = self.windows.remove(at).expect("found above");
            self.windows.push_front(shown);
            return;
        }
        let full = Path::new(&self.project)
            .join(&path)
            .to_string_lossy()
            .into_owned();
        let params = json!({ "path": full, "line": line, "before": BEFORE, "after": AFTER });
        let answer = self.core.request("fileWindow", params);
        self._reading = cx.spawn(async move |pane, cx| {
            let window = answer.await.and_then(|window| {
                serde_json::from_value::<FileWindow>(window).map_err(|error| error.to_string())
            });
            pane.update(cx, |pane, cx| {
                match window {
                    Ok(window) => {
                        pane.windows.push_front(Shown {
                            path,
                            window,
                            colours: None,
                            coloured_in: 0,
                        });
                        pane.windows.truncate(WINDOWS_KEPT);
                        pane.colour_window(cx);
                    }
                    Err(error) => pane.error = Some(format!("could not load {path}: {error}")),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn colour_window(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.windows.front() else {
            return;
        };
        let Some(language) =
            crate::file_icons::language(&shown.path, cx).filter(|language| syntax::knows(language))
        else {
            return;
        };
        let (path, start) = (shown.path.clone(), shown.window.start_line);
        let version = syntax::version(cx);
        let task = syntax::colour_lines(&language, shown.window.doc.clone().into(), cx);
        cx.spawn(async move |pane, cx| {
            let Some(lines) = task.await else {
                return;
            };
            pane.update(cx, |pane, cx| {
                if let Some(shown) = pane
                    .windows
                    .iter_mut()
                    .find(|shown| shown.path == path && shown.window.start_line == start)
                {
                    shown.colours = Some(lines);
                    shown.coloured_in = version;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens the chosen match in the editor, at its line and first match.
    fn open(&self, window: &mut Window, cx: &mut App) {
        if let Some(open) = self.open_at() {
            window.dispatch_action(Box::new(open), cx);
        }
    }

    fn open_at(&self) -> Option<OpenAt> {
        let (file, hit, _) = self.chosen_hit()?;
        Some(OpenAt {
            path: Path::new(&self.project).join(&file.path),
            line: hit.line.saturating_sub(1),
            column: hit.ranges.first().map(|range| range.start),
        })
    }

    /// Replaces in two steps: the first says how many matches in how many files would
    /// change, the second changes them.
    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.trim().is_empty() || self.replacing {
            return;
        }
        let dry_run = self.previewed.is_none();
        self.replacing = true;
        let params = json!({ "project": self.project, "query": self.query, "replace": self.replace, "options": self.options, "dryRun": dry_run });
        let answer = self.core.request("searchReplace", params);
        cx.spawn_in(window, async move |pane, cx| {
            let results = answer.await.and_then(|results| {
                serde_json::from_value::<ReplaceResults>(results).map_err(|error| error.to_string())
            });
            pane.update_in(cx, |pane, window, cx| {
                pane.replacing = false;
                match results {
                    Ok(results) if dry_run => pane.previewed = Some(results),
                    Ok(results) => {
                        let note = replaced(&results);
                        window.push_notification(note, cx);
                        pane.replaced_with(results);
                    }
                    Err(error) => {
                        pane.previewed = None;
                        let note = toast::error(format!("replace failed: {error}"));
                        window.push_notification(note.autohide(false), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn replaced_with(&mut self, results: ReplaceResults) {
        let changed: HashSet<&str> = results
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        self.files
            .retain(|file| !changed.contains(file.path.as_str()));
        if let Some(summary) = &mut self.summary {
            summary.file_count = summary.file_count.saturating_sub(results.files.len());
            summary.match_count = summary.match_count.saturating_sub(results.match_count);
        }
        self.previewed = None;
        self.relist();
    }

    /// Notes that the project's files changed, so the results may be out of date.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        if !self.files.is_empty() && !self.stale {
            self.stale = true;
            cx.notify();
        }
    }

    fn rerun(&mut self, cx: &mut Context<Self>) {
        if self.query.trim().is_empty() {
            return;
        }
        let streaming = self.start(cx);
        self._searching = cx.spawn(async move |pane, cx| listen(pane, streaming, cx).await);
    }

    fn toggle(&mut self, option: fn(&mut SearchOptions) -> &mut bool, cx: &mut Context<Self>) {
        let value = option(&mut self.options);
        *value = !*value;
        self.search_soon(cx);
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn show_replace(&mut self, _: &ShowReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_open = true;
        if let Some(fields) = &self.fields {
            let replace = fields.replace.clone();
            replace.update(cx, |replace, cx| replace.focus(window, cx));
        }
        cx.notify();
    }
}

/// What a replace did, as a note: how many matches in how many files, and how many
/// files it skipped.
fn replaced(results: &ReplaceResults) -> Notification {
    let matches = if results.match_count == 1 {
        "match"
    } else {
        "matches"
    };
    let files = if results.file_count == 1 {
        "file"
    } else {
        "files"
    };
    let said = format!(
        "replaced {} {matches} in {} {files}",
        results.match_count, results.file_count
    );
    match results.errors.len() {
        0 => toast::success(said),
        skipped => toast::info(format!("{said} · {skipped} skipped")),
    }
}

/// Takes a search's parts as they come until it is done or a newer one replaces it.
async fn listen(
    pane: gpui_kit::WeakEntity<SearchPane>,
    mut streaming: Streaming,
    cx: &mut gpui_kit::AsyncApp,
) {
    while let Some(part) = streaming.parts.recv().await {
        let done = part.done;
        let kept = pane.update(cx, |pane, cx| {
            pane.found(part.items, cx);
            if let Some(error) = part.error {
                pane.status = Status::Failed;
                pane.error = Some(
                    error["message"]
                        .as_str()
                        .map_or_else(|| error.to_string(), str::to_owned),
                );
            } else if done {
                pane.status = Status::Done;
            }
            cx.notify();
        });
        if kept.is_err() || done {
            return;
        }
    }
}

impl Focusable for SearchPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SearchPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        self.fields(window, cx);
        let theme = syntax::version(cx);
        let stale = self
            .windows
            .front()
            .is_some_and(|shown| shown.colours.is_some() && shown.coloured_in != theme);
        if stale {
            self.colour_window(cx);
        }
        div()
            .id("search-pane")
            .key_context("SearchPane")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::MONO)
            .text_color(t.text_secondary)
            .on_key_down(cx.listener(Self::key))
            .on_action(cx.listener(Self::show_replace))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(relative(0.4))
                            .min_w(px(300.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(t.border)
                            .child(head(self, cx))
                            .child(results(self, cx)),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(preview(self, cx))),
            )
            .child(foot(cx))
    }
}

fn head(pane: &SearchPane, cx: &mut Context<SearchPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let fields = pane.fields.as_ref().expect("made before drawing");
    let toggle = |id: &'static str,
                  label: &'static str,
                  tip: &'static str,
                  on: bool,
                  act: fn(&mut SearchPane, &mut Context<SearchPane>)| {
        BaseButton::new(id)
            .accessibility_label(tip)
            .tooltip(ui::tooltip(tip))
            .py(px(2.))
            .border_b_1()
            .border_color(if on {
                t.text_primary
            } else {
                gpui_kit::transparent_black()
            })
            .text_size(crate::theme::ui_text(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if on { t.text_primary } else { t.text_tertiary })
            .hover(move |style| style.text_color(t.text_primary))
            .on_click(cx.listener(move |pane, _, _, cx| act(pane, cx)))
            .child(label)
    };
    let row = || {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .min_w_0()
    };
    let mark = |glyph: AnyElement, ink: Hsla| {
        div()
            .size(px(16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::theme::ui_text(12.))
            .text_color(ink)
            .child(glyph)
    };
    let scope_field = |label: &'static str, field: &Entity<InputState>| {
        div()
            .flex()
            .items_center()
            .px(px(10.))
            .py(px(4.))
            .child(
                div()
                    .w(px(28.))
                    .flex_none()
                    .text_size(crate::theme::ui_text(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_tertiary)
                    .child(label.to_uppercase()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::theme::ui_text(11.5))
                    .child(Input::new(field).appearance(false)),
            )
    };
    let summary = pane.summary.as_ref();
    let has_results =
        summary.is_some_and(|summary| summary.match_count > 0) || !pane.files.is_empty();
    let can_replace = has_results && !pane.replacing;
    let matches = summary.map_or_else(
        || pane.files.iter().map(|file| file.matches.len()).sum(),
        |summary| summary.match_count,
    );
    let replace_label = if pane.replacing {
        "…".to_owned()
    } else if let Some(previewed) = &pane.previewed {
        format!("commit {}", previewed.match_count)
    } else if matches > 0 {
        format!("preview {matches}")
    } else {
        "preview".to_owned()
    };
    let replace_tip = match &pane.previewed {
        Some(previewed) => format!(
            "Commit {} replacements across {} files",
            previewed.match_count, previewed.file_count
        ),
        None if can_replace => "Preview replacements".to_owned(),
        None => "Run a search first".to_owned(),
    };
    div()
        .flex_none()
        .border_b_1()
        .border_color(t.border)
        .child(
            row()
                .border_b_1()
                .border_color(t.border)
                .child(
                    BaseButton::new("search-replace-open")
                        .accessibility_label(if pane.replace_open {
                            "Hide replace"
                        } else {
                            "Show replace"
                        })
                        .tooltip(ui::tooltip(if pane.replace_open {
                            "Hide replace"
                        } else {
                            "Show replace"
                        }))
                        .w(px(14.))
                        .h(px(18.))
                        .flex_none()
                        .text_size(crate::theme::ui_text(10.))
                        .text_color(if pane.replace_open {
                            t.text_primary
                        } else {
                            t.text_tertiary
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.replace_open = !pane.replace_open;
                            cx.notify();
                        }))
                        .child(if pane.replace_open { "▾" } else { "▸" }),
                )
                .child(mark(
                    ui::icon(Icon::Search, 11.)
                        .text_color(t.accent)
                        .into_any_element(),
                    t.accent,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(crate::theme::ui_text(12.))
                        .child(Input::new(&fields.find).appearance(false)),
                )
                .when(pane.status == Status::Searching, |row| {
                    row.child(
                        div()
                            .text_size(crate::theme::ui_text(10.))
                            .text_color(t.accent)
                            .child("…"),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .ml(px(4.))
                        .child(toggle(
                            "search-case",
                            "Aa",
                            "Match case",
                            pane.options.case_sensitive,
                            |pane, cx| pane.toggle(|options| &mut options.case_sensitive, cx),
                        ))
                        .child(toggle(
                            "search-word",
                            "ab",
                            "Whole word",
                            pane.options.whole_word,
                            |pane, cx| pane.toggle(|options| &mut options.whole_word, cx),
                        ))
                        .child(toggle(
                            "search-regex",
                            ".*",
                            "Regex",
                            pane.options.is_regex,
                            |pane, cx| pane.toggle(|options| &mut options.is_regex, cx),
                        )),
                ),
        )
        .when(pane.replace_open, |head| {
            head.child(
                row()
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(px(14.)).flex_none())
                    .child(mark(div().child("↻").into_any_element(), t.live))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::theme::ui_text(12.))
                            .text_color(t.live)
                            .child(Input::new(&fields.replace).appearance(false)),
                    ),
            )
        })
        .when(pane.scope_open, |head| {
            head.child(scope_field("in", &fields.include))
                .child(scope_field("not", &fields.exclude))
        })
        .child(
            row()
                .when(pane.scope_open, |row| {
                    row.border_t_1().border_color(t.border)
                })
                .py(px(6.))
                .child(toggle(
                    "search-scope",
                    "…",
                    if pane.scope_open {
                        "Hide file scope"
                    } else {
                        "Files to include / exclude"
                    },
                    pane.scope_open,
                    |pane, cx| {
                        pane.scope_open = !pane.scope_open;
                        cx.notify();
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .justify_end()
                        .items_center()
                        .gap(px(4.))
                        .text_size(crate::theme::ui_text(10.))
                        .text_color(t.text_tertiary)
                        .when(pane.stale, |stats| {
                            stats.child(
                                BaseButton::new("search-stale")
                                    .accessibility_label("Files changed on disk — rerun the search")
                                    .tooltip(ui::tooltip(
                                        "Files changed on disk — rerun the search",
                                    ))
                                    .mr(px(2.))
                                    .pr(px(6.))
                                    .border_b_1()
                                    .border_color(t.warn.opacity(0.5))
                                    .text_color(t.warn)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .on_click(cx.listener(|pane, _, _, cx| pane.rerun(cx)))
                                    .child("stale ↻"),
                            )
                        })
                        .map(|stats| match summary {
                            Some(summary) => stats
                                .child(
                                    div()
                                        .text_color(t.text_secondary)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(summary.match_count.to_string()),
                                )
                                .child("/")
                                .child(
                                    div()
                                        .text_color(t.text_secondary)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(summary.file_count.to_string()),
                                )
                                .child(div().ml(px(4.)).child(format!("{}ms", summary.elapsed_ms)))
                                .when(summary.truncated, |stats| {
                                    stats.child(div().ml(px(4.)).text_color(t.warn).child("trunc"))
                                })
                                .when(summary.cancelled, |stats| {
                                    stats
                                        .child(div().ml(px(4.)).text_color(t.warn).child("partial"))
                                }),
                            None => stats.child("·"),
                        }),
                )
                .when(pane.replace_open, |row| {
                    let commit = pane.previewed.is_some();
                    row.child(
                        BaseButton::new("search-replace-all")
                            .accessibility_label(SharedString::from(replace_tip.clone()))
                            .tooltip(ui::tooltip(replace_tip))
                            .disabled(!can_replace)
                            .py(px(2.))
                            .border_b_1()
                            .border_color(if !can_replace {
                                gpui_kit::transparent_black()
                            } else if commit {
                                t.live
                            } else {
                                t.border
                            })
                            .text_size(crate::theme::ui_text(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if !can_replace {
                                t.text_disabled
                            } else if commit {
                                t.live
                            } else {
                                t.text_primary
                            })
                            .on_click(
                                cx.listener(|pane, _, window, cx| pane.replace_all(window, cx)),
                            )
                            .child(replace_label),
                    )
                }),
        )
}

fn results(pane: &SearchPane, cx: &mut Context<SearchPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let notice = |text: String, sub: Option<&'static str>, ink: Hsla| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .px(px(14.))
            .py(px(24.))
            .text_size(crate::theme::ui_text(12.))
            .text_color(ink)
            .child(text)
            .children(sub.map(|sub| {
                div()
                    .text_size(crate::theme::ui_text(10.))
                    .opacity(0.7)
                    .text_color(t.text_tertiary)
                    .child(sub)
            }))
            .into_any_element()
    };
    if pane.status == Status::Failed {
        let error = pane.error.clone().unwrap_or_else(|| "search failed".into());
        let error = if error.contains("invalid") {
            format!("invalid regex: {error}")
        } else {
            error
        };
        return notice(error, None, t.danger);
    }
    if pane.query.trim().is_empty() {
        return notice(
            "Search across your project".into(),
            Some("Enter a word, symbol, or pattern above. Expand Replace to edit matching text."),
            t.text_tertiary,
        );
    }
    if pane.rows.is_empty() && pane.status == Status::Searching {
        return notice("searching…".into(), None, t.text_tertiary);
    }
    if pane.rows.is_empty() {
        return notice(
            format!("No matches for “{}”", pane.query),
            Some("Try a shorter term or adjust the case, pattern, and file filters above."),
            t.text_tertiary,
        );
    }
    let view = cx.entity();
    list(pane.list.clone(), move |index, _, cx| {
        let cx = &*cx;
        let pane = view.read(cx);
        match pane.rows.get(index) {
            Some(Row::Header { file }) => file_header(&view, &pane.files[*file], pane, cx),
            Some(Row::Hit { file, hit }) => hit_row(&view, &pane.files[*file], *hit, pane, cx),
            None => div().into_any_element(),
        }
    })
    .flex_1()
    .into_any_element()
}

fn file_header(
    view: &Entity<SearchPane>,
    file: &SearchFile,
    pane: &SearchPane,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let collapsed = pane.collapsed.contains(&file.path);
    let (dir, name) = match file.path.rsplit_once('/') {
        Some((dir, name)) => (Some(format!("{dir}/")), name.to_owned()),
        None => (None, file.path.clone()),
    };
    let glyph = match crate::file_icons::icon(&name, cx) {
        Some(icon) => img(icon).size(px(13.)).flex_none().into_any_element(),
        None => ui::icon(Icon::File, 13.)
            .text_color(t.text_tertiary)
            .into_any_element(),
    };
    let (folding, path) = (view.clone(), file.path.clone());
    div()
        .id(SharedString::from(format!("search-file:{}", file.path)))
        .w_full()
        .h(px(HEADER_HEIGHT))
        .pl(px(8.))
        .pr(px(12.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(crate::theme::ui_text(11.))
        .cursor_pointer()
        .hover(|row| row.bg(gpui_kit::hsla(0., 0., 1., 0.03)))
        .tooltip(ui::tooltip(if collapsed {
            "Expand file"
        } else {
            "Collapse file"
        }))
        .on_click(move |_, _, cx| {
            folding.update(cx, |pane, cx| {
                if !pane.collapsed.remove(&path) {
                    pane.collapsed.insert(path.clone());
                }
                pane.relist();
                cx.notify();
            })
        })
        .child(
            div()
                .w(px(10.))
                .flex_none()
                .text_size(crate::theme::ui_text(10.))
                .text_color(if collapsed {
                    t.text_tertiary
                } else {
                    t.text_secondary
                })
                .child(if collapsed { "▸" } else { "▾" }),
        )
        .child(glyph)
        .child(
            div()
                .flex_none()
                .text_color(t.text_primary)
                .font_weight(FontWeight::SEMIBOLD)
                .child(name),
        )
        .children(dir.map(|dir| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(10.))
                .text_color(t.text_tertiary)
                .child(dir)
        }))
        .child(
            div()
                .flex_none()
                .ml_auto()
                .text_size(crate::theme::ui_text(10.))
                .text_color(t.text_tertiary)
                .child(file.matches.len().to_string()),
        )
        .into_any_element()
}

fn hit_row(
    view: &Entity<SearchPane>,
    file: &SearchFile,
    at: usize,
    pane: &SearchPane,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hit = &file.matches[at];
    let chosen = pane
        .chosen
        .as_ref()
        .is_some_and(|(path, hit)| path == &file.path && *hit == at);
    let marks: Vec<(std::ops::Range<usize>, gpui_kit::HighlightStyle)> = hit
        .ranges
        .iter()
        .filter_map(|range| {
            let (start, end) = (range.start as usize, range.end as usize);
            (start < end
                && end <= hit.text.len()
                && hit.text.is_char_boundary(start)
                && hit.text.is_char_boundary(end))
            .then(|| {
                (
                    start..end,
                    gpui_kit::HighlightStyle {
                        color: Some(gpui_kit::rgb(0xffe27a).into()),
                        background_color: Some(gpui_kit::hsla(48. / 360., 1., 0.68, 0.2)),
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    let (choosing, opening, path) = (view.clone(), view.clone(), file.path.clone());
    let replaced = (!pane.replace.is_empty()).then(|| format!(" → {}", pane.replace));
    div()
        .id(SharedString::from(format!("search-hit:{}:{at}", file.path)))
        .w_full()
        .h(px(HIT_HEIGHT))
        .pl(px(20.))
        .pr(px(12.))
        .flex()
        .items_center()
        .rounded(RADIUS_2)
        .text_size(crate::theme::ui_text(11.))
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_secondary
        })
        .cursor_pointer()
        .map(|row| {
            if chosen {
                row.bg(t.surface_chosen)
            } else {
                row.hover(move |row| {
                    row.bg(gpui_kit::hsla(0., 0., 1., 0.03))
                        .text_color(t.text_primary)
                })
            }
        })
        .on_click(move |event, window, cx| {
            let path = path.clone();
            choosing.update(cx, |pane, cx| {
                window.focus(&pane.focus, cx);
                pane.choose(path, at, cx);
            });
            if event.click_count() > 1 {
                if let Some(open) = opening.read(cx).open_at() {
                    window.dispatch_action(Box::new(open), cx);
                }
            }
        })
        .child(
            div()
                .w(px(48.))
                .flex_none()
                .pr(px(10.))
                .text_right()
                .text_size(crate::theme::ui_text(10.))
                .text_color(if chosen {
                    t.text_primary
                } else {
                    t.text_tertiary
                })
                .child(hit.line.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .flex()
                .child(StyledText::new(SharedString::from(hit.text.clone())).with_highlights(marks))
                .children(replaced.map(|replaced| {
                    div()
                        .pl(px(4.))
                        .text_color(t.live)
                        .opacity(0.9)
                        .child(replaced)
                })),
        )
        .into_any_element()
}

fn preview(pane: &SearchPane, cx: &mut Context<SearchPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let notice = |text: &str| {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::theme::ui_text(12.))
            .text_color(t.text_tertiary)
            .child(text.to_owned())
            .into_any_element()
    };
    if pane.query.trim().is_empty() {
        return notice("preview will appear here");
    }
    if pane.files.is_empty() {
        return notice(if pane.status == Status::Searching {
            "searching…"
        } else {
            "nothing to preview"
        });
    }
    let Some((file, hit, at)) = pane.chosen_hit() else {
        return notice("no match selected");
    };
    let (dir, name) = match file.path.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_owned()),
        None => (String::new(), file.path.clone()),
    };
    let glyph = match crate::file_icons::icon(&name, cx) {
        Some(icon) => img(icon).size(px(13.)).flex_none().into_any_element(),
        None => ui::icon(Icon::File, 13.)
            .text_color(t.text_tertiary)
            .into_any_element(),
    };
    let shown = pane.windows.iter().find(|shown| {
        shown.path == file.path
            && hit.line >= shown.window.start_line
            && hit.line < shown.window.start_line + shown.window.doc.lines().count().max(1) as u32
    });
    let body: AnyElement = match shown {
        None => notice("loading…"),
        Some(shown) => window_lines(shown, hit, &t),
    };
    let tip = format!("Open {} at line {}", file.path, hit.line);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            BaseButton::new("search-preview-head")
                .accessibility_label(SharedString::from(tip.clone()))
                .tooltip(ui::tooltip(tip))
                .w_full()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .items_center()
                .justify_start()
                .gap(px(8.))
                .border_b_1()
                .border_color(t.border)
                .bg(gpui_kit::hsla(0., 0., 0., 0.22))
                .text_size(crate::theme::ui_text(11.))
                .hover(move |style| style.bg(t.surface_hover))
                .on_click(cx.listener(|pane, _, window, cx| pane.open(window, cx)))
                .child(glyph)
                .child(
                    div()
                        .text_color(t.text_primary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(10.))
                        .text_color(t.text_tertiary)
                        .child(dir),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .text_color(t.text_tertiary)
                        .child(format!(
                            "line {} · {} of {}",
                            hit.line,
                            at + 1,
                            file.matches.len()
                        ))
                        .when(!pane.replace.is_empty(), |meta| {
                            meta.child(
                                div()
                                    .text_color(t.live)
                                    .child(format!(" → {}", pane.replace)),
                            )
                        }),
                )
                .child(div().text_color(t.text_tertiary).child("↗")),
        )
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// The previewed lines, numbered, coloured as their language, with the chosen match's
/// line lit and its matches marked; scrolled so that line sits in the middle.
fn window_lines(shown: &Shown, hit: &SearchHit, t: &Tokens) -> AnyElement {
    let start = shown.window.start_line;
    let lines: Vec<&str> = shown.window.doc.split('\n').collect();
    let hit_at = hit.line.saturating_sub(start) as usize;
    let mark = gpui_kit::HighlightStyle {
        background_color: Some(gpui_kit::hsla(48. / 360., 1., 0.68, 0.25)),
        ..Default::default()
    };
    let rows = lines.iter().enumerate().map(|(index, line)| {
        let mut runs: Vec<(std::ops::Range<usize>, gpui_kit::HighlightStyle)> = shown
            .colours
            .as_ref()
            .and_then(|colours| colours.get(index))
            .map(|runs| runs.to_vec())
            .unwrap_or_default();
        if index == hit_at {
            let marks: Vec<_> = hit
                .ranges
                .iter()
                .map(|range| (range.start as usize..range.end as usize, mark))
                .filter(|(range, _)| {
                    range.end <= line.len()
                        && line.is_char_boundary(range.start)
                        && line.is_char_boundary(range.end)
                })
                .collect();
            runs = gpui_kit::combine_highlights(runs, marks).collect();
        }
        div()
            .h(px(PREVIEW_LINE))
            .flex()
            .when(index == hit_at, |row| row.bg(t.accent.opacity(0.08)))
            .child(
                div()
                    .w(px(38.))
                    .flex_none()
                    .px(px(6.))
                    .mr(px(10.))
                    .text_right()
                    .text_size(crate::theme::ui_text(11.))
                    .text_color(t.ink_faint)
                    .child((start as usize + index).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .px(px(14.))
                    .text_color(t.text_primary)
                    .child(
                        StyledText::new(SharedString::from(line.to_string())).with_highlights(runs),
                    ),
            )
    });
    let scroll = gpui_kit::ScrollHandle::new();
    let offset = (hit_at as f32 * PREVIEW_LINE - 200.).max(0.);
    scroll.set_offset(gpui_kit::point(px(0.), px(-offset)));
    div()
        .id("search-preview-lines")
        .size_full()
        .py(px(8.))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .text_size(crate::theme::ui_text(12.))
        .children(rows)
        .into_any_element()
}

fn foot(cx: &App) -> Div {
    let t = Tokens::get(cx);
    let hint = |keys: &'static str, what: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(ui::kbd(keys, cx))
            .child(what)
    };
    div()
        .flex_none()
        .flex()
        .gap(px(14.))
        .px(px(12.))
        .py(px(5.))
        .border_t_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(10.5))
        .text_color(t.text_tertiary)
        .child(hint("⏎", "open"))
        .child(hint("tab", "replace"))
        .child(hint("⌘⏎", "replace all"))
        .child(hint("⌘⇧F", "focus"))
}
