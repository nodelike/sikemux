use crate::ui::keyed::{self, KeyedRows};
use crate::ui::Clicks;
use crate::workbench::actions;
use sikemux_view::prefs::PreferenceChange;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Div, DragMoveEvent, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, WeakEntity, Window,
};
use sikemux_calls::hosts::Found as HostFound;
use sikemux_calls::hosts::HostRow;
use sikemux_calls::hosts::Reach as HostReach;
use sikemux_view::device::AccountInfo;
use sikemux_view::host_stats::AlertLevel;
use sikemux_view::layout::PaneKind;
use sikemux_view::prefs::{Preferences, ProjectSort, TabRailPage, TabRailScope};
use sikemux_view::workspace::ProjectStanding;
use sikemux_wire::protocol::{AgentGroup, AgentStatus};

use crate::agents::rows::AgentRow;
use crate::copies::Folded;
use crate::hosts::install_pane::{SetUp, Words};
use crate::hosts::HostChoice;
use crate::icons::Icon;
use crate::theme::{
    radius_1, radius_2, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS,
    TEXT_3XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{Button, Group, IconButton, Row, Tone, Variant};
use crate::workbench::{Strip, Workbench};
use crate::{keymap, motion, tabs, ui};
use sikemux_calls::call;
use sikemux_calls::workspace::UpdateProjectParams;

/// The two rails: the side rail of projects, and the tab rail of agents and chats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rail {
    Side,
    Agents,
}

impl Rail {
    pub fn widths(self) -> RailWidth {
        match self {
            Rail::Side => SIDE_RAIL,
            Rail::Agents => crate::agents::TAB_RAIL,
        }
    }

    /// What `pair`, the side rail's first, keeps for this rail.
    pub(crate) fn of<T>(self, pair: &[T; 2]) -> &T {
        let [side, agents] = pair;
        match self {
            Rail::Side => side,
            Rail::Agents => agents,
        }
    }

    pub(crate) fn of_mut<T>(self, pair: &mut [T; 2]) -> &mut T {
        let [side, agents] = pair;
        match self {
            Rail::Side => side,
            Rail::Agents => agents,
        }
    }
}

/// The window's edge a closed rail comes out from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

/// How wide a rail can be dragged, and how wide it starts.
#[derive(Clone, Copy)]
pub struct RailWidth {
    pub min: f32,
    pub max: f32,
    pub initial: f32,
}

/// A rail's handle while it is dragged.
#[derive(Clone)]
pub(crate) struct DraggedRail(pub(crate) Rail);

/// How far past its own width a peeking rail starts and ends, out of sight beyond the window.
const PEEK_GAP: f32 = 10.;

/// How long a peeking rail stays after the pointer leaves it.
const PEEK_CLOSE: Duration = Duration::from_millis(180);

/// How close to the window's edge the pointer brings out a closed rail.
const PEEK_REACH: f32 = 28.;

impl Rails {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        let scroll = gpui_kit::ScrollHandle::default();
        let keyed = [
            KeyedRows::new(cx.focus_handle(), gpui_kit::ScrollHandle::default()),
            KeyedRows::new(cx.focus_handle(), scroll.clone()),
        ];
        Self {
            side_open: true,
            tab_open: true,
            side_width: SIDE_RAIL.initial,
            tab_width: crate::agents::TAB_RAIL.initial,
            drawn: [SIDE_RAIL.initial, crate::agents::TAB_RAIL.initial],
            resizers: [cx.focus_handle(), cx.focus_handle()],
            peeking: None,
            unpeek: None,
            page: TabRailPage::Agents,
            copy_leaves_uncommitted: false,
            copy_notes: HashMap::new(),
            keeping: None,
            all_projects: false,
            all_harnesses: false,
            tag: None,
            folded_teams: HashSet::new(),
            folded_groups: Vec::new(),
            seen: Default::default(),
            scroll,
            glide: Default::default(),
            frame: Rc::default(),
            slide: crate::track::Slide::default(),
            keyed,
        }
    }

    pub(crate) fn keyed(&self, rail: Rail) -> &KeyedRows {
        rail.of(&self.keyed)
    }

    /// Opens, sizes and pages the rails as the person's preferences keep them.
    pub fn follow(&mut self, preferences: &Preferences) {
        self.side_open = preferences.side_rail_open;
        self.tab_open = preferences.tab_rail_open;
        let width =
            |width: f64, range: RailWidth| (width as f32).clamp(range.min, range.max).round();
        self.side_width = width(preferences.side_rail_width, SIDE_RAIL);
        self.tab_width = width(preferences.tab_rail_width, crate::agents::TAB_RAIL);
        self.page = preferences.tab_rail_page;
        self.all_projects = preferences.tab_rail_scope == TabRailScope::All;
        self.all_harnesses = preferences.tab_rail_all_agents;
        self.folded_groups
            .clone_from(&preferences.folded_rail_groups);
    }

    pub(crate) fn open(&self, rail: Rail) -> bool {
        match rail {
            Rail::Side => self.side_open,
            Rail::Agents => self.tab_open,
        }
    }

    pub(crate) fn folded(&self, group: &str) -> bool {
        self.folded_groups.iter().any(|folded| folded == group)
    }
}

/// The two rails' state: which stay open and how wide, the page the tab rail shows and what it
/// lists, and what their drawing remembers between frames.
pub struct Rails {
    /// Which rails stay open; a closed one comes out over the stage while the pointer is at its edge.
    pub(crate) side_open: bool,
    pub(crate) tab_open: bool,
    pub(crate) side_width: f32,
    pub(crate) tab_width: f32,
    /// How wide each rail was last drawn, by [`Rail`]: as kept, unless the window is narrow.
    pub(crate) drawn: [f32; 2],
    /// The handles beside the rails, which arrow keys move once focused.
    pub(crate) resizers: [gpui_kit::FocusHandle; 2],
    pub(crate) peeking: Option<Rail>,
    pub(crate) unpeek: Option<gpui_kit::Task<()>>,
    /// Which of its pages the tab rail shows.
    pub page: TabRailPage,
    /// A new copy of a project leaves the uncommitted changes where they are, as the person
    /// chose in the copy switch.
    pub(crate) copy_leaves_uncommitted: bool,
    /// How each copy the person asked for went, by its project's identity, until dismissed.
    pub(crate) copy_notes: HashMap<String, crate::copies::CopyNote>,
    /// The rails' state waiting to be kept in the core's preferences, once a drag settles.
    pub(crate) keeping: Option<gpui_kit::Task<()>>,
    pub(crate) all_projects: bool,
    /// The rail lists the recent chats of every agent, not one's.
    pub(crate) all_harnesses: bool,
    /// The tag the agent rail lists only the chats of, when the person picked one.
    pub(crate) tag: Option<String>,
    /// The leads whose teams the agent rail folds away.
    pub(crate) folded_teams: std::collections::HashSet<String>,
    /// The rail groups folded away, by key, such as `pinned`.
    pub(crate) folded_groups: Vec<String>,
    /// The agent rows the rail drew last, for telling the rows that have just arrived.
    pub(crate) seen: std::cell::RefCell<crate::agents::RailSeen>,
    /// The tab rail's list of agents and chats, whose foot brings older chats into view.
    pub(crate) scroll: gpui_kit::ScrollHandle,
    /// Where the rail's agent rows were drawn, for the chosen surface to glide between them.
    pub(crate) glide: std::cell::RefCell<crate::motion::GlideTrack>,
    /// Where the tab rail's frame was last drawn, left, top, width and height, for the driver.
    pub(crate) frame: Rc<std::cell::Cell<[f32; 4]>>,
    /// The tab rail's pages while they move between one another.
    pub(crate) slide: crate::track::Slide<TabRailPage>,
    /// The side rail's rows and the tab rail's, as the keys walk them.
    pub(crate) keyed: [crate::ui::keyed::KeyedRows; 2],
}

pub const SIDE_RAIL: RailWidth = RailWidth {
    min: 180.,
    max: 480.,
    initial: 258.,
};

pub fn side_rail(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let space = bench.space();
    let listed = bench.folded();
    let projects: Vec<_> = (listed.iter())
        .filter(|folded| space.is_none_or(|space| bench.space_of(folded) == Some(&space.id)))
        .map(|folded| project_tree(bench, folded, cx).into_any_element())
        .collect();
    let empty = match space {
        Some(space) => t!("no projects in {space}", space = space.name),
        None => t!("no projects").to_owned(),
    };
    let rail = ui::panel(cx)
        .w(px(bench.rail_width(Rail::Side)))
        .flex_none();
    keyed_rail(rail, Rail::Side, bench, cx)
        .child(masthead(bench, cx))
        .child(
            div()
                .id("side-rail-scroll")
                .flex_1()
                .min_h_0()
                .pt(SPACE_2H)
                .px(SPACE_2H)
                .pb(SPACE_4)
                .track_scroll(&bench.rails.keyed(Rail::Side).scroll)
                .overflow_y_scroll()
                .child(
                    Group::new(t!("Projects"), empty)
                        .first()
                        .actions(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_0)
                                .children((listed.len() > 1).then(|| project_sort(bench, cx)))
                                .child(actions(
                                    keymap::hint(actions::OpenProject::ID, cx),
                                    [IconButton::new(
                                        "add-project",
                                        Icon::Plus,
                                        t!("Open project"),
                                    )
                                    .on_click(
                                        |_, window, cx| {
                                            keymap::dispatch(actions::OpenProject::ID, window, cx)
                                        },
                                    )],
                                    cx,
                                )),
                        )
                        .toolbar((!listed.is_empty()).then(|| space_switch(bench, cx)))
                        .children(projects),
                )
                .child(
                    Group::new(t!("Hosts"), t!("no hosts"))
                        .actions(actions(
                            keymap::hint(actions::OpenHost::ID, cx),
                            [
                                IconButton::new("hosts-overview", Icon::Gauge, hosts_overview(cx))
                                    .on_click(|_, window, cx| {
                                        keymap::dispatch(actions::FocusHosts::ID, window, cx)
                                    }),
                                IconButton::new("edit-ssh", Icon::Pencil, t!("Edit ~/.ssh/config"))
                                    .on_click(cx.listener(|bench, _, _, cx| {
                                        bench.edit_ssh_config(None, cx)
                                    })),
                                IconButton::new("add-host", Icon::Plus, t!("Find a host"))
                                    .on_click(|_, window, cx| {
                                        keymap::dispatch(actions::OpenHost::ID, window, cx)
                                    }),
                            ],
                            cx,
                        ))
                        .child(this_host(bench, cx))
                        .children(hosts(bench, cx)),
                ),
        )
        .children(crate::updates::rail_chip(bench, cx))
        .child(account(bench, cx))
}

/// The Hosts header's gauge, naming its shortcut.
fn hosts_overview(cx: &App) -> String {
    match keymap::hint(actions::FocusHosts::ID, cx) {
        Some(keys) => t!("Hosts overview — {keys}", keys = keys),
        None => t!("Hosts overview").to_owned(),
    }
}

/// "Host details" at the top of a host's menu, opening the Hosts page on `host`.
fn details_item(bench: WeakEntity<Workbench>, host: String) -> PopupMenuItem {
    PopupMenuItem::new(t!("Host details"))
        .icon(Icon::Gauge)
        .on_click(move |_, _, cx| {
            let host = host.clone();
            let _ = bench.update(cx, |bench, _| {
                bench.open_hosts(Some(host));
            });
        })
}

/// The host this window runs on, where new work starts.
/// The folder a project's row is marked with, wherever a project is named.
pub(crate) fn project_glyph(size: f32, cx: &App) -> gpui_kit::Svg {
    ui::icon(Icon::Folder, size).text_color(Tokens::get(cx).live)
}

/// The laptop a host's row is marked with: a linked host's colour, else an SSH one's.
pub(crate) fn host_glyph(size: f32, linked: bool, cx: &App) -> gpui_kit::Svg {
    let t = Tokens::get(cx);
    ui::icon(Icon::Laptop, size).text_color(if linked { t.host } else { t.ssh })
}

fn this_host(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let agents = bench
        .this_view()
        .map_or(0, |view| view.workspace.agents.len());
    let alert = bench.host_alert(None, true).cloned();
    let alert = alert.as_ref();
    let row = Row::new("this-host", t!("This host"))
        .mark(
            16.,
            ui::icon(Icon::Laptop, crate::theme::ROW_GLYPH)
                .text_color(t.text_secondary)
                .group_hover(ui::ROW, move |style| style.text_color(t.text_primary)),
        )
        .quiet()
        .chosen(
            !bench.settings.read(cx).is_open()
                && bench.chosen_host() == crate::hosts::HostChoice::This,
        )
        .on_click(
            cx.listener(|bench, _, _, cx| bench.choose_host(crate::hosts::HostChoice::This, cx)),
        )
        .when(agents > 0, |row| {
            row.detail(match agents {
                1 => t!("1 agent").to_owned(),
                agents => t!("{count} agents", count = agents),
            })
        })
        .status(Some(ui::dot(
            match alert.map(|alert| alert.level) {
                Some(AlertLevel::Danger) => Tone::Failed,
                _ => Tone::Ok,
            },
            cx,
        )));
    with_menu(
        "this-host-menu".into(),
        this_host_menu(cx.weak_entity(), bench.feeds.this.approval, true),
        row,
        cx,
    )
}

/// What this host's menu offers, on its row and on its page.
pub(crate) fn this_host_menu(
    bench: WeakEntity<Workbench>,
    approval: Option<bool>,
    details: bool,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |popup, _, _| {
        let terminal = bench.clone();
        let popup = match details {
            true => popup
                .item(details_item(bench.clone(), HostChoice::This.key().into()))
                .separator(),
            false => popup,
        };
        popup
            .item(
                PopupMenuItem::new(t!("New terminal here"))
                    .icon(Icon::Terminal)
                    .on_click(move |_, _, cx| {
                        let _ = terminal.update(cx, |bench, cx| {
                            bench.choose_host(HostChoice::This, cx);
                            bench.new_loose_terminal(cx);
                        });
                    }),
            )
            .separator()
            .item(approval_item(bench.clone(), None, approval))
    }
}

/// A row with its menu on a right-click and on a ⋯ that shows while the pointer is over the
/// row.
pub(crate) fn with_menu(
    id: SharedString,
    menu: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    row: Row,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let menu = Rc::new(menu);
    let more = menu.clone();
    div()
        .id(id.clone())
        .relative()
        .group(MENU_ROW)
        .context_menu(ui::menu(move |popup, window, cx| menu(popup, window, cx)))
        .child(row.hover_padding(MENU_ROW, 32.))
        .child(
            ui::hover_actions(MENU_ROW).top_0().bottom_0().child(
                KitButton::new(SharedString::from(format!("{id}-more")))
                    .ghost()
                    .compact()
                    .rounded(radius_2())
                    .bg(t.surface_hover)
                    .child(ui::icon(Icon::More, 12.).text_color(t.text_secondary))
                    .dropdown_menu_with_anchor(
                        Anchor::TopRight,
                        ui::menu(move |popup, window, cx| more(popup, window, cx)),
                    ),
            ),
        )
        .into_any_element()
}

/// The hover group of a host's row, whose ⋯ shows on hover.
const MENU_ROW: &str = "menu-row";

/// "Require approval for new devices" on `host`, this one when none, ticked while it does.
fn approval_item(
    bench: WeakEntity<Workbench>,
    host: Option<String>,
    approval: Option<bool>,
) -> PopupMenuItem {
    PopupMenuItem::new(t!("Require approval for new devices"))
        .icon(Icon::Shield)
        .checked(approval == Some(true))
        .disabled(approval.is_none())
        .on_click(move |_, _, cx| {
            let host = host.clone();
            let _ = bench.update(cx, |bench, cx| bench.flip_approval(host, cx));
        })
}

/// The account's other hosts, then the SSH hosts a terminal is open to.
fn hosts(bench: &Workbench, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    (bench.reads.hosts.iter())
        .map(|host| {
            let name = host.name.clone();
            let linked = host.id.is_some();
            let reach = crate::hosts::reach(bench, host);
            let tone = match reach {
                HostReach::Connected => Tone::Ok,
                HostReach::Connecting => Tone::Pending,
                HostReach::Failed => Tone::Failed,
                HostReach::Away => Tone::Idle,
            };
            let view = (host.id.as_ref()).and_then(|id| bench.feeds.host(id)?.view.as_ref());
            let agents = view.map_or(0, |view| view.workspace.agents.len());
            let connected = reach == HostReach::Connected;
            let updates = view
                .and_then(crate::hosts::host_updates)
                .filter(|_| connected);
            let detail = match reach {
                _ if linked && bench.feeds.unavailable.is_some() => {
                    Some(t!("needs the signed app").to_owned())
                }
                HostReach::Away if linked => Some(t!("out of reach").to_owned()),
                HostReach::Connecting if linked => Some(t!("connecting").to_owned()),
                _ if agents > 0 => Some(match agents {
                    1 => t!("1 agent").to_owned(),
                    count => t!("{count} agents", count = count),
                }),
                _ => None,
            };
            let found = match host.found {
                Some(HostFound::SignedOut) => Some(t!("not signed in").to_owned()),
                Some(HostFound::Other) => Some(t!("on another account").to_owned()),
                Some(HostFound::Stopped) => Some(t!("stopped").to_owned()),
                Some(HostFound::Outdated) => Some(t!("older release").to_owned()),
                _ => (host.unanswered.as_ref()).map(|unanswered| {
                    crate::hosts::install_pane::unanswered_detail(unanswered).to_owned()
                }),
            };
            let tip = match (
                host.setup.as_ref().and_then(|setup| setup.problem.as_ref()),
                &host.unanswered,
                &host.ssh,
            ) {
                (Some(problem), _, _) => Some(t!("Not set up: {problem}", problem = problem)),
                (None, Some(unanswered), Some(alias)) => Some(
                    crate::hosts::install_pane::unanswered_says(unanswered, alias),
                ),
                _ => None,
            };
            let setup = host.setup.as_ref();
            let detail = match setup.map(|setup| (setup.work, setup.problem.as_deref())) {
                Some((_, Some(problem))) => Some(t!("not set up: {problem}", problem = problem)),
                Some((SetUp::Install, None)) => Some(t!("installing").to_owned()),
                Some((SetUp::Update, None)) => Some(t!("updating").to_owned()),
                Some((SetUp::Start, None)) => Some(t!("starting").to_owned()),
                Some((SetUp::SignIn, None)) => Some(t!("signing in").to_owned()),
                None => found.or(detail),
            };
            let alias = host.ssh.clone().filter(|_| !linked);
            let front = host.ssh.is_some() && bench.strip() == host.ssh.clone().map(Strip::Host);
            let key = host.id.clone();
            let chosen = !bench.settings.read(cx).is_open()
                && (front
                    || key.as_ref().is_some_and(|key| {
                        bench.chosen_host() == crate::hosts::HostChoice::Account(key.clone())
                    }));
            let row_key = (host.id.clone())
                .or_else(|| host.ssh.as_ref().map(|alias| format!("ssh:{alias}")))
                .unwrap_or_else(|| name.to_string());
            let row = Row::new(SharedString::from(format!("host-{row_key}")), name)
                .chosen(chosen)
                .when_some(alias.clone(), |row, alias| {
                    row.on_click(cx.listener(move |bench, _, _, cx| {
                        bench.choose_host(crate::hosts::HostChoice::Ssh(alias.clone()), cx)
                    }))
                })
                .when_some(key, |row, key| {
                    row.on_click(cx.listener(move |bench, _, _, cx| {
                        bench.choose_host(crate::hosts::HostChoice::Account(key.clone()), cx)
                    }))
                })
                .mark(16., host_glyph(crate::theme::ROW_GLYPH, linked, cx))
                .quiet()
                .when_some(
                    {
                        let parts: Vec<String> = [
                            detail,
                            updates.as_ref().and_then(crate::hosts::updates_label),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        (!parts.is_empty()).then(|| parts.join(" · "))
                    },
                    |row, detail| row.detail(detail),
                )
                .when_some(tip, |row, tip| row.detail_tip(tip));
            let alert = (host.id.as_deref())
                .and_then(|key| bench.host_alert(Some(key), connected))
                .cloned();
            let tone = match alert.as_ref().map(|alert| alert.level) {
                Some(AlertLevel::Danger) => Tone::Failed,
                _ => tone,
            };
            let row = row.status(Some(ui::dot(tone, cx)));
            let bench_entity = cx.weak_entity();
            let menu_id = SharedString::from(format!("host-menu-{row_key}"));
            if let Some(menu) = account_host_menu(bench, host, true, cx) {
                return with_menu(menu_id, menu, row, cx);
            }
            let Some(alias) = alias else {
                return row.into_any_element();
            };
            let set_up_item = set_up_offer(bench, host);
            let dismiss_item = dismiss_offer(host);
            with_menu(
                menu_id,
                move |popup, _, _| {
                    let (bench, alias) = (bench_entity.clone(), alias.clone());
                    let edited = (bench.clone(), alias.clone());
                    let popup = popup.item(
                        PopupMenuItem::new(t!("New SSH terminal"))
                            .icon(Icon::Terminal)
                            .on_click(move |_, _, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.open_ssh_terminal(&alias, cx));
                            }),
                    );
                    let popup = match set_up_item(bench_entity.clone(), true) {
                        Some(item) => popup.item(item),
                        None => popup,
                    };
                    let popup = match dismiss_item(bench_entity.clone()) {
                        Some(item) => popup.item(item),
                        None => popup,
                    };
                    popup.separator().item(
                        PopupMenuItem::new(t!("Edit in ~/.ssh/config"))
                            .icon(Icon::Pencil)
                            .on_click(move |_, _, cx| {
                                let (bench, alias) = &edited;
                                let _ = bench.update(cx, |bench, cx| {
                                    bench.edit_ssh_config(Some(alias.clone()), cx)
                                });
                            }),
                    )
                },
                row,
                cx,
            )
        })
        .collect()
}

/// The item that sets Sikemux up on an SSH host, installing, updating, starting or signing it
/// in as it needs, while it needs that.
fn set_up_offer(
    bench: &Workbench,
    host: &HostRow,
) -> impl Fn(WeakEntity<Workbench>, bool) -> Option<PopupMenuItem> + Clone + 'static {
    let setting_up = (host.setup.as_ref()).is_some_and(|setup| setup.problem.is_none());
    let set_up = (host.ssh.as_deref())
        .and_then(|alias| bench.set_up_offered(alias))
        .zip(host.ssh.clone());
    move |bench: WeakEntity<Workbench>, here: bool| {
        set_up.clone().map(|(set_up, alias)| {
            let label = match set_up {
                SetUp::Install if here => t!("Install Sikemux here"),
                _ => set_up.menu_label(),
            };
            PopupMenuItem::new(label)
                .icon(set_up.icon())
                .disabled(setting_up)
                .on_click(move |_, _, cx| {
                    let _ = bench.update(cx, |bench, cx| bench.set_up_host(&alias, set_up, cx));
                })
        })
    }
}

/// The item that takes a set-up that stopped short off the host's row.
fn dismiss_offer(
    host: &HostRow,
) -> impl Fn(WeakEntity<Workbench>) -> Option<PopupMenuItem> + Clone + 'static {
    let alias = (host.ssh.clone())
        .filter(|_| (host.setup.as_ref()).is_some_and(|setup| setup.problem.is_some()));
    move |bench: WeakEntity<Workbench>| {
        alias.clone().map(|alias| {
            PopupMenuItem::new(t!("Dismiss the failed set-up"))
                .icon(Icon::Close)
                .on_click(move |_, _, cx| {
                    let _ = bench.update(cx, |bench, _| bench.dismiss_set_up(&alias));
                })
        })
    }
}

/// What an account host's menu offers, on its row and on its page; none for an SSH host
/// without Sikemux.
pub(crate) fn account_host_menu(
    bench: &Workbench,
    host: &HostRow,
    details: bool,
    cx: &mut Context<Workbench>,
) -> Option<impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static> {
    let key = host.id.clone()?;
    let name = host.name.clone();
    let view = bench.feeds.host(&key).and_then(|feed| feed.view.as_ref());
    let has_project = view.is_some_and(|view| !view.workspace.projects.is_empty());
    let updates = view
        .and_then(crate::hosts::host_updates)
        .filter(|_| crate::hosts::reach(bench, host) == HostReach::Connected);
    let approval = bench.feeds.host(&key).and_then(|feed| feed.approval);
    let set_up_item = set_up_offer(bench, host);
    let dismiss_item = dismiss_offer(host);
    let ssh = host.ssh.clone();
    let bench_entity = cx.weak_entity();
    Some(
        move |popup: PopupMenu, _: &mut Window, cx: &mut Context<PopupMenu>| {
            let bench = bench_entity.clone();
            let mut popup = popup;
            if details {
                popup = popup
                    .item(details_item(bench.clone(), key.clone()))
                    .separator();
            }
            if let Some(alias) = ssh.clone() {
                popup = popup.item(
                    PopupMenuItem::new(t!("New SSH terminal"))
                        .icon(Icon::Terminal)
                        .on_click({
                            let bench = bench.clone();
                            move |_, _, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.open_ssh_terminal(&alias, cx));
                            }
                        }),
                );
            }
            if let Some(item) = set_up_item(bench.clone(), false) {
                popup = popup.item(item);
            }
            if let Some(item) = dismiss_item(bench.clone()) {
                popup = popup.item(item);
            }
            if ssh.is_some() {
                popup = popup.separator();
            }
            let on = |run: fn(&mut Workbench, &str, &mut Context<Workbench>)| {
                let (bench, key) = (bench.clone(), key.clone());
                move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
                    let _ = bench.update(cx, |bench, cx| run(bench, &key, cx));
                }
            };
            let named = name.clone();
            let popup = popup
                .item(
                    PopupMenuItem::new(t!("New chat here"))
                        .icon(Icon::Plus)
                        .disabled(!has_project)
                        .on_click(on(|bench, key, cx| bench.new_chat_on(key, cx))),
                )
                .item(
                    PopupMenuItem::new(t!("New terminal here"))
                        .icon(Icon::Terminal)
                        .on_click(on(|bench, key, cx| bench.new_terminal_on(key, cx))),
                )
                .item(
                    PopupMenuItem::new(t!("Files"))
                        .icon(Icon::Folder)
                        .disabled(!has_project)
                        .on_click(on(|bench, key, cx| {
                            bench.choose_host(HostChoice::Account(key.to_owned()), cx);
                            bench.show_rail_page(TabRailPage::Files, cx);
                        })),
                )
                .separator()
                .item(approval_item(bench.clone(), Some(key.clone()), approval));
            let popup = match &updates {
                Some(updates) => {
                    let (bench, key) = (bench.clone(), key.clone());
                    crate::hosts::updates_items(popup, updates, move |on, cx| {
                        let host = key.clone();
                        let _ = bench.update(cx, |bench, cx| bench.set_host_updates(host, on, cx));
                    })
                }
                None => popup,
            };
            popup
                .item(
                    PopupMenuItem::new(t!("Rename…"))
                        .icon(Icon::Pencil)
                        .on_click({
                            let (bench, key, named) = (bench.clone(), key.clone(), named.clone());
                            move |_, _, cx| {
                                let _ = bench.update(cx, |bench, cx| {
                                    bench.rename_on_account(&key, &named, cx)
                                });
                            }
                        }),
                )
                .item(
                    ui::MenuRow::new(t!("Remove from account"))
                        .icon(Icon::Trash)
                        .danger(true)
                        .item(cx)
                        .on_click({
                            let (bench, key) = (bench.clone(), key.clone());
                            move |_, _, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.remove_host(&key, &named, cx));
                            }
                        }),
                )
        },
    )
}

fn masthead(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let (release, channel) = split_version(&bench.connection.version);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_1H)
        .h(px(40.))
        .pl(px(18.))
        .pr(SPACE_2)
        .border_b(crate::theme::border())
        .border_color(t.border)
        .child(ui::icon(Icon::Logo, 14.).text_color(t.accent))
        .child(
            ui::text("Sikemux", TEXT_SM, FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .ml(px(1.)),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .pt(px(1.))
                .child(ui::mono(format!("v{release}"), TEXT_3XS, cx))
                .children(channel.map(|channel| {
                    ui::text(channel, TEXT_3XS, FontWeight::MEDIUM)
                        .px(SPACE_1)
                        .py(px(1.))
                        .line_height(crate::theme::ui_text(TEXT_XS))
                        .border(crate::theme::border())
                        .border_color(t.line)
                        .rounded(radius_1())
                        .text_color(t.text_tertiary)
                })),
        )
        .child(
            div().ml_auto().flex_none().child(
                IconButton::new(
                    "hide-side-rail",
                    Icon::PanelLeft,
                    if bench.rails.side_open {
                        t!("Hide the side rail")
                    } else {
                        t!("Keep the side rail open")
                    },
                )
                .on_click(cx.listener(|bench, _, _, cx| bench.toggle_rail(Rail::Side, cx))),
            ),
        )
}

/// `0.4.0-nightly.10` as the release `0.4.0` and the channel `nightly 10`.
fn split_version(version: &str) -> (String, Option<String>) {
    match version.split_once('-') {
        Some((release, channel)) => (release.into(), Some(channel.replacen('.', " ", 1))),
        None => (version.into(), None),
    }
}

/// The menu that sorts the projects: the one worked in last first, the first added first, or
/// as the person drags them.
fn project_sort(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let chosen = bench.project_sort();
    let choose = Rc::new(cx.listener(|bench, sort: &ProjectSort, _, _| {
        crate::settings::set(bench, PreferenceChange::ProjectSort(*sort));
    }));
    ui::menu_button(
        "project-sort",
        gpui_kit::Anchor::TopRight,
        IconButton::new("project-sort-button", Icon::Sort, t!("Sort projects")).small(),
        move |mut popup, _, cx| {
            popup = popup.label(t!("Sort projects"));
            for (sort, label) in [
                (ProjectSort::Recent, t!("Recent")),
                (ProjectSort::Created, t!("Created")),
                (ProjectSort::Manual, t!("Manual")),
            ] {
                let choose = choose.clone();
                let row = ui::MenuRow::new(label)
                    .when(sort == ProjectSort::Manual, |row| row.hint(t!("drag")));
                popup = popup.item(
                    row.item(cx)
                        .checked(sort == chosen)
                        .on_click(move |_, window, cx| choose(&sort, window, cx)),
                );
            }
            popup
        },
    )
    .into_any_element()
}

fn actions<const N: usize>(
    keys: Option<SharedString>,
    buttons: [IconButton; N],
    cx: &App,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .children(keys.map(|keys| ui::kbd(keys, cx)))
        .children(buttons.map(IconButton::small))
}

fn project_tree(
    bench: &Workbench,
    folded: &Folded,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let in_front = bench.copy_in_front(folded).cloned();
    let active = !bench.settings.read(cx).is_open()
        && in_front
            .as_ref()
            .is_some_and(|copy| bench.strip() == Some(Strip::Project(copy.project.clone())));
    let agents = bench.agents_of_folded(folded);
    let key = folded.key();
    let menu = project_menu(bench, folded, cx);
    let front = bench.front_copy(folded).cloned();
    let closing = (front.clone())
        .filter(|copy| {
            bench
                .view_at(&copy.place)
                .is_some_and(|view| view.workspace.projects.len() > 1)
        })
        .and_then(|copy| {
            let project = bench.project_on(&copy.place, &copy.project)?.clone();
            Some((copy.place, project))
        });
    let close = closing.map(|(place, project)| {
        ui::hover_actions(PROJECT_ROW)
            .top_0()
            .bottom_0()
            .child(ui::close_button(
                SharedString::from(format!("close-project-{key}")),
                t!("Close {name}", name = project.name),
                cx.listener(move |bench, _, _, cx| {
                    bench.close_project(place.clone(), project.clone(), cx)
                }),
                cx,
            ))
    });
    let shared = (front.as_ref()).and_then(|copy| {
        bench
            .project_on(&copy.place, &copy.project)?
            .shared
            .as_ref()
    });
    let mark = match shared.filter(|shared| !shared.name.is_empty()) {
        Some(shared) => ui::org_mark(&shared.name, &shared.org, 14.).into_any_element(),
        None => project_glyph(13., cx).into_any_element(),
    };
    let go = {
        let (front, identity) = (front, folded.identity.clone());
        cx.listener(move |bench, _, _, cx| {
            if let Some(front) = &front {
                bench.go_to_copy(front, identity.as_deref(), cx)
            }
        })
    };
    let name = folded.name.clone();
    let row = Row::new(
        SharedString::from(format!("project-{key}")),
        folded.name.clone(),
    )
    .mark(16., mark)
    .quiet()
    .bold(active)
    .chosen(active)
    .padding_right(4.)
    .when(close.is_some(), |row| row.hover_padding(PROJECT_ROW, 28.))
    .on_click(go)
    .child(faces(&agents, cx))
    .children(crate::copies::copy_note(bench, folded, cx))
    .children(crate::copies::clone_note(bench, folded, cx))
    .children(crate::copies::where_held(folded, cx))
    .status(rollup(bench, folded, cx))
    .into_any_element();
    let row = crate::drag::project(
        row,
        crate::drag::DraggedProject {
            id: key.clone(),
            label: folded.name.clone().into(),
        },
        &cx.entity(),
        cx,
    )
    .into_any_element();
    let row = div()
        .id(SharedString::from(format!("project-menu-{key}")))
        .group(PROJECT_ROW)
        .relative()
        .children(ui::spot(|| format!("project-row-{name}")))
        .context_menu(ui::menu(menu))
        .child(row)
        .children(close);
    let row = (bench.rails.keyed(Rail::Side)).row(format!("project-{key}"), name, row, cx);
    div().flex().flex_col().child(row).when(active, |tree| {
        tree.pb(SPACE_1)
            .child(surfaces(bench, lit_surface(bench), cx))
    })
}

/// The surface the project tree lights: the tab rail's page while it is open, unless it
/// lists no surface or the tab in front is a surface the tab rail has no page for, such as Git.
fn lit_surface(bench: &Workbench) -> Option<PaneKind> {
    let front = bench
        .tab()
        .and_then(|tab| tabs::first_view(&tab.pane))
        .map(|(kind, _)| kind);
    let paged = front.is_none_or(|kind| crate::pane_kind::page_of(kind).is_some());
    if !(bench.rails.tab_open && paged) {
        return front;
    }
    crate::pane_kind::listed_on(bench.rails.page).or(front)
}

/// The hover group of a project's row, whose × shows on hover.
const PROJECT_ROW: &str = "project-row";

/// What right-clicking a project offers: the settings of the copy the person works on, its
/// copies and a copy on each host without one, and the space the rail lists this host's copy
/// under.
fn project_menu(
    bench: &Workbench,
    folded: &Folded,
    cx: &Context<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let bench_entity = cx.weak_entity();
    let front = bench.front_copy(folded).cloned();
    let own = (folded.own.as_ref()).map(|own| own.id.clone());
    let clones: Vec<String> = folded.here().map(|project| project.id.clone()).collect();
    let identity = folded.identity.clone();
    let chosen = (bench.space_of(folded))
        .filter(|_| {
            folded.own.is_none()
                || (folded.here()).any(|project| project.space.is_some() && !project.space_followed)
        })
        .map(str::to_owned);
    let unchosen = bench.this_view().map_or_else(
        || t!("No Space").into(),
        |view| crate::space_editor::unchosen_label(&view.workspace, folded.here()),
    );
    let copies = crate::copies::copy_menu(bench, folded, cx);
    let spaces: Vec<(String, String, Option<Icon>)> =
        bench.this_view().map_or(Vec::new(), |view| {
            view.workspace
                .spaces
                .iter()
                .map(|space| {
                    (
                        space.id.clone(),
                        space.name.clone(),
                        Icon::named(&space.icon),
                    )
                })
                .collect()
        });
    move |popup, window, cx| {
        let act = |work: Rc<dyn Fn(&mut Workbench)>| {
            let bench = bench_entity.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = bench.update(cx, |bench, _| work(bench));
            }
        };
        let mut popup = match &front {
            Some(front) => {
                let front = front.clone();
                popup.item(
                    PopupMenuItem::new(t!("Playbook"))
                        .icon(Icon::Book)
                        .on_click(act(Rc::new(move |bench: &mut Workbench| {
                            bench.open_copy_settings(&front)
                        }))),
                )
            }
            None => popup,
        };
        popup = copies(popup.separator(), window, cx);
        if own.is_none() && identity.is_none() {
            return popup;
        }
        let move_to = |space: Option<String>| {
            let (own, identity, clones) = (own.clone(), identity.clone(), clones.clone());
            act(Rc::new(move |bench: &mut Workbench| {
                match (&own, &identity) {
                    (Some(_), _) => {
                        for id in &clones {
                            crate::core::Target::here(&bench.asks)
                                .call_typed::<call::UpdateProject>(UpdateProjectParams::space(
                                    id.clone(),
                                    space.clone(),
                                ))
                        }
                    }
                    (None, Some(identity)) => bench.set_elsewhere_space(identity, space.clone()),
                    (None, None) => {}
                }
            }))
        };
        popup = popup.separator();
        for (space, name, glyph) in &spaces {
            let mut item = PopupMenuItem::new(name.clone())
                .checked(chosen.as_ref() == Some(space))
                .on_click(move_to(Some(space.clone())));
            if let Some(glyph) = glyph {
                item = item.icon(*glyph);
            }
            popup = popup.item(item);
        }
        if let Some(id) = &own {
            let new_space = {
                let (bench, id) = (bench_entity.clone(), id.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let _ = bench.update(cx, |bench, cx| bench.new_space(Some(id.clone()), cx));
                }
            };
            popup = popup.item(
                PopupMenuItem::new(t!("New Space…"))
                    .icon(Icon::Plus)
                    .on_click(new_space),
            );
        }
        popup.separator().item(
            PopupMenuItem::new(unchosen.clone())
                .icon(Icon::Move)
                .checked(chosen.is_none())
                .on_click(move_to(None)),
        )
    }
}

/// Up to five agents' faces, each overlapping the one before by 6px, then how many more.
fn faces(agents: &[AgentRow], cx: &App) -> impl IntoElement {
    let shown = agents.len().min(5);
    let more = agents.len() - shown;
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(3.))
        .child(
            div()
                .relative()
                .flex_none()
                .h(px(20.))
                .w(px(if shown == 0 {
                    0.
                } else {
                    20. + 14. * (shown - 1) as f32
                }))
                .children(agents.iter().take(shown).enumerate().map(|(index, agent)| {
                    ui::mark_box(20., agent.mark(20., cx))
                        .absolute()
                        .left(px(14. * index as f32))
                })),
        )
        .when(more > 0, |faces| {
            faces.child(
                ui::text(format!("+{more}"), TEXT_3XS, FontWeight::MEDIUM)
                    .font_features(crate::theme::tabular())
                    .text_color(t.text_tertiary),
            )
        })
}

/// How a project's open agents stand together on every host that holds it: amber while the core
/// says it needs the person, else as each core rolls its agents up; none while all are idle.
fn rollup(bench: &Workbench, folded: &Folded, cx: &App) -> Option<gpui_kit::AnyElement> {
    let key = format!("project:{}", folded.key());
    let needs_you = (folded.copies.iter())
        .filter_map(|copy| bench.project_on(&copy.place, &copy.project))
        .any(|project| project.standing == ProjectStanding::NeedsYou);
    if needs_you {
        return ui::status_mark(&key, AgentStatus::Blocked, cx);
    }
    let group = (folded.copies.iter())
        .filter_map(|copy| {
            let view = bench.view_at(&copy.place)?;
            Some(view.inbox.projects.get(&copy.project)?.group)
        })
        .filter(|group| *group != AgentGroup::Blocked)
        .min();
    let status = match group {
        Some(AgentGroup::Working) => AgentStatus::Working,
        Some(AgentGroup::Done) => AgentStatus::Done,
        _ => return None,
    };
    ui::status_mark(&key, status, cx)
}

/// The project's surfaces, its pages and its Playbook, hung off a spine with a tick to each, the
/// one in front lit.
fn surfaces(
    workbench: &Workbench,
    front: Option<PaneKind>,
    cx: &Context<Workbench>,
) -> impl IntoElement {
    let keyed = workbench.rails.keyed(Rail::Side);
    let bench = cx.entity().downgrade();
    let surfaces = crate::pane_kind::SURFACES
        .into_iter()
        .enumerate()
        .map(|(index, surface)| {
            let (kind, command) = (surface.kind, surface.command);
            let bench = bench.clone();
            let row = tree_row(
                SharedString::from(format!("surface-{index}")),
                crate::pane_kind::of(kind).icon,
                surface.label,
                Some(kind) == front,
                false,
                keymap::hint(command, cx),
                cx,
            )
            .clicks(move |_, window, cx| {
                if let Some(page) = crate::pane_kind::page_of(kind) {
                    let _ = bench.update(cx, |bench, cx| {
                        if bench.rails.tab_open {
                            bench.show_rail_page(page, cx);
                        }
                    });
                }
                keymap::dispatch(command, window, cx);
            });
            keyed.row(
                format!("surface-{index}"),
                crate::i18n::tr(surface.label),
                row,
                cx,
            )
        });
    let pages = crate::workbench::actions::FocusPages::ID;
    let pages = tree_row(
        "surface-pages".into(),
        Icon::Presentation,
        n!("Artifacts"),
        front == Some(PaneKind::Pages),
        false,
        keymap::hint(pages, cx),
        cx,
    )
    .clicks(move |_, window, cx| keymap::dispatch(pages, window, cx));
    let pages = keyed.row("surface-pages", t!("Artifacts"), pages, cx);
    let playbook = crate::workbench::actions::FocusPlaybook::ID;
    let playbook = tree_row(
        "surface-playbook".into(),
        Icon::Book,
        n!("Playbook"),
        front == Some(PaneKind::ProjectSettings),
        true,
        keymap::hint(playbook, cx),
        cx,
    )
    .clicks(move |_, window, cx| keymap::dispatch(playbook, window, cx));
    let playbook = keyed.row("surface-playbook", t!("Playbook"), playbook, cx);
    div()
        .flex()
        .flex_col()
        .ml(px(13.5))
        .py(SPACE_0)
        .pr(SPACE_1H)
        .children(surfaces)
        .child(pages)
        .child(playbook)
}

/// One row of a project's tree; the `tail` ends the spine.
fn tree_row(
    id: SharedString,
    glyph: Icon,
    label: &'static str,
    lit: bool,
    tail: bool,
    keys: Option<SharedString>,
    cx: &Context<Workbench>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    let spine = div().absolute().left_0().top_0();
    div()
        .id(id)
        .relative()
        .h(px(25.))
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(radius_2())
        .text_color(if lit { t.accent } else { t.text_tertiary })
        .when(!lit, |row| {
            row.hover(move |style| style.text_color(t.text_primary))
        })
        .child(if tail {
            spine
                .w(px(9.))
                .h(px(13.))
                .border_l(crate::theme::border())
                .border_b(crate::theme::border())
                .border_color(t.tree_spine)
                .rounded_bl(px(4.))
        } else {
            spine.bottom_0().w(px(1.)).bg(t.tree_spine)
        })
        .when(!tail, |row| {
            row.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(12.))
                    .w(px(9.))
                    .h(px(1.))
                    .bg(t.tree_tick),
            )
        })
        .child(div().w(px(9.)).flex_none())
        .child(ui::mark_box(
            16.,
            ui::icon(glyph, 13.).text_color(if lit { t.live } else { t.text_tertiary }),
        ))
        .child(ui::text(crate::i18n::tr(label), TEXT_XS_H, FontWeight::MEDIUM).letter_spacing(0.01))
        .when_some(keys, |row, keys| {
            let keys = ui::kbd(keys, cx).when(lit, |keys| keys.text_color(t.accent));
            row.child(
                div()
                    .ml_auto()
                    .when(!lit, |keys| keys.opacity(0.5))
                    .child(keys),
            )
        })
}

/// The spaces the person sorts projects into, which filter the projects below them.
fn space_switch(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let spaces = bench
        .this_view()
        .map_or(&[][..], |view| view.workspace.spaces.as_slice());
    if spaces.is_empty() {
        return div()
            .mb(SPACE_1H)
            .child(
                Button::new("new-space", t!("New space"), Variant::Dashed)
                    .icon(Icon::Plus)
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_space(None, cx))),
            )
            .into_any_element();
    }
    let chosen = bench.space.clone();
    let chip = |id: Option<String>, glyph: Option<Icon>, name: SharedString| {
        let on = chosen == id;
        let chip = div()
            .id(SharedString::from(format!(
                "space-{}",
                id.as_deref().unwrap_or("all")
            )))
            .flex_grow(1.)
            .flex_shrink_0()
            .flex()
            .justify_center()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(SPACE_2)
            .rounded(radius_2())
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .whitespace_nowrap()
            .text_color(if on { t.text_primary } else { t.text_tertiary })
            .when(on, |chip| chip.bg(t.surface_active))
            .hover(move |style| style.text_color(t.text_primary))
            .children(glyph.map(|glyph| {
                ui::icon(glyph, 12.).text_color(if on { t.text_primary } else { t.text_tertiary })
            }))
            .child(
                div()
                    .max_w(px(140.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .clicks({
                let id = id.clone();
                cx.listener(move |bench, event: &ClickEvent, _, cx| {
                    match (event.click_count(), &id) {
                        (2, Some(space)) => bench.rename_space(space.clone(), cx),
                        _ => bench.show_space(id.clone(), cx),
                    }
                })
            });
        match id {
            Some(space) => {
                let bench = cx.entity();
                let icon = spaces
                    .iter()
                    .find(|known| known.id == space)
                    .map(|known| known.icon.clone())
                    .unwrap_or_default();
                chip.context_menu(ui::menu(move |popup, window, cx| {
                    space_menu(popup, window, cx, &bench, &space, &icon)
                }))
                .into_any_element()
            }
            None => chip.into_any_element(),
        }
    };
    let track = div()
        .id("space-track")
        .flex()
        .gap(SPACE_0)
        .p(SPACE_0)
        .overflow_x_scroll()
        .border(crate::theme::border())
        .border_color(t.border)
        .rounded(radius_2() + px(3.))
        .context_menu(ui::menu({
            let bench = cx.entity();
            move |popup, _, _| {
                let (made, edited) = (bench.clone(), bench.clone());
                popup
                    .item(
                        PopupMenuItem::new(t!("New Space…"))
                            .icon(Icon::Plus)
                            .on_click(move |_, _, cx| {
                                made.update(cx, |bench, cx| bench.new_space(None, cx));
                            }),
                    )
                    .item(
                        PopupMenuItem::new(t!("Edit Spaces…"))
                            .icon(Icon::Wrench)
                            .on_click(move |_, _, cx| {
                                edited.update(cx, |bench, cx| bench.open_space_editor(cx));
                            }),
                    )
            }
        }))
        .child(chip(None, Some(Icon::Stack), t!("All").into()))
        .children(spaces.iter().map(|space| {
            chip(
                Some(space.id.clone()),
                Icon::named(&space.icon),
                space.name.clone().into(),
            )
        }));
    let width = bench.rail_width(Rail::Side) - 20.;
    div()
        .mb(SPACE_1H)
        .child(track)
        .child(crate::space_editor::anchor(bench, width, cx))
        .into_any_element()
}

/// What a right click on a space offers, as the Mac's space switch does.
fn space_menu(
    popup: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    bench: &Entity<Workbench>,
    space: &str,
    icon: &str,
) -> PopupMenu {
    let item = |label: &'static str,
                icon: Icon,
                act: fn(&mut Workbench, String, &mut Context<Workbench>)| {
        let (bench, space) = (bench.clone(), space.to_owned());
        PopupMenuItem::new(label)
            .icon(icon)
            .on_click(move |_, _, cx| {
                bench.update(cx, |bench, cx| act(bench, space.clone(), cx));
            })
    };
    let icons = {
        let (bench, space, worn) = (bench.downgrade(), space.to_owned(), icon.to_owned());
        move |submenu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
            crate::space_editor::icon_choices(submenu, bench.clone(), &space, &worn)
        }
    };
    let mut popup = popup
        .item(item(t!("Rename…"), Icon::Pencil, |bench, space, cx| {
            bench.rename_space(space, cx)
        }))
        .submenu_with_icon(
            Some(Icon::Pencil.into()),
            t!("Change Icon…"),
            window,
            cx,
            ui::menu(icons),
        );
    if !icon.is_empty() {
        popup = popup.item(item(t!("Remove Icon"), Icon::Trash, |bench, space, _| {
            bench.set_space_icon(&space, "")
        }));
    }
    popup
        .separator()
        .item(item(t!("New Space…"), Icon::Plus, |bench, _, cx| {
            bench.new_space(None, cx)
        }))
        .item(item(t!("Edit Spaces…"), Icon::Wrench, |bench, _, cx| {
            bench.open_space_editor(cx)
        }))
        .separator()
        .item(
            ui::MenuRow::new(t!("Delete Space…"))
                .icon(Icon::Trash)
                .danger(true)
                .item(cx)
                .on_click({
                    let (bench, space) = (bench.clone(), space.to_owned());
                    move |_, _, cx| {
                        bench.update(cx, |bench, cx| bench.delete_space(space.clone(), cx))
                    }
                }),
        )
}

/// Who the host is signed in as, or the card that signs it in.
fn account(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let Some(view) = bench.this_view() else {
        return foot(&t)
            .justify_end()
            .child(settings_button())
            .into_any_element();
    };
    let Some(account) = view.account.as_deref() else {
        return foot(&t).child(sign_in(bench, cx)).into_any_element();
    };
    let who = account
        .name
        .clone()
        .unwrap_or_else(|| t!("Signed in").into());
    let row = Row::new("account", who)
        .mark(24., avatar(bench, account, 24., &t))
        .centered();
    use crate::settings::Page;
    let settings_keys = keymap::hint(actions::ToggleSettings::ID, cx);
    let act = |act: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
        Rc::new(cx.listener(move |bench, _: &ClickEvent, window, cx| act(bench, window, cx)))
    };
    let items = [
        (
            n!("Manage account"),
            Icon::User,
            Some(Trailing::Icon(Icon::ExternalLink)),
            act(|_, _, cx| cx.open_url(&sikemux_corekit::accounts::app_base())),
        ),
        (
            n!("Devices"),
            Icon::Laptop,
            None,
            act(|bench, window, cx| {
                if bench.settings.read(cx).is_open() {
                    crate::settings::close(bench, window, cx);
                }
                keymap::count(actions::FocusHosts::ID.name(), cx);
                bench.open_hosts(None);
            }),
        ),
        (
            n!("Connectors"),
            Icon::Plug,
            None,
            act(|bench, window, cx| bench.open_settings(Page::Connectors, window, cx)),
        ),
        (
            n!("Settings"),
            Icon::Settings,
            settings_keys.map(Trailing::Keys),
            act(|bench, window, cx| {
                keymap::count(actions::ToggleSettings::ID.name(), cx);
                bench.open_settings(Page::default(), window, cx)
            }),
        ),
    ];
    let sign_out = act(|bench, _, _| bench.sign_out());
    let menu_width = px(bench.rail_width(Rail::Side)) - SPACE_2H * 2. - px(2.);
    let menu = ui::menu_button(
        "account-menu",
        Anchor::BottomLeft,
        row,
        move |mut popup, _, _| {
            popup = popup.min_w(menu_width).max_w(menu_width);
            for (label, icon, trailing, act) in items.iter().cloned() {
                popup = popup.item(
                    account_item(label, icon, trailing)
                        .on_click(move |event, window, cx| act(event, window, cx)),
                );
            }
            let sign_out = sign_out.clone();
            popup.separator().item(
                account_item(n!("Sign out"), Icon::LogOut, None)
                    .on_click(move |event, window, cx| sign_out(event, window, cx)),
            )
        },
    );
    foot(&t)
        .child(div().flex_1().min_w_0().child(menu))
        .into_any_element()
}

/// The rail's last band, under a line, that holds the account row.
fn foot(t: &Tokens) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .px(SPACE_2H)
        .py(SPACE_1H)
        .border_t(crate::theme::border())
        .border_color(t.border)
}

/// A row of the account menu, roomier than other menus' rows since it opens the width of the rail.
#[derive(Clone)]
enum Trailing {
    Keys(SharedString),
    Icon(Icon),
}

fn account_item(label: &'static str, icon: Icon, trailing: Option<Trailing>) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        let t = Tokens::get(cx);
        div()
            .id(label)
            .aria_label(crate::i18n::tr(label))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .py(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_SM))
            .child(ui::icon(icon, 16.).text_color(t.text_secondary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(crate::i18n::tr(label)),
            )
            .children(trailing.clone().map(|trailing| {
                match trailing {
                    Trailing::Keys(keys) => div()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(keys)
                        .into_any_element(),
                    Trailing::Icon(icon) => ui::icon(icon, 13.)
                        .text_color(t.text_tertiary)
                        .into_any_element(),
                }
            }))
    })
}

/// Settings, in the rail's foot until the person signs in and it moves into the account menu.
fn settings_button() -> IconButton {
    IconButton::new("settings", Icon::Settings, t!("Settings"))
        .glyph(14.)
        .width(28.)
        .on_click(|_, window, cx| {
            cx.stop_propagation();
            keymap::dispatch(actions::ToggleSettings::ID, window, cx)
        })
}

/// The account's picture in a circle, else its initials.
pub(crate) fn avatar(bench: &Workbench, account: &AccountInfo, size: f32, t: &Tokens) -> Div {
    let initials = initials(account.name.as_deref(), account.email.as_deref());
    face(size, t)
        .text_size(px(size * 0.375))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_secondary)
        .map(|face| match bench.picture() {
            Some(picture) => face.child(gpui_kit::img(picture).size_full().rounded_full()),
            None => face.child(initials),
        })
}

/// The circle an account's picture sits in, and the sign-in mark in its place.
fn face(size: f32, t: &Tokens) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_raised)
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
}

/// The first letters of the first and last words of the name, else the email's first.
fn initials(name: Option<&str>, email: Option<&str>) -> String {
    let words: Vec<&str> = name.unwrap_or_default().split_whitespace().collect();
    let first = |word: &str| {
        word.chars()
            .next()
            .map(|letter| letter.to_uppercase().to_string())
            .unwrap_or_default()
    };
    match words.as_slice() {
        [] => email.map(first).unwrap_or_default(),
        [only] => first(only),
        [head, .., tail] => first(head) + &first(tail),
    }
}

/// The account row while no one is signed in: it signs in, and its gear opens Settings.
fn sign_in(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx);
    let mark = face(24., t).child(ui::icon(Icon::LogIn, 13.).text_color(t.accent));
    let row = match bench.signing_in {
        true => Row::new("account", t!("Finish signing in in your browser"))
            .detail(t!("Cancel"))
            .on_click(cx.listener(|bench, _, _, cx| {
                bench.cancel_sign_in();
                cx.notify();
            })),
        false => Row::new("account", t!("Sign in")).on_click(cx.listener(|bench, _, _, cx| {
            bench.sign_in(cx);
        })),
    };
    div().flex_1().min_w_0().child(
        row.mark(24., mark)
            .centered()
            .padding_right(2.)
            .child(settings_button()),
    )
}

/// `rail` taking the keys, whose arrows walk the rows it lists.
pub(crate) fn keyed_rail(
    rail: Div,
    which: Rail,
    bench: &Workbench,
    cx: &Context<Workbench>,
) -> Div {
    fn on<A: gpui_kit::Action>(
        which: Rail,
        cx: &Context<Workbench>,
    ) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
        let this = cx.weak_entity();
        move |key: &A, window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| {
                if which.of_mut(&mut bench.rails.keyed).key(key, window, cx) {
                    bench.redraw_rail(which, cx);
                }
            })
            .ok();
        }
    }
    rail.track_focus(&bench.rails.keyed(which).focus)
        .key_context(keyed::CONTEXT)
        .on_action(on::<keyed::Previous>(which, cx))
        .on_action(on::<keyed::Next>(which, cx))
        .on_action(on::<keyed::First>(which, cx))
        .on_action(on::<keyed::Last>(which, cx))
        .on_action(on::<keyed::Open>(which, cx))
        .on_action(on::<keyed::ShowMenu>(which, cx))
        .on_action(
            cx.listener(|bench, _: &keyed::Leave, window, cx| bench.keys_to_stage(window, cx)),
        )
}

impl Workbench {
    pub(crate) fn redraw_rail(&self, rail: Rail, cx: &mut App) {
        let part = match rail {
            Rail::Side => &self.parts.side,
            Rail::Agents => &self.parts.agents,
        };
        self.parts.redraw(part, cx);
    }

    /// Gives the keys to the view in front, else the window.
    pub(crate) fn keys_to_stage(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focused_view() {
            Some(view) => self.focus_view(&view, window, cx),
            None => self.keys.focus.focus(window, cx),
        }
    }

    /// Moves the keys `by` one area along the window's, in the order the skeleton reads them,
    /// leaving out a closed rail, and around.
    pub(crate) fn step_area(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let areas: Vec<Option<Rail>> = (self.skeleton.areas())
            .filter(|area| area.is_none_or(|rail| self.rails.open(rail)))
            .collect();
        if areas.is_empty() {
            return;
        }
        let now = [Rail::Side, Rail::Agents]
            .into_iter()
            .find(|rail| self.rails.keyed(*rail).focus.contains_focused(window, cx));
        let at = areas.iter().position(|area| *area == now).unwrap_or(0);
        let to = areas.get((at as isize + by).rem_euclid(areas.len() as isize) as usize);
        match to.copied().flatten() {
            Some(rail) => {
                rail.of_mut(&mut self.rails.keyed).take_keys(window, cx);
                self.redraw_rail(rail, cx);
            }
            None => self.keys_to_stage(window, cx),
        }
    }

    /// Keeps a rail open, or closes it; a peeking rail that is kept stays where it is.
    pub fn toggle_rail(&mut self, rail: Rail, cx: &mut Context<Self>) {
        let open = match rail {
            Rail::Side => &mut self.rails.side_open,
            Rail::Agents => &mut self.rails.tab_open,
        };
        *open = !*open;
        self.rails.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    /// How wide `rail` is drawn, which a narrow window can make less than the width kept.
    pub fn rail_width(&self, rail: Rail) -> f32 {
        *rail.of(&self.rails.drawn)
    }

    fn kept_width(&self, rail: Rail) -> f32 {
        match rail {
            Rail::Side => self.rails.side_width,
            Rail::Agents => self.rails.tab_width,
        }
    }

    /// Sets a rail's width, within what that rail allows, and draws it at once.
    pub fn resize_rail(
        &mut self,
        rail: Rail,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = rail.widths();
        let kept = match rail {
            Rail::Side => &mut self.rails.side_width,
            Rail::Agents => &mut self.rails.tab_width,
        };
        let width = width.clamp(range.min, range.max).round();
        if *kept != width {
            *kept = width;
            window.refresh();
            self.keep_rails(cx);
        }
    }

    /// Moves `rail`'s edge beside the stage `by` points to the right.
    pub(crate) fn nudge_rail(
        &mut self,
        rail: Rail,
        by: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outward = match self.skeleton.peek_edge(rail) {
            Edge::Left => by,
            Edge::Right => -by,
        };
        self.resize_rail(rail, self.kept_width(rail) + outward, window, cx);
    }

    /// The handle between a rail and the stage, dragged or moved by arrow keys to resize the rail.
    pub(crate) fn rail_resizer(&self, rail: Rail, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::keymap::{EdgeFarLeft, EdgeFarRight, EdgeLeft, EdgeRight};
        let id = match rail {
            Rail::Side => "resize-side-rail",
            Rail::Agents => "resize-tab-rail",
        };
        crate::panes::handle(id, DraggedRail(rail), true)
            .key_context("RailResizer")
            .track_focus(rail.of(&self.rails.resizers))
            .clicks_keep_cursor(cx.listener(move |bench, _, window, cx| {
                rail.of(&bench.rails.resizers).focus(window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeLeft, window, cx| {
                bench.nudge_rail(rail, -16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeRight, window, cx| {
                bench.nudge_rail(rail, 16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarLeft, window, cx| {
                bench.nudge_rail(rail, -40., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarRight, window, cx| {
                bench.nudge_rail(rail, 40., window, cx)
            }))
    }

    /// Takes the rails as the person last left them, from the core's preferences.
    pub(crate) fn follow_rails(&mut self, preferences: &Preferences) {
        self.rails.follow(preferences);
        self.recent_commands = preferences
            .recent_command_keys
            .iter()
            .map(|key| SharedString::from(key.clone()))
            .collect();
    }

    /// Keeps the rails and the commands run lately in the core's preferences once changes
    /// settle, so a drag is written once, and the rails come back as they were. Nothing is
    /// kept before the preferences are first read, when the rails hold only the window's defaults.
    pub fn keep_rails(&mut self, cx: &mut Context<Self>) {
        if self.preferences.is_none() {
            return;
        }
        self.rails.keeping = Some(cx.spawn(async move |bench, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = bench.update(cx, |bench, _| {
                bench.rails.keeping = None;
                bench.write_rails();
            });
        }));
    }

    pub(crate) fn write_rails(&self) {
        let page = self.rails.page;
        let scope = if self.rails.all_projects {
            TabRailScope::All
        } else {
            TabRailScope::Project
        };
        let recent: Vec<String> = self
            .recent_commands
            .iter()
            .map(ToString::to_string)
            .collect();
        let now = [
            PreferenceChange::SideRailOpen(self.rails.side_open),
            PreferenceChange::SideRailWidth(self.rails.side_width.into()),
            PreferenceChange::TabRailOpen(self.rails.tab_open),
            PreferenceChange::TabRailWidth(self.rails.tab_width.into()),
            PreferenceChange::TabRailPage(page),
            PreferenceChange::TabRailScope(scope),
            PreferenceChange::TabRailAllAgents(self.rails.all_harnesses),
            PreferenceChange::FoldedRailGroups(self.rails.folded_groups.clone()),
            PreferenceChange::RecentCommandKeys(recent),
        ];
        let kept = self.preferences.as_ref();
        for change in now {
            if !kept.is_some_and(|kept| change.is_in(kept)) {
                crate::settings::set(self, change);
            }
        }
    }

    /// The order the side rail lists projects in.
    pub(crate) fn project_sort(&self) -> ProjectSort {
        self.preferences
            .as_ref()
            .map_or_else(ProjectSort::default, |preferences| preferences.project_sort)
    }

    /// Folds the rail group `key` away, or opens it again, and keeps that.
    pub fn fold_group(&mut self, key: &str, cx: &mut Context<Self>) {
        let folded = &mut self.rails.folded_groups;
        match folded.iter().position(|kept| kept == key) {
            Some(at) => {
                folded.remove(at);
            }
            None => folded.push(key.to_owned()),
        }
        self.keep_rails(cx);
        cx.notify();
    }

    /// Opens both rails when either is closed, else closes both.
    pub fn toggle_focus_mode(&mut self, cx: &mut Context<Self>) {
        let open = !(self.rails.side_open && self.rails.tab_open);
        self.rails.side_open = open;
        self.rails.tab_open = open;
        self.rails.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    /// Shows a closed rail over the stage while the pointer is at its edge or over it, and puts
    /// it away a moment after the pointer leaves.
    pub(crate) fn peek(&mut self, rail: Rail, inside: bool, cx: &mut Context<Self>) {
        if inside {
            self.rails.unpeek = None;
            if self.rails.peeking != Some(rail) {
                self.rails.peeking = Some(rail);
                cx.notify();
            }
            return;
        }
        self.rails.unpeek = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PEEK_CLOSE).await;
            let _ = this.update(cx, |bench, cx| {
                if bench.rails.peeking == Some(rail) {
                    bench.rails.peeking = None;
                    cx.notify();
                }
            });
        }));
    }

    /// A docked rail and its handle, `tuck` of the way under the stage, the handle after it
    /// when `handle_after`. It keeps its width as it goes, so nothing in it wraps again; the
    /// stage takes the room it gives up.
    pub(crate) fn docked_rail(
        &self,
        rail: Rail,
        tuck: f32,
        handle_after: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = self.rail_width(rail);
        let drawn = crate::settings::leaves(div().h_full().flex_none(), cx).child(
            self.rail_part(rail)
                .clone()
                .cached(StyleRefinement::default().w(px(width)).h_full().flex_none()),
        );
        let drawer = div()
            .id(match rail {
                Rail::Side => "side-rail-drawer",
                Rail::Agents => "tab-rail-drawer",
            })
            .flex_none()
            .h_full()
            .w(px((width + crate::theme::gap()) * (1. - tuck)))
            .overflow_hidden()
            .flex()
            .opacity(1. - tuck)
            .on_drag_move(cx.listener(
                move |bench, event: &DragMoveEvent<DraggedRail>, window, cx| {
                    if event.drag(cx).0 != rail {
                        return;
                    }
                    let (x, bounds) = (event.event.position.x, event.bounds);
                    let width = match handle_after {
                        true => x - bounds.left(),
                        false => bounds.right() - x,
                    };
                    let width = f32::from(width) - crate::theme::gap() / 2.;
                    bench.resize_rail(rail, width, window, cx);
                },
            ));
        match handle_after {
            true => drawer.child(drawn).child(self.rail_resizer(rail, cx)),
            false => drawer
                .justify_end()
                .child(self.rail_resizer(rail, cx))
                .child(drawn),
        }
    }

    fn rail_part(&self, rail: Rail) -> &Entity<crate::workbench::Part> {
        match rail {
            Rail::Side => &self.parts.side,
            Rail::Agents => &self.parts.agents,
        }
    }

    /// A closed rail's edge of the window, which brings the rail out over the stage.
    pub(crate) fn rail_edge(
        &self,
        rail: Rail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = self.kept_width(rail);
        let edge = self.skeleton.peek_edge(rail);
        let peeking = self.rails.peeking == Some(rail);
        let shown = motion::follow(
            match rail {
                Rail::Side => "peek-side-shown",
                Rail::Agents => "peek-agents-shown",
            },
            if peeking { 1. } else { 0. },
            if peeking {
                motion::PEEK_IN
            } else {
                motion::PEEK_OUT
            },
            window,
            cx,
        );
        let away = (width + PEEK_GAP) * (1. - shown);
        let zone = div()
            .id(match rail {
                Rail::Side => "peek-side",
                Rail::Agents => "peek-agents",
            })
            .absolute()
            .top(px(tabs::bar_height() + crate::skeleton::room_beside_bar()))
            .bottom(px(crate::theme::inset()))
            .w(px(if shown > 0. { width + 20. } else { PEEK_REACH }))
            .px(SPACE_2H)
            .on_hover(cx.listener(move |bench, inside: &bool, _, cx| bench.peek(rail, *inside, cx)))
            .when(shown > 0., |zone| {
                zone.child(
                    div()
                        .h_full()
                        .relative()
                        .left(px(match edge {
                            Edge::Left => -away,
                            Edge::Right => away,
                        }))
                        .opacity(shown)
                        .child(
                            crate::settings::leaves(div().h_full(), cx).child(
                                self.rail_part(rail)
                                    .clone()
                                    .cached(StyleRefinement::default().w(px(width)).h_full()),
                            ),
                        ),
                )
            });
        match edge {
            Edge::Left => zone.left_0(),
            Edge::Right => zone.right_0(),
        }
    }
}
