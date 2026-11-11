//! Git as the window shows it: a file's diff, and what an editor's text changed since
//! a revision.

use sikemux_core::server::call;
use sikemux_core::server::drafts::DraftsParams;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, font, px, Action, App, AppContext, Context, Div, HighlightStyle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled,
    StyledText, Task, Window,
};
use sikemux_core::git::diff::{changes, DiffRow, ROW_ADDED, ROW_DELETED, ROW_HIDDEN};
use sikemux_core::git::log::GitOverview;
use sikemux_core::git::status::DiscoveredRepo;
use sikemux_view::layout::PaneKind;
use sikemux_view::workspace::ProjectCodeHost;

use sikemux_view::owners::{CommitLines, FileOwners, Owners};

use crate::editor::OpenAt;
use crate::keyboard::TakesKeys;
use crate::reads::Read;
use crate::syntax::LineRuns;
use crate::theme::{Tokens, TEXT_2XS};
use crate::workbench::Workbench;
use crate::{syntax, tabs, theme, ui};
use sikemux_core::server::git::call as git_call;

/// What the Git tab holds: the local workbench pane.
#[derive(Default)]
pub struct GitTab {
    /// The Git pane of the project whose Git tab was last in front.
    pub(crate) pane: Option<gpui_kit::Entity<GitPane>>,
    /// The repository picked from the list inside a project folder, by the folder.
    pub(crate) picked: Option<(String, String)>,
    /// The list of repositories inside the project folder shows in the Git tab.
    pub(crate) listing: bool,
    /// The repository changed while the Git pane was out of sight, so it reads it and its
    /// review again once it shows rather than on every change.
    pub(crate) behind: bool,
    /// The commit to show once the pane exists.
    pub(crate) wanted_commit: Option<String>,
    /// The slice a change pinned on a line of that commit goes to.
    pub(crate) wanted_pin: Option<review::DiffPin>,
}

impl GitTab {
    /// Hands the repository's overview to the Git pane, or marks it behind while it is out of
    /// sight.
    pub fn overview(
        &mut self,
        overview: &Result<GitOverview, String>,
        in_front: bool,
        cx: &mut App,
    ) {
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
}

pub use pane::GitPane;
pub use sikemux_core::git::diff::DiffKind;

pub(crate) mod pane;
pub mod repos;

/// The key the core keeps `repo`'s commit message under.
fn draft_key(repo: &str) -> String {
    format!("git:{repo}")
}

/// The project's Git view: the repository picker while none is chosen, else its pane, which
/// takes the keys when it was just opened.
pub(crate) fn pane(
    bench: &Workbench,
    window: &gpui_kit::Window,
    cx: &mut gpui_kit::Context<Workbench>,
) -> gpui_kit::AnyElement {
    if bench.git_repo().is_none() && bench.git_repos().is_some() {
        return ui::pane(cx)
            .child(repos::picker(bench, cx))
            .into_any_element();
    }
    match &bench.git.pane {
        Some(pane) => {
            if bench.claims_keys(crate::keyboard::TakesKeys::Git) {
                let focused = pane.clone();
                cx.defer_in(window, move |_, window, cx| {
                    focused.update(cx, |pane, cx| pane.take_keys(window, cx));
                });
            }
            pane.clone().into_any_element()
        }
        None => ui::pane(cx).into_any_element(),
    }
}
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
}

/// What changed from `base` to `live`, worked out off the main thread.
pub fn hunks(base: SharedString, live: String, cx: &App) -> Task<Vec<Hunk>> {
    cx.background_executor().spawn(async move {
        changes(&base, &live)
            .into_iter()
            .map(|change| Hunk {
                kind: change.kind(),
                lines: change.new_start as u32..(change.new_start + change.new_len) as u32,
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
    target.request_typed::<git_call::FileAt>(git_call::FileAt {
        repo: repo.to_owned(),
        rev: rev.to_owned(),
        path: path.to_owned(),
    })
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

    pub fn len(&self) -> usize {
        self.rows.len()
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
                    gutter.cursor_pointer().on_click(move |_, window, cx| {
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

/// Who wrote which lines, as the core said of a checkout or of a commit.
#[derive(Default)]
pub struct WhoWrote<'a> {
    names: Vec<&'a str>,
    files: &'a [FileOwners],
}

impl<'a> WhoWrote<'a> {
    pub fn of_checkout(owners: &'a Owners) -> Self {
        Self {
            names: owners
                .authors
                .iter()
                .map(|author| author.name.as_str())
                .collect(),
            files: &owners.files,
        }
    }

    pub fn of_commit(lines: &'a CommitLines) -> Self {
        Self {
            names: lines
                .authors
                .iter()
                .map(|author| author.name.as_str())
                .collect(),
            files: &lines.files,
        }
    }
}

/// What the core says to note beside a file's changed lines, in the app's words: who
/// wrote each line, and who took out the lines just above a line.
#[derive(Default)]
pub struct Notes {
    lines: HashMap<u32, SharedString>,
    gaps: HashMap<u32, SharedString>,
}

impl Notes {
    /// The notes for `path` in what the core said of who wrote what.
    pub fn of(owners: &WhoWrote<'_>, path: &str) -> Self {
        let name = |author: usize| owners.names.get(author).copied();
        let Some(file) = owners.files.iter().find(|file| file.path == path) else {
            return Self::default();
        };
        let lines = (file.notes.iter())
            .map(|note| {
                let mut said = Vec::new();
                if note.named {
                    let author = note.author.and_then(name);
                    said.push(author.unwrap_or("unattributed").to_owned());
                }
                if let Some(over) = note.over.and_then(name) {
                    said.push(format!("changed {over}'s edit"));
                }
                (note.line as u32, said.join(" · ").into())
            })
            .collect();
        let gaps = (file.removed.iter())
            .filter_map(|removal| {
                let name = name(removal.author)?;
                Some((removal.at as u32, name.to_owned().into()))
            })
            .collect();
        Self { lines, gaps }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.gaps.is_empty()
    }
}

/// The width of a character of code, which every character of the code font has.
pub(crate) fn advance(window: &Window) -> Pixels {
    let text = window.text_system();
    let id = text.resolve_font(&font(theme::MONO));
    text.advance(id, CODE_SIZE, 'm')
        .map_or(CODE_SIZE * 0.6, |size| size.width)
}

impl Workbench {
    /// Tells the Git pane whether its host is in reach, and its repository's code host.
    pub(crate) fn tell_reach(&self, cx: &mut Context<Self>) {
        let away = self.out_of_reach();
        if let Some(pane) = &self.git.pane {
            let host = self.code_host_of(&pane.read(cx).repo);
            pane.update(cx, |pane, cx| {
                pane.set_out_of_reach(away, cx);
                pane.set_code_host(host, cx);
            });
        }
    }

    /// The code host of the project holding `repo`, with the account chosen for it there.
    pub(crate) fn code_host_of(&self, repo: &str) -> Option<(ProjectCodeHost, Option<String>)> {
        let project = self
            .projects()
            .iter()
            .filter(|project| Path::new(repo).starts_with(&project.path))
            .max_by_key(|project| project.path.as_os_str().len())?;
        let host = project.code_host.clone()?;
        let account = project
            .code_hosts
            .get(&host.host)
            .and_then(|choice| choice.account.clone());
        Some((host, account))
    }

    /// The repository the Git views of the project in front open on: one picked from the
    /// list, else the core's pick, the folder when it is one or the one chosen among those
    /// inside it. None while the person has to pick.
    pub(crate) fn git_repo(&self) -> Option<String> {
        let folder = self.watched.project.as_ref()?;
        if self.git.listing {
            return None;
        }
        if let Some((at, repo)) = &self.git.picked {
            if at == folder {
                return Some(repo.clone());
            }
        }
        let found = self.feeds.in_front().git_project_repo.as_ref()?;
        match &found.repo {
            Some(repo) => Some(repo.clone()),
            None if found.repos.is_some() => None,
            None => Some(folder.clone()),
        }
    }

    /// The repositories directly inside the project folder, while it is not one itself.
    pub(crate) fn git_repos(&self) -> Option<&Vec<DiscoveredRepo>> {
        self.feeds
            .in_front()
            .git_project_repo
            .as_ref()?
            .repos
            .as_ref()
    }

    /// Opens `repo` in the project's Git views until the project changes.
    pub(crate) fn pick_git_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git.picked = self.watched.project.clone().map(|folder| (folder, repo));
        self.git.listing = false;
        self.follow_git_repo(was, cx);
    }

    /// Shows the repositories inside the project folder again.
    pub(crate) fn list_git_repos(&mut self, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git.listing = true;
        self.follow_git_repo(was, cx);
    }

    /// Points the git overview at the repository shown, when it moved from `was`.
    pub(crate) fn follow_git_repo(&mut self, was: Option<String>, cx: &mut Context<Self>) {
        let repo = self.git_repo();
        if let Some(repo) = repo.as_ref().filter(|repo| {
            Some(*repo) != was.as_ref() || self.feeds.in_front().git_overview.as_ref().is_none()
        }) {
            let target = self.target();
            target.watch_folder::<git_call::Overview>(
                Read::GitOverview,
                git_call::Overview { repo: repo.clone() },
                repo.clone(),
            );
            let draft = draft_key(repo);
            target.watch::<call::Drafts>(
                Read::GitDraft,
                Some(DraftsParams {
                    keys: Some(vec![draft]),
                }),
            );
        }
        self.parts.redraw_stage(cx);
    }

    pub(crate) fn git_in_front(&self) -> bool {
        self.tab()
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| kind == PaneKind::Git)
    }

    /// Makes the Git pane when the project's Git tab comes to the front.
    pub(crate) fn git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        let Some(repo) = self.git_repo() else {
            return;
        };
        if project.path.to_string_lossy() != *self.watched.project.as_deref().unwrap_or_default() {
            return;
        }
        let target = self.target();
        if let Some(pane) = self.git.pane.as_ref().filter(|pane| {
            let pane = pane.read(cx);
            pane.repo == repo && pane.place() == target.place()
        }) {
            if std::mem::take(&mut self.git.behind) {
                let overview = self.feeds.in_front().git_overview.as_ref();
                pane.update(cx, |pane, cx| {
                    if let Some(overview) = overview {
                        pane.overview(overview, cx);
                    }
                    pane.git_moved(None, None, cx);
                });
            }
            return;
        }
        self.git.behind = false;
        let bench = cx.entity().downgrade();
        let away = self.out_of_reach();
        let host = self.code_host_of(&repo);
        let pane = cx.new(|cx| GitPane::new(target, repo, bench, window, cx));
        pane.update(cx, |pane, cx| {
            pane.set_out_of_reach(away, cx);
            pane.set_code_host(host, cx);
        });
        if let Some(overview) = self.feeds.in_front().git_overview.as_ref() {
            pane.update(cx, |pane, cx| pane.overview(overview, cx));
        }
        if !self.reads.catalog.is_empty() {
            let catalog = &self.reads.catalog;
            pane.update(cx, |pane, cx| pane.catalog(catalog, cx));
        }
        if let Some(drafts) = self.feeds.in_front().git_draft.as_ref() {
            pane.update(cx, |pane, cx| pane.drafts(drafts, cx));
        }
        if let Some(pin) = self.git.wanted_pin.take() {
            pane.update(cx, |pane, _| pane.review.pin_on(pin));
        }
        if let Some(sha) = self.git.wanted_commit.take() {
            pane.update(cx, |pane, cx| pane.show_commit(&sha, cx));
        }
        self.git.pane = Some(pane);
    }

    /// Brings the project's Git tab to the front, opening one when it has none.
    pub fn open_git(&mut self, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Git, cx);
        if let Some(project) = self.project() {
            self.open_in_front(OpenTab::new(
                At::project(&project.id),
                PaneKind::Git,
                project.path.to_string_lossy(),
            ));
        }
    }

    /// Opens the project's Git on the change from `base` to `sha` touching `files`, whose lines
    /// take a change for the slice `pinning` names.
    pub(crate) fn open_change_pinned(
        &mut self,
        base: String,
        sha: &str,
        files: Vec<String>,
        pinning: crate::goals::Pinning,
        cx: &mut Context<Self>,
    ) {
        let pin = review::DiffPin {
            pinning,
            base,
            head: sha.to_owned(),
            files,
            bench: cx.entity().downgrade(),
        };
        match self.git.pane.clone() {
            Some(pane) => pane.update(cx, |pane, _| pane.review.pin_on(pin)),
            None => self.git.wanted_pin = Some(pin),
        }
        self.open_commit(sha, cx);
    }

    pub fn open_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        self.open_git(cx);
        match self.git.pane.clone() {
            Some(pane) => pane.update(cx, |pane, cx| pane.show_commit(sha, cx)),
            None => self.git.wanted_commit = Some(sha.to_owned()),
        }
    }
}
