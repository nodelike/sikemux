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
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_core::git::log::{GitCommit, GitOverview};
use sikemux_core::git::stash::GitStash;
use sikemux_core::git::status::GitFile;
use sikemux_wire::protocol::ProjectCodeHost;

use branches::{branch_card, branches_page, Branches};
use history::{commit_card, commit_row, graph, CommitOwner, Graph, Inks};

mod authors;
mod branches;
mod history;
use super::review::{review, Review};
use crate::codehost::inbox::Inbox;
use crate::core::Target;
use crate::editor::OpenAt;
use crate::files::Mark;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    SPACE_4, SPACE_5, TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::track::{self, Scrolled, Slide};
use crate::ui::{self, Cheatsheet, Choice, Choices, Confirm, Prompt};
use crate::workbench::{AgentRow, Workbench};

/// Brings the project's Git tab to the front, opening one when it has none.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "window.git")]
pub struct FocusGit;

pub(crate) const ROW_HEIGHT: f32 = 26.;
/// The pane's keys, by page, as its cheatsheet lists them.
const KEYS: [ui::Section; 4] = [
    (
        "Global",
        &[
            ("1 2 3", "Changes, History, Branches"),
            ("h", "switch between Changes and History"),
            ("?", "open this cheatsheet"),
            ("@", "toggle command log"),
            ("/", "filter the list"),
            ("r", "refresh repo state"),
            ("F", "fetch all remotes"),
            ("P / p", "push / pull"),
            ("^P", "open pull-request page"),
            ("esc", "close modal / clear filter or range"),
        ],
    ),
    (
        "Changes",
        &[
            ("space", "stage / unstage selected (or range)"),
            ("v", "toggle range select"),
            ("a", "toggle stage all"),
            ("c", "focus commit message"),
            ("C", "commit the typed message"),
            ("g", "generate commit message (AI)"),
            ("⌘⏎", "commit (from message box)"),
            ("d", "discard menu"),
            ("s", "stash menu"),
        ],
    ),
    (
        "History",
        &[
            ("enter / space", "actions menu"),
            ("b", "create branch from commit"),
            ("r", "reset to commit menu"),
            ("v", "revert commit"),
        ],
    ),
    (
        "Branches",
        &[
            (
                "enter / space",
                "checkout (a remote branch gets a local one)",
            ),
            ("n / N", "new branch (from selected / from HEAD)"),
            ("M", "merge menu"),
            ("d", "delete menu"),
            ("R", "rename branch"),
            ("c", "checkout by name"),
            ("f", "fetch the selected remote"),
            ("u", "set remote branch as upstream"),
        ],
    ),
];

/// How long typing rests before the commit message is kept as the draft.
const DRAFT_PAUSE: Duration = Duration::from_millis(300);

/// The side column's pages.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Changes,
    History,
    Branches,
    Inbox,
}

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

pub fn is_staged(file: &GitFile) -> bool {
    file.index != " " && file.index != "?"
}

pub fn has_unstaged(file: &GitFile) -> bool {
    file.worktree != " "
}

/// How a file reads once it has moved to `side`, before git has said so.
fn moved_to(file: &GitFile, side: Side) -> GitFile {
    let mut moved = file.clone();
    match side {
        Side::Staged if has_unstaged(file) => {
            if !is_staged(file) {
                moved.index = if file.worktree == "?" {
                    "A".into()
                } else {
                    file.worktree.clone()
                };
            }
            moved.worktree = " ".into();
        }
        Side::Unstaged if is_staged(file) => {
            let worktree = if has_unstaged(file) {
                file.worktree.clone()
            } else if file.index == "A" {
                "?".into()
            } else {
                file.index.clone()
            };
            moved.index = if worktree == "?" { "?" } else { " " }.into();
            moved.worktree = worktree;
        }
        _ => {}
    }
    moved
}

/// What git's status letter for one side of a file means: the letter shown, and its colour.
fn decoration(code: &str, t: &Tokens) -> Option<(&'static str, Hsla, &'static str)> {
    Some(match code.trim() {
        "" => return None,
        "?" => ("U", Mark::Untracked.colour(t), "untracked"),
        "U" => ("U", Mark::Conflicted.colour(t), "unmerged"),
        "A" => ("A", Mark::Added.colour(t), "added"),
        "D" => ("D", Mark::Deleted.colour(t), "deleted"),
        "R" => ("R", Mark::Renamed.colour(t), "renamed"),
        "C" => ("C", Mark::Renamed.colour(t), "copied"),
        "T" => ("T", Mark::Modified.colour(t), "type changed"),
        _ => ("M", Mark::Modified.colour(t), "modified"),
    })
}

/// A file's status letter as a review card shows it.
pub fn status_mark(code: &str, cx: &App) -> Option<Div> {
    let (letter, colour, _) = decoration(code, Tokens::get(cx))?;
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

/// The most operations the command log keeps.
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
    /// The pane's keys.
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
    /// Someone is signed in to that host, which the inbox needs.
    signed_in: bool,
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
    /// Each page's filter, by `Page`.
    filters: [Filter; 4],
    /// The code host's notifications, once the Inbox page first shows.
    inbox: Option<Entity<Inbox>>,
    authors: authors::Authors,
    draft: Entity<TextareaState>,
    /// A message written into the draft from outside the window's turn, set when it next draws.
    written: Option<String>,
    /// The git operations run from the pane, oldest first.
    commands: VecDeque<Logged>,
    commands_open: bool,
    /// The store, for the agents whose faces the changed files show.
    bench: WeakEntity<Workbench>,
    /// Who wrote the uncommitted lines, as the core's `changeOwners` answers.
    owners: Value,
    /// Who wrote the staged lines, read while a file has both staged and unstaged changes.
    staged_owners: Value,
    /// Who wrote the lines of the commit shown, as its note says.
    commit_lines: Value,
    /// The agents each commit's signed footer names, and the commits last asked about.
    commit_owners: HashMap<String, Vec<CommitOwner>>,
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
        signed_in: bool,
        cx: &mut Context<Self>,
    ) {
        if self.code_host == host && self.signed_in == signed_in {
            return;
        }
        self.code_host = host;
        self.signed_in = signed_in;
        if self.page == Page::Branches {
            self.read_branch_pulls(cx);
        }
        if self.page == Page::Inbox && !self.has_inbox() {
            self.show_page(Page::Changes, cx);
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
            signed_in: false,
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
            filters: Default::default(),
            inbox: None,
            authors: Default::default(),
            branch_rows: ListState::new(0, ListAlignment::Top, px(400.)),
            draft,
            written: None,
            bench,
            owners: Value::Null,
            staged_owners: Value::Null,
            commit_lines: Value::Null,
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
    pub fn overview(&mut self, value: &Value, cx: &mut Context<Self>) {
        let first = self.overview.is_none();
        self.overview = Some(GitOverview::deserialize(value).map_err(|error| error.to_string()));
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
            None => ("hermes".to_owned(), None),
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
            super::relist(&self.commit_rows, &self.hashes, &hashes);
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
        if page == Page::Inbox || (page == Page::Changes && !filter.open && filter.query.is_empty())
        {
            return None;
        }
        if let Some(field) = &filter.field {
            return Some(field.clone());
        }
        let placeholder = match page {
            Page::Changes => "Filter files",
            Page::History => "Search commits",
            Page::Branches | Page::Inbox => "Filter branches",
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
            Page::Inbox => {}
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

    /// The host's inbox is a page while someone is signed in to it, as on main.
    fn has_inbox(&self) -> bool {
        self.code_host.is_some() && self.signed_in
    }

    /// Lists the code host's notifications, or says there is no host to list them from.
    fn open_inbox(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((host, account)) = self.host_of().filter(|_| self.signed_in) else {
            self.inbox = None;
            return false;
        };
        match &self.inbox {
            Some(inbox) if inbox.read(cx).is_for(&host, &account) => {
                inbox.update(cx, |inbox, cx| inbox.read(cx));
            }
            _ => {
                let (core, bench) = (self.core.clone(), self.bench.clone());
                self.inbox = Some(cx.new(|cx| Inbox::new(core, bench, host, account, cx)));
            }
        }
        true
    }

    /// The side column's pages in their order, the inbox last while the repository's host
    /// has one.
    fn page_order(&self) -> Vec<Page> {
        let mut order = vec![Page::Changes, Page::History, Page::Branches];
        if self.has_inbox() {
            order.push(Page::Inbox);
        }
        order
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
        if page == Page::Inbox && !self.open_inbox(cx) {
            return false;
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
                let params = self.repo_call(json!({ "rev": hash }));
                let answer = self.core.request("git.commitFiles", params);
                cx.spawn(async move |pane, cx| {
                    let paths: Vec<String> = answer
                        .await
                        .ok()
                        .and_then(|paths| serde_json::from_value(paths).ok())
                        .unwrap_or_default();
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
            (Side::Staged, is_staged as fn(&GitFile) -> bool),
            (Side::Unstaged, has_unstaged),
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
        super::relist(&self.rows, &self.entries, &entries);
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

    /// Runs one git operation at a time, saying in the head what it is while it runs and
    /// what went wrong if it fails.
    fn run(
        &mut self,
        label: impl Into<SharedString>,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static,
    ) {
        if self.busy.is_some() {
            return;
        }
        if let Some(host) = &self.out_of_reach {
            let said = format!("{host} is out of reach, so this waits until it is back.");
            self.fail(&said, cx);
            return;
        }
        let label = label.into();
        self.busy = Some(label.clone());
        let logged = self.logged(label);
        let answer = self.core.request(method, params);
        cx.spawn(async move |pane, cx| {
            let answer = answer.await;
            pane.update(cx, |pane, cx| {
                pane.busy = None;
                pane.settle(logged, &answer);
                match &answer {
                    Err(error) => pane.fail(error, cx),
                    Ok(Value::String(said)) if method != "git.aiMessage" => pane.done(said, cx),
                    Ok(_) => {}
                }
                then(pane, answer, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Notes an operation in the command log as running, answering its place there.
    fn logged(&mut self, label: SharedString) -> u64 {
        let at = self.commands.back().map_or(0, |last| last.id + 1);
        self.commands.push_back(Logged {
            id: at,
            time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
            label,
            done: None,
            detail: None,
        });
        while self.commands.len() > LOG_KEPT {
            self.commands.pop_front();
        }
        at
    }

    /// Notes how a logged operation ended, with the first line of what it said.
    fn settle(&mut self, id: u64, answer: &Result<Value, String>) {
        let Some(entry) = self.commands.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        entry.done = Some(answer.is_ok());
        let said = match answer {
            Ok(Value::String(text)) => text.as_str(),
            Ok(_) => "",
            Err(error) => error.as_str(),
        };
        let first = first_line(said);
        entry.detail = (!first.is_empty()).then(|| first.into());
    }

    /// Says what went wrong, by the first line of it, until it is dismissed.
    fn fail(&self, error: &str, cx: &mut Context<Self>) {
        self.say(toast::error(first_line(error)).autohide(false), cx);
    }

    /// Says what an operation answered, by its first line, when it said anything.
    fn done(&self, said: &str, cx: &mut Context<Self>) {
        if !said.trim().is_empty() {
            self.say(toast::success(first_line(said)), cx);
        }
    }

    fn say(&self, note: Notification, cx: &mut Context<Self>) {
        self.bench
            .update(cx, |bench, cx| bench.notify(note, cx))
            .ok();
    }

    /// Copies `text`, saying it copied `what`.
    fn copy(&self, text: String, what: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.say(toast::success(format!("Copied {what}")), cx);
    }

    fn repo_call(&self, extra: Value) -> Value {
        let mut params = json!({ "repo": self.repo });
        if let (Some(params), Value::Object(extra)) = (params.as_object_mut(), extra) {
            params.extend(extra);
        }
        params
    }

    /// Moves files between the groups at once; git's next overview confirms the move
    /// or puts them back. Clicks in quick succession wait their turn.
    fn shift(
        &mut self,
        paths: Vec<String>,
        side: Side,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            self.moving.insert(path.clone(), side);
        }
        self.queue.push_back(Move {
            paths,
            side,
            method,
            params,
        });
        self.list(cx);
        self.next_move(cx);
    }

    fn next_move(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(next) = self.queue.pop_front() else {
            return;
        };
        self.running = true;
        let logged = self.logged(format!("{} {}", next.method, next.paths.join(" ")).into());
        let answer = self.core.request(next.method, next.params);
        cx.spawn(async move |pane, cx| {
            let answer = answer.await;
            pane.update(cx, |pane, cx| {
                pane.running = false;
                pane.settle(logged, &answer);
                if let Err(error) = &answer {
                    pane.fail(error, cx);
                }
                let later: HashSet<&String> =
                    pane.queue.iter().flat_map(|queued| &queued.paths).collect();
                let settled: Vec<String> = next
                    .paths
                    .into_iter()
                    .filter(|path| {
                        !later.contains(path) && pane.moving.get(path) == Some(&next.side)
                    })
                    .collect();
                pane.moved.extend(settled);
                pane.next_move(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn stage(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({ "path": file.path }));
        self.shift(
            vec![file.path.clone()],
            Side::Staged,
            "git.stage",
            params,
            cx,
        );
    }

    fn unstage(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({ "path": file.path }));
        self.shift(
            vec![file.path.clone()],
            Side::Unstaged,
            "git.unstage",
            params,
            cx,
        );
    }

    fn stage_all(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths(has_unstaged);
        let params = self.repo_call(json!({}));
        self.shift(paths, Side::Staged, "git.stageAll", params, cx);
    }

    fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths(is_staged);
        let params = self.repo_call(json!({}));
        self.shift(paths, Side::Unstaged, "git.unstageAll", params, cx);
    }

    fn paths(&self, wanted: fn(&GitFile) -> bool) -> Vec<String> {
        self.files()
            .into_iter()
            .filter(wanted)
            .map(|file| file.path)
            .collect()
    }

    fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        if self.range.is_some() {
            let mut paths: Vec<String> = Vec::new();
            for entry in self.in_range() {
                if let Entry::File {
                    side: Side::Unstaged,
                    file,
                } = entry
                {
                    if !paths.contains(&file.path) {
                        paths.push(file.path.clone());
                    }
                }
            }
            self.range = None;
            let params = self.repo_call(json!({ "paths": paths }));
            self.shift(paths, Side::Staged, "git.stagePaths", params, cx);
            return;
        }
        match self.chosen().map(|(side, file)| (side, file.clone())) {
            Some((Side::Staged, file)) => self.unstage(&file, cx),
            Some((Side::Unstaged, file)) => self.stage(&file, cx),
            None => {}
        }
    }

    /// Asks whether to go ahead with something that cannot be undone.
    fn ask(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        then: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.confirm(title, body, yes, true, then, cx);
    }

    fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            destructive,
            then: Rc::new(then),
        });
        cx.notify();
    }

    /// Asks for a line of text, such as a name, and goes on with what was typed.
    fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        initial: impl Into<String>,
        then: impl Fn(&mut GitPane, String, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Prompt {
            title: title.into(),
            placeholder: placeholder.into(),
            initial: initial.into(),
            field: None,
            then: Rc::new(then),
        });
        cx.notify();
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

    fn menu(&mut self, title: impl Into<SharedString>, choices: Vec<Pick>, cx: &mut Context<Self>) {
        self.asking = Some(Asking::Menu {
            title: title.into(),
            choices,
        });
        cx.notify();
    }

    /// The question being asked, as a dialog, making a prompt's field the first time.
    fn asked(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane = cx.entity();
        let done = move |pane: &Entity<GitPane>| {
            let pane = pane.clone();
            move |window: &mut Window, cx: &mut App| {
                pane.update(cx, |pane, cx| {
                    pane.asking = None;
                    window.focus(&pane.focus, cx);
                    cx.notify();
                })
            }
        };
        Some(match self.asking.as_mut()? {
            Asking::Confirm {
                title,
                body,
                yes,
                destructive,
                then,
            } => {
                let (then, going) = (then.clone(), pane.clone());
                Confirm::new(title.clone(), body.clone(), yes.clone())
                    .destructive(*destructive)
                    .on_yes(move |_, cx| {
                        let then = then.clone();
                        going.update(cx, |pane, cx| {
                            pane.asking = None;
                            then(pane, cx);
                            cx.notify();
                        })
                    })
                    .on_no(done(&pane))
                    .into_any_element()
            }
            Asking::Prompt {
                title,
                placeholder,
                initial,
                field,
                then,
            } => {
                let field = field
                    .get_or_insert_with(|| {
                        let (placeholder, initial) = (placeholder.clone(), initial.clone());
                        cx.new(|cx| {
                            let mut state = InputState::new(window, cx).placeholder(placeholder);
                            state.set_value(initial, window, cx);
                            state.focus(window, cx);
                            state
                        })
                    })
                    .clone();
                let (then, going) = (then.clone(), pane.clone());
                Prompt::new(title.clone(), &field)
                    .on_yes(move |typed, _, cx| {
                        let then = then.clone();
                        going.update(cx, |pane, cx| {
                            pane.asking = None;
                            then(pane, typed.trim().to_owned(), cx);
                            cx.notify();
                        })
                    })
                    .on_no(done(&pane))
                    .into_any_element()
            }
            Asking::Keys => Cheatsheet::new("Git pane keybindings", &KEYS)
                .on_close(done(&pane))
                .into_any_element(),
            Asking::Menu { title, choices } => {
                let choices = choices
                    .iter()
                    .map(|pick| {
                        let (then, going) = (pick.then.clone(), pane.clone());
                        let mut choice = Choice::new(pick.key, pick.label.clone(), move |_, cx| {
                            let then = then.clone();
                            going.update(cx, |pane, cx| {
                                pane.asking = None;
                                then(pane, cx);
                                cx.notify();
                            })
                        })
                        .disabled(pick.disabled);
                        if let Some(hint) = &pick.hint {
                            choice = choice.hint(hint.clone());
                        }
                        if pick.destructive {
                            choice = choice.destructive();
                        }
                        choice
                    })
                    .collect();
                Choices::new(title.clone(), choices)
                    .on_no(done(&pane))
                    .into_any_element()
            }
        })
    }

    fn discard(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_owned();
        let body = if file.worktree == "?" {
            "The file is new and will be deleted. This can't be undone."
        } else {
            "Unstaged changes to this file will be lost. This can't be undone."
        };
        let path = file.path.clone();
        self.ask(
            format!("Discard changes to {name}?"),
            body,
            "discard",
            move |pane, cx| {
                let params = pane.repo_call(json!({ "paths": [path], "mode": "unstaged" }));
                pane.run(
                    format!("discarding {path}"),
                    "git.discardFiles",
                    params,
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    fn discard_all(&mut self, cx: &mut Context<Self>) {
        let unstaged: Vec<GitFile> = self.files().into_iter().filter(has_unstaged).collect();
        if unstaged.is_empty() {
            return;
        }
        let added = unstaged.iter().filter(|file| file.worktree == "?").count();
        let count = format!(
            "{} file{}",
            unstaged.len(),
            if unstaged.len() == 1 { "" } else { "s" }
        );
        let fate = match added {
            0 => "Every file goes back to what is staged or committed. ".to_owned(),
            1 => "1 new file is deleted and the rest go back to what is staged or committed. ".to_owned(),
            added => format!("{added} new files are deleted and the rest go back to what is staged or committed. "),
        };
        let paths: Vec<String> = unstaged.into_iter().map(|file| file.path).collect();
        self.ask(
            format!("Discard unstaged changes in {count}?"),
            format!("{fate}Staged changes are kept. This can't be undone."),
            "discard all",
            move |pane, cx| {
                let params = pane.repo_call(json!({ "paths": paths, "mode": "unstaged" }));
                pane.run(
                    format!("discarding {count}"),
                    "git.discardFiles",
                    params,
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    fn stash_menu(&mut self, cx: &mut Context<Self>) {
        let stash = |mode: &'static str, title: &'static str| {
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                pane.prompt(
                    title,
                    "(optional) stash message",
                    "",
                    move |pane, message, cx| {
                        let message = (!message.is_empty()).then_some(message);
                        let params = pane.repo_call(json!({ "mode": mode, "message": message }));
                        pane.run(
                            format!("stashing ({mode})"),
                            "git.stashPush",
                            params,
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            "Stash",
            vec![
                Pick::new(
                    's',
                    "stash everything (working tree + index + untracked)",
                    stash("all", "Stash everything"),
                ),
                Pick::new(
                    'i',
                    "stash staged only",
                    stash("staged", "Stash staged changes"),
                ),
                Pick::new(
                    'u',
                    "stash unstaged only (keep index)",
                    stash("unstaged", "Stash unstaged changes"),
                ),
            ],
            cx,
        );
    }

    /// The ways to throw away the chosen file's changes, each asked about once more.
    /// The ways to throw away the changes of the chosen file, or of every file in the
    /// range, each asked about once more.
    fn discard_menu(&mut self, cx: &mut Context<Self>) {
        let mut paths: Vec<String> = Vec::new();
        for entry in self.in_range() {
            if let Entry::File { file, .. } = entry {
                if !paths.contains(&file.path) {
                    paths.push(file.path.clone());
                }
            }
        }
        let Some(first) = paths.first().cloned() else {
            return;
        };
        let count = paths.len();
        let files = format!("{count} file{}", if count > 1 { "s" } else { "" });
        let label = if count > 1 {
            format!("Discard {count} files")
        } else {
            format!(
                "Discard changes — {}",
                first.rsplit('/').next().unwrap_or(&first)
            )
        };
        let apply = |mode: &'static str, body: String, yes: &'static str| {
            let (paths, label, files) = (paths.clone(), label.clone(), files.clone());
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let (paths, files) = (paths.clone(), files.clone());
                pane.range = None;
                pane.ask(
                    label.clone(),
                    body.clone(),
                    yes,
                    move |pane, cx| {
                        let params = pane.repo_call(json!({ "paths": paths, "mode": mode }));
                        pane.run(
                            format!("discarding {files} ({mode})"),
                            "git.discardFiles",
                            params,
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            label.clone(),
            vec![
                Pick::new('d', "discard all changes", apply("all", format!("This will discard BOTH staged and unstaged changes in {files}. Cannot be undone."), "discard"))
                    .hint("staged + worktree")
                    .destructive(),
                Pick::new('u', "discard unstaged changes", apply("unstaged", format!("Discard unstaged changes in {files}? (staged changes preserved)"), "discard"))
                    .destructive(),
                Pick::new('s', "unstage changes", apply("staged", format!("Unstage {files} from the index? (worktree preserved)"), "unstage")),
            ],
            cx,
        );
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

    fn read_owners(&mut self, cx: &mut Context<Self>) {
        let read = |staged: bool| {
            let params = json!({ "dir": self.repo, "staged": staged });
            self.core.request("changeOwners", params)
        };
        let owners = read(false);
        let halves = self
            .files()
            .iter()
            .any(|file| is_staged(file) && has_unstaged(file));
        let staged = halves.then(|| read(true));
        cx.spawn(async move |pane, cx| {
            let owners = owners.await.unwrap_or(Value::Null);
            let staged = match staged {
                Some(staged) => staged.await.unwrap_or(Value::Null),
                None => Value::Null,
            };
            pane.update(cx, |pane, cx| {
                pane.owners = owners;
                pane.staged_owners = staged;
                pane.annotate();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the core which agents the footers of the log's commits name, when those
    /// commits changed.
    fn read_commit_owners(&mut self, cx: &mut Context<Self>) {
        let footered: Vec<String> = self
            .log()
            .iter()
            .filter(|commit| commit.footer)
            .map(|commit| commit.full_hash.clone())
            .collect();
        if footered == self.owners_asked {
            return;
        }
        self.owners_asked = footered.clone();
        if footered.is_empty() {
            self.commit_owners.clear();
            return;
        }
        let params = json!({ "dir": self.repo, "commits": footered });
        let answer = self.core.request("commitOwners", params);
        cx.spawn(async move |pane, cx| {
            let owners = answer
                .await
                .ok()
                .and_then(|owners| serde_json::from_value(owners).ok())
                .unwrap_or_default();
            pane.update(cx, |pane, cx| {
                if pane.owners_asked == footered {
                    pane.commit_owners = owners;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Who wrote the lines a diff of `side` shows, as the core last said.
    pub(super) fn owners_of(&self, side: Option<Side>) -> &Value {
        if self.review.commit().is_some() {
            &self.commit_lines
        } else if side == Some(Side::Staged) {
            &self.staged_owners
        } else {
            &self.owners
        }
    }

    fn read_commit_lines(&mut self, hash: &str, cx: &mut Context<Self>) {
        self.commit_lines = Value::Null;
        let params = json!({ "dir": self.repo, "commit": hash });
        let answer = self.core.request("commitLines", params);
        let hash = hash.to_owned();
        cx.spawn(async move |pane, cx| {
            let lines = answer.await.unwrap_or(Value::Null);
            pane.update(cx, |pane, cx| {
                if pane.asked_commit.as_deref() == Some(hash.as_str()) {
                    pane.commit_lines = lines;
                    pane.annotate();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Notes beside the lines the review shows who wrote them.
    fn annotate(&mut self) {
        let (owners, staged) = if self.review.commit().is_some() {
            (&self.commit_lines, &Value::Null)
        } else {
            (&self.owners, &self.staged_owners)
        };
        self.review.annotate(owners, staged);
    }

    /// The agents that wrote some of `path`'s uncommitted lines, by agent id, with the
    /// agent they ran on, in the order they first appear; and every writer's name.
    fn writers(&self, path: &str) -> (Vec<(String, String)>, Vec<String>) {
        let authors = self.owners["authors"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        let Some(file) = self.owners["files"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|file| file["path"] == path)
        else {
            return (Vec::new(), Vec::new());
        };
        let mut seen = Vec::new();
        for run in ["lines", "removed"]
            .iter()
            .flat_map(|field| file[*field].as_array().into_iter().flatten())
        {
            if let Some(author) = run["author"].as_u64().map(|at| at as usize) {
                if !seen.contains(&author) {
                    seen.push(author);
                }
            }
        }
        let mut faces: Vec<(String, String)> = Vec::new();
        let mut names = Vec::new();
        for author in seen.iter().filter_map(|at| authors.get(*at)) {
            names.push(author["name"].as_str().unwrap_or_default().to_owned());
            let cli = author["cli"].as_str().unwrap_or_default().to_owned();
            let shared: Vec<&str> = author["shared"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let ids = if shared.is_empty() {
                vec![author["agentId"].as_str().unwrap_or_default()]
            } else {
                shared
            };
            for id in ids {
                if !faces.iter().any(|(known, _)| known == id) {
                    faces.push((id.to_owned(), cli.clone()));
                }
            }
        }
        (faces, names)
    }

    fn read_stashes(&mut self, cx: &mut Context<Self>) {
        let answer = self
            .core
            .request("git.stashList", self.repo_call(json!({})));
        cx.spawn(async move |pane, cx| {
            let stashes: Vec<GitStash> = answer
                .await
                .ok()
                .and_then(|stashes| serde_json::from_value(stashes).ok())
                .unwrap_or_default();
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
        let answer = self.core.request("git.overview", self.repo_call(json!({})));
        cx.spawn(async move |pane, cx| {
            if let Ok(overview) = answer.await {
                pane.update(cx, |pane, cx| pane.overview(&overview, cx))
                    .ok();
            }
        })
        .detach();
        self.read_remotes(cx);
    }

    fn add_remote(&mut self, cx: &mut Context<Self>) {
        self.prompt(
            "Add remote",
            "name (e.g. upstream) — then you'll enter the URL",
            "",
            |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                pane.prompt(
                    format!("Add remote · {name}"),
                    "URL (https://… or git@…)",
                    "",
                    move |pane, url, cx| {
                        if url.is_empty() {
                            return;
                        }
                        let params = pane.repo_call(json!({ "name": name, "url": url }));
                        pane.run(
                            format!("adding remote {name}"),
                            "git.remoteAdd",
                            params,
                            cx,
                            |pane, _, cx| pane.read_remotes(cx),
                        );
                    },
                    cx,
                );
            },
            cx,
        );
    }

    /// The stashes, each opening what can be done with it.
    fn stashes_menu(&mut self, cx: &mut Context<Self>) {
        let picks = self
            .stashes
            .iter()
            .enumerate()
            .map(|(index, stash)| {
                let key = (index < 9)
                    .then(|| char::from_digit(index as u32 + 1, 10))
                    .flatten();
                let chosen = stash.clone();
                Pick::new(key, stash.message.clone(), move |pane, cx| {
                    pane.stash_menu_of(&chosen, cx)
                })
                .hint(stash.refname.clone())
            })
            .collect();
        self.menu("Stashes", picks, cx);
    }

    fn stash_menu_of(&mut self, stash: &GitStash, cx: &mut Context<Self>) {
        let call = |method: &'static str, label: String, extra: Value| {
            let (refname, sha) = (stash.refname.clone(), stash.sha.clone());
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let mut params = json!({ "refname": refname, "sha": sha });
                if let (Some(params), Value::Object(extra)) =
                    (params.as_object_mut(), extra.clone())
                {
                    params.extend(extra);
                }
                let params = pane.repo_call(params);
                pane.run(label.clone(), method, params, cx, |pane, _, cx| {
                    pane.read_stashes(cx)
                });
            }
        };
        let apply = call(
            "git.stashApply",
            format!("applying {}", stash.refname),
            json!({}),
        );
        let pop = call(
            "git.stashPop",
            format!("popping {}", stash.refname),
            json!({}),
        );
        let drop = call(
            "git.stashDrop",
            format!("dropping {}", stash.refname),
            json!({}),
        );
        let (branched, renamed) = (stash.clone(), stash.clone());
        let refname = stash.refname.clone();
        self.menu(
            format!("{} · {}", stash.refname, stash.message),
            vec![
                Pick::new('a', "apply stash", apply).hint("keep stash"),
                Pick::new('p', "pop stash", pop).hint("apply + drop"),
                Pick::new('b', "create branch from stash", move |pane, cx| {
                    let stash = branched.clone();
                    pane.prompt(format!("Branch from {}", stash.refname), "branch name", "", move |pane, name, cx| {
                        if name.is_empty() {
                            return;
                        }
                        let params = pane.repo_call(json!({ "refname": stash.refname, "sha": stash.sha, "name": name }));
                        pane.run(format!("branching from {}", stash.refname), "git.stashBranch", params, cx, |pane, _, cx| pane.read_stashes(cx));
                    }, cx);
                }),
                Pick::new('r', "rename stash", move |pane, cx| {
                    let stash = renamed.clone();
                    pane.prompt(format!("Rename {}", stash.refname), "", stash.message.clone(), move |pane, message, cx| {
                        if message.is_empty() || message == stash.message {
                            return;
                        }
                        let params = pane.repo_call(json!({ "refname": stash.refname, "sha": stash.sha, "newMessage": message }));
                        pane.run(format!("renaming {}", stash.refname), "git.stashRename", params, cx, |pane, _, cx| pane.read_stashes(cx));
                    }, cx);
                }),
                Pick::new('d', "drop stash", move |pane, cx| {
                    let drop = drop.clone();
                    pane.ask(format!("Drop {refname}?"), "This permanently deletes the stash entry.", "drop", move |pane, cx| drop(pane, cx), cx);
                })
                .destructive(),
            ],
            cx,
        );
    }

    fn chosen_commit(&self) -> Option<GitCommit> {
        self.commits.get(self.commit).cloned()
    }

    /// What can be done with the commit chosen in the history.
    fn commit_menu(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.chosen_commit() else {
            return;
        };
        let full = if commit.full_hash.is_empty() {
            commit.hash.clone()
        } else {
            commit.full_hash.clone()
        };
        let short = commit.hash.clone();
        self.menu(
            format!("{} · {}", commit.hash, commit.subject),
            vec![
                Pick::new(
                    'b',
                    "create branch from commit",
                    GitPane::branch_from_commit,
                ),
                Pick::new('r', "reset to this commit", GitPane::reset_menu)
                    .hint("choose soft / mixed / hard"),
                Pick::new('v', "revert this commit", GitPane::revert_commit)
                    .hint("new inverse commit"),
                Pick::new('y', "copy hash", move |pane, cx| {
                    pane.copy(full.clone(), short.clone(), cx)
                }),
            ],
            cx,
        );
    }

    fn branch_from_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.branch_from(commit.hash, cx);
        }
    }

    pub(super) fn branch_from(&mut self, hash: String, cx: &mut Context<Self>) {
        self.prompt(
            format!("Branch from {hash}"),
            "branch name",
            "",
            move |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                let params = pane.repo_call(json!({ "name": name, "startPoint": hash }));
                pane.run(
                    format!("creating {name} from {hash}"),
                    "git.branchCreate",
                    params,
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    fn reset_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.reset_to(commit.hash, cx);
        }
    }

    pub(super) fn reset_to(&mut self, hash: String, cx: &mut Context<Self>) {
        let reset = |mode: &'static str, body: &'static str| {
            let hash = hash.clone();
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let hash = hash.clone();
                pane.confirm(
                    format!("{mode} reset to {hash}?"),
                    body,
                    format!("{mode} reset"),
                    mode == "hard",
                    move |pane, cx| {
                        let params = pane.repo_call(json!({ "rev": hash, "mode": mode }));
                        pane.run(
                            format!("reset --{mode} {hash}"),
                            "git.reset",
                            params,
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            format!("Reset to {hash}"),
            vec![
                Pick::new('s', "soft reset", reset("soft", "Moves HEAD to this commit and keeps all later changes staged.")).hint("keep changes staged"),
                Pick::new('m', "mixed reset", reset("mixed", "Moves HEAD to this commit and keeps all later changes in the working tree.")).hint("keep changes unstaged"),
                Pick::new('h', "hard reset", reset("hard", "Moves HEAD to this commit and discards all later changes from the index and working tree."))
                    .hint("discard later changes")
                    .destructive(),
            ],
            cx,
        );
    }

    fn revert_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.revert(commit.hash, cx);
        }
    }

    pub(super) fn revert(&mut self, hash: String, cx: &mut Context<Self>) {
        self.confirm(
            format!("Revert {hash}?"),
            "Creates a new commit that reverses this commit. Existing history is preserved.",
            "revert",
            false,
            move |pane, cx| {
                let params = pane.repo_call(json!({ "rev": hash }));
                pane.run(
                    format!("reverting {hash}"),
                    "git.revert",
                    params,
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    fn staged_count(&self) -> usize {
        self.files().iter().filter(|file| is_staged(file)).count()
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.draft.read(cx).value().trim().to_owned();
        if message.is_empty() || self.busy.is_some() {
            return;
        }
        if self.staged_count() == 0 {
            self.fail("Stage the changes you want to commit first.", cx);
            return;
        }
        let params = json!({ "dir": self.repo, "message": message });
        self.run(
            "Commit staged changes",
            "commitStaged",
            params,
            cx,
            move |pane, answer, cx| {
                if answer.is_ok() && pane.draft.read(cx).value().trim() == message {
                    pane.clear_draft(cx);
                }
            },
        );
    }

    fn clear_draft(&mut self, cx: &mut Context<Self>) {
        self.write(String::new(), cx);
    }

    /// The key the core keeps the repository's commit message under.
    fn draft_key(&self) -> String {
        format!("git:{}", self.repo)
    }

    /// Takes the drafts the core keeps. The commit message follows the core's only while
    /// the person has typed nothing since they last agreed, so their own words are never
    /// overwritten and their cursor never jumps.
    pub fn drafts(&mut self, value: &Value, cx: &mut Context<Self>) {
        let kept = value[self.draft_key().as_str()]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
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
        self.core
            .call("setDraft", json!({ "key": self.draft_key(), "text": text }));
    }

    fn generate(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let before = self.draft.read(cx).value().to_string();
        self.generating = true;
        let (provider, label, model) = match self.writer() {
            Some((kind, label, model)) => (Some(kind), label, Some(model)),
            None => (None, "Hermes".to_owned(), None),
        };
        let params = self.repo_call(json!({ "provider": provider, "model": model }));
        self.run(format!("Generate message with {label}"), "git.aiMessage", params, cx, move |pane, answer, cx| {
            pane.generating = false;
            let Ok(Value::String(message)) = answer else {
                return;
            };
            if message.trim().is_empty() {
                pane.fail("The provider returned an empty message. Your draft was kept.", cx);
            } else if pane.draft.read(cx).value() != before {
                pane.fail("Your draft changed during generation. It was kept; generate again when ready.", cx);
            } else {
                pane.write(message, cx);
                pane.done("Commit message generated. Review it before committing.", cx);
            }
        });
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({ "remote": null }));
        self.run(
            "fetching all remotes…",
            "git.fetch",
            params,
            cx,
            |_, _, _| {},
        );
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({}));
        self.run("pulling…", "git.pull", params, cx, |_, _, _| {});
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({}));
        self.run("pushing…", "git.push", params, cx, |_, _, _| {});
    }

    fn pull_request(&mut self, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({}));
        self.run(
            "opening PR…",
            "git.pullRequestUrl",
            params,
            cx,
            |_, answer, cx| {
                if let Ok(Value::String(url)) = answer {
                    cx.open_url(&url);
                }
            },
        );
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        if self.draft.focus_handle(cx).is_focused(window) || self.asking.is_some() {
            return;
        }
        let modifiers = keys.modifiers;
        if modifiers.alt || modifiers.platform {
            return;
        }
        if modifiers.control {
            if keys.key == "p" {
                self.pull_request(cx);
                cx.stop_propagation();
            }
            return;
        }
        if self.busy.is_some() {
            return;
        }
        let typed = match keys.key.as_str() {
            named @ ("enter" | "space" | "up" | "down" | "escape") => named,
            key => keys.key_char.as_deref().unwrap_or(key),
        };
        let history = self.page == Page::History;
        match typed {
            "/" => self.find(window, cx),
            "?" => {
                self.asking = Some(Asking::Keys);
                cx.notify();
            }
            "@" => {
                self.commands_open = !self.commands_open;
                cx.notify();
            }
            "escape" if self.range.is_some() => {
                self.range = None;
                cx.notify();
            }
            "escape" => {
                if !self.unfind(window, cx) {
                    return;
                }
            }
            "1" => self.turn(Page::Changes, cx),
            "2" => self.turn(Page::History, cx),
            "3" => self.turn(Page::Branches, cx),
            "4" => self.turn(Page::Inbox, cx),
            "F" => self.fetch(cx),
            "P" => self.push(cx),
            "p" => self.pull(cx),
            "r" if !history => self.refresh(cx),
            _ if self.page == Page::Inbox => return,
            _ if self.page == Page::Branches && self.branch_key(typed, cx) => {}
            _ if self.page == Page::Branches => return,
            "h" if history => self.turn(Page::Changes, cx),
            "h" => self.turn(Page::History, cx),
            "j" | "down" if history => self.step_commit(1, cx),
            "k" | "up" if history => self.step_commit(-1, cx),
            " " | "space" | "enter" if history => self.commit_menu(cx),
            "b" if history => self.branch_from_commit(cx),
            "r" if history => self.reset_menu(cx),
            "v" if history => self.revert_commit(cx),
            _ if history => return,
            "j" => self.step(1, cx),
            "k" => self.step(-1, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            " " | "space" => self.toggle_stage(cx),
            "a" if self.files().iter().any(has_unstaged) => self.stage_all(cx),
            "a" => self.unstage_all(cx),
            "c" => {
                let draft = self.draft.clone();
                draft.update(cx, |draft, cx| draft.focus(window, cx));
            }
            "C" => self.commit(cx),
            "g" => self.generate(cx),
            "d" => self.discard_menu(cx),
            "v" => {
                self.range = match self.range {
                    Some(_) => None,
                    None => Some(self.selected),
                };
                cx.notify();
            }
            "s" => self.stash_menu(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for GitPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(written) = self.written.take() {
            self.draft
                .update(cx, |draft, cx| draft.set_value(written, window, cx));
        }
        if matches!(self.page, Page::History | Page::Branches) {
            self.read_authors(cx);
        }
        let reviewed = review(self, window, cx);
        let pushed =
            matches!(&self.overview, Some(Ok(overview)) if overview.status.upstream.is_some());
        let card = if self.page == Page::Branches {
            self.chosen_branch()
                .and_then(|entry| branch_card(self, &entry, cx))
        } else {
            self.review.commit().and_then(|hash| {
                let commit = self.log().iter().find(|commit| commit.full_hash == hash)?;
                Some(commit_card(self, commit, pushed, cx))
            })
        };
        let asking = self.asked(window, cx);
        div()
            .id("git-pane")
            .key_context("GitPane")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .gap(SPACE_2)
            .font_family(theme::UI)
            .on_key_down(cx.listener(Self::key))
            .child(side(self, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(card.map(|card| card.mb(SPACE_2)))
                    .child(div().flex_1().min_h_0().child(reviewed)),
            )
            .children(asking)
            .children(host_menu(self, cx))
    }
}

/// The side column: the branch with its sync actions, then the changes and the commit box.
fn side(pane: &mut GitPane, window: &mut Window, cx: &mut Context<GitPane>) -> impl IntoElement {
    let order = pane.page_order();
    let (shown, start, motion) = pane.slide.shown(pane.page, &order);
    let cards = shown
        .into_iter()
        .map(|(page, place)| (side_page(pane, page, window, cx), place))
        .collect();
    let this = cx.entity().downgrade();
    let pages_row = track::row(
        "git-side-pages",
        &pane.slide.width(),
        cards,
        start,
        motion,
        track::Between::Seam(Tokens::get(cx).border),
        move |event, cx| {
            this.update(cx, |pane, cx| pane.swipe(event, cx))
                .unwrap_or(false)
        },
    )
    .flex_1()
    .min_h_0()
    .w_full();
    let commands = pane.commands_open.then(|| command_log(pane, cx));
    div()
        .w(relative(0.3))
        .min_w(px(320.))
        .max_w(px(400.))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            ui::panel(cx)
                .flex_1()
                .min_h_0()
                .child(head(pane, cx))
                .children(
                    pane.out_of_reach
                        .clone()
                        .map(|host| crate::codehost::away_note(&host, cx).mx(SPACE_2)),
                )
                .child(pages_row)
                .child(pages(pane, cx)),
        )
        .children(commands)
}

/// One of the side column's pages, whole, with its filter.
fn side_page(
    pane: &mut GitPane,
    page: Page,
    window: &mut Window,
    cx: &mut Context<GitPane>,
) -> AnyElement {
    let count = (page == Page::History).then(|| pane.log().len());
    let filter = pane
        .filter_field(page, window, cx)
        .map(|field| filter_row(&field, count, cx));
    let body = div().size_full().flex().flex_col();
    match page {
        Page::Changes => body.child(changes(pane, filter, cx)),
        Page::History => body.children(filter).child(history_list(pane, cx)),
        Page::Branches => body.children(filter).child(branches_page(pane, cx)),
        Page::Inbox => body.child(div().flex_1().min_h_0().children(pane.inbox.clone())),
    }
    .into_any_element()
}

/// The git operations the pane ran, newest first.
fn command_log(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let button = |id: &'static str, label: &'static str, tip: &'static str, disabled: bool| {
        BaseButton::new(id)
            .accessibility_label(tip)
            .tooltip(ui::tooltip(tip))
            .disabled(disabled)
            .px(SPACE_2)
            .py(SPACE_0)
            .border_1()
            .border_color(t.border)
            .text_size(crate::theme::ui_text(TEXT_3XS_H))
            .text_color(t.text_secondary)
            .when(disabled, |button| button.opacity(0.5))
            .hover(move |style| {
                style
                    .text_color(t.text_primary)
                    .border_color(t.border_strong)
            })
            .child(label)
    };
    let rows = pane.commands.iter().rev().map(|entry| {
        let (glyph, ink) = match entry.done {
            None => ("…", t.warn),
            Some(true) => ("✓", t.live),
            Some(false) => ("✗", t.danger),
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .px(SPACE_3)
            .py(px(3.))
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .text_size(crate::theme::ui_text(TEXT_3XS))
                    .text_color(t.text_tertiary)
                    .child(entry.time.clone()),
            )
            .child(div().w(px(12.)).flex_none().text_color(ink).child(glyph))
            .child(
                div()
                    .flex_none()
                    .text_color(t.text_primary)
                    .child(entry.label.clone()),
            )
            .children(entry.detail.clone().map(|detail| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_tertiary)
                    .child(detail)
            }))
    });
    let empty = pane.commands.is_empty();
    div()
        .flex_none()
        .max_h(px(220.))
        .flex()
        .flex_col()
        .bg(t.surface)
        .border_t_1()
        .border_color(t.border)
        .font_family(theme::MONO)
        .text_color(t.text_secondary)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .px(SPACE_3)
                .py(SPACE_1H)
                .border_b_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .child(ui::kbd("@", cx))
                .child(
                    div()
                        .text_color(t.text_primary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("command log"),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(pane.commands.len().to_string()),
                )
                .child(div().flex_1())
                .child(
                    button("git-log-clear", "clear", "Clear log", empty).on_click(cx.listener(
                        |pane, _, _, cx| {
                            pane.commands.clear();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button("git-log-close", "close", "Hide log (@)", false).on_click(cx.listener(
                        |pane, _, _, cx| {
                            pane.commands_open = false;
                            cx.notify();
                        },
                    )),
                ),
        )
        .child(
            div()
                .id("git-log-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .py(SPACE_1)
                .when(empty, |rows| {
                    rows.child(
                        div()
                            .px(SPACE_4)
                            .py(SPACE_3H)
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .italic()
                            .text_color(t.text_tertiary)
                            .child("log empty — every git command will appear here"),
                    )
                })
                .children(rows),
        )
}

/// The icons at the side column's foot that turn its pages.
fn pages(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let page = |id: &'static str, icon: Icon, label: &'static str, which: Page| {
        ui::IconButton::new(id, icon, label)
            .width(32.)
            .glyph(14.)
            .pressed(pane.page == which)
            .on_click(cx.listener(move |pane, _, _, cx| pane.turn(which, cx)))
    };
    div()
        .flex_none()
        .flex()
        .justify_center()
        .gap(SPACE_1)
        .px(SPACE_2)
        .py(SPACE_1)
        .border_t_1()
        .border_color(t.border)
        .child(page(
            "git-page-changes",
            Icon::File,
            "Changes — 1",
            Page::Changes,
        ))
        .child(page(
            "git-page-history",
            Icon::Commit,
            "History — 2",
            Page::History,
        ))
        .child(page(
            "git-page-branches",
            Icon::Git,
            "Branches — 3",
            Page::Branches,
        ))
        .when(pane.has_inbox(), |pages| {
            pages.child(page(
                "git-page-inbox",
                Icon::Inbox,
                "Inbox — 4",
                Page::Inbox,
            ))
        })
}

/// The History page: the branch's commits on the graph.
fn history_list(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let inks = Inks::new(&t);
    let empty = pane.hashes.is_empty();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .when(empty, |page| {
            page.child(
                div()
                    .p(SPACE_5)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child("No commits on this branch yet."),
            )
        })
        .child(
            list(pane.commit_rows.clone(), move |index, _, cx| {
                let cx = &*cx;
                let pane = view.read(cx);
                commit_row(&view, pane, index, &inks, cx)
            })
            .flex_1()
            .px(SPACE_1H)
            .py(SPACE_1),
        )
}

/// The way back to the repositories inside the project folder, while one of them is open.
fn other_repos(pane: &GitPane, cx: &mut Context<GitPane>) -> Option<impl IntoElement> {
    let bench = pane.bench.upgrade()?;
    let inside = bench
        .read(cx)
        .git_folder()
        .is_some_and(|folder| folder != pane.repo && bench.read(cx).git_repos().is_some());
    inside.then(|| {
        ui::IconButton::new(
            "git-other-repos",
            Icon::Chevron,
            "Other repositories in this folder",
        )
        .turned(std::f32::consts::PI)
        .small()
        .on_click(move |_, _, cx| bench.update(cx, |bench, cx| bench.list_git_repos(cx)))
    })
}

fn head(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let status = match &pane.overview {
        Some(Ok(overview)) => Some(&overview.status),
        _ => None,
    };
    let branch = match (&pane.overview, status) {
        (None, _) => "…".to_owned(),
        (_, Some(status)) if !status.branch.is_empty() => status.branch.clone(),
        _ => "detached".to_owned(),
    };
    let (ahead, behind) = status.map_or((0, 0), |status| (status.ahead, status.behind));
    let upstream = status.is_some_and(|status| status.upstream.is_some());
    let failed = matches!(pane.overview, Some(Err(_)));
    let busy = pane.busy.clone();
    let held = if busy.is_some() {
        Some("Wait for the running git command")
    } else if pane.branches.remotes_read && pane.branches.remotes.is_empty() {
        Some("This repository has no remote yet")
    } else {
        None
    };
    let count = |value: i32, colour: Hsla, arrow: &str| {
        (value > 0).then(|| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(colour)
                .child(format!("{arrow}{value}"))
        })
    };
    let sync = |id: &'static str,
                icon: Icon,
                label: &'static str,
                tip: String,
                number: i32,
                primary: bool,
                act: fn(&mut GitPane, &mut Context<GitPane>),
                cx: &mut Context<GitPane>| {
        let primary = primary && held.is_none();
        BaseButton::new(id)
            .accessibility_label(label)
            .tooltip(ui::tooltip(held.map_or(tip, String::from)))
            .h(px(30.))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .map(|button| {
                if held.is_some() {
                    button.text_color(t.text_disabled)
                } else if primary {
                    button.bg(t.accent).text_color(t.on_accent)
                } else {
                    button
                        .text_color(t.text_secondary)
                        .hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
                }
            })
            .when(held.is_none(), |button| {
                button.on_click(cx.listener(move |pane, _, _, cx| act(pane, cx)))
            })
            .child(ui::icon(icon, 12.).text_color(if primary {
                t.on_accent
            } else if held.is_some() {
                t.text_disabled
            } else {
                t.text_tertiary
            }))
            .child(label)
            .when(number > 0, |button| {
                button.child(
                    div()
                        .text_color(if primary { t.on_accent } else { t.accent })
                        .child(number.to_string()),
                )
            })
    };
    let plural = |count: i32| if count == 1 { "" } else { "s" };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .flex_none()
        .pt(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1)
                .min_w_0()
                .children(other_repos(pane, cx))
                .child(branch_switch(pane, branch, cx))
                .children(count(ahead, t.live, "↑"))
                .children(count(behind, t.warn, "↓"))
                .child(div().flex_1())
                .when(failed, |title| {
                    title.child(
                        div()
                            .h(px(18.))
                            .px(SPACE_1H)
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(t.danger.opacity(0.32))
                            .rounded(px(4.))
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.danger)
                            .child("git error"),
                    )
                })
                .children(busy.map(|busy| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.warn)
                        .child(busy)
                }))
                .children(host_account(pane, cx))
                .child(more_menu(pane, cx)),
        )
        .child(
            div()
                .flex()
                .border_t_1()
                .border_color(t.border)
                .child(sync(
                    "git-fetch",
                    Icon::Fetch,
                    "Fetch",
                    "Fetch all remotes (F)".into(),
                    0,
                    false,
                    GitPane::fetch,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-pull",
                    Icon::Pull,
                    "Pull",
                    if behind > 0 {
                        format!("Pull {behind} commit{} (p)", plural(behind))
                    } else {
                        "Pull (p)".into()
                    },
                    behind,
                    false,
                    GitPane::pull,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-push",
                    Icon::Push,
                    if upstream { "Push" } else { "Publish" },
                    if !upstream {
                        "Publish branch and set upstream (P)".into()
                    } else if ahead > 0 {
                        format!("Push {ahead} commit{} (P)", plural(ahead))
                    } else {
                        "Push (P)".into()
                    },
                    ahead,
                    ahead > 0 || !upstream,
                    GitPane::push,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-pr",
                    Icon::PullRequest,
                    "PR",
                    "Open pull request (⌃P)".into(),
                    0,
                    false,
                    GitPane::pull_request,
                    cx,
                )),
        )
}

/// A line of one of the head's menus, doing `act` to the pane.
fn menu_line(
    view: &Entity<GitPane>,
    row: ui::MenuRow,
    act: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
    cx: &App,
) -> PopupMenuItem {
    let view = view.clone();
    row.item(cx)
        .on_click(move |_, _, cx| view.update(cx, |pane, cx| act(pane, cx)))
}

/// The branch in the head, which opens a menu to make, find or switch to a branch.
fn branch_switch(pane: &GitPane, branch: String, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let branches: Vec<(String, bool, Option<String>)> = match &pane.overview {
        Some(Ok(overview)) => overview
            .branches
            .iter()
            .map(|branch| (branch.name.clone(), branch.current, branch.upstream.clone()))
            .collect(),
        _ => Vec::new(),
    };
    ui::menu_button(
        "git-branch-switch",
        Anchor::TopLeft,
        BaseButton::new("git-switch")
            .accessibility_label("Switch branch")
            .tooltip(ui::tooltip("Switch branch"))
            .h(px(26.))
            .px(SPACE_1H)
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(RADIUS_2)
            .text_color(t.text_primary)
            .hover(move |style| style.bg(t.surface_hover))
            .child(ui::icon(Icon::Git, 13.).text_color(t.text_tertiary))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .font_weight(FontWeight::MEDIUM)
                    .child(branch),
            )
            .child(
                ui::icon(Icon::Chevron, 9.)
                    .text_color(t.text_tertiary)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2))),
            ),
        move |mut popup, _, cx| {
            popup = popup
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("New branch…").icon(Icon::Plus).hint("N"),
                    |pane, cx| pane.new_branch(None, cx),
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Check out by name…")
                        .icon(Icon::Checkout)
                        .hint("c"),
                    |pane, cx| pane.checkout_by_name(cx),
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Show all branches")
                        .icon(Icon::Git)
                        .hint("3"),
                    |pane, cx| pane.turn(Page::Branches, cx),
                    cx,
                ))
                .separator();
            for (name, current, upstream) in &branches {
                let checked = name.clone();
                let hint = match current {
                    true => Some("current".to_owned()),
                    false => upstream.clone(),
                };
                popup = popup.item(
                    menu_line(
                        &view,
                        ui::MenuRow::new(name.clone())
                            .icon(Icon::Git)
                            .when_some(hint, ui::MenuRow::hint),
                        move |pane, cx| pane.checkout(checked.clone(), cx),
                        cx,
                    )
                    .disabled(*current),
                );
            }
            popup
        },
    )
}

/// The project's code host account, lent by its areas: the person's face and its menu, or
/// Sign in while signed out.
fn host_account(pane: &GitPane, cx: &mut Context<GitPane>) -> Option<AnyElement> {
    let host = pane.bench.upgrade()?.read(cx).git.host.clone()?;
    Some(host.update(cx, |area, cx| crate::codehost::shell::account(area, cx)))
}

/// The code host's account menu, while the person has it open from the head.
fn host_menu(pane: &GitPane, cx: &mut Context<GitPane>) -> Option<ui::Choices> {
    let host = pane.bench.upgrade()?.read(cx).git.host.clone()?;
    host.update(cx, |area, cx| area.account_menu(cx))
}

/// The head's menu of everything else: remotes, stashes, and the pane's own tools.
fn more_menu(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let view = cx.entity();
    let changed = !pane.files().is_empty();
    let unstaged = pane.files().iter().any(has_unstaged);
    let stashes = pane.stashes.len();
    let commands_open = pane.commands_open;
    let host = pane
        .bench
        .upgrade()
        .and_then(|bench| bench.read(cx).git.host.clone());
    ui::menu_button(
        "git-more",
        Anchor::TopRight,
        ui::IconButton::new("git-more-button", Icon::More, "Remotes, stashes and more").glyph(13.),
        move |mut popup, _, cx| {
            if let Some(host) = host.clone() {
                popup = popup
                    .item(
                        ui::MenuRow::new("Choose another repository…")
                            .icon(Icon::Git)
                            .item(cx)
                            .on_click(move |_, window, cx| {
                                host.update(cx, |area, cx| area.pick_repo(window, cx));
                            }),
                    )
                    .separator();
            }
            popup
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Fetch all remotes")
                        .icon(Icon::Fetch)
                        .hint("F"),
                    GitPane::fetch,
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Add remote…").icon(Icon::Plus),
                    GitPane::add_remote,
                    cx,
                ))
                .separator()
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new("Stash changes…")
                            .icon(Icon::Stack)
                            .hint("s"),
                        GitPane::stash_menu,
                        cx,
                    )
                    .disabled(!changed),
                )
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new("Discard unstaged changes…")
                            .icon(Icon::Discard)
                            .danger(true),
                        GitPane::discard_all,
                        cx,
                    )
                    .disabled(!unstaged),
                )
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new(if stashes > 0 {
                            format!("Stashes ({stashes})…")
                        } else {
                            "No stashes".to_owned()
                        })
                        .icon(Icon::Stack),
                        GitPane::stashes_menu,
                        cx,
                    )
                    .disabled(stashes == 0),
                )
                .separator()
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Refresh").icon(Icon::Refresh).hint("r"),
                    GitPane::refresh,
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new(if commands_open {
                        "Hide command log"
                    } else {
                        "Show command log"
                    })
                    .icon(Icon::LogLines)
                    .hint("@"),
                    |pane, cx| {
                        pane.commands_open = !pane.commands_open;
                        cx.notify();
                    },
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Keyboard shortcuts")
                        .icon(Icon::Keyboard)
                        .hint("?"),
                    |pane, cx| {
                        pane.asking = Some(Asking::Keys);
                        cx.notify();
                    },
                    cx,
                ))
        },
    )
}

/// A page's filter: what to find, and how many there are when the page counts them.
fn filter_row(field: &Entity<InputState>, count: Option<usize>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .h(px(34.))
        .mb(SPACE_1)
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(ui::icon(Icon::Search, 12.).text_color(t.text_tertiary))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(field).appearance(false)),
        )
        .children(count.filter(|count| *count > 0).map(|count| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(count.to_string())
        }))
}

fn changes(pane: &mut GitPane, filter: Option<Div>, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let empty = pane.entries.is_empty();
    let query = pane.filters[Page::Changes as usize].query.clone();
    let note = match &pane.overview {
        _ if !empty => None,
        None => Some((
            "Loading changed files".to_owned(),
            String::new(),
            t.text_tertiary,
        )),
        Some(Err(error)) => Some(("Git error".to_owned(), error.clone(), t.danger)),
        Some(Ok(_)) if !query.is_empty() => Some((
            "No matches".to_owned(),
            format!("Nothing matches \"{query}\"."),
            t.text_tertiary,
        )),
        Some(Ok(_)) => Some((
            "Nothing to commit".to_owned(),
            "The working tree is clean.".to_owned(),
            t.text_tertiary,
        )),
    };
    let view = cx.entity();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .pb(SPACE_1H)
        .child(composer(pane, cx))
        .children(filter)
        .children(note.map(|(title, message, colour)| {
            div()
                .p(SPACE_5)
                .flex()
                .flex_col()
                .items_center()
                .gap(SPACE_1)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(div().text_color(colour).child(title))
                .child(div().text_color(t.text_tertiary).child(message))
        }))
        .child(
            list(pane.rows.clone(), move |index, window, cx| {
                let cx = &*cx;
                let pane = view.read(cx);
                match pane.entries.get(index).cloned() {
                    Some(entry) => entry_row(&view, pane, entry, index, window, cx),
                    None => div().into_any_element(),
                }
            })
            .flex_1()
            .px(SPACE_1H),
        )
}

/// The 28px button beside the message's writer that picks the agent and model.
fn writer_picker(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let writers = pane.writers.clone();
    let current = pane.writer().map(|(kind, _, model)| (kind, model));
    ui::menu_button(
        "git-writer",
        Anchor::TopLeft,
        BaseButton::new("git-writer-pick")
            .accessibility_label("Pick the agent and model")
            .tooltip(ui::tooltip("Pick the agent and model"))
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(t.border)
            .hover(move |style| style.bg(t.surface_hover).border_color(t.border_strong))
            .child(
                ui::icon(Icon::Chevron, 9.)
                    .text_color(t.text_tertiary)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2))),
            ),
        move |mut popup, _, cx| {
            for (index, (kind, label, models)) in writers.iter().enumerate() {
                if index > 0 {
                    popup = popup.separator();
                }
                for model in models {
                    let picked = Some((kind.clone(), model.clone()));
                    let checked = picked == current;
                    let (view, kind) = (view.clone(), kind.clone());
                    popup = popup.item(
                        ui::MenuRow::new(format!("{label} · {model}"))
                            .mark(move || ui::mark(&kind, 13.))
                            .item(cx)
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let picked = picked.clone();
                                view.update(cx, |pane, cx| {
                                    pane.writer = picked;
                                    cx.notify();
                                });
                            }),
                    );
                }
            }
            popup
        },
    )
}

fn composer(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let writer_label = match pane.writer() {
        Some((_, label, model)) => {
            let model = model
                .split_once('/')
                .map_or(model.as_str(), |(_, model)| model);
            format!("{label} · {model}")
        }
        None => "Write message".to_owned(),
    };
    let staged = pane.staged_count();
    let message = !pane.draft.read(cx).value().trim().is_empty();
    let can_commit = pane.busy.is_none() && staged > 0 && message;
    let label = if staged > 0 {
        format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" })
    } else {
        "Commit".to_owned()
    };
    let tip = if staged > 0 {
        format!("{label} (⌘⏎)")
    } else {
        "Stage files to commit them".to_owned()
    };
    div()
        .flex_none()
        .mb(SPACE_1H)
        .border_b_1()
        .border_color(t.border)
        .capture_key_down(cx.listener(|pane, event: &KeyDownEvent, window, cx| {
            let keys = &event.keystroke;
            if keys.key == "enter" && (keys.modifiers.platform || keys.modifiers.control) {
                pane.commit(cx);
                cx.stop_propagation();
            } else if keys.key == "escape" {
                window.focus(&pane.focus, cx);
                cx.stop_propagation();
            }
        }))
        .child(
            div()
                .px(SPACE_2H)
                .py(px(9.))
                .text_size(crate::theme::ui_text(TEXT_SM))
                .line_height(crate::theme::ui_text(19.5))
                .child(
                    Textarea::new(&pane.draft)
                        .appearance(false)
                        .text_size(crate::theme::ui_text(TEXT_SM)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .px(SPACE_2H)
                .pb(SPACE_2)
                .child(
                    BaseButton::new("git-generate")
                        .accessibility_label("Write the message from the staged changes")
                        .tooltip(ui::tooltip("Write the message from the staged changes (g)"))
                        .h(px(28.))
                        .px(SPACE_2)
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .border_1()
                        .border_color(t.border)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .hover(move |style| {
                            style
                                .bg(t.surface_hover)
                                .border_color(t.border_strong)
                                .text_color(t.text_primary)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| pane.generate(cx)))
                        .child(ui::icon(Icon::Sparkle, 11.).text_color(t.cmd))
                        .child(div().min_w_0().truncate().child(if pane.generating {
                            "writing…".to_owned()
                        } else {
                            writer_label
                        })),
                )
                .child(writer_picker(pane, cx))
                .child(
                    BaseButton::new("git-commit")
                        .accessibility_label(SharedString::from(label.clone()))
                        .tooltip(ui::tooltip(tip))
                        .disabled(!can_commit)
                        .ml_auto()
                        .h(px(28.))
                        .px(SPACE_3)
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(t.accent)
                        .bg(t.accent)
                        .text_color(t.on_accent)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .font_weight(FontWeight::MEDIUM)
                        .when(!can_commit, |button| button.opacity(0.45))
                        .on_click(cx.listener(|pane, _, _, cx| pane.commit(cx)))
                        .child(label),
                ),
        )
}

/// The most faces a changed file shows of the agents that wrote it.
const MAX_FACES: usize = 4;

/// The faces of the agents that wrote some of `path`'s changes: each agent's own face,
/// else its agent's mark.
fn faces(pane: &GitPane, path: &str, cx: &App) -> Option<AnyElement> {
    let (writers, names) = pane.writers(path);
    if writers.is_empty() {
        return None;
    }
    let rows: Vec<AgentRow> = pane.bench.upgrade().map_or_else(Vec::new, |bench| {
        let bench = bench.read(cx);
        bench
            .projects()
            .iter()
            .flat_map(|project| bench.agents_of(&project.id))
            .collect()
    });
    let marks: Vec<AnyElement> = writers
        .iter()
        .take(MAX_FACES)
        .map(|(id, cli)| match rows.iter().find(|row| &row.id == id) {
            Some(row) => row.mark(14., cx),
            None => ui::mark(cli, 14.),
        })
        .collect();
    Some(
        div()
            .id(SharedString::from(format!("faces:{path}")))
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_0)
            .tooltip(ui::tooltip(format!("Written by {}", names.join(", "))))
            .children(marks)
            .into_any_element(),
    )
}

/// A glyph a row shows while the pointer is on it.
fn row_button(
    id: impl Into<SharedString>,
    icon: Icon,
    glyph: f32,
    label: impl Into<SharedString>,
    cx: &App,
) -> BaseButton {
    let t = Tokens::get(cx);
    let (rest, lit, wash) = (t.text_tertiary, t.text_primary, t.surface_active);
    let label = label.into();
    BaseButton::new(id.into())
        .group(ROW_ACTION)
        .accessibility_label(label.clone())
        .tooltip(ui::tooltip(label))
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(wash))
        .child(
            ui::icon(icon, glyph)
                .text_color(rest)
                .group_hover(ROW_ACTION, move |style| style.text_color(lit)),
        )
}

/// The hover group a row's glyph button puts its glyph in, so the glyph lights with it.
const ROW_ACTION: &str = "git-row-action";

/// The hover actions of a changed file, as its list row and its review card show them:
/// open it, discard its unstaged change, and move it to the other group.
pub fn file_actions(
    view: &Entity<GitPane>,
    file: &GitFile,
    side: Option<Side>,
    cx: &App,
) -> Vec<AnyElement> {
    let action = |id: String, icon: Icon, label: String, act: Act| {
        let view = view.clone();
        row_button(id, icon, 12., label, cx)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let act = act.clone();
                view.update(cx, |pane, cx| act(pane, cx));
            })
            .into_any_element()
    };
    let unstaged = side.map_or(has_unstaged(file), |side| side == Side::Unstaged);
    let staged = side.map_or(is_staged(file), |side| side == Side::Staged);
    let key = format!("{}:{:?}", file.path, side);
    let mut actions = Vec::new();
    if side.is_some() {
        let target = OpenAt {
            path: Path::new(&view.read(cx).repo).join(&file.path),
            line: 0,
            column: None,
        };
        actions.push(
            row_button(
                format!("open:{key}"),
                Icon::File,
                12.,
                format!("Open {}", file.path),
                cx,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(target.boxed_clone(), cx);
            })
            .into_any_element(),
        );
    }
    if unstaged {
        let discarded = file.clone();
        actions.push(action(
            format!("discard:{key}"),
            Icon::Discard,
            "Discard changes".into(),
            Rc::new(move |pane, cx| pane.discard(&discarded, cx)),
        ));
    }
    if staged {
        let moved = file.clone();
        actions.push(action(
            format!("unstage:{key}"),
            Icon::Minus,
            format!("Unstage {}", file.path),
            Rc::new(move |pane, cx| pane.unstage(&moved, cx)),
        ));
    }
    if unstaged {
        let moved = file.clone();
        actions.push(action(
            format!("stage:{key}"),
            Icon::Plus,
            format!("Stage {}", file.path),
            Rc::new(move |pane, cx| pane.stage(&moved, cx)),
        ));
    }
    actions
}

fn entry_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    entry: Entry,
    index: usize,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    match entry {
        Entry::Group { side, count } => group_row(view, pane, side, count, index, cx),
        Entry::File { side, file } => {
            let first = !matches!(
                pane.entries.get(index.wrapping_sub(1)),
                Some(Entry::File { .. })
            );
            let last = !matches!(pane.entries.get(index + 1), Some(Entry::File { .. }));
            let chosen = index == pane.selected && pane.focus.contains_focused(window, cx);
            let ranged = pane.range.is_some_and(|anchor| {
                (anchor.min(pane.selected)..=anchor.max(pane.selected)).contains(&index)
            });
            let (dir, base) = match file.path.trim_end_matches('/').rsplit_once('/') {
                Some((dir, base)) => (Some(dir.to_owned()), base.to_owned()),
                None => (None, file.path.clone()),
            };
            let code = match side {
                Side::Staged => file.index.as_str(),
                Side::Unstaged => file.worktree.as_str(),
            };
            let letter = decoration(code, &t).map(|(letter, colour, label)| {
                let colour = if side == Side::Unstaged && letter == "M" {
                    t.git_renamed
                } else {
                    colour
                };
                div()
                    .id(SharedString::from(format!("status:{}:{side:?}", file.path)))
                    .flex_none()
                    .w(px(14.))
                    .text_center()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colour)
                    .tooltip(ui::tooltip(label))
                    .child(letter)
            });
            let glyph = match crate::file_icons::icon(&base, cx) {
                Some(icon) => img(icon).size(px(14.)).flex_none().into_any_element(),
                None => ui::icon(Icon::File, 14.)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            };
            let key = SharedString::from(format!("row:{}:{side:?}", file.path));
            let selecting = view.clone();
            div()
                .id(SharedString::from(format!(
                    "git-file:{}:{side:?}",
                    file.path
                )))
                .on_hover(hover(view, key.clone()))
                .relative()
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(18.))
                .pr(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .rounded(RADIUS_2)
                .text_size(crate::theme::ui_text(TEXT_SM))
                .text_color(if chosen {
                    t.text_primary
                } else {
                    t.text_secondary
                })
                .when(chosen, |row| row.bg(t.surface_chosen))
                .when(ranged && !chosen, |row| row.bg(t.warn.opacity(0.1)))
                .when(!chosen, |row| {
                    row.hover(move |row| row.bg(t.surface_hover).text_color(t.text_primary))
                })
                .on_click(move |_, window, cx| {
                    selecting.update(cx, |pane, cx| {
                        window.focus(&pane.focus, cx);
                        pane.select(index, cx);
                    })
                })
                .children(tree_lines(first, last, &t))
                .children(ui::spot(|| format!("git-file-{}", file.path)))
                .child(glyph)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().flex_none().child(base))
                        .children(dir.map(|dir| {
                            div()
                                .ml(SPACE_1H)
                                .min_w_0()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_XS))
                                .text_color(t.text_tertiary)
                                .child(dir)
                        })),
                )
                .when(pane.hovered.as_ref() != Some(&key), |row| {
                    row.children(faces(pane, &file.path, cx))
                })
                .when(pane.hovered.as_ref() == Some(&key), |row| {
                    row.child(div().flex().gap(SPACE_0).children(file_actions(
                        view,
                        &file,
                        Some(side),
                        cx,
                    )))
                })
                .children(letter)
                .into_any_element()
        }
    }
}

/// The lines a group's row hangs from, as the project tree's children do: a spine from
/// under the group's chevron, a tick to the row, and an elbow at the last.
pub(crate) fn tree_lines(first: bool, last: bool, t: &Tokens) -> Vec<Div> {
    let top = px(if first { -13. } else { -1. });
    if last {
        return vec![div()
            .absolute()
            .left(px(12.))
            .top(top)
            .w(px(5.))
            .h(px(if first { 27. } else { 14.5 }))
            .border_l_1()
            .border_b_1()
            .border_color(t.tree_spine)
            .rounded_bl(px(4.))];
    }
    vec![
        div()
            .absolute()
            .left(px(12.))
            .top(top)
            .bottom(px(-1.))
            .w(px(1.))
            .bg(t.tree_spine),
        div()
            .absolute()
            .left(px(13.))
            .top(px(12.))
            .w(px(4.))
            .h(px(1.))
            .bg(t.tree_tick),
    ]
}

fn group_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    side: Side,
    count: usize,
    index: usize,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let folded = pane.folded.contains(&side);
    let staged = side == Side::Staged;
    let key = SharedString::from(format!("group:{side:?}"));
    let button =
        |id: &str, icon: Icon, label: &str, act: fn(&mut GitPane, &mut Context<GitPane>)| {
            let view = view.clone();
            row_button(format!("{id}-{side:?}"), icon, 13., label.to_owned(), cx).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    view.update(cx, act);
                },
            )
        };
    let folding = view.clone();
    div()
        .when(index > 0, |row| row.pt(SPACE_1H))
        .child(
            div()
                .id(SharedString::from(format!("git-group:{side:?}")))
                .on_hover(hover(view, key.clone()))
                .h(px(ROW_HEIGHT))
                .pl(SPACE_0)
                .pr(SPACE_1)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .rounded(RADIUS_2)
                .hover(move |row| row.bg(t.surface_hover))
                .child(
                    div()
                        .id(SharedString::from(format!("git-group-toggle:{side:?}")))
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px(SPACE_1)
                        .items_center()
                        .gap(SPACE_1)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            folding.update(cx, |pane, cx| {
                                if !pane.folded.remove(&side) {
                                    pane.folded.insert(side);
                                }
                                pane.list(cx);
                            })
                        })
                        .child(motion::turn(
                            format!("{key}:chevron"),
                            motion::fold_angle(!folded),
                            motion::LIST_FOLD,
                            ui::icon(Icon::Chevron, 13.).text_color(t.text_tertiary),
                        ))
                        .child(
                            ui::icon(if staged { Icon::FilePlus } else { Icon::File }, 13.)
                                .text_color(t.text_tertiary),
                        )
                        .child(
                            ui::text(
                                if staged { "Staged Changes" } else { "Changes" },
                                TEXT_XS,
                                FontWeight::MEDIUM,
                            )
                            .text_color(t.text_primary),
                        ),
                )
                .when(pane.hovered.as_ref() == Some(&key), |row| {
                    row.child(
                        div()
                            .flex()
                            .gap(SPACE_0)
                            .when(index == 0, |actions| {
                                actions.child(button(
                                    "git-fold-review",
                                    if pane.review_folded {
                                        Icon::ExpandAll
                                    } else {
                                        Icon::CollapseAll
                                    },
                                    if pane.review_folded {
                                        "Expand all diffs"
                                    } else {
                                        "Collapse all diffs"
                                    },
                                    |pane, cx| {
                                        pane.review_folded = !pane.review_folded;
                                        pane.review.fold_all(pane.review_folded);
                                        cx.notify();
                                    },
                                ))
                            })
                            .when(index == 0, |actions| {
                                actions.child(button(
                                    "git-stash",
                                    Icon::Stash,
                                    "Stash",
                                    GitPane::stash_menu,
                                ))
                            })
                            .when(!staged, |actions| {
                                actions.child(button(
                                    "git-discard-all",
                                    Icon::Discard,
                                    "Discard all",
                                    GitPane::discard_all,
                                ))
                            })
                            .map(|actions| {
                                if staged {
                                    actions.child(button(
                                        "git-unstage-all",
                                        Icon::Minus,
                                        "Unstage all",
                                        GitPane::unstage_all,
                                    ))
                                } else {
                                    actions.child(button(
                                        "git-stage-all",
                                        Icon::Plus,
                                        "Stage all",
                                        GitPane::stage_all,
                                    ))
                                }
                            }),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .min_w(px(18.))
                        .h(px(18.))
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(t.surface_chosen)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_secondary)
                        .child(count.to_string()),
                ),
        )
        .into_any_element()
}

/// Keeps the pane's `hovered` on `key` while the pointer is inside it.
pub fn hover(
    view: &Entity<GitPane>,
    key: SharedString,
) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |inside, _, cx| {
        view.update(cx, |pane, cx| {
            if *inside {
                pane.hovered = Some(key.clone());
            } else if pane.hovered.as_ref() == Some(&key) {
                pane.hovered = None;
            }
            cx.notify();
        })
    }
}

fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().to_owned()
}
