//! The Git pane: the branch and what syncs it, the changes to commit with the commit
//! box, and every changed file's diff beside them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::FRAC_PI_2;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, list, px, radians, relative, Action, Anchor, AnyElement, App, AppContext,
    ClipboardItem, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ListAlignment, ListState, ParentElement, Render,
    ScrollEvent, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    Transformation, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_core::git::log::{GitCommit, GitOverview};
use sikemux_core::git::stash::GitStash;
use sikemux_core::git::status::{GitChange, GitFile};
use sikemux_core::server::call;
use sikemux_core::server::drafts::SetDraftParams;
use sikemux_view::workspace::ProjectCodeHost;

use branches::{branch_card, branches_page, Branches};
use history::{commit_card, commit_row, graph, Graph, Inks};

mod actions;
mod branches;
mod history;
mod keys;
mod menus;
mod owners;
mod view;

use super::review::{review, Review};
use crate::agents::rows::AgentRow;
use crate::core::Target;
use crate::editor::OpenAt;
use crate::files::Mark;
use crate::git::WhoWrote;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    SPACE_4, SPACE_5, TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::track::{self, Scrolled, Slide};
use crate::ui::{self, Cheatsheet, Choice, Choices, Confirm, Prompt};
use crate::workbench::Workbench;
use actions::*;
use keys::*;
use serde::Serialize;
use sikemux_core::server::call::Call;
use sikemux_core::server::git::call as git_call;
use sikemux_view::owners::{
    AgentOwner, CheckoutParams, CommitLines, CommitParams, CommitsParams, Owners,
};
use view::*;
pub(super) use view::{file_actions, hover};

pub(crate) const ROW_HEIGHT: f32 = 26.;
/// A page's filter: its field once shown, and what it finds.
#[derive(Default)]
struct Filter {
    field: Option<Entity<InputState>>,
    query: String,
    /// The Changes filter shows once asked for; the other pages' always do.
    open: bool,
    typing: Option<Subscription>,
}

/// What the review column shows.
enum Shown {
    Changes,
    Commit(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Staged,
    Unstaged,
}

/// How a file reads once it has moved to `side`, before git has said so.
fn moved_to(file: &GitFile, side: Side) -> GitFile {
    match side {
        Side::Staged => file.staged(),
        Side::Unstaged => file.unstaged(),
    }
}

/// What git's status letter for one side of a file means: the letter shown, and its colour.
fn decoration(change: GitChange, t: &Tokens) -> Option<(&'static str, Hsla, &'static str)> {
    Some(match change {
        GitChange::Unchanged => return None,
        GitChange::Untracked => ("U", Mark::Untracked.colour(t), "untracked"),
        GitChange::Unmerged => ("U", Mark::Conflicted.colour(t), "unmerged"),
        GitChange::Added => ("A", Mark::Added.colour(t), "added"),
        GitChange::Deleted => ("D", Mark::Deleted.colour(t), "deleted"),
        GitChange::Renamed => ("R", Mark::Renamed.colour(t), "renamed"),
        GitChange::Copied => ("C", Mark::Renamed.colour(t), "copied"),
        GitChange::TypeChanged => ("T", Mark::Modified.colour(t), "type changed"),
        GitChange::Modified => ("M", Mark::Modified.colour(t), "modified"),
    })
}

/// A file's status letter as a review card shows it.
pub fn status_mark(change: GitChange, cx: &App) -> Option<Div> {
    let (letter, colour, _) = decoration(change, Tokens::get(cx))?;
    Some(
        div()
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .font_weight(FontWeight::BOLD)
            .text_color(colour)
            .child(letter),
    )
}

/// Something done to the pane, held by a button or a question until it is pressed.
type Act = Rc<dyn Fn(&mut GitPane, &mut Context<GitPane>)>;

#[derive(Clone, PartialEq)]
enum Entry {
    Group { side: Side, count: usize },
    File { side: Side, file: GitFile },
}

const LOG_KEPT: usize = 200;

/// A git operation the pane ran, as the command log lists it: when it started, what it
/// was, whether it went through, and the first line of what it said.
struct Logged {
    id: u64,
    time: SharedString,
    label: SharedString,
    done: Option<bool>,
    detail: Option<SharedString>,
}

/// Files moved between the groups by one click, which run one after another.
struct Move {
    paths: Vec<String>,
    side: Side,
    method: &'static str,
    params: Value,
}

/// Something done with a line of text the person typed, such as a name.
type Named = Rc<dyn Fn(&mut GitPane, String, &mut Context<GitPane>)>;

/// What the pane asks the person before going on.
enum Asking {
    /// Whether to go ahead with something, `destructive` when it cannot be undone.
    Confirm {
        title: SharedString,
        body: SharedString,
        yes: SharedString,
        destructive: bool,
        then: Act,
    },
    /// A line of text, such as a name; its field is made when the pane next draws.
    Prompt {
        title: SharedString,
        placeholder: SharedString,
        initial: String,
        field: Option<Entity<InputState>>,
        then: Named,
    },
    /// One of several things to do, each with its key.
    Menu {
        title: SharedString,
        choices: Vec<Pick>,
    },
    Keys,
}

/// One choice of a menu the pane asks with.
struct Pick {
    key: Option<char>,
    label: SharedString,
    hint: Option<SharedString>,
    destructive: bool,
    disabled: bool,
    then: Act,
}

impl Pick {
    fn new(
        key: impl Into<Option<char>>,
        label: impl Into<SharedString>,
        then: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            hint: None,
            destructive: false,
            disabled: false,
            then: Rc::new(then),
        }
    }

    fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

pub struct GitPane {
    pub(super) core: Target,
    /// The host the pane asks, while it is out of reach: the pane shows what it last said
    /// and takes no changes.
    out_of_reach: Option<SharedString>,
    pub repo: String,
    /// The repository's code host and the account chosen for it, as the workbench last said.
    code_host: Option<(ProjectCodeHost, Option<String>)>,
    overview: Option<Result<GitOverview, String>>,
    /// Files moved to a group before git says so, by path.
    moving: HashMap<String, Side>,
    /// Moves git has made, whose files return to what git says at its next overview.
    moved: Vec<String>,
    queue: VecDeque<Move>,
    running: bool,
    busy: Option<SharedString>,
    folded: HashSet<Side>,
    selected: usize,
    entries: Vec<Entry>,
    rows: ListState,
    pub(super) review: Review,
    review_folded: bool,
    page: Page,
    slide: Slide<Page>,
    /// Where a range of rows started, which Space stages and `d` discards at once.
    range: Option<usize>,
    /// The commit chosen in the history, by its place in the log.
    commit: usize,
    graph: Rc<Graph>,
    hashes: Vec<String>,
    commit_rows: ListState,
    /// The commit whose files were last asked for, so it is asked for once.
    asked_commit: Option<String>,
    /// The review shows the changed files the list does, so moving the selection only
    /// focuses another of them.
    reviewing_list: bool,
    branches: Branches,
    branch_rows: ListState,
    stashes: Vec<GitStash>,
    /// The agents that write commit messages: their kind, name and models, the
    /// default first, as the core's catalog lists them.
    writers: Vec<(String, String, Vec<String>)>,
    /// The agent and model the person picked to write messages, else the core's default.
    writer: Option<(String, String)>,
    /// The commits the history's search finds, which the graph lays out.
    commits: Vec<GitCommit>,
    /// The commit asked to be shown, until the history is read.
    wanted_commit: Option<String>,
    /// Each page's filter, by `Page`.
    filters: [Filter; 3],
    draft: Entity<TextareaState>,
    /// A message written into the draft from outside the window's turn, set when it next draws.
    written: Option<String>,
    /// The git operations run from the pane, oldest first.
    commands: VecDeque<Logged>,
    commands_open: bool,
    /// The store, for the agents whose faces the changed files show.
    bench: WeakEntity<Workbench>,
    /// Who wrote the uncommitted lines, as the core's `changeOwners` answers.
    owners: Owners,
    /// Who wrote the staged lines, read while a file has both staged and unstaged changes.
    staged_owners: Owners,
    /// Who wrote the lines of the commit shown, as its note says.
    commit_lines: CommitLines,
    /// The agents each commit's signed footer names, and the commits last asked about.
    commit_owners: HashMap<String, Vec<AgentOwner>>,
    owners_asked: Vec<String>,
    /// The commit message as the core last kept it.
    synced: String,
    _keeping: Task<()>,
    generating: bool,
    asking: Option<Asking>,
    /// The row or card header the pointer is on, whose actions show.
    pub(super) hovered: Option<SharedString>,
    focus: FocusHandle,
    _drafting: Subscription,
}

impl GitPane {
    /// The host whose core the pane asks.
    pub fn place(&self) -> &crate::hosts::Place {
        self.core.place()
    }

    /// Hears that the pane's host went out of reach, by its name, or came back.
    pub fn set_out_of_reach(&mut self, host: Option<SharedString>, cx: &mut Context<Self>) {
        if self.out_of_reach != host {
            self.out_of_reach = host;
            cx.notify();
        }
    }

    pub fn set_code_host(
        &mut self,
        host: Option<(ProjectCodeHost, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        if self.code_host == host {
            return;
        }
        self.code_host = host;
        if self.page == Page::Branches {
            self.read_branch_pulls(cx);
        }
        cx.notify();
    }

    pub fn new(
        core: Target,
        repo: String,
        bench: WeakEntity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 9)
                .placeholder("Message (⌘⏎ to commit)")
        });
        let drafting = cx.subscribe(&draft, |pane, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                pane.typed(cx);
                cx.notify();
            }
        });
        Self {
            core,
            out_of_reach: None,
            repo,
            code_host: None,
            overview: None,
            moving: HashMap::new(),
            moved: Vec::new(),
            queue: VecDeque::new(),
            running: false,
            busy: None,
            folded: HashSet::new(),
            selected: 0,
            entries: Vec::new(),
            rows: ListState::new(0, ListAlignment::Top, px(400.)),
            review: Review::default(),
            review_folded: false,
            page: Page::Changes,
            slide: Slide::default(),
            range: None,
            commit: 0,
            graph: Rc::new(graph(&[])),
            hashes: Vec::new(),
            commit_rows: ListState::new(0, ListAlignment::Top, px(400.)),
            asked_commit: None,
            reviewing_list: false,
            branches: Branches::default(),
            stashes: Vec::new(),
            writers: Vec::new(),
            writer: None,
            commits: Vec::new(),
            wanted_commit: None,
            filters: Default::default(),
            branch_rows: ListState::new(0, ListAlignment::Top, px(400.)),
            draft,
            written: None,
            bench,
            owners: Owners::default(),
            staged_owners: Owners::default(),
            commit_lines: CommitLines::default(),
            commit_owners: HashMap::new(),
            owners_asked: Vec::new(),
            commands: VecDeque::new(),
            commands_open: false,
            synced: String::new(),
            _keeping: Task::ready(()),
            generating: false,
            asking: None,
            hovered: None,
            focus: cx.focus_handle(),
            _drafting: drafting,
        }
    }

    /// Takes what `git.overview` last answered for the repository.
    pub fn overview(&mut self, overview: &Result<GitOverview, String>, cx: &mut Context<Self>) {
        let first = self.overview.is_none();
        self.overview = Some(overview.clone());
        for path in self.moved.drain(..) {
            self.moving.remove(&path);
        }
        self.list_commits();
        self.list_branches();
        if first || self.page == Page::Branches {
            self.read_remotes(cx);
        }
        self.read_stashes(cx);
        self.read_owners(cx);
        self.read_commit_owners(cx);
        self.list(cx);
        self.choose_wanted(cx);
    }

    /// Reads again the diffs a git change in the folder `project` moved: those of `paths`,
    /// named from that folder, or every one when the change names none or git's own state.
    pub fn git_moved(
        &mut self,
        project: Option<&str>,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        if self.review.commit().is_some() {
            return;
        }
        let repo = Path::new(&self.repo);
        let project = project.map(Path::new);
        if project.is_some_and(|project| !repo.starts_with(project) && !project.starts_with(repo)) {
            return;
        }
        let in_git = |path: &String| {
            Path::new(path)
                .components()
                .any(|part| part.as_os_str() == ".git")
        };
        match (project, paths.filter(|paths| !paths.iter().any(in_git))) {
            (Some(project), Some(paths)) => {
                let moved: Vec<_> = paths.iter().map(|path| project.join(path)).collect();
                self.review.reread_files(&moved);
            }
            _ => self.review.reread(),
        }
        cx.notify();
    }

    /// Takes the agents the core's catalog says write commit messages.
    pub fn catalog(&mut self, catalog: &crate::reads::Catalog, cx: &mut Context<Self>) {
        self.writers = catalog
            .agents()
            .filter(|agent| !agent.commit_models.is_empty())
            .map(|agent| {
                (
                    agent.kind.clone(),
                    agent.label.clone(),
                    agent.commit_models.clone(),
                )
            })
            .collect();
        cx.notify();
    }

    /// The agent and model that write the next message: the one picked, else the core's
    /// default, which is Hermes's first model.
    fn writer(&self) -> Option<(String, String, String)> {
        let (kind, model) = match &self.writer {
            Some((kind, model)) => (kind.clone(), Some(model.clone())),
            None => (sikemux_core::git::ai::DEFAULT_WRITER.to_owned(), None),
        };
        let (_, label, models) = self.writers.iter().find(|(writer, _, _)| *writer == kind)?;
        let model = model.or_else(|| models.first().cloned())?;
        Some((kind, label.clone(), model))
    }

    /// Lists the commits the history's search finds, laying the graph out again when
    /// they changed.
    fn list_commits(&mut self) {
        let query = self.filters[Page::History as usize].query.clone();
        let shown: Vec<GitCommit> = self
            .log()
            .iter()
            .filter(|commit| {
                query.is_empty()
                    || [&commit.subject, &commit.hash, &commit.author]
                        .iter()
                        .any(|field| field.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        let hashes: Vec<String> = shown
            .iter()
            .map(|commit| commit.full_hash.clone())
            .collect();
        if hashes != self.hashes {
            self.graph = Rc::new(graph(&shown));
            crate::ui::relist(&self.commit_rows, &self.hashes, &hashes);
            self.hashes = hashes;
            self.commit = self.commit.min(self.hashes.len().saturating_sub(1));
        }
        self.commits = shown;
    }

    /// Makes the page's filter field when it shows one, and keeps its query as the person
    /// types.
    fn filter_field(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<InputState>> {
        let filter = &self.filters[page as usize];
        if page == Page::Changes && !filter.open && filter.query.is_empty() {
            return None;
        }
        if let Some(field) = &filter.field {
            return Some(field.clone());
        }
        let placeholder = match page {
            Page::Changes => "Filter files",
            Page::History => "Search commits",
            Page::Branches => "Filter branches",
        };
        let field = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let typing = cx.subscribe_in(
            &field,
            window,
            move |pane, field, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    pane.filters[page as usize].query = field.read(cx).value().to_lowercase();
                    pane.refilter(page, cx);
                }
                InputEvent::PressEnter { .. } => {
                    pane.filters[page as usize].open = false;
                    window.focus(&pane.focus, cx);
                }
                _ => {}
            },
        );
        let filter = &mut self.filters[page as usize];
        filter.field = Some(field.clone());
        filter.typing = Some(typing);
        Some(field)
    }

    fn refilter(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Changes => {
                self.selected = 0;
                self.list(cx);
            }
            Page::History => {
                self.commit = 0;
                self.list_commits();
                self.follow(cx);
            }
            Page::Branches => {
                self.branches.chosen = 0;
                self.list_branches();
                self.follow(cx);
            }
        }
        cx.notify();
    }

    /// Puts the keyboard in the page's filter, opening the Changes one.
    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filters[self.page as usize].open = true;
        if let Some(field) = self.filter_field(self.page, window, cx) {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
        cx.notify();
    }

    /// Clears the page's filter, closing the Changes one; false when there was none.
    fn unfind(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let filter = &mut self.filters[self.page as usize];
        if filter.query.is_empty() && !filter.open {
            return false;
        }
        filter.open = false;
        filter.query.clear();
        if let Some(field) = filter.field.clone() {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        let page = self.page;
        self.refilter(page, cx);
        window.focus(&self.focus, cx);
        true
    }

    fn log(&self) -> &[GitCommit] {
        match &self.overview {
            Some(Ok(overview)) => &overview.log,
            _ => &[],
        }
    }

    /// Gives the pane the keyboard, so its single keys work at once.
    pub fn take_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub(super) fn choose_commit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit = index;
        window.focus(&self.focus, cx);
        self.follow(cx);
        cx.notify();
    }

    /// Turns to the history with commit `sha` chosen, as soon as the history is read.
    pub fn show_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        self.show_page(Page::History, cx);
        self.wanted_commit = Some(sha.to_owned());
        if self.overview.is_some() {
            self.choose_wanted(cx);
        }
    }

    /// Chooses the commit asked for, or says it is older than the history lists.
    fn choose_wanted(&mut self, cx: &mut Context<Self>) {
        let Some(sha) = self.wanted_commit.take() else {
            return;
        };
        match self.hashes.iter().position(|hash| *hash == sha) {
            Some(at) => {
                self.commit = at;
                self.commit_rows.scroll_to_reveal_item(at);
                self.follow(cx);
            }
            None => {
                let short: String = sha.chars().take(9).collect();
                let note = toast::info(format!("{short} is older than the commits listed here"));
                let bench = self.bench.clone();
                cx.defer(move |cx| {
                    let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
                });
            }
        }
        cx.notify();
    }

    fn step_commit(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.hashes.is_empty() {
            return;
        }
        self.commit =
            (self.commit as isize + step).clamp(0, self.hashes.len() as isize - 1) as usize;
        self.commit_rows.scroll_to_reveal_item(self.commit);
        self.follow(cx);
        cx.notify();
    }

    /// The code host of the project this repository is in, and the account it picked there.
    fn host_of(&self) -> Option<(ProjectCodeHost, Option<String>)> {
        self.code_host.clone()
    }

    fn page_order(&self) -> Vec<Page> {
        vec![Page::Changes, Page::History, Page::Branches]
    }

    /// Turns to another page, springing over to it.
    fn turn(&mut self, page: Page, cx: &mut Context<Self>) {
        let was = self.page;
        if !self.show_page(page, cx) {
            return;
        }
        let order = self.page_order();
        if let Some(moved) = self.slide.turn(was, page, &order) {
            track::settle(self, moved, |pane: &mut Self| &mut pane.slide, cx);
        }
    }

    fn show_page(&mut self, page: Page, cx: &mut Context<Self>) -> bool {
        if page == Page::Branches && self.page != page {
            self.read_remotes(cx);
        }
        self.page = page;
        self.follow(cx);
        cx.notify();
        true
    }

    /// Follows a scroll over the side column's pages; true when it took it as a swipe.
    fn swipe(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
        let order = self.page_order();
        match self.slide.scroll(event, &order, &self.page) {
            Scrolled::Ignored => return false,
            Scrolled::Glided => return true,
            Scrolled::Moved => {}
            Scrolled::Landed((shown, from, speed, target)) => {
                if let Some(target) = target {
                    self.show_page(target, cx);
                }
                let moved = self.slide.travel(shown, from, Some(speed));
                track::settle(self, moved, |pane: &mut Self| &mut pane.slide, cx);
            }
        }
        cx.notify();
        true
    }

    /// What the review column shows: the changes while there are some and Changes is in
    /// front, else the commit chosen in the history, else the latest.
    fn shown(&self) -> Option<Shown> {
        let changes = self.page == Page::Changes && !self.entries.is_empty();
        if changes {
            return Some(Shown::Changes);
        }
        if self.page == Page::Branches {
            let entry = self.chosen_branch()?;
            return entry.rev().map(|rev| Shown::Commit(rev.to_owned()));
        }
        let commit = if self.page == Page::History {
            self.commits.get(self.commit)
        } else {
            self.log().first()
        };
        commit.map(|commit| Shown::Commit(commit.full_hash.clone()))
    }

    /// Points the review at what it shows, asking for a commit's files the first time.
    fn follow(&mut self, cx: &mut Context<Self>) {
        match self.shown() {
            Some(Shown::Changes) => {
                if self.review.commit().is_some() || !self.reviewing_list {
                    let files = self.listed_files();
                    self.review.show_changes(&self.repo, &files);
                    self.reviewing_list = true;
                }
                let path = self.chosen().map(|(_, file)| file.path.clone());
                self.review.focus(path.as_deref());
            }
            Some(Shown::Commit(hash)) => {
                if self.review.commit() == Some(hash.as_str())
                    || self.asked_commit.as_deref() == Some(hash.as_str())
                {
                    return;
                }
                self.asked_commit = Some(hash.clone());
                self.read_commit_lines(&hash, cx);
                let answer =
                    self.core
                        .request_typed::<git_call::CommitFiles>(git_call::CommitFiles {
                            repo: self.repo.clone(),
                            rev: hash.clone(),
                        });
                cx.spawn(async move |pane, cx| {
                    let paths = answer.await.unwrap_or_default();
                    pane.update(cx, |pane, cx| {
                        if pane.asked_commit.as_deref() == Some(hash.as_str()) {
                            let repo = pane.repo.clone();
                            pane.review.show_commit(&repo, &hash, &paths);
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            None => {
                self.review.show_changes(&self.repo, &[]);
                self.reviewing_list = false;
            }
        }
    }

    /// The repository's changed files, moved as the person moved them.
    fn files(&self) -> Vec<GitFile> {
        let Some(Ok(overview)) = &self.overview else {
            return Vec::new();
        };
        overview
            .status
            .files
            .iter()
            .map(|file| match self.moving.get(&file.path) {
                Some(side) => moved_to(file, *side),
                None => file.clone(),
            })
            .collect()
    }

    /// The changed files the Changes filter finds.
    fn listed_files(&self) -> Vec<GitFile> {
        let query = &self.filters[Page::Changes as usize].query;
        let mut files = self.files();
        if !query.is_empty() {
            files.retain(|file| file.path.to_lowercase().contains(query.as_str()));
        }
        files
    }

    fn list(&mut self, cx: &mut Context<Self>) {
        let files = self.listed_files();
        let mut entries = Vec::new();
        for (side, wanted) in [
            (Side::Staged, GitFile::is_staged as fn(&GitFile) -> bool),
            (Side::Unstaged, GitFile::has_unstaged),
        ] {
            let listed: Vec<&GitFile> = files.iter().filter(|file| wanted(file)).collect();
            if listed.is_empty() {
                continue;
            }
            entries.push(Entry::Group {
                side,
                count: listed.len(),
            });
            if !self.folded.contains(&side) {
                entries.extend(listed.into_iter().map(|file| Entry::File {
                    side,
                    file: file.clone(),
                }));
            }
        }
        crate::ui::relist(&self.rows, &self.entries, &entries);
        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.reviewing_list = false;
        self.follow(cx);
        cx.notify();
    }

    /// The file the list's selection is on, else the first file listed.
    fn chosen(&self) -> Option<(Side, &GitFile)> {
        fn file(entry: &Entry) -> Option<(Side, &GitFile)> {
            match entry {
                Entry::File { side, file } => Some((*side, file)),
                Entry::Group { .. } => None,
            }
        }
        self.entries
            .get(self.selected)
            .and_then(file)
            .or_else(|| self.entries.iter().find_map(file))
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        self.follow(cx);
        cx.notify();
    }

    /// Moves the selection by `step` files, over the groups' rows.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let files: Vec<usize> = (0..self.entries.len())
            .filter(|index| matches!(self.entries[*index], Entry::File { .. }))
            .collect();
        let Some(at) = files
            .iter()
            .position(|index| *index >= self.selected)
            .or(files.len().checked_sub(1))
        else {
            return;
        };
        let next = (at as isize + step).clamp(0, files.len() as isize - 1) as usize;
        self.select(files[next], cx);
        self.rows.scroll_to_reveal_item(files[next]);
    }

    /// The menu the keys opened, as its title and its choices' labels, for the test bundle's
    /// driver to check.
    #[cfg(feature = "drive")]
    pub fn drive_state(&self) -> Value {
        match &self.asking {
            Some(Asking::Menu { title, choices }) => json!({
                "menu": title,
                "choices": choices.iter().map(|pick| pick.label.clone()).collect::<Vec<_>>(),
            }),
            _ => json!({ "menu": null }),
        }
    }

    /// The rows from the range's anchor to the selection, else the chosen file's.
    fn in_range(&self) -> Vec<&Entry> {
        match self.range {
            Some(anchor) => {
                let (from, to) = (anchor.min(self.selected), anchor.max(self.selected));
                self.entries.iter().skip(from).take(to + 1 - from).collect()
            }
            None => self
                .entries
                .get(self.selected)
                .filter(|entry| matches!(entry, Entry::File { .. }))
                .into_iter()
                .collect(),
        }
    }

    fn read_stashes(&mut self, cx: &mut Context<Self>) {
        let answer = self
            .core
            .request_typed::<git_call::StashList>(git_call::StashList {
                repo: self.repo.clone(),
            });
        cx.spawn(async move |pane, cx| {
            let stashes: Vec<GitStash> = answer.await.unwrap_or_default();
            pane.update(cx, |pane, cx| {
                pane.stashes = stashes;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads everything the pane shows from the repository again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.review.commit().is_none() {
            self.review.reread();
        }
        let answer = self
            .core
            .request_typed::<git_call::Overview>(git_call::Overview {
                repo: self.repo.clone(),
            });
        cx.spawn(async move |pane, cx| {
            if let Ok(overview) = answer.await {
                let overview = Ok(overview);
                pane.update(cx, |pane, cx| pane.overview(&overview, cx))
                    .ok();
            }
        })
        .detach();
        self.read_remotes(cx);
    }

    fn chosen_commit(&self) -> Option<GitCommit> {
        self.commits.get(self.commit).cloned()
    }

    fn staged_count(&self) -> usize {
        self.files().iter().filter(|file| file.is_staged()).count()
    }

    fn clear_draft(&mut self, cx: &mut Context<Self>) {
        self.write(String::new(), cx);
    }

    /// The key the core keeps the repository's commit message under.
    fn draft_key(&self) -> String {
        super::draft_key(&self.repo)
    }

    /// Takes the drafts the core keeps. The commit message follows the core's only while
    /// the person has typed nothing since they last agreed, so their own words are never
    /// overwritten and their cursor never jumps.
    pub fn drafts(&mut self, drafts: &crate::reads::Drafts, cx: &mut Context<Self>) {
        let kept = (drafts.get(&self.draft_key()))
            .map(|draft| draft.text.clone())
            .unwrap_or_default();
        if kept == self.synced {
            return;
        }
        if self.draft.read(cx).value().as_ref() == self.synced {
            self.written = Some(kept.clone());
            cx.notify();
        }
        self.synced = kept;
    }

    /// Keeps what was typed once the person pauses.
    fn typed(&mut self, cx: &mut Context<Self>) {
        self._keeping = cx.spawn(async move |pane, cx| {
            cx.background_executor().timer(DRAFT_PAUSE).await;
            pane.update(cx, |pane, cx| {
                let text = pane.draft.read(cx).value().to_string();
                pane.keep_draft(text);
            })
            .ok();
        });
    }

    /// Puts `text` in the commit box and keeps it as the draft.
    fn write(&mut self, text: String, cx: &mut Context<Self>) {
        self.keep_draft(text.clone());
        self.written = Some(text);
        cx.notify();
    }

    fn keep_draft(&mut self, text: String) {
        if text == self.synced {
            return;
        }
        self.synced = text.clone();
        self.core.call_typed::<call::SetDraft>(SetDraftParams {
            key: self.draft_key(),
            text,
            extra: None,
            rev: None,
        });
    }
}
