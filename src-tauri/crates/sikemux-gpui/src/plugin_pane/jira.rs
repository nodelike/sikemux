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
use serde_json::{json, Value};
use sikemux_wire::jira::{
    Activity, BoardPage, Column, IssueList, IssueListKind, IssueSummary, IssueView, Parent, Person,
    Spaces,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::toast;
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_1, RADIUS_2};
use crate::ui::{self, Button, IconButton, Segmented, Toggle, Variant};
use crate::workbench::Workbench;

const API_TOKENS_PAGE: &str = "https://id.atlassian.com/manage-profile/security/api-tokens";
const SPACES_SHOWN: usize = 8;
const FACES: usize = 6;

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

#[derive(Default)]
pub struct Jira {
    list: List,
    issue: Option<String>,
    /// The site picked; empty for the first the plugin is signed in to.
    site: String,
    /// The site the pane last drew.
    shown_site: String,
    search: Option<Entity<InputState>>,
    space_find: Option<Entity<InputState>>,
    card_find: Option<Entity<InputState>>,
    assign_find: Option<Entity<InputState>>,
    draft: Option<Entity<TextareaState>>,
    form: Option<TokenForm>,
    typing: Vec<Subscription>,
    /// The statuses a list is narrowed to; none shows them all.
    statuses: BTreeSet<String>,
    /// The spaces opened or closed against the one open by default.
    toggled: HashSet<String>,
    every_space: bool,
    /// The people a board is narrowed to by account, with `me` and `none` for nobody.
    picked: BTreeSet<String>,
    every_face: bool,
    folded: HashSet<String>,
    /// Cards moved here, by key, to the column they were moved to, until the board comes again.
    moves: HashMap<String, usize>,
    last_board: Option<Rc<BoardPage>>,
    busy: bool,
    assigning: bool,
    /// The people the open issue can be given to, as the search found them.
    people: Vec<Person>,
    shown: Shown,
    commenting: bool,
    /// Each text drawn as markdown, by its words, for the issue open.
    docs: HashMap<String, Rc<Markdown>>,
    /// How wide the open issue is while its edge is dragged and after.
    detail_width: Option<f32>,
}

impl Panel for Jira {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let status = match host.status(cx).read::<Value>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let sites: Vec<String> = status["sites"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|site| site["host"].as_str().map(str::to_owned))
            .collect();
        if host.signed_in(cx) == Some(false) || sites.is_empty() {
            return self.sign_in(&status, window, cx);
        }
        self.form = None;
        let site = if sites.contains(&self.site) {
            self.site.clone()
        } else {
            sites[0].clone()
        };
        self.shown_site = site.clone();
        let sidebar = self.sidebar(host, &sites, &site, window, cx);
        let middle = match &self.list {
            List::Board { id, .. } => self.board(host, *id, &site, window, cx),
            _ => self.issues(host, &site, cx),
        };
        let board = matches!(self.list, List::Board { .. });
        let detail = match self.issue.clone() {
            Some(key) => Some(self.detail(host, &key, &site, window, cx)),
            None if !board => Some(super::empty(
                "Pick an issue to read it, comment, move it along or assign it.",
                cx,
            )),
            None => None,
        };
        let saved = host.settings(cx)["detailWidth"]
            .as_f64()
            .map(|width| width as f32);
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
            .child(div().flex_1().min_w_0().h_full().child(middle))
            .when_some(detail, |columns, detail| {
                columns
                    .child(crate::panes::handle("jira-detail-edge", DetailEdge, true))
                    .child(
                        div()
                            .map(|column| match width {
                                Some(width) => column.w(px(width)).flex_none(),
                                None => column.flex_1().min_w(px(DETAIL_MIN)).max_w(px(560.)),
                            })
                            .h_full()
                            .border_l_1()
                            .border_color(t.border)
                            .child(detail),
                    )
            })
            .into_any_element()
    }
}

const SIDEBAR: f32 = 212.;
const LIST_MIN: f32 = 260.;
const DETAIL_MIN: f32 = 360.;

/// What a drag of the edge between the list and the open issue carries.
#[derive(Clone)]
struct DetailEdge;

/// The value a view's `site` takes: none for the plugin's default site.
fn site_of(site: &str) -> Value {
    match site {
        "" => Value::Null,
        site => json!(site),
    }
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
        if self.list != list {
            self.statuses.clear();
            self.picked.clear();
            self.moves.clear();
            self.last_board = None;
            self.every_face = false;
            self.folded.clear();
        }
        self.list = list;
    }

    fn open(&mut self, key: Option<String>) {
        if key != self.issue {
            self.people.clear();
            self.assigning = false;
            self.assign_find = None;
            self.docs.clear();
            self.busy = false;
        }
        self.issue = key;
    }

    /// Runs a write; the plugin's views are read again after it.
    fn write(
        &mut self,
        host: &Host,
        doing: &'static str,
        method: &str,
        params: Value,
        cx: &mut Context<PluginPane>,
    ) {
        self.busy = true;
        let call = host.call(method, params);
        after::<Jira, _>(
            call,
            move |jira, host, done, cx| {
                jira.busy = false;
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
            host.save_settings(
                |kept| {
                    kept.insert("detailWidth".into(), json!(width.round()));
                },
                cx,
            );
        }
    }

    fn markdown(&mut self, text: &str) -> Rc<Markdown> {
        self.docs
            .entry(text.to_owned())
            .or_insert_with(|| Rc::new(crate::editor::preview::render(text)))
            .clone()
    }

    fn sign_in(
        &mut self,
        status: &Value,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if self.form.is_none() {
            let mut made = Vec::new();
            for (placeholder, masked) in [
                ("your-team.atlassian.net", false),
                ("you@example.com", false),
                ("ATATT…", true),
            ] {
                let mut slot = None;
                made.push(field(
                    &mut slot,
                    &mut self.typing,
                    placeholder,
                    masked,
                    (None, None),
                    window,
                    cx,
                ));
            }
            let [site, email, token] = <[_; 3]>::try_from(made).expect("three fields");
            self.form = Some(TokenForm {
                site,
                email,
                token,
                busy: false,
            });
        }
        let form = self.form.as_ref().expect("made just now");
        let filled = [&form.site, &form.email, &form.token]
            .iter()
            .all(|input| !input.read(cx).value().trim().is_empty());
        let labelled = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(label, cx))
                .child(ui::Field::new(input))
        };
        let browser = status["browserSignIn"] == Value::Bool(true);
        let message = status["message"].as_str().map(str::to_owned);
        div()
            .id("jira-sign-in")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(380.))
                    .py(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(ui::icon(Icon::Jira, 26.).text_color(jira_blue()))
                    .child(
                        ui::text("Sign in to Jira", 15., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    )
                    .child(
                        ui::text(
                            "The tickets you are working on, beside the code, and tools for your agents to read and update them.",
                            12.5,
                            FontWeight::NORMAL,
                        )
                        .text_color(t.text_secondary),
                    )
                    .children(message.map(|message| {
                        ui::text(message, 12., FontWeight::NORMAL).text_color(t.danger)
                    }))
                    .when(browser, |page| {
                        page.child(
                            Button::new("jira-browser", "Continue with Atlassian", Variant::Primary)
                                .icon(Icon::Jira)
                                .on_click(listen::<Jira, _>(cx, |_, host, _, _, cx| {
                                    host.sign_in_with_browser(cx)
                                })),
                        )
                        .child(ui::meta("Or use an API token", cx).mt(px(8.)))
                    })
                    .child(labelled("Jira site", &form.site))
                    .child(labelled("Atlassian account email", &form.email))
                    .child(labelled("API token", &form.token))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Button::new(
                                    "jira-token-sign-in",
                                    if form.busy { "Signing in…" } else { "Sign in" },
                                    if browser {
                                        Variant::Outline
                                    } else {
                                        Variant::Primary
                                    },
                                )
                                .disabled(!filled || form.busy)
                                .on_click(listen::<Jira, _>(cx, |jira, host, _, _, cx| {
                                    jira.sign_in_with_token(host, cx)
                                })),
                            )
                            .child(
                                Button::new("jira-make-token", "Create an API token", Variant::Outline)
                                    .icon(Icon::Globe)
                                    .on_click(|_, _, cx| cx.open_url(API_TOKENS_PAGE)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn sign_in_with_token(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let params = json!({
            "site": value(&form.site),
            "email": value(&form.email),
            "token": value(&form.token),
        });
        form.busy = true;
        let call = host.call("signIn", params);
        after::<Jira, _>(
            call,
            |jira, host, done, cx| {
                if let Some(form) = &mut jira.form {
                    form.busy = false;
                }
                if let Err(failure) = done {
                    host.said(&failure.titled("Sign in to Jira"), cx);
                }
            },
            cx,
        );
        cx.notify();
    }

    fn sidebar(
        &mut self,
        host: &mut Host,
        sites: &[String],
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let head = if sites.len() > 1 {
            let picks = sites
                .iter()
                .map(|host| ui::Pick::new(host.clone(), host.clone()))
                .collect();
            ui::select(
                "jira-site",
                site.to_owned(),
                picks,
                Some(site.to_owned().into()),
                false,
                listen::<Jira, SharedString>(cx, |jira, _, picked, _, cx| {
                    jira.site = picked.to_string();
                    jira.open(None);
                    if matches!(jira.list, List::Filter { .. }) {
                        jira.show(List::Mine);
                    }
                    cx.notify();
                }),
                cx,
            )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui::icon(Icon::Jira, 16.).text_color(jira_blue()))
                .child(
                    ui::text(
                        site.trim_end_matches(".atlassian.net").to_owned(),
                        13.,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary)
                    .truncate(),
                )
                .into_any_element()
        };
        let search = field(
            &mut self.search,
            &mut self.typing,
            "Search",
            false,
            (
                None,
                Some(|jira, _, text, _| {
                    if !text.is_empty() {
                        jira.show(List::Search(text.to_owned()));
                    }
                }),
            ),
            window,
            cx,
        );
        let mut lists = div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(heading("Your work", cx));
        for (list, label, icon) in my_work() {
            lists = lists.child(self.list_row(list, label, icon, cx));
        }
        let spaces = host
            .view("spacesView", json!({ "site": site_of(site) }), cx)
            .read::<Spaces>()
            .ok()
            .flatten();
        let spaces_part = spaces
            .as_ref()
            .map(|spaces| self.spaces(spaces, window, cx));
        let filters = spaces.as_ref().map(|spaces| {
            let mut part = div().flex().flex_col().gap(px(1.));
            if !spaces.filters.is_empty() {
                part = part.child(heading("Starred filters", cx));
            }
            for filter in &spaces.filters {
                let list = List::Filter {
                    id: filter.id.clone(),
                    name: filter.name.clone(),
                    jql: filter.jql.clone(),
                };
                part = part.child(self.list_row(list, &filter.name, Icon::Star, cx));
            }
            part
        });
        let site_out = site.to_owned();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(head)
                    .child(ui::Field::new(&search).icon(Icon::Search)),
            )
            .child(
                div()
                    .id("jira-lists")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(lists)
                    .children(spaces_part)
                    .children(filters),
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        ui::Row::new("jira-sign-out", "Sign out")
                            .quiet()
                            .mark(22., ui::icon(Icon::LogOut, 14.).text_color(t.text_tertiary))
                            .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                                let params = json!({ "site": site_out });
                                jira.write(host, "Sign out of Jira", "signOut", params, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn list_row(
        &self,
        list: List,
        label: &str,
        icon: Icon,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let id = SharedString::from(format!("jira-list-{}", list.id()));
        ui::Row::new(id, label.to_owned())
            .chosen(self.list == list)
            .mark(22., ui::icon(icon, 14.).text_color(t.text_tertiary))
            .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                jira.show(list.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn spaces(
        &mut self,
        listed: &Spaces,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if listed.spaces.is_empty()
            && listed.other_boards.is_empty()
            && listed.boards_failed.is_none()
        {
            return div().into_any_element();
        }
        let active = match &self.list {
            List::Project { key, .. } | List::Unassigned { key, .. } => Some(key.clone()),
            List::Board { id, .. } => listed
                .spaces
                .iter()
                .find(|space| space.boards.iter().any(|board| board.id == *id))
                .map(|space| space.project.key.clone()),
            _ => None,
        };
        let open_by_default = active.clone().or_else(|| {
            listed
                .spaces
                .iter()
                .find(|space| !space.boards.is_empty())
                .or(listed.spaces.first())
                .map(|space| space.project.key.clone())
        });
        let find = (listed.spaces.len() > SPACES_SHOWN).then(|| {
            field(
                &mut self.space_find,
                &mut self.typing,
                "Find a space or board",
                false,
                (None, None),
                window,
                cx,
            )
        });
        let wanted = words(&self.space_find, cx);
        let matching: Vec<_> = listed
            .spaces
            .iter()
            .filter(|space| {
                wanted.is_empty()
                    || space.project.name.to_lowercase().contains(&wanted)
                    || space.project.key.to_lowercase().contains(&wanted)
                    || space
                        .boards
                        .iter()
                        .any(|board| board.name.to_lowercase().contains(&wanted))
            })
            .collect();
        let shown: Vec<_> = matching
            .iter()
            .enumerate()
            .filter(|(index, space)| {
                !wanted.is_empty()
                    || self.every_space
                    || *index < SPACES_SHOWN
                    || Some(&space.project.key) == active.as_ref()
            })
            .map(|(_, space)| *space)
            .collect();
        let mut part = div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(heading("Spaces", cx))
            .children(find.map(|find| div().px(px(2.)).pb(px(4.)).child(ui::Field::new(&find))));
        for space in &shown {
            let key = space.project.key.clone();
            let open = !wanted.is_empty()
                || ((Some(&key) == open_by_default.as_ref()) != self.toggled.contains(&key));
            let flip = key.clone();
            part = part.child(
                ui::Row::new(
                    SharedString::from(format!("jira-space-{key}")),
                    space.project.name.clone(),
                )
                .mark(22., ui::initials(&space.project.name, &key, 16., cx))
                .status(Some(chevron(open, cx)))
                .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                    if !jira.toggled.remove(&flip) {
                        jira.toggled.insert(flip.clone());
                    }
                    cx.notify();
                })),
            );
            if !open {
                continue;
            }
            let mut children = div().pl(px(14.)).flex().flex_col().gap(px(1.));
            for board in &space.boards {
                let list = List::Board {
                    id: board.id,
                    name: board.name.clone(),
                };
                children = children.child(self.list_row(list, &board.name, Icon::Board, cx));
            }
            let name = space.project.name.clone();
            let all = List::Project {
                key: key.clone(),
                name: name.clone(),
            };
            children = children
                .child(self.list_row(all, "All open", Icon::Stack, cx))
                .child(self.list_row(
                    List::Unassigned { key, name },
                    "Unassigned",
                    Icon::Inbox,
                    cx,
                ));
            part = part.child(children);
        }
        if wanted.is_empty() && !self.every_space && matching.len() > shown.len() {
            part = part.child(
                ui::Row::new(
                    "jira-every-space",
                    format!("Show all {} spaces", matching.len()),
                )
                .quiet()
                .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                    jira.every_space = true;
                    cx.notify();
                })),
            );
        }
        if !listed.other_boards.is_empty() {
            part = part.child(heading("Other boards", cx));
            for board in &listed.other_boards {
                let list = List::Board {
                    id: board.id,
                    name: board.name.clone(),
                };
                part = part.child(self.list_row(list, &board.name, Icon::Board, cx));
            }
        }
        if let Some(failed) = &listed.boards_failed {
            part = part.child(
                div()
                    .px(px(8.))
                    .text_size(crate::theme::ui_text(11.))
                    .text_color(t.danger)
                    .child(format!("{failed} Sign out and in again to retry.")),
            );
        }
        part.into_any_element()
    }

    fn issues(&mut self, host: &mut Host, site: &str, cx: &mut Context<PluginPane>) -> AnyElement {
        let Some(asked) = self.list.asked() else {
            return div().into_any_element();
        };
        let read = host
            .view(
                "listView",
                json!({ "list": asked, "site": site_of(site) }),
                cx,
            )
            .read::<IssueList>();
        let page = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(page)) if page.issues.is_empty() => return super::empty(self.list.empty(), cx),
            Ok(Some(page)) => page,
        };
        self.statuses
            .retain(|status| page.statuses.iter().any(|each| each.status == *status));
        let t = Tokens::get(cx).clone();
        let chips = (page.statuses.len() > 1).then(|| {
            div()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    Toggle::new(
                        "jira-status-all",
                        format!("All {}", page.issues.len()),
                        self.statuses.is_empty(),
                    )
                    .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                        jira.statuses.clear();
                        cx.notify();
                    })),
                )
                .children(page.statuses.iter().enumerate().map(|(index, count)| {
                    let status = count.status.clone();
                    Toggle::new(
                        ("jira-status", index),
                        format!("{} {}", count.status, count.count),
                        self.statuses.contains(&count.status),
                    )
                    .on_click(listen::<Jira, _>(
                        cx,
                        move |jira, _, _, _, cx| {
                            if !jira.statuses.remove(&status) {
                                jira.statuses.insert(status.clone());
                            }
                            cx.notify();
                        },
                    ))
                }))
        });
        let rows: Vec<AnyElement> = page
            .issues
            .iter()
            .filter(|issue| self.statuses.is_empty() || self.statuses.contains(&issue.status))
            .enumerate()
            .map(|(index, issue)| {
                let chosen = self.issue.as_deref() == Some(issue.key.as_str());
                issue_row(host, issue, index, chosen, cx)
            })
            .collect();
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(chips)
            .child(
                div()
                    .id("jira-issues")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn board(
        &mut self,
        host: &mut Host,
        id: u64,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let people: Vec<&String> = self
            .picked
            .iter()
            .filter(|who| *who != "me" && *who != "none")
            .collect();
        let params = json!({
            "id": id,
            "site": site_of(site),
            "people": people,
            "mine": self.picked.contains("me"),
            "unassigned": self.picked.contains("none"),
        });
        let page = match host.view("boardView", params, cx).read::<BoardPage>() {
            Ok(Some(page)) => {
                if !self
                    .last_board
                    .as_ref()
                    .is_some_and(|last| Rc::ptr_eq(last, &page))
                {
                    self.moves.clear();
                    self.last_board = Some(page.clone());
                }
                page
            }
            Ok(None) => match &self.last_board {
                Some(last) => last.clone(),
                None => return super::loading(cx),
            },
            Err(failure) => return super::failure(&failure, cx),
        };
        let board = &page.board;
        let find = field(
            &mut self.card_find,
            &mut self.typing,
            "Find a card",
            false,
            (None, None),
            window,
            cx,
        );
        let wanted = words(&self.card_find, cx);
        let hidden = if self.every_face {
            0
        } else {
            page.people.len().saturating_sub(FACES)
        };
        let faces: Vec<_> = page
            .people
            .iter()
            .enumerate()
            .filter(|(index, person)| {
                *index < page.people.len() - hidden || self.picked.contains(&person.account_id)
            })
            .map(|(index, person)| {
                let who = person.account_id.clone();
                let picked = self.picked.contains(&who);
                div()
                    .id(("jira-face", index))
                    .rounded_full()
                    .border_2()
                    .border_color(if picked {
                        t.accent
                    } else {
                        Hsla::transparent_black()
                    })
                    .cursor_pointer()
                    .tooltip(ui::tooltip(format!("{} · {}", person.name, person.count)))
                    .child(host.face(&person.name, person.avatar_url.as_deref(), 22., cx))
                    .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                        if !jira.picked.remove(&who) {
                            jira.picked.insert(who.clone());
                        }
                        cx.notify();
                    }))
            })
            .collect();
        let narrowed = !self.picked.is_empty() || !wanted.is_empty();
        let filters = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .w(px(200.))
                    .child(ui::Field::new(&find).icon(Icon::Search)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .children(faces)
                    .when(hidden > 0, |faces| {
                        faces.child(
                            Button::new("jira-more-faces", format!("+{hidden}"), Variant::Outline)
                                .tooltip(format!("Show {hidden} more"))
                                .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                                    jira.every_face = true;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                Toggle::new(
                    "jira-only-mine",
                    "Only my issues",
                    self.picked.contains("me"),
                )
                .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                    if !jira.picked.remove("me") {
                        jira.picked.insert("me".into());
                    }
                    cx.notify();
                })),
            )
            .when(page.unassigned > 0, |filters| {
                filters.child(
                    Toggle::new(
                        "jira-nobody",
                        format!("Unassigned {}", page.unassigned),
                        self.picked.contains("none"),
                    )
                    .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                        if !jira.picked.remove("none") {
                            jira.picked.insert("none".into());
                        }
                        cx.notify();
                    })),
                )
            })
            .when(narrowed, |filters| {
                filters.child(
                    Button::new("jira-clear", "Clear", Variant::Outline).on_click(
                        listen::<Jira, _>(cx, |jira, _, _, window, cx| {
                            jira.picked.clear();
                            if let Some(find) = &jira.card_find {
                                find.update(cx, |find, cx| find.set_value("", window, cx));
                            }
                            cx.notify();
                        }),
                    ),
                )
            });
        let sprint = board.sprint.as_ref().map(|sprint| {
            let mut said = sprint.name.clone();
            if let Some(left) = sprint_left(sprint.end.as_deref()) {
                said.push_str(&format!(" · {left}"));
            }
            ui::meta(said, cx)
        });
        let goal = board
            .sprint
            .as_ref()
            .and_then(|sprint| sprint.goal.clone())
            .map(|goal| ui::text(goal, 12., FontWeight::NORMAL).text_color(t.text_secondary));
        let head = div()
            .px(px(14.))
            .pt(px(12.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        ui::text(board.name.clone(), 15., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary)
                            .truncate(),
                    )
                    .child(tag(&board.kind, cx))
                    .children(sprint)
                    .child(div().flex_1())
                    .child(
                        IconButton::new("jira-board-refresh", Icon::Refresh, "Refresh the board")
                            .small()
                            .on_click(listen::<Jira, _>(cx, |_, host, _, _, _| {
                                host.read_again(Some("boardView"))
                            })),
                    ),
            )
            .children(goal)
            .when(board.truncated, |head| {
                head.child(ui::meta(
                    "Showing the first 300 issues. Pick people or narrow the board in Jira to see the rest.",
                    cx,
                ))
            })
            .child(filters);
        if board.kind == "scrum" && board.sprint.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(head)
                .child(super::empty("No sprint is running on this board.", cx))
                .into_any_element();
        }
        let shape = Shape {
            columns: &board.columns,
            wanted: &wanted,
            site,
        };
        let body = if page.lanes.is_empty() {
            self.columns(host, &shape, &board.columns, "all", cx)
                .into_any_element()
        } else {
            let mut lanes = div().flex().flex_col().gap(px(10.));
            for lane in &page.lanes {
                let id = lane.id.clone();
                let folded = self.folded.contains(&id);
                let name = lane
                    .person
                    .as_ref()
                    .map_or("Unassigned".to_owned(), |person| person.name.clone());
                let items = match lane.count {
                    1 => "1 work item".to_owned(),
                    count => format!("{count} work items"),
                };
                let avatar = lane
                    .person
                    .as_ref()
                    .and_then(|person| person.avatar_url.as_deref());
                lanes = lanes.child(
                    div()
                        .id(SharedString::from(format!("jira-lane-{id}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .child(chevron(!folded, cx))
                        .child(host.face(&name, avatar, 20., cx))
                        .child(
                            ui::text(name.clone(), 12.5, FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        )
                        .child(ui::meta(format!("({items})"), cx))
                        .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                            if !jira.folded.remove(&id) {
                                jira.folded.insert(id.clone());
                            }
                            cx.notify();
                        })),
                );
                if !folded {
                    lanes = lanes.child(self.columns(host, &shape, &lane.columns, &lane.id, cx));
                }
            }
            lanes.into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("jira-board")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .p(px(12.))
                    .child(body),
            )
            .into_any_element()
    }

    /// A lane's columns with their cards, each card where it was moved to here.
    fn columns(
        &self,
        host: &Host,
        shape: &Shape,
        columns: &[Column],
        lane: &str,
        cx: &mut Context<PluginPane>,
    ) -> Div {
        let t = Tokens::get(cx).clone();
        let mut row = div().flex().gap(px(10.));
        for (at, (column, cards)) in columns
            .iter()
            .zip(with_moves(columns, &self.moves))
            .enumerate()
        {
            let cards: Vec<&IssueSummary> = cards
                .into_iter()
                .filter(|issue| {
                    shape.wanted.is_empty()
                        || issue.key.to_lowercase().contains(shape.wanted)
                        || issue.summary.to_lowercase().contains(shape.wanted)
                })
                .collect();
            let mut list = div()
                .w(px(250.))
                .flex_none()
                .p(px(6.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(RADIUS_2)
                .bg(t.surface_sunken)
                .child(
                    div()
                        .px(px(4.))
                        .py(px(2.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(ui::caps(&column.name, cx))
                        .child(ui::meta(cards.len().to_string(), cx)),
                );
            for issue in cards {
                list = list.child(self.card(host, shape, issue, at, lane, cx));
            }
            row = row.child(list);
        }
        row
    }

    fn card(
        &self,
        host: &Host,
        shape: &Shape,
        issue: &IssueSummary,
        at: usize,
        lane: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let key = issue.key.clone();
        let chosen = self.issue.as_deref() == Some(issue.key.as_str());
        let moving = self.moves.contains_key(&issue.key);
        let pane = cx.weak_entity();
        let targets: Vec<(String, Vec<String>)> = shape
            .columns
            .iter()
            .map(|column| (column.name.clone(), column.status_ids.clone()))
            .collect();
        let (menu_key, site) = (issue.key.clone(), shape.site.to_owned());
        let chips = (issue.parent.is_some() || issue.due.is_some()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(issue.parent.as_ref().map(|parent| epic_chip(parent, cx)))
                .children(
                    issue
                        .due
                        .as_ref()
                        .map(|due| due_chip(due, issue.status_category == "done", cx)),
                )
        });
        let who = issue
            .assignee
            .as_ref()
            .map_or("Unassigned", |person| person.name.as_str());
        let avatar = issue
            .assignee
            .as_ref()
            .and_then(|person| person.avatar_url.as_deref());
        div()
            .id(SharedString::from(format!(
                "jira-card-{lane}-{}",
                issue.key
            )))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(if chosen { t.border_selected } else { t.border })
            .bg(if chosen {
                t.surface_chosen
            } else {
                t.surface_raised
            })
            .when(moving, |card| card.opacity(0.6))
            .cursor_pointer()
            .hover(move |card| card.border_color(t.border_strong))
            .child(
                ui::text(issue.summary.clone(), 12.5, FontWeight::NORMAL)
                    .text_color(t.text_primary),
            )
            .children(chips)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(type_mark(issue.issue_type.as_deref(), cx))
                    .child(ui::mono(issue.key.clone(), 11., cx).text_color(t.text_secondary))
                    .children(
                        issue
                            .priority
                            .as_deref()
                            .map(|priority| priority_mark(priority, cx)),
                    )
                    .child(div().flex_1())
                    .child(host.face(who, avatar, 20., cx)),
            )
            .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                let open = (jira.issue.as_deref() != Some(key.as_str())).then(|| key.clone());
                jira.open(open);
                cx.notify();
            }))
            .context_menu(move |popup, _, _| {
                let mut popup = popup;
                for (to, (name, status_ids)) in targets.iter().enumerate() {
                    let (pane, key, site) = (pane.clone(), menu_key.clone(), site.clone());
                    let (column, status_ids) = (name.clone(), status_ids.clone());
                    popup = popup.item(
                        PopupMenuItem::new(format!("Move to {name}"))
                            .icon(Icon::Move)
                            .disabled(to == at)
                            .on_click(move |_, _, cx| {
                                let moved = Moved {
                                    key: &key,
                                    to,
                                    column: &column,
                                    status_ids: &status_ids,
                                    site: &site,
                                };
                                move_card(&pane, moved, cx)
                            }),
                    );
                }
                popup
            })
            .into_any_element()
    }

    fn detail(
        &mut self,
        host: &mut Host,
        key: &str,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view(
                "issueView",
                json!({ "key": key, "site": site_of(site) }),
                cx,
            )
            .read::<IssueView>();
        let view = match read {
            Ok(Some(view)) => view,
            Ok(None) => return super::empty(format!("Loading {key}"), cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let t = Tokens::get(cx).clone();
        let issue = &view.issue;
        let url = issue.url.clone();
        let (open_url, copy_url, key_url) = (url.clone(), url.clone(), url);
        let sent = view.clone();
        let head = div()
            .id("jira-detail-head")
            .px(px(16.))
            .pt(px(12.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(type_mark(issue.issue_type.as_deref(), cx))
                    .child(
                        div()
                            .id("jira-detail-key")
                            .cursor_pointer()
                            .tooltip(ui::tooltip("Open in browser"))
                            .child(ui::mono(issue.key.clone(), 12., cx).text_color(jira_blue()))
                            .on_click(move |_, _, cx| cx.open_url(&key_url)),
                    )
                    .child(
                        IconButton::new("jira-copy-link", Icon::Copy, "Copy link")
                            .small()
                            .on_click(listen::<Jira, _>(cx, move |_, host, _, _, cx| {
                                host.copy(copy_url.clone(), "the link", cx)
                            })),
                    )
                    .child(
                        IconButton::new("jira-open", Icon::Globe, "Open in browser")
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(&open_url)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("jira-send", "Send to agent board", Variant::Outline)
                            .icon(Icon::Agent)
                            .tooltip(
                                "Put this issue on the open project's board as a card for agents",
                            )
                            .on_click(listen::<Jira, _>(cx, move |_, host, _, _, cx| {
                                send_to_board(host.bench(), &sent, cx)
                            })),
                    )
                    .child(
                        IconButton::new("jira-close", Icon::Close, "Close the issue")
                            .small()
                            .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                                jira.open(None);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                ui::text(issue.summary.clone(), 15., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary),
            )
            .context_menu(issue_menu(issue, host.bench().clone()));
        let mut style = crate::editor::preview::reading_style(13., &t);
        let pane = cx.weak_entity();
        let (task_key, task_site) = (issue.key.clone(), site.to_owned());
        style.on_task = Some(Rc::new(move |index, text, done, _, cx| {
            let params = json!({
                "key": task_key,
                "index": index,
                "text": text,
                "done": done,
                "site": site_of(&task_site),
            });
            let _ = pane.update(cx, |pane, cx| {
                if let Some((jira, host)) = pane.parts::<Jira>() {
                    jira.write(host, "Update the task", "setTask", params, cx);
                }
            });
        }));
        let pen = Pen::new(Rc::new(style));
        let actions = self.actions(&view, site, window, cx);
        let fields = self.fields(host, &view, cx);
        let description = self.description(&pen, &view, window, cx);
        let commits = commits(host, &issue.key, cx);
        let activity = self.activity(host, &pen, &view, site, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("jira-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(actions)
                    .child(fields)
                    .child(description)
                    .children(commits)
                    .child(activity),
            )
            .into_any_element()
    }

    fn actions(
        &mut self,
        view: &IssueView,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let issue = &view.issue;
        let busy = self.busy;
        let picks = view
            .transitions
            .iter()
            .map(|transition| {
                let pick = ui::Pick::new(transition.id.clone(), transition.to.clone());
                if transition.name.eq_ignore_ascii_case(&transition.to) {
                    pick
                } else {
                    pick.hint(transition.name.clone())
                }
            })
            .collect();
        let at = |key: &str| (key.to_owned(), site.to_owned());
        let (move_key, move_site) = at(&issue.key);
        let status = ui::select(
            "jira-transition",
            format!("Status: {}", issue.status),
            picks,
            None,
            busy || view.transitions.is_empty(),
            listen::<Jira, SharedString>(cx, move |jira, host, to, _, cx| {
                let params = json!({
                    "key": move_key,
                    "to": to.to_string(),
                    "site": site_of(&move_site),
                });
                jira.write(host, "Move issue", "transition", params, cx);
            }),
            cx,
        );
        let (me_key, me_site) = at(&issue.key);
        let (none_key, none_site) = at(&issue.key);
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(status)
            .child(
                Button::new("jira-assign-me", "Assign to me", Variant::Outline)
                    .icon(Icon::User)
                    .disabled(busy)
                    .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                        let params =
                            json!({ "key": me_key, "me": true, "site": site_of(&me_site) });
                        jira.write(host, "Assign", "assign", params, cx);
                    })),
            )
            .child(
                Button::new("jira-unassign", "Unassign", Variant::Outline)
                    .icon(Icon::Minus)
                    .disabled(busy || issue.assignee.is_none())
                    .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                        let params = json!({
                            "key": none_key,
                            "accountId": null,
                            "site": site_of(&none_site),
                        });
                        jira.write(host, "Unassign", "assign", params, cx);
                    })),
            )
            .child(
                Button::new("jira-assign", "Assign…", Variant::Outline)
                    .icon(Icon::Search)
                    .pressed(self.assigning)
                    .disabled(busy)
                    .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                        jira.assigning = !jira.assigning;
                        jira.people.clear();
                        cx.notify();
                    })),
            );
        let finder =
            self.assigning.then(|| {
                let find = field(
                    &mut self.assign_find,
                    &mut self.typing,
                    "Find a person",
                    false,
                    (Some(Jira::find_people), None),
                    window,
                    cx,
                );
                let chips =
                    self.people.iter().enumerate().map(|(index, person)| {
                        let (key, site) = at(&issue.key);
                        let account = person.account_id.clone();
                        Button::new(("jira-person", index), person.name.clone(), Variant::Outline)
                    .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                        jira.assigning = false;
                        let params =
                            json!({ "key": key, "accountId": account, "site": site_of(&site) });
                        jira.write(host, "Assign", "assign", params, cx);
                    }))
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(ui::Field::new(&find).icon(Icon::Search))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(chips))
            });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(row)
            .children(finder)
            .into_any_element()
    }

    fn find_people(&mut self, host: &mut Host, query: &str, cx: &mut Context<PluginPane>) {
        let Some(key) = self.issue.clone() else {
            return;
        };
        let params = json!({ "key": key, "query": query, "site": site_of(&self.shown_site) });
        let call = host.call("assignable", params);
        let asked = query.to_owned();
        after::<Jira, _>(
            call,
            move |jira, _, found, _| {
                let still = jira
                    .assign_find
                    .as_ref()
                    .is_some_and(|_| jira.issue.as_deref() == Some(key.as_str()));
                if let (true, Ok(found)) = (still, found) {
                    jira.people = serde_json::from_value(found).unwrap_or_default();
                }
                let _ = asked;
            },
            cx,
        );
    }

    fn fields(&self, host: &Host, view: &IssueView, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let issue = &view.issue;
        let fact = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_h(px(24.))
                .child(div().w(px(90.)).flex_none().child(ui::meta(label, cx)))
                .child(div().flex_1().min_w_0().child(value))
        };
        let person = |who: Option<&Person>| {
            let name = who.map_or("Unassigned", |person| person.name.as_str());
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(crate::theme::ui_text(12.5))
                .text_color(t.text_primary)
                .child(host.face(
                    name,
                    who.and_then(|person| person.avatar_url.as_deref()),
                    20.,
                    cx,
                ))
                .child(name.to_owned())
                .into_any_element()
        };
        let plain = |text: String| {
            ui::text(text, 12.5, FontWeight::NORMAL)
                .text_color(t.text_primary)
                .into_any_element()
        };
        let mut facts = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(ui::caps("Details", cx))
            .child(fact("Assignee", person(issue.assignee.as_ref())));
        if let Some(reporter) = &view.reporter {
            facts = facts.child(fact("Reporter", person(Some(reporter))));
        }
        if let Some(priority) = &issue.priority {
            facts = facts.child(fact(
                "Priority",
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(priority_mark(priority, cx))
                    .child(plain(priority.clone()))
                    .into_any_element(),
            ));
        }
        if let Some(due) = &issue.due {
            facts = facts.child(fact(
                "Due date",
                due_chip(due, issue.status_category == "done", cx),
            ));
        }
        if let Some(parent) = &issue.parent {
            facts = facts.child(fact("Parent", epic_chip(parent, cx)));
        }
        if let Some(kind) = &issue.issue_type {
            facts = facts.child(fact(
                "Type",
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(type_mark(Some(kind), cx))
                    .child(plain(kind.clone()))
                    .into_any_element(),
            ));
        }
        if let Some(sprint) = &issue.sprint {
            facts = facts.child(fact("Sprint", plain(sprint.clone())));
        }
        if !view.labels.is_empty() {
            facts = facts.child(fact(
                "Labels",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(view.labels.iter().map(|label| tag(label, cx)))
                    .into_any_element(),
            ));
        }
        facts.into_any_element()
    }

    fn description(
        &mut self,
        pen: &Pen,
        view: &IssueView,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = if view.description.trim().is_empty() {
            ui::meta("No description.", cx).into_any_element()
        } else {
            let doc = self.markdown(&view.description);
            pen.markdown("jira-description", &doc, window, cx)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_color(t.text_primary)
            .child(ui::caps("Description", cx))
            .child(body)
            .into_any_element()
    }

    /// The issue's comments and edits, newest first, with a box for a new comment on top.
    fn activity(
        &mut self,
        host: &Host,
        pen: &Pen,
        view: &IssueView,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let chosen = SHOWN
            .iter()
            .position(|shown| *shown == self.shown)
            .unwrap_or_default();
        let switch = Segmented::words(
            "jira-activity",
            vec![
                ("All".into(), None),
                ("Comments".into(), Some(view.comment_count)),
                ("History".into(), None),
            ],
            chosen,
        )
        .on_select(listen::<Jira, usize>(cx, |jira, _, index, _, cx| {
            jira.shown = SHOWN[*index];
            cx.notify();
        }));
        let composer = (self.shown != Shown::History).then(|| {
            let draft = self
                .draft
                .get_or_insert_with(|| {
                    cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .auto_grow(3, 12)
                            .placeholder("Add a comment")
                    })
                })
                .clone();
            let written = !draft.read(cx).value().trim().is_empty();
            let (key, site) = (view.issue.key.clone(), site.to_owned());
            div()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .text_size(crate::theme::ui_text(12.5))
                        .child(
                            Textarea::new(&draft)
                                .appearance(false)
                                .text_size(crate::theme::ui_text(12.5)),
                        ),
                )
                .child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .flex()
                        .justify_end()
                        .border_t_1()
                        .border_color(t.border)
                        .child(
                            Button::new(
                                "jira-comment",
                                if self.commenting {
                                    "Sending…"
                                } else {
                                    "Comment"
                                },
                                Variant::Primary,
                            )
                            .disabled(self.commenting || !written)
                            .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                                jira.comment(host, &key, &site, cx)
                            })),
                        ),
                )
        });
        let entries: Vec<&Activity> = view
            .activity
            .iter()
            .filter(|entry| {
                matches!(
                    (self.shown, entry),
                    (Shown::All, _)
                        | (Shown::Comments, Activity::Comment { .. })
                        | (Shown::History, Activity::Change { .. })
                )
            })
            .collect();
        let shown_comments = view
            .activity
            .iter()
            .filter(|entry| matches!(entry, Activity::Comment { .. }))
            .count() as u64;
        let hidden = view.comment_count.saturating_sub(shown_comments);
        let mut part = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(ui::caps("Activity", cx))
                    .child(div().flex_1())
                    .child(switch),
            )
            .children(composer);
        if entries.is_empty() {
            part = part.child(ui::meta(
                if self.shown == Shown::History {
                    "No changes yet."
                } else {
                    "No comments yet."
                },
                cx,
            ));
        }
        for (index, entry) in entries.into_iter().enumerate() {
            part = part.child(match entry {
                Activity::Comment { comment } => {
                    let doc = self.markdown(&comment.body);
                    let body = pen.markdown(("jira-comment-body", index), &doc, window, cx);
                    said(
                        host,
                        &comment.author,
                        comment.author_avatar_url.as_deref(),
                        &comment.created,
                        body,
                        cx,
                    )
                }
                Activity::Change { change } => {
                    let lines =
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .children(change.fields.iter().map(|field| {
                                let mut line = div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(crate::theme::ui_text(12.))
                                    .text_color(t.text_secondary)
                                    .child("changed")
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text_primary)
                                            .child(field.field.clone()),
                                    );
                                if field.from.is_some() || field.to.is_some() {
                                    line = line
                                        .child(
                                            div()
                                                .line_through()
                                                .text_color(t.text_tertiary)
                                                .child(field.from.clone().unwrap_or("None".into())),
                                        )
                                        .child("→")
                                        .child(
                                            div()
                                                .text_color(t.text_primary)
                                                .child(field.to.clone().unwrap_or("None".into())),
                                        );
                                }
                                line
                            }));
                    said(
                        host,
                        &change.author,
                        change.author_avatar_url.as_deref(),
                        &change.created,
                        lines.into_any_element(),
                        cx,
                    )
                }
            });
        }
        if self.shown != Shown::History && hidden > 0 {
            let comments = if hidden == 1 { "comment" } else { "comments" };
            part = part.child(ui::meta(
                format!(
                    "{hidden} older {comments} not shown; open the issue in Jira to read them."
                ),
                cx,
            ));
        }
        part.into_any_element()
    }

    fn comment(&mut self, host: &Host, key: &str, site: &str, cx: &mut Context<PluginPane>) {
        let Some(draft) = self.draft.clone() else {
            return;
        };
        let body = draft.read(cx).value().trim().to_owned();
        if body.is_empty() || self.commenting {
            return;
        }
        self.commenting = true;
        let call = host.call(
            "comment",
            json!({ "key": key, "body": body, "site": site_of(site) }),
        );
        let pane = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let done = call.await;
            cx.update(|cx| {
                let window = cx.active_window();
                let _ = pane.update(cx, |pane, cx| {
                    let Some((jira, host)) = pane.parts::<Jira>() else {
                        return;
                    };
                    jira.commenting = false;
                    match done {
                        Ok(_) => {
                            if let (Some(draft), Some(window)) = (&jira.draft, window) {
                                let draft = draft.clone();
                                let _ = window.update(cx, |_, window, cx| {
                                    draft.update(cx, |draft, cx| draft.set_value("", window, cx))
                                });
                            }
                        }
                        Err(failure) => host.said(&failure.titled("Comment"), cx),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }
}

/// What a board's columns and cards share: every column, the words a card is found by,
/// and the site.
struct Shape<'a> {
    columns: &'a [Column],
    wanted: &'a str,
    site: &'a str,
}

/// A card moved to another column.
struct Moved<'a> {
    key: &'a str,
    to: usize,
    column: &'a str,
    status_ids: &'a [String],
    site: &'a str,
}

fn heading(label: &str, cx: &App) -> Div {
    div()
        .px(px(8.))
        .pt(px(4.))
        .pb(px(2.))
        .child(ui::caps(label, cx))
}

fn chevron(open: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let chevron = ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary);
    match open {
        true => chevron
            .with_transformation(Transformation::rotate(radians(std::f32::consts::FRAC_PI_2)))
            .into_any_element(),
        false => chevron.into_any_element(),
    }
}

fn tag(text: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(t.surface_active)
        .text_size(crate::theme::ui_text(10.5))
        .text_color(t.text_secondary)
        .child(text.to_owned())
        .into_any_element()
}

/// A status, coloured by the bucket Jira puts it in: to do, in progress or done.
fn status_chip(status: &str, category: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let ink = match category {
        "indeterminate" => jira_blue(),
        "done" => t.live,
        _ => t.text_secondary,
    };
    let ground = match category {
        "indeterminate" | "done" => ink.opacity(0.16),
        _ => t.surface_hover,
    };
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(ground)
        .text_size(crate::theme::ui_text(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .whitespace_nowrap()
        .child(status.to_owned())
}

fn type_mark(kind: Option<&str>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let Some(kind) = kind else {
        return div().into_any_element();
    };
    let ink = match kind.to_lowercase().as_str() {
        "bug" => t.danger,
        "story" => t.live,
        "epic" => t.accent,
        _ => jira_blue(),
    };
    div()
        .id(SharedString::from(format!("jira-type-{kind}")))
        .flex_none()
        .tooltip(ui::tooltip(kind.to_owned()))
        .child(ui::icon(Icon::Issue, 13.).text_color(ink))
        .into_any_element()
}

fn priority_mark(priority: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (icon, ink) = match priority.to_lowercase().as_str() {
        "highest" | "high" | "critical" | "blocker" => (Icon::ArrowUp, t.danger),
        "low" | "lowest" | "minor" | "trivial" => (Icon::ArrowDown, jira_blue()),
        _ => (Icon::Minus, t.warn),
    };
    div()
        .id(SharedString::from(format!("jira-priority-{priority}")))
        .flex_none()
        .tooltip(ui::tooltip(format!("{priority} priority")))
        .child(ui::icon(icon, 12.).text_color(ink))
        .into_any_element()
}

fn epic_chip(parent: &Parent, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .id(SharedString::from(format!("jira-parent-{}", parent.key)))
        .max_w(px(200.))
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(t.accent.opacity(0.14))
        .text_size(crate::theme::ui_text(10.5))
        .text_color(t.text_primary)
        .truncate()
        .tooltip(ui::tooltip(format!("{} {}", parent.key, parent.summary)))
        .child(parent.summary.clone())
        .into_any_element()
}

/// The day an issue is due, red once it has passed and the issue is not done.
fn due_chip(due: &str, done: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let day = chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d").ok();
    let late = !done && day.is_some_and(|day| day < chrono::Local::now().date_naive());
    let said = day.map_or(due.to_owned(), |day| day.format("Due %-d %b").to_string());
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(if late {
            t.danger.opacity(0.16)
        } else {
            t.surface_active
        })
        .text_size(crate::theme::ui_text(10.5))
        .text_color(if late { t.danger } else { t.text_secondary })
        .child(said)
        .into_any_element()
}

/// How long is left in a sprint, in the words a standup uses.
fn sprint_left(end: Option<&str>) -> Option<String> {
    let end = chrono::DateTime::parse_from_rfc3339(end?).ok()?;
    let left = end.timestamp_millis() - chrono::Utc::now().timestamp_millis();
    if left < 0 {
        return Some("ended".into());
    }
    Some(match left / 86_400_000 {
        0 => "ends today".into(),
        1 => "1 day left".into(),
        days => format!("{days} days left"),
    })
}

/// When something happened, as how long ago, from Jira's time.
fn ago(at: &str) -> String {
    let at = chrono::DateTime::parse_from_rfc3339(at)
        .or_else(|_| chrono::DateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%S%.f%z"));
    let Ok(at) = at else {
        return String::new();
    };
    match crate::agents::age(at.timestamp_millis().max(0) as u64) {
        now if now == "now" => now,
        age => format!("{age} ago"),
    }
}

/// Someone's comment or change: their picture, their name and when, and what they did.
fn said(
    host: &Host,
    author: &str,
    avatar: Option<&str>,
    at: &str,
    body: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .gap(px(10.))
        .child(host.face(author, avatar, 24., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            ui::text(author.to_owned(), 12.5, FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        )
                        .child(ui::meta(ago(at), cx)),
                )
                .child(body),
        )
        .into_any_element()
}

/// Each column's cards after the moves still on their way to Jira.
fn with_moves<'a>(
    columns: &'a [Column],
    moves: &HashMap<String, usize>,
) -> Vec<Vec<&'a IssueSummary>> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let staying = column
                .issues
                .iter()
                .filter(|issue| !moves.contains_key(&issue.key));
            let arriving = columns
                .iter()
                .flat_map(|other| other.issues.iter())
                .filter(|issue| moves.get(&issue.key) == Some(&index));
            staying.chain(arriving).collect()
        })
        .collect()
}

/// Moves a card to another column through whichever transition leads to one of its
/// statuses, showing it there until the board comes again.
fn move_card(pane: &WeakEntity<PluginPane>, moved: Moved, cx: &mut App) {
    let key = moved.key.to_owned();
    let params = json!({
        "key": moved.key,
        "statusIds": moved.status_ids,
        "column": moved.column,
        "site": site_of(moved.site),
    });
    let to = moved.to;
    let _ = pane.update(cx, |pane, cx| {
        let Some((jira, host)) = pane.parts::<Jira>() else {
            return;
        };
        jira.moves.insert(key.clone(), to);
        let call = host.call("moveIssue", params);
        after::<Jira, _>(
            call,
            move |jira, host, done, cx| {
                if let Err(failure) = done {
                    jira.moves.remove(&key);
                    host.said(&failure.titled(&format!("Move {key}")), cx);
                }
            },
            cx,
        );
        cx.notify();
    });
}

fn issue_row(
    host: &Host,
    issue: &IssueSummary,
    index: usize,
    chosen: bool,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = issue.key.clone();
    let who = issue
        .assignee
        .as_ref()
        .map_or("Unassigned", |person| person.name.as_str());
    let avatar = issue
        .assignee
        .as_ref()
        .and_then(|person| person.avatar_url.as_deref());
    div()
        .id(("jira-issue", index))
        .px(px(12.))
        .py(px(8.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .when(index > 0, |row| row.border_t_1().border_color(t.border))
        .when(chosen, |row| row.bg(t.surface_chosen))
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(type_mark(issue.issue_type.as_deref(), cx))
                .child(ui::mono(issue.key.clone(), 11., cx).text_color(t.text_secondary))
                .child(status_chip(&issue.status, &issue.status_category, cx)),
        )
        .child(ui::text(issue.summary.clone(), 12.5, FontWeight::MEDIUM).text_color(t.text_primary))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(
                    issue
                        .priority
                        .as_deref()
                        .map(|priority| priority_mark(priority, cx)),
                )
                .children(
                    issue
                        .sprint
                        .clone()
                        .map(|sprint| ui::meta(sprint, cx).truncate()),
                )
                .child(div().flex_1())
                .child(ui::meta(who.to_owned(), cx).truncate())
                .child(host.face(who, avatar, 18., cx)),
        )
        .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
            jira.open(Some(key.clone()));
            cx.notify();
        }))
        .context_menu(issue_menu(issue, host.bench().clone()))
        .into_any_element()
}

/// Ways to take an issue out of Sikemux: to the browser, or to the clipboard in the shapes
/// people paste.
fn issue_menu(
    issue: &IssueSummary,
    bench: WeakEntity<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let (key, summary, url) = (issue.key.clone(), issue.summary.clone(), issue.url.clone());
    move |popup, _, _| {
        let copies = [
            ("Copy link", url.clone(), "the link"),
            ("Copy key", key.clone(), "the key"),
            (
                "Copy key and title",
                format!("{key} {summary}"),
                "the key and title",
            ),
            (
                "Copy as Markdown link",
                format!("[{key}]({url}) {summary}"),
                "the Markdown link",
            ),
        ];
        let url = url.clone();
        let mut popup = popup
            .item(
                PopupMenuItem::new("Open in browser")
                    .icon(Icon::Globe)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .separator();
        for (label, text, what) in copies {
            let bench = bench.clone();
            popup = popup.item(
                PopupMenuItem::new(label)
                    .icon(Icon::Copy)
                    .on_click(move |_, _, cx| super::copy(&bench, text.clone(), what, cx)),
            );
        }
        popup
    }
}

/// Puts the issue on the front project's board as a card for agents, its open subtasks
/// under it.
fn send_to_board(bench: &WeakEntity<Workbench>, view: &IssueView, cx: &mut App) {
    let Some(bench) = bench.upgrade() else {
        return;
    };
    let ticket = |issue: &IssueSummary| json!({ "key": issue.key, "url": issue.url, "source": issue.issue_type, "provider": "jira" });
    let Some((folder, name)) = bench
        .read(cx)
        .project()
        .map(|project| (project.path.clone(), project.name.clone()))
    else {
        bench.update(cx, |bench, cx| {
            bench.notify(
                toast::error("Open a project to put this on its board"),
                cx,
            )
        });
        return;
    };
    let subtasks: Vec<Value> = view
        .subtasks
        .iter()
        .filter(|sub| sub.status_category != "done")
        .map(|sub| json!({ "title": sub.summary, "details": "", "ticket": ticket(sub) }))
        .collect();
    let params = json!({
        "project": folder,
        "title": view.issue.summary,
        "details": view.description,
        "ticket": ticket(&view.issue),
        "subtasks": subtasks,
        "blockedBy": view.blocked_by,
        "blocks": view.blocks,
    });
    let key = view.issue.key.clone();
    let asked = bench.read(cx).target().request("boardAddTicket", params);
    let weak = bench.downgrade();
    cx.spawn(async move |cx| {
        let answer = asked.await;
        let _ = weak.update(cx, |bench, cx| match answer {
            Ok(board) => {
                for problem in board["problems"].as_array().into_iter().flatten() {
                    if let Some(problem) = problem.as_str() {
                        bench.notify(toast::error(problem.to_owned()), cx);
                    }
                }
                let put = format!("Put {key} on {name}'s board");
                bench.notify(toast::success(put), cx);
            }
            Err(error) => {
                let failed = format!("Send to the board: {error}");
                bench.notify(toast::error(failed), cx)
            }
        });
    })
    .detach();
}

/// The front project's recent commits that name the issue.
fn commits(host: &Host, key: &str, cx: &App) -> Option<AnyElement> {
    let bench = host.bench().upgrade()?;
    let log = bench.read(cx).read("git-overview")?["log"]
        .as_array()?
        .clone();
    let named: Vec<&Value> = log
        .iter()
        .filter(|commit| mentions(commit["subject"].as_str().unwrap_or_default(), key))
        .collect();
    if named.is_empty() {
        return None;
    }
    let t = Tokens::get(cx);
    let line = |commit: &Value, field: &str| commit[field].as_str().unwrap_or_default().to_owned();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(ui::caps("Commits in this project", cx))
            .children(named.into_iter().map(|commit| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(ui::mono(line(commit, "hash"), 11., cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::theme::ui_text(12.))
                            .text_color(t.text_primary)
                            .child(line(commit, "subject")),
                    )
                    .child(ui::meta(line(commit, "author"), cx))
            }))
            .into_any_element(),
    )
}

/// Whether a commit message names the issue as a whole word, in any case, so ABC-12 is
/// not ABC-123.
fn mentions(text: &str, key: &str) -> bool {
    let (text, key) = (text.to_lowercase(), key.to_lowercase());
    !key.is_empty()
        && text.match_indices(&key).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + key.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphanumeric())
                && !after.is_some_and(|c| c.is_ascii_digit())
        })
}
