//! Jira's pane: the person's lists, spaces and starred filters beside the issues of the one
//! picked, as a list or as a board, and the issue opened beside them to read, move along,
//! assign and comment on.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, rgb, AnyElement, App, AppContext, Context, Div, DragMoveEvent, Entity,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseUpEvent, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Transformation, WeakEntity,
    Window,
};
use gpui_transcript::{Markdown, Pen};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_core::git::log::GitCommit;
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_jira_views::{
    Activity, AssignRequest, AssignableRequest, BoardPage, BoardRequest, Column, CommentRequest,
    IssueList, IssueListKind, IssueRequest, IssueSummary, IssueView, ListRequest, MoveRequest,
    Parent, Person, SiteRequest, Sites, Spaces, TaskRequest, TransitionRequest,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_1, RADIUS_2};
use crate::toast;
use crate::ui::{self, sign_in, Button, IconButton, Segmented, Toggle, Variant};

mod board;
mod issue;
mod list;
mod parts;
mod sidebar;
mod signing_in;

use self::list::issue_menu;
use self::parts::{
    chevron, due_chip, epic_chip, heading, priority_mark, said, status_chip, tag, type_mark,
};

const API_TOKENS_PAGE: &str = "https://id.atlassian.com/manage-profile/security/api-tokens";
const SPACES_SHOWN: usize = 8;
const FACES: usize = 6;

/// The Jira sites signed in to, as the plugin's `status` says.
type Status = SignInStatus<Sites>;

fn jira_blue() -> Hsla {
    rgb(0x7aa7f5).into()
}

/// What the middle of the pane shows.
#[derive(Clone, PartialEq, Default)]
enum List {
    #[default]
    Mine,
    Reported,
    Watching,
    Recent,
    Sprint,
    Project {
        key: String,
        name: String,
    },
    Unassigned {
        key: String,
        name: String,
    },
    Board {
        id: u64,
        name: String,
    },
    Filter {
        id: String,
        name: String,
        jql: String,
    },
    Search(String),
}

impl List {
    /// What the plugin lists for it; a board is read as the board itself.
    fn asked(&self) -> Option<IssueListKind> {
        Some(match self {
            List::Mine => IssueListKind::Mine,
            List::Reported => IssueListKind::Reported,
            List::Watching => IssueListKind::Watching,
            List::Recent => IssueListKind::Recent,
            List::Sprint => IssueListKind::Sprint,
            List::Project { key, .. } => IssueListKind::Project { key: key.clone() },
            List::Unassigned { key, .. } => IssueListKind::Unassigned { key: key.clone() },
            List::Filter { jql, .. } => IssueListKind::Filter { jql: jql.clone() },
            List::Search(text) => IssueListKind::Search { text: text.clone() },
            List::Board { .. } => return None,
        })
    }

    fn empty(&self) -> &'static str {
        match self {
            List::Mine => {
                "Nothing is assigned to you that is open or was done in the last two weeks."
            }
            List::Reported => "Nothing you reported is open or was done in the last two weeks.",
            List::Watching => "You are not watching any open issue.",
            List::Recent => "No issue viewed lately.",
            List::Unassigned { .. } => "Every open issue here has someone on it.",
            _ => "No issues here.",
        }
    }

    fn id(&self) -> String {
        match self {
            List::Mine => "mine".into(),
            List::Reported => "reported".into(),
            List::Watching => "watching".into(),
            List::Recent => "recent".into(),
            List::Sprint => "sprint".into(),
            List::Project { key, .. } => format!("project-{key}"),
            List::Unassigned { key, .. } => format!("unassigned-{key}"),
            List::Board { id, .. } => format!("board-{id}"),
            List::Filter { id, .. } => format!("filter-{id}"),
            List::Search(text) => format!("search-{text}"),
        }
    }
}

fn my_work() -> [(List, &'static str, Icon); 5] {
    [
        (List::Mine, "Assigned to me", Icon::User),
        (List::Reported, "Reported by me", Icon::Pencil),
        (List::Watching, "Watching", Icon::Eye),
        (List::Recent, "Recently viewed", Icon::Clock),
        (List::Sprint, "Current sprint", Icon::Timer),
    ]
}

/// What the open issue's activity shows.
#[derive(Clone, Copy, Default, PartialEq)]
enum Shown {
    All,
    #[default]
    Comments,
    History,
}

const SHOWN: [Shown; 3] = [Shown::All, Shown::Comments, Shown::History];

/// The fields of the sign-in with an API token.
struct TokenForm {
    site: Entity<InputState>,
    email: Entity<InputState>,
    token: Entity<InputState>,
    busy: bool,
}

/// What a field does as the person types in it or presses Return.
type Typed = fn(&mut Jira, &mut Host, &str, &mut Context<PluginPane>);

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.jira",
    surface: "issues",
    pane: super::boxed::<Jira>,
    top_bar: None,
    palettes: &[],
};

/// What the pane keeps between sessions, saved with the workspace under the plugin's id.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    detail_width: Option<f32>,
}

#[derive(Default)]
pub struct Jira {
    listing: Listing,
    issue: Option<OpenIssue>,
    /// The site picked; empty for the first the plugin is signed in to.
    site: String,
    /// The site the pane last drew.
    shown_site: String,
    search: Option<Entity<InputState>>,
    space_find: Option<Entity<InputState>>,
    card_find: Option<Entity<InputState>>,
    draft: Option<Entity<TextareaState>>,
    form: Option<TokenForm>,
    typing: Vec<Subscription>,
    /// The spaces opened or closed against the one open by default.
    toggled: HashSet<String>,
    every_space: bool,
    shown: Shown,
    commenting: bool,
    /// How wide the open issue is while its edge is dragged and after.
    detail_width: Option<f32>,
    /// Shows only the open issue, as a desk does a link to it.
    alone: bool,
}

/// The list picked and how it is narrowed, which starts over when another list is picked.
#[derive(Default)]
struct Listing {
    list: List,
    /// The statuses a list is narrowed to; none shows them all.
    statuses: BTreeSet<String>,
    /// The people a board is narrowed to by account, with `me` and `none` for nobody.
    picked: BTreeSet<String>,
    every_face: bool,
    folded: HashSet<String>,
    /// Cards moved here, by key, to the column they were moved to, until the board comes again.
    moves: HashMap<String, usize>,
    last_board: Option<Rc<BoardPage>>,
}

/// The issue open beside the list and what is under way on it, which starts over when
/// another issue opens.
struct OpenIssue {
    key: String,
    busy: bool,
    assigning: bool,
    assign_find: Option<Entity<InputState>>,
    /// The people the issue can be given to, as the search found them.
    people: Vec<Person>,
    /// Each text drawn as markdown, by its words.
    docs: HashMap<String, Rc<Markdown>>,
    /// The commits in the project in front that name the issue, by the repository and newest
    /// commit they were asked at; none while the core is asked.
    commits: Option<(Asked, Option<Vec<GitCommit>>)>,
}

/// The repository and its newest commit, which commits naming an issue were asked at.
type Asked = (String, Option<String>);

impl OpenIssue {
    fn new(key: String) -> Self {
        Self {
            key,
            busy: false,
            assigning: false,
            assign_find: None,
            people: Vec::new(),
            docs: HashMap::new(),
            commits: None,
        }
    }
}

impl Panel for Jira {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let status = match host.status(cx).read::<Status>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let sites: Vec<String> = status
            .details
            .sites
            .iter()
            .map(|site| site.host.clone())
            .collect();
        if host.signed_in(cx) == Some(false) || sites.is_empty() {
            return self.sign_in(host, &status, window, cx);
        }
        self.form = None;
        let site = if sites.contains(&self.site) {
            self.site.clone()
        } else {
            sites[0].clone()
        };
        self.shown_site = site.clone();
        if let Some(key) = self.open_key().filter(|_| self.alone).map(str::to_owned) {
            return self.detail(host, &key, &site, window, cx);
        }
        let sidebar = self.sidebar(host, &sites, &site, window, cx);
        let middle = match &self.listing.list {
            List::Board { id, .. } => self.board(host, *id, &site, window, cx),
            _ => self.issues(host, &site, cx),
        };
        let board = matches!(self.listing.list, List::Board { .. });
        let detail = match self.open_key().map(str::to_owned) {
            Some(key) => Some(self.detail(host, &key, &site, window, cx)),
            None if !board => Some(super::empty(
                "Pick an issue to read it, comment, move it along or assign it.",
                cx,
            )),
            None => None,
        };
        let saved = host.read_settings::<Settings>(cx).detail_width;
        let width = self.detail_width.or(saved);
        let t = Tokens::get(cx);
        div()
            .id("jira-columns")
            .size_full()
            .flex()
            .overflow_hidden()
            .on_drag_move(listen::<Jira, DragMoveEvent<DetailEdge>>(
                cx,
                |jira, _, event, _, cx| {
                    let bounds = event.bounds;
                    let room =
                        f32::from(bounds.size.width) - SIDEBAR - LIST_MIN - crate::panes::GAP;
                    let wanted =
                        f32::from(bounds.right() - event.event.position.x) - crate::panes::GAP / 2.;
                    jira.detail_width = Some(wanted.clamp(DETAIL_MIN, room.max(DETAIL_MIN)));
                    cx.notify();
                },
            ))
            .on_drop(listen::<Jira, DetailEdge>(cx, |jira, host, _, _, cx| {
                jira.keep_width(host, cx)
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                listen::<Jira, MouseUpEvent>(cx, |jira, host, _, _, cx| jira.keep_width(host, cx)),
            )
            .child(
                div()
                    .w(px(SIDEBAR))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(t.border)
                    .child(sidebar),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(LIST_MIN))
                    .h_full()
                    .overflow_hidden()
                    .child(middle),
            )
            .when_some(detail, |columns, detail| {
                columns
                    .child(crate::panes::handle("jira-detail-edge", DetailEdge, true))
                    .child(
                        div()
                            .map(|column| match width {
                                Some(width) => {
                                    column.w(px(width)).flex_shrink(1.).min_w(px(DETAIL_MIN))
                                }
                                None => column.flex_1().min_w(px(DETAIL_MIN)).max_w(px(560.)),
                            })
                            .overflow_hidden()
                            .h_full()
                            .border_l_1()
                            .border_color(t.border)
                            .child(detail),
                    )
            })
            .into_any_element()
    }

    fn show_alone(&mut self, at: &Value, _: &mut Host, _: &mut Context<PluginPane>) -> bool {
        let (Some(site), Some(key)) = (at["site"].as_str(), at["key"].as_str()) else {
            return false;
        };
        self.site = site.to_owned();
        self.issue = Some(OpenIssue::new(key.to_owned()));
        self.alone = true;
        true
    }

    fn alone_title(&self) -> Option<String> {
        self.open_key().map(str::to_owned)
    }
}

const SIDEBAR: f32 = 212.;
const LIST_MIN: f32 = 260.;
const DETAIL_MIN: f32 = 360.;

/// What a drag of the edge between the list and the open issue carries.
#[derive(Clone)]
struct DetailEdge;

/// The value a view's `site` takes: none for the plugin's default site.
fn site_of(site: &str) -> Option<String> {
    (!site.is_empty()).then(|| site.to_owned())
}

/// A one-line field, made the first time the pane draws it.
fn field(
    slot: &mut Option<Entity<InputState>>,
    typing: &mut Vec<Subscription>,
    placeholder: &'static str,
    masked: bool,
    (changed, entered): (Option<Typed>, Option<Typed>),
    window: &mut Window,
    cx: &mut Context<PluginPane>,
) -> Entity<InputState> {
    if let Some(input) = slot {
        return input.clone();
    }
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(masked)
    });
    typing.push(
        cx.subscribe(&input, move |pane, input, event: &InputEvent, cx| {
            let typed = match event {
                InputEvent::Change => changed,
                InputEvent::PressEnter { .. } => entered,
                _ => None,
            };
            let text = input.read(cx).value().trim().to_owned();
            if let (Some(typed), Some((jira, host))) = (typed, pane.parts::<Jira>()) {
                typed(jira, host, &text, cx);
            }
            cx.notify();
        }),
    );
    *slot = Some(input.clone());
    input
}

fn words(input: &Option<Entity<InputState>>, cx: &App) -> String {
    input
        .as_ref()
        .map(|input| input.read(cx).value().trim().to_lowercase())
        .unwrap_or_default()
}

impl Jira {
    fn show(&mut self, list: List) {
        if self.listing.list != list {
            self.listing = Listing {
                list,
                ..Listing::default()
            };
        }
    }

    fn open(&mut self, key: Option<String>) {
        if key.as_deref() != self.open_key() {
            self.issue = key.map(OpenIssue::new);
        }
    }

    fn open_key(&self) -> Option<&str> {
        self.issue.as_ref().map(|issue| issue.key.as_str())
    }

    /// Runs a write; the plugin's views are read again after it.
    fn write(
        &mut self,
        host: &Host,
        doing: &'static str,
        method: &str,
        params: impl Serialize,
        cx: &mut Context<PluginPane>,
    ) {
        if let Some(issue) = &mut self.issue {
            issue.busy = true;
        }
        let call = host.call(method, params);
        after::<Jira, _>(
            call,
            move |jira, host, done, cx| {
                if let Some(issue) = &mut jira.issue {
                    issue.busy = false;
                }
                if let Err(failure) = done {
                    host.said(&failure.titled(doing), cx);
                }
            },
            cx,
        );
        cx.notify();
    }

    /// Saves the open issue's width once its edge is let go, as every pane's.
    fn keep_width(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        if let Some(width) = self.detail_width {
            host.update_settings(
                |settings: &mut Settings| settings.detail_width = Some(width.round()),
                cx,
            );
        }
    }

    fn markdown(&mut self, text: &str) -> Rc<Markdown> {
        let drawn = || Rc::new(crate::editor::preview::render(text));
        match &mut self.issue {
            Some(issue) => issue
                .docs
                .entry(text.to_owned())
                .or_insert_with(drawn)
                .clone(),
            None => drawn(),
        }
    }
}
