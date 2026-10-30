//! Git as the window shows it: a file's diff, and what an editor's text changed since
//! a revision.

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, font, list, px, Action, App, Context, Div, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Pixels, Render,
    ScrollEvent, SharedString, StatefulInteractiveElement, Styled, StyledText, Task, Window,
};
use serde_json::{json, Value};
use sikemux_core::git::diff::{changes, DiffRow, ROW_ADDED, ROW_DELETED, ROW_HIDDEN};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{request, Ask};
use crate::editor::OpenAt;
use crate::syntax::{self, LineRuns};
use crate::theme::{self, Tokens, SPACE_2, SPACE_3, TEXT_2XS};
use crate::ui;

/// What the Git tab shows and holds: the local workbench pane or one of the code host's areas.
pub struct GitTab {
    /// The Git pane of the project whose Git tab was last in front.
    pub(crate) pane: Option<gpui_kit::Entity<GitPane>>,
    /// What the Git tab shows: the local workbench, or one of the code host's areas.
    pub(crate) area: crate::codehost::Area,
    /// The code host's areas of the project whose Git tab last showed one.
    pub(crate) host: Option<gpui_kit::Entity<crate::codehost::HostArea>>,
    /// The repository picked from the list inside a project folder, by the folder.
    pub(crate) picked: Option<(String, String)>,
    /// The list of repositories inside the project folder shows in the Git tab.
    pub(crate) listing: bool,
    /// A pull request or issue to open once the code host's area shows.
    pub(crate) opening: Option<(crate::codehost::Area, u64, String)>,
    /// The repository changed while the Git pane was out of sight, so it reads it and its
    /// review again once it shows rather than on every change.
    pub(crate) behind: bool,
}

impl GitTab {
    /// Hands the repository's overview to the Git pane, or marks it behind while it is out of
    /// sight.
    pub fn overview(&mut self, overview: &Value, in_front: bool, cx: &mut App) {
        match (&self.pane, in_front) {
            (Some(pane), true) => pane.update(cx, |pane, cx| pane.overview(overview, cx)),
            (Some(_), false) => self.behind = true,
            (None, _) => {}
        }
    }

    pub fn drafts(&self, drafts: &crate::reads::Drafts, cx: &mut App) {
        if let Some(pane) = &self.pane {
            pane.update(cx, |pane, cx| pane.drafts(drafts, cx));
        }
    }

    pub fn catalog(&self, catalog: &crate::reads::Catalog, cx: &mut App) {
        if let Some(pane) = &self.pane {
            pane.update(cx, |pane, cx| pane.catalog(catalog, cx));
        }
    }

    /// Hands the code host's areas what the core read for them under `key`.
    pub fn host_read(&self, key: &str, value: &Value, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |host, cx| host.read(key, value, cx));
        }
    }
}

pub use crate::codehost::{Area, OpenArea};
pub use pane::{FocusGit, GitPane};
pub use sikemux_core::git::diff::DiffKind;

pub(crate) mod pane;
pub mod repos;
mod review;

/// A diff side longer than this is drawn without colours.
const COLOURED_MAX_LINES: usize = 10_000;
const ROW_HEIGHT: Pixels = px(19.);
const HIDDEN_HEIGHT: Pixels = px(4.);
const CODE_SIZE: Pixels = px(12.);
const TAB_SIZE: usize = 2;

/// A run of an editor's lines that differs from the revision it is compared with.
pub struct Hunk {
    pub kind: DiffKind,
    /// The run in the editor's text, counted from 0. A deletion's is empty, at the line
    /// it sits above.
    pub lines: Range<u32>,
    /// What the run was in the revision.
    pub old: SharedString,
}

/// What changed from `base` to `live`, worked out off the main thread.
pub fn hunks(base: SharedString, live: String, cx: &App) -> Task<Vec<Hunk>> {
    cx.background_executor().spawn(async move {
        let old: Vec<&str> = base.split_inclusive('\n').collect();
        changes(&base, &live)
            .into_iter()
            .map(|change| Hunk {
                kind: change.kind(),
                lines: change.new_start as u32..(change.new_start + change.new_len) as u32,
                old: old
                    .get(change.old_start..change.old_start + change.old_len)
                    .map_or_else(String::new, |lines| lines.concat())
                    .into(),
            })
            .collect()
    })
}

/// A file's text at a revision, such as `HEAD` or `:index`.
pub fn file_at(
    target: &crate::core::Target,
    repo: &str,
    rev: &str,
    path: &str,
) -> impl Future<Output = Result<String, String>> {
    let answer = target.request(
        "git.fileAt",
        json!({ "repo": repo, "rev": rev, "path": path }),
    );
    async move {
        match answer.await? {
            Value::String(text) => Ok(text),
            other => Err(format!("the core answered git.fileAt with {other}")),
        }
    }
}

/// Which file a diff compares, and between what.
#[derive(Clone, PartialEq, Eq)]
pub struct Compared {
    pub repo: String,
    /// From the repository's root.
    pub path: String,
    pub base: String,
    /// None compares with the working tree.
    pub head: Option<String>,
}

/// A file's diff as the core's rows give it, with each side's text kept whole so it
/// colours as a file of its own.
pub struct Diff {
    rows: Vec<DiffRow>,
    /// The old side holds unchanged and deleted lines, the new side unchanged and added.
    sides: [SharedString; 2],
    /// Each row's line in the side it is drawn from.
    at: Vec<u32>,
    /// Each row's line in the new text, from 0: where a deleted row sits above.
    targets: Vec<u32>,
    digits: usize,
    widest: usize,
}

impl Diff {
    pub fn new(mut rows: Vec<DiffRow>) -> Self {
        let mut sides: [Vec<&str>; 2] = [Vec::new(), Vec::new()];
        for row in &mut rows {
            if row.2.contains('\t') {
                row.2 = expand_tabs(&row.2);
            }
        }
        let at = rows
            .iter()
            .map(|row| {
                let side = usize::from(row.0 != ROW_DELETED);
                let line = sides[side].len() as u32;
                match row.0 {
                    ROW_HIDDEN => {}
                    ROW_ADDED | ROW_DELETED => sides[side].push(&row.2),
                    _ => sides.iter_mut().for_each(|lines| lines.push(&row.2)),
                }
                line
            })
            .collect();
        let mut next = rows
            .iter()
            .rev()
            .find(|row| row.0 != ROW_DELETED && row.0 != ROW_HIDDEN)
            .map_or(0, |row| row.1);
        let mut targets: Vec<u32> = rows
            .iter()
            .rev()
            .map(|row| {
                if row.0 != ROW_DELETED && row.0 != ROW_HIDDEN {
                    next = row.1.saturating_sub(1);
                }
                next
            })
            .collect();
        targets.reverse();
        let largest = rows.iter().map(|row| row.1).max().unwrap_or(0);
        Self {
            sides: sides.map(|lines| lines.join("\n").into()),
            digits: largest.to_string().len().max(3),
            widest: rows
                .iter()
                .map(|row| row.2.chars().count())
                .max()
                .unwrap_or(0),
            at,
            targets,
            rows,
        }
    }

    pub fn row(&self, index: usize) -> Option<&DiffRow> {
        self.rows.get(index)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// How many lines it adds and how many it takes out.
    pub fn stat(&self) -> (u64, u64) {
        let count = |kind| self.rows.iter().filter(|row| row.0 == kind).count() as u64;
        (count(ROW_ADDED), count(ROW_DELETED))
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The line in the new text a row stands for, from 0, as an editor opens it.
    pub fn target(&self, row: usize) -> u32 {
        self.targets[row]
    }

    /// Each row's note: an added row's by its line, and where lines were taken out with
    /// nothing put in their place, the first of them by the line below.
    pub fn notes(&self, notes: &Notes) -> Vec<Option<SharedString>> {
        if notes.is_empty() {
            return Vec::new();
        }
        let mut shown = vec![None; self.rows.len()];
        let mut block: Option<(usize, bool)> = None;
        for (index, DiffRow(kind, number, _)) in self.rows.iter().enumerate() {
            match *kind {
                ROW_ADDED => {
                    shown[index] = notes.lines.get(number).cloned();
                    block = Some((block.map_or(index, |(start, _)| start), true));
                }
                ROW_DELETED => block = Some(block.unwrap_or((index, false))),
                _ => {
                    if let Some((start, false)) = block.take() {
                        shown[start] = notes.gaps.get(&(self.targets[start] + 1)).cloned();
                    }
                }
            }
        }
        if let Some((start, false)) = block {
            shown[start] = notes.gaps.get(&(self.targets[start] + 1)).cloned();
        }
        shown
    }

    /// The widest the diff is drawn, so it scrolls sideways only as far as its longest line.
    pub fn width(&self, advance: Pixels) -> Pixels {
        advance * (self.digits + self.widest + 5) as f32
    }

    fn colourable(&self) -> bool {
        self.sides
            .iter()
            .all(|side| side.lines().count() <= COLOURED_MAX_LINES)
    }
}

fn expand_tabs(line: &str) -> String {
    let mut expanded = String::with_capacity(line.len() + TAB_SIZE);
    for character in line.chars() {
        if character == '\t' {
            let column = expanded.chars().count();
            expanded.extend(std::iter::repeat_n(' ', TAB_SIZE - column % TAB_SIZE));
        } else {
            expanded.push(character);
        }
    }
    expanded
}

/// Each side's colours, in the theme they were worked out in.
#[derive(Default)]
pub struct Colours {
    sides: [Option<Arc<[LineRuns]>>; 2],
    version: Option<u64>,
}

impl Colours {
    /// Starts colouring `diff`'s sides in `language` when they are not coloured in the
    /// current theme, telling `view` when they are.
    pub fn ask<V: 'static>(
        &mut self,
        diff: &Diff,
        language: Option<&str>,
        cx: &mut Context<V>,
        coloured: impl Fn(&mut V) -> Option<&mut Colours> + Clone + 'static,
    ) {
        let version = syntax::version(cx);
        if self.version == Some(version) {
            return;
        }
        self.version = Some(version);
        self.sides = [None, None];
        let Some(language) = language.filter(|_| diff.colourable()) else {
            return;
        };
        for (side, text) in diff.sides.iter().enumerate() {
            let task = syntax::colour_lines(language, text.clone(), cx);
            let coloured = coloured.clone();
            cx.spawn(async move |view, cx| {
                let Some(lines) = task.await else {
                    return;
                };
                view.update(cx, |view, cx| {
                    if let Some(colours) =
                        coloured(view).filter(|colours| colours.version == Some(version))
                    {
                        colours.sides[side] = Some(lines);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn of(&self, diff: &Diff, row: usize) -> &[(Range<usize>, HighlightStyle)] {
        let side = usize::from(diff.rows[row].0 != ROW_DELETED);
        self.sides[side]
            .as_ref()
            .and_then(|lines| lines.get(diff.at[row] as usize))
            .map_or(&[], Vec::as_slice)
    }
}

/// How a diff's rows are drawn: the theme's colours, and how far it is scrolled sideways.
pub struct RowLook {
    /// Said after the row's code, such as the agent that wrote it.
    pub note: Option<SharedString>,
    /// How strongly an added or deleted row's colour washes its code.
    pub wash: f32,
    pub added: Hsla,
    pub deleted: Hsla,
    pub number: Hsla,
    pub gutter: Hsla,
    pub ink: Hsla,
    pub hover: Hsla,
    pub advance: Pixels,
    pub scrolled: Pixels,
}

impl RowLook {
    pub fn new(t: &Tokens, advance: Pixels, scrolled: Pixels) -> Self {
        Self {
            note: None,
            wash: if t.dark { 0.08 } else { 0.14 },
            added: t.live,
            deleted: t.danger,
            number: t.ink_faint,
            gutter: t.surface_sunken.opacity(0.3),
            ink: t.text_primary,
            hover: t.surface_hover,
            advance,
            scrolled,
        }
    }
}

/// One row of a diff: its one number column, which opens `file` in the editor at its
/// line, then its code. A hidden row is a bar that calls `expand` when clicked.
pub fn row(
    diff: &Diff,
    colours: &Colours,
    index: usize,
    look: &RowLook,
    id: SharedString,
    file: Option<&Path>,
    expand: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let DiffRow(kind, number, text) = &diff.rows[index];
    if *kind == ROW_HIDDEN {
        let hover = look.hover;
        let lines = if *number == 1 { "line" } else { "lines" };
        return div().child(
            div()
                .id(id)
                .h(HIDDEN_HEIGHT)
                .w_full()
                .cursor_pointer()
                .hover(move |bar| bar.bg(hover))
                .tooltip(ui::tooltip(format!("Show {number} unchanged {lines}")))
                .on_click(move |_, window, cx| expand(window, cx)),
        );
    }
    let tint = match *kind {
        ROW_ADDED => Some(look.added),
        ROW_DELETED => Some(look.deleted),
        _ => None,
    };
    let digits = look.advance * diff.digits as f32;
    let open = file.map(|file| OpenAt {
        path: file.to_owned(),
        line: diff.target(index),
        column: None,
    });
    div()
        .h(ROW_HEIGHT)
        .w_full()
        .flex()
        .overflow_hidden()
        .child(
            div()
                .id(id)
                .flex_none()
                .w(digits + look.advance * 3. + px(3.))
                .pl(look.advance * 2.)
                .pr(look.advance)
                .border_r_2()
                .whitespace_nowrap()
                .bg(look.gutter)
                .text_right()
                .text_color(tint.unwrap_or(look.number))
                .when_some(open, |gutter, open| {
                    gutter
                        .cursor_pointer()
                        .tooltip(ui::tooltip(format!("Open at line {}", open.line + 1)))
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(open.boxed_clone(), cx)
                        })
                })
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .when_some(tint, |code, tint| code.bg(tint.opacity(look.wash)))
                .child(
                    div()
                        .ml(-look.scrolled)
                        .px(look.advance)
                        .flex()
                        .whitespace_nowrap()
                        .text_color(look.ink)
                        .child(
                            StyledText::new(SharedString::from(text.clone()))
                                .with_highlights(colours.of(diff, index).iter().cloned()),
                        )
                        .when_some(look.note.clone(), |code, note| {
                            code.child(
                                div()
                                    .ml(look.advance * 4.)
                                    .font_family(theme::UI)
                                    .text_size(crate::theme::ui_text(TEXT_2XS))
                                    .text_color(look.number)
                                    .child(note),
                            )
                        }),
                ),
        )
}

/// What the core says to note beside a file's changed lines, in the app's words: who
/// wrote each line, and who took out the lines just above a line.
#[derive(Default)]
pub struct Notes {
    lines: HashMap<u32, SharedString>,
    gaps: HashMap<u32, SharedString>,
}

impl Notes {
    /// The notes for `path` in what `changeOwners` answered.
    pub fn of(owners: &Value, path: &str) -> Self {
        let names: Vec<&str> = owners["authors"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|author| author["name"].as_str().unwrap_or_default())
            .collect();
        let name = |author: &Value| {
            author
                .as_u64()
                .and_then(|author| names.get(author as usize))
                .copied()
        };
        let Some(file) = owners["files"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|file| file["path"] == path)
        else {
            return Self::default();
        };
        let lines = file["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|note| {
                let mut said = Vec::new();
                if note["named"] == true {
                    said.push(name(&note["author"]).unwrap_or("unattributed").to_owned());
                }
                if let Some(over) = name(&note["over"]) {
                    said.push(format!("changed {over}'s edit"));
                }
                Some((note["line"].as_u64()? as u32, said.join(" · ").into()))
            })
            .collect();
        let gaps = file["removed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|removal| {
                let name = name(&removal["author"])?;
                Some((removal["at"].as_u64()? as u32, name.to_owned().into()))
            })
            .collect();
        Self { lines, gaps }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.gaps.is_empty()
    }
}

/// One file's diff in a pane of its own, read from the core and coloured as the file's
/// language, drawn only as far as it shows.
pub struct DiffView {
    asks: UnboundedSender<Ask>,
    compared: Compared,
    language: Option<String>,
    /// Asks for every unchanged line, after the person opened a hidden run.
    whole: bool,
    diff: Option<Result<Diff, String>>,
    colours: Colours,
    rows: ListState,
    scrolled: Pixels,
    /// How wide the pane last drew.
    shown: Rc<Cell<Pixels>>,
    _reading: Task<()>,
}

impl DiffView {
    pub fn new(asks: UnboundedSender<Ask>, compared: Compared, cx: &mut Context<Self>) -> Self {
        let language = crate::file_icons::language(&compared.path, cx)
            .filter(|language| syntax::knows(language));
        let mut view = Self {
            asks,
            compared,
            language,
            whole: false,
            diff: None,
            colours: Colours::default(),
            rows: ListState::new(0, ListAlignment::Top, px(600.)),
            scrolled: px(0.),
            shown: Rc::default(),
            _reading: Task::ready(()),
        };
        view.read(cx);
        view
    }

    /// Reads the diff again, as after the file or the repository changed.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let Compared {
            repo,
            path,
            base,
            head,
        } = &self.compared;
        let params = json!({ "repo": repo, "path": path, "baseRev": base, "headRev": head, "full": self.whole });
        let answer = request(&self.asks, "git.fileDiff", params);
        self._reading = cx.spawn(async move |view, cx| {
            let diff = answer.await.and_then(|rows| {
                serde_json::from_value::<Vec<DiffRow>>(rows).map_err(|error| error.to_string())
            });
            view.update(cx, |view, cx| view.read_as(diff.map(Diff::new), cx))
                .ok();
        });
    }

    fn read_as(&mut self, diff: Result<Diff, String>, cx: &mut Context<Self>) {
        if let Ok(diff) = &diff {
            let first = self.rows.logical_scroll_top();
            self.rows.reset(diff.len());
            self.rows.scroll_to(first);
            self.colours = Colours::default();
        }
        self.diff = Some(diff);
        cx.notify();
    }

    fn expand(&mut self, cx: &mut Context<Self>) {
        if !self.whole {
            self.whole = true;
            self.read(cx);
        }
    }

    fn wheel(&mut self, event: &ScrollEvent, window: &Window, width: Pixels) -> bool {
        let delta = event.delta.pixel_delta(window.line_height());
        if delta.x == px(0.) || delta.x.abs() < delta.y.abs() {
            return false;
        }
        let widest = (width - self.shown.get()).max(px(0.));
        let scrolled = (self.scrolled - delta.x).clamp(px(0.), widest);
        let moved = scrolled != self.scrolled;
        self.scrolled = scrolled;
        moved
    }
}

/// The width of a character of code, which every character of the code font has.
pub(crate) fn advance(window: &Window) -> Pixels {
    let text = window.text_system();
    let id = text.resolve_font(&font(theme::MONO));
    text.advance(id, CODE_SIZE, 'm')
        .map_or(CODE_SIZE * 0.6, |size| size.width)
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let message = |text: String, color| {
            div()
                .p(SPACE_3)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let diff = match &self.diff {
            None => return message("loading diff...".into(), t.text_tertiary),
            Some(Err(error)) => return message(format!("x {error}"), t.danger),
            Some(Ok(diff)) if diff.is_empty() => {
                return message("No changes.".into(), t.text_tertiary)
            }
            Some(Ok(diff)) => diff,
        };
        self.colours
            .ask(diff, self.language.as_deref(), cx, |view: &mut Self| {
                Some(&mut view.colours)
            });
        let advance = advance(window);
        let width = diff.width(advance);
        let view = cx.entity();
        let scrolled = self.scrolled;
        let file = Path::new(&self.compared.repo).join(&self.compared.path);
        let rows = list(self.rows.clone(), move |index, _, cx| {
            let this = view.read(cx);
            let Some(Ok(diff)) = &this.diff else {
                return div().into_any_element();
            };
            let look = RowLook::new(Tokens::get(cx), advance, scrolled);
            let expanding = view.clone();
            row(
                diff,
                &this.colours,
                index,
                &look,
                SharedString::from(format!("diff-row-{index}")),
                Some(&file),
                move |_, cx| expanding.update(cx, |view, cx| view.expand(cx)),
            )
            .into_any_element()
        });
        let shown = self.shown.clone();
        div()
            .id("diff")
            .size_full()
            .py(SPACE_2)
            .font_family(theme::MONO)
            .text_size(CODE_SIZE)
            .line_height(ROW_HEIGHT)
            .on_scroll(cx.listener(move |view, event: &ScrollEvent, window, cx| {
                if view.wheel(event, window, width) {
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| shown.set(bounds.size.width),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(rows.size_full())
            .into_any_element()
    }
}

/// Tells `list` only the run of `old` that `new` replaced, so what is in view stays put.
pub(crate) fn relist<T: PartialEq>(list: &ListState, old: &[T], new: &[T]) {
    if list.item_count() != old.len() {
        list.reset(new.len());
        return;
    }
    let start = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old == new)
        .count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    list.splice(start..old.len() - end, new.len() - start - end);
}
