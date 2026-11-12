use crate::workbench::actions;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::prefs::PreferenceChange;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity, Window,
};
use sikemux_core::server::hosts::HostFound;
use sikemux_core::server::hosts::HostReach;
use sikemux_core::server::hosts::HostRow;
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
    Tokens, RADIUS_1, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS,
    TEXT_3XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{Button, Group, IconButton, Row, Tone, Variant};
use crate::workbench::{Strip, Workbench};
use crate::{agents, keymap, motion, rail, tabs, ui};
use sikemux_core::server::call;
use sikemux_core::server::workspace::UpdateProjectParams;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// How wide a rail can be dragged, and how wide it starts.
#[derive(Clone, Copy)]
pub struct RailWidth {
    pub min: f32,
    pub max: f32,
    pub initial: f32,
}

/// A rail's handle while it is dragged.
pub(crate) struct DraggedRail(pub(crate) Edge);

/// How far past its own width a peeking rail starts and ends, out of sight beyond the window.
const PEEK_GAP: f32 = 10.;

impl Render for DraggedRail {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The handle between a docked rail and the stage.
const RESIZER_WIDTH: f32 = 10.;

/// How long a peeking rail stays after the pointer leaves it.
const PEEK_CLOSE: Duration = Duration::from_millis(180);

/// How close to the window's edge the pointer brings out a closed rail.
const PEEK_REACH: f32 = 28.;

impl Rails {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        Self {
            side_open: true,
            tab_open: true,
            side_width: SIDE_RAIL.initial,
            tab_width: crate::agents::TAB_RAIL.initial,
            resizers: [cx.focus_handle(), cx.focus_handle()],
            peeking: None,
            unpeek: None,
            page: TabRailPage::Agents,
            copy_leaves_uncommitted: false,
            copy_notes: HashMap::new(),
            keeping: None,
            all_projects: false,
            all_harnesses: false,
            folded_teams: HashSet::new(),
            folded_groups: Vec::new(),
            seen: Default::default(),
            scroll: Default::default(),
            glide: Default::default(),
            frame: Rc::default(),
            slide: crate::track::Slide::default(),
        }
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
    /// The handles beside the rails, which arrow keys move once focused.
    pub(crate) resizers: [gpui_kit::FocusHandle; 2],
    pub(crate) peeking: Option<Edge>,
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
        Some(space) => format!("no projects in {}", space.name),
        None => "no projects".to_owned(),
    };
    ui::panel(cx)
        .w(px(bench.rail_width(Edge::Start)))
        .flex_none()
        .child(masthead(bench, cx))
        .child(
            div()
                .id("side-rail-scroll")
                .flex_1()
                .min_h_0()
                .pt(SPACE_2H)
                .px(SPACE_2H)
                .pb(SPACE_4)
                .overflow_y_scroll()
                .child(
                    Group::new("Projects", empty)
                        .first()
                        .actions(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_0)
                                .children((listed.len() > 1).then(|| project_sort(bench, cx)))
                                .child(actions(
                                    keymap::hint(actions::OpenProject::ID, cx),
                                    [IconButton::new("add-project", Icon::Plus, "Open project")
                                        .on_click(|_, window, cx| {
                                            keymap::dispatch(actions::OpenProject::ID, window, cx)
                                        })],
                                    cx,
                                )),
                        )
                        .toolbar((!listed.is_empty()).then(|| space_switch(bench, cx)))
                        .children(projects),
                )
                .child(
                    Group::new("Hosts", "no hosts")
                        .actions(actions(
                            keymap::hint(actions::OpenHost::ID, cx),
                            [
                                IconButton::new("hosts-overview", Icon::Gauge, hosts_overview(cx))
                                    .on_click(|_, window, cx| {
                                        keymap::dispatch(actions::FocusHosts::ID, window, cx)
                                    }),
                                IconButton::new("edit-ssh", Icon::Pencil, "Edit ~/.ssh/config")
                                    .on_click(cx.listener(|bench, _, _, cx| {
                                        bench.edit_ssh_config(None, cx)
                                    })),
                                IconButton::new("add-host", Icon::Plus, "Find a host").on_click(
                                    |_, window, cx| {
                                        keymap::dispatch(actions::OpenHost::ID, window, cx)
                                    },
                                ),
                            ],
                            cx,
                        ))
                        .child(this_host(bench, cx))
                        .children(hosts(bench, cx)),
                )
                .child(
                    Group::new("Mods", "no mods")
                        .actions(actions(
                            None,
                            [
                                IconButton::new("add-mod", Icon::Plus, "Make a mod with an agent")
                                    .on_click(
                                        cx.listener(|bench, _, _, cx| bench.ask_for_mod(None, cx)),
                                    ),
                            ],
                            cx,
                        ))
                        .children(mods(bench, cx)),
                )
                .child(
                    Group::new("Terminals", "no terminals")
                        .actions(actions(
                            None,
                            [
                                IconButton::new("add-terminal", Icon::Plus, "New terminal")
                                    .on_click(
                                        cx.listener(|bench, _, _, cx| bench.new_loose_terminal(cx)),
                                    ),
                            ],
                            cx,
                        ))
                        .children(terminals(bench, cx)),
                ),
        )
        .children(crate::updates::rail_chip(bench, cx))
        .child(account(bench, cx))
}

/// The Hosts header's gauge, naming its shortcut.
fn hosts_overview(cx: &App) -> String {
    match keymap::hint(actions::FocusHosts::ID, cx) {
        Some(keys) => format!("Hosts overview — {keys}"),
        None => "Hosts overview".to_owned(),
    }
}

/// "Host details" at the top of a host's menu, opening the Hosts page on `host`.
fn details_item(bench: WeakEntity<Workbench>, host: String) -> PopupMenuItem {
    PopupMenuItem::new("Host details")
        .icon(Icon::Gauge)
        .on_click(move |_, _, cx| {
            let host = host.clone();
            let _ = bench.update(cx, |bench, _| {
                bench.open_hosts(Some(host));
            });
        })
}

/// The host this window runs on, where new work starts.
fn this_host(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let agents = bench
        .this_view()
        .map_or(0, |view| view.workspace.agents.len());
    let alert = bench.host_alert(None, true).cloned();
    let alert = alert.as_ref();
    let row = Row::new("this-host", "This host")
        .mark(
            16.,
            ui::icon(Icon::Laptop, 14.)
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
            row.detail(format!(
                "{agents} {}",
                if agents == 1 { "agent" } else { "agents" }
            ))
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
                PopupMenuItem::new("New terminal here")
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

/// A host's row with its menu on a right-click and on a ⋯ that shows while the pointer is
/// over the row, as main's host rows have it.
fn with_menu(
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
        .group(HOST_ROW)
        .context_menu(ui::menu(move |popup, window, cx| menu(popup, window, cx)))
        .child(row.hover_padding(HOST_ROW, 32.))
        .child(
            ui::hover_actions(HOST_ROW).top_0().bottom_0().child(
                KitButton::new(SharedString::from(format!("{id}-more")))
                    .ghost()
                    .compact()
                    .rounded(RADIUS_2)
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
const HOST_ROW: &str = "host-row";

/// "Require approval for new devices" on `host`, this one when none, ticked while it does.
fn approval_item(
    bench: WeakEntity<Workbench>,
    host: Option<String>,
    approval: Option<bool>,
) -> PopupMenuItem {
    PopupMenuItem::new("Require approval for new devices")
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
    let t = Tokens::get(cx).clone();
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
                    Some("needs the signed app".to_owned())
                }
                HostReach::Away if linked => Some("out of reach".to_owned()),
                HostReach::Connecting if linked => Some("connecting".to_owned()),
                _ if agents > 0 => Some(match agents {
                    1 => "1 agent".to_owned(),
                    count => format!("{count} agents"),
                }),
                _ => None,
            };
            let found = match host.found {
                Some(HostFound::SignedOut) => Some("not signed in".to_owned()),
                Some(HostFound::Other) => Some("on another account".to_owned()),
                Some(HostFound::Stopped) => Some("stopped".to_owned()),
                Some(HostFound::Outdated) => Some("older release".to_owned()),
                _ => (host.unanswered.as_ref()).map(|unanswered| {
                    crate::hosts::install_pane::unanswered_detail(unanswered).to_owned()
                }),
            };
            let tip = match (
                host.setup.as_ref().and_then(|setup| setup.problem.as_ref()),
                &host.unanswered,
                &host.ssh,
            ) {
                (Some(problem), _, _) => Some(format!("Not set up: {problem}")),
                (None, Some(unanswered), Some(alias)) => Some(
                    crate::hosts::install_pane::unanswered_says(unanswered, alias),
                ),
                _ => None,
            };
            let setup = host.setup.as_ref();
            let detail = match setup.map(|setup| (setup.work, setup.problem.as_deref())) {
                Some((_, Some(problem))) => Some(format!("not set up: {problem}")),
                Some((SetUp::Install, None)) => Some("installing".to_owned()),
                Some((SetUp::Update, None)) => Some("updating".to_owned()),
                Some((SetUp::Start, None)) => Some("starting".to_owned()),
                Some((SetUp::SignIn, None)) => Some("signing in".to_owned()),
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
            let row = Row::new(SharedString::from(format!("host-{row_key}")), name.clone())
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
                .mark(
                    16.,
                    ui::icon(Icon::Server, 14.).text_color(if linked { t.host } else { t.ssh }),
                )
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
                        PopupMenuItem::new("New SSH terminal")
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
                        PopupMenuItem::new("Edit in ~/.ssh/config")
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
                SetUp::Install if here => "Install Sikemux here",
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
            PopupMenuItem::new("Dismiss the failed set-up")
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
                    PopupMenuItem::new("New SSH terminal")
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
                    PopupMenuItem::new("New chat here")
                        .icon(Icon::Plus)
                        .disabled(!has_project)
                        .on_click(on(|bench, key, cx| bench.new_chat_on(key, cx))),
                )
                .item(
                    PopupMenuItem::new("New terminal here")
                        .icon(Icon::Terminal)
                        .on_click(on(|bench, key, cx| bench.new_terminal_on(key, cx))),
                )
                .item(
                    PopupMenuItem::new("Files")
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
                .item(PopupMenuItem::new("Rename…").icon(Icon::Pencil).on_click({
                    let (bench, key, named) = (bench.clone(), key.clone(), named.clone());
                    move |_, _, cx| {
                        let _ =
                            bench.update(cx, |bench, cx| bench.rename_on_account(&key, &named, cx));
                    }
                }))
                .item(
                    ui::MenuRow::new("Remove from account")
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

/// The mods of the host in front that show in the project in front, each opening in a tab of its
/// own, with the few words its page shows beside its name.
fn mods(bench: &Workbench, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let front = bench
        .tab()
        .and_then(|tab| tabs::first_view(&tab.pane))
        .filter(|(kind, _)| *kind == PaneKind::Mod)
        .map(|(_, target)| target.clone());
    let settings_open = bench.settings.read(cx).is_open();
    let project = bench.project().map(|project| project.name.clone());
    bench
        .mods_here()
        .map(|shown| {
            let (id, name) = (shown.id.clone(), shown.name.clone());
            let row = Row::new(SharedString::from(format!("mod-{id}")), name.clone())
                .mark(16., ui::picture(crate::mods::icon(shown), 13., ink))
                .quiet()
                .chosen(!settings_open && front.as_ref() == Some(&id))
                .when_some(shown.badge.clone(), |row, badge| row.detail(badge))
                .when(shown.problem.is_some(), |row| {
                    row.status(Some(ui::dot(Tone::Failed, cx)))
                })
                .on_click(cx.listener(move |bench, _, _, _| bench.open_mod(&id, &name)));
            let (bench, shown, project) = (cx.weak_entity(), shown.clone(), project.clone());
            with_menu(
                SharedString::from(format!("mod-{}-menu", shown.id)),
                move |popup, _, _| {
                    let pinned = shown.project.is_some();
                    let pin = match (pinned, &project) {
                        (true, _) => "Show in every project".to_owned(),
                        (false, Some(project)) => format!("Show only in {project}"),
                        (false, None) => "Show only in this project".to_owned(),
                    };
                    let (pinning, editing, removing) =
                        (bench.clone(), bench.clone(), bench.clone());
                    let (id, edited, removed) = (shown.id.clone(), shown.clone(), shown.clone());
                    popup
                        .item(
                            PopupMenuItem::new(pin)
                                .icon(Icon::Pin)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        pinning.update(cx, |bench, _| bench.pin_mod(&id, pinned));
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Edit with an agent")
                                .icon(Icon::Sparkle)
                                .on_click(move |_, _, cx| {
                                    let edited = Some(edited.clone());
                                    let _ = editing
                                        .update(cx, |bench, cx| bench.ask_for_mod(edited, cx));
                                }),
                        )
                        .separator()
                        .item(PopupMenuItem::new("Remove").icon(Icon::Trash).on_click(
                            move |_, _, cx| {
                                let _ =
                                    removing.update(cx, |bench, cx| bench.remove_mod(&removed, cx));
                            },
                        ))
                },
                row,
                cx,
            )
        })
        .collect()
}

/// Terminals started outside every project, each opening in the terminals' strip.
fn terminals(bench: &Workbench, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    let live = Tokens::get(cx).live;
    let front = (bench.strip() == Some(Strip::Terminals))
        .then(|| bench.tab().and_then(|tab| tabs::first_view(&tab.pane)))
        .flatten()
        .filter(|(kind, _)| *kind == PaneKind::Terminal)
        .map(|(_, target)| target.clone());
    bench
        .loose_terminals()
        .into_iter()
        .map(|session| {
            let id = session.id.to_string();
            Row::new(
                SharedString::from(format!("terminal-{id}")),
                session.rail_name.clone().unwrap_or_default(),
            )
            .chosen(!bench.settings.read(cx).is_open() && front.as_ref() == Some(&id))
            .on_click(cx.listener(move |bench, _, _, _| {
                bench.open_here(OpenTab::new(
                    At::terminals(),
                    PaneKind::Terminal,
                    id.clone(),
                ));
            }))
            .mark(16., ui::icon(Icon::Terminal, 13.).text_color(live))
            .quiet()
            .into_any_element()
        })
        .collect()
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
        .border_b_1()
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
                        .border_1()
                        .border_color(t.line)
                        .rounded(RADIUS_1)
                        .text_color(t.text_tertiary)
                })),
        )
        .child(
            div().ml_auto().flex_none().child(
                IconButton::new(
                    "hide-side-rail",
                    Icon::PanelLeft,
                    if bench.rails.side_open {
                        "Hide the side rail"
                    } else {
                        "Keep the side rail open"
                    },
                )
                .on_click(cx.listener(|bench, _, _, cx| bench.toggle_rail(Edge::Start, cx))),
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
        IconButton::new("project-sort-button", Icon::Sort, "Sort projects").small(),
        move |mut popup, _, cx| {
            popup = popup.label("Sort projects");
            for (sort, label) in [
                (ProjectSort::Recent, "Recent"),
                (ProjectSort::Created, "Created"),
                (ProjectSort::Manual, "Manual"),
            ] {
                let choose = choose.clone();
                let row = ui::MenuRow::new(label)
                    .when(sort == ProjectSort::Manual, |row| row.hint("drag"));
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
    let t = Tokens::get(cx).clone();
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
                format!("Close {}", project.name),
                cx.listener(move |bench, _, _, cx| {
                    bench.close_project(place.clone(), project.clone(), cx)
                }),
                cx,
            ))
    });
    let go = {
        let (front, identity) = (front.clone(), folded.identity.clone());
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
    .mark(16., ui::icon(Icon::Folder, 13.).text_color(t.live))
    .quiet()
    .bold(active)
    .chosen(active)
    .padding_right(4.)
    .when(close.is_some(), |row| row.hover_padding(PROJECT_ROW, 28.))
    .on_click(go)
    .child(faces(&agents, cx))
    .children(crate::copies::copy_note(bench, folded, cx))
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
    let copies = (folded.copies.len() > 1)
        .then(|| crate::copies::copy_row(bench, folded, cx))
        .flatten();
    div().flex().flex_col().child(row).when(active, |tree| {
        tree.pb(SPACE_1)
            .children(copies.map(|copies| div().ml(px(13.5)).pt(SPACE_0).child(copies)))
            .child(surfaces(lit_surface(bench), cx))
    })
}

/// The surface the project tree lights: the tab rail's page while it is open, unless the
/// tab in front is a surface the tab rail has no page for, such as Git.
fn lit_surface(bench: &Workbench) -> Option<PaneKind> {
    let front = bench
        .tab()
        .and_then(|tab| tabs::first_view(&tab.pane))
        .map(|(kind, _)| kind);
    let paged = front.is_none_or(|kind| crate::pane_kind::page_of(kind).is_some());
    if !(bench.rails.tab_open && paged) {
        return front;
    }
    Some(crate::pane_kind::listed_on(bench.rails.page))
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
    let clones: Vec<String> = (folded.own.iter().chain(&folded.clones))
        .map(|project| project.id.clone())
        .collect();
    let identity = folded.identity.clone();
    let current = bench.space_of(folded).map(str::to_owned);
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
                    PopupMenuItem::new("Playbook")
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
                .checked(current.as_ref() == Some(space))
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
                PopupMenuItem::new("New Space…")
                    .icon(Icon::Plus)
                    .on_click(new_space),
            );
        }
        popup.separator().item(
            PopupMenuItem::new("No Space")
                .icon(Icon::Move)
                .checked(current.is_none())
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

/// The project's surfaces and its Playbook, hung off a spine with a tick to each, the one in
/// front lit.
fn surfaces(front: Option<PaneKind>, cx: &Context<Workbench>) -> impl IntoElement {
    let bench = cx.entity().downgrade();
    let surfaces = crate::pane_kind::SURFACES
        .into_iter()
        .enumerate()
        .map(|(index, surface)| {
            let (kind, command) = (surface.kind, surface.command);
            let bench = bench.clone();
            tree_row(
                SharedString::from(format!("surface-{index}")),
                crate::pane_kind::of(kind).icon,
                surface.label,
                Some(kind) == front,
                false,
                keymap::hint(command, cx),
                cx,
            )
            .on_click(move |_, window, cx| {
                if let Some(page) = crate::pane_kind::page_of(kind) {
                    let _ = bench.update(cx, |bench, cx| {
                        if bench.rails.tab_open {
                            bench.show_rail_page(page, cx);
                        }
                    });
                }
                keymap::dispatch(command, window, cx);
            })
            .into_any_element()
        });
    let playbook = crate::workbench::actions::FocusPlaybook::ID;
    let playbook = tree_row(
        "surface-playbook".into(),
        Icon::Book,
        "Playbook",
        front == Some(PaneKind::ProjectSettings),
        true,
        keymap::hint(playbook, cx),
        cx,
    )
    .on_click(move |_, window, cx| keymap::dispatch(playbook, window, cx));
    div()
        .flex()
        .flex_col()
        .ml(px(13.5))
        .py(SPACE_0)
        .pr(SPACE_1H)
        .children(surfaces)
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
        .rounded(RADIUS_2)
        .text_color(if lit { t.accent } else { t.text_tertiary })
        .when(!lit, |row| {
            row.hover(move |style| style.text_color(t.text_primary))
        })
        .child(if tail {
            spine
                .w(px(9.))
                .h(px(13.))
                .border_l_1()
                .border_b_1()
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
        .child(ui::text(label, TEXT_XS_H, FontWeight::MEDIUM).letter_spacing(0.01))
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
                Button::new("new-space", "New space", Variant::Dashed)
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
            .rounded(RADIUS_2)
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
            .on_click({
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
        .border_1()
        .border_color(t.border)
        .rounded(RADIUS_2 + px(3.))
        .context_menu(ui::menu({
            let bench = cx.entity();
            move |popup, _, _| {
                let (made, edited) = (bench.clone(), bench.clone());
                popup
                    .item(PopupMenuItem::new("New Space…").icon(Icon::Plus).on_click(
                        move |_, _, cx| {
                            made.update(cx, |bench, cx| bench.new_space(None, cx));
                        },
                    ))
                    .item(
                        PopupMenuItem::new("Edit Spaces…")
                            .icon(Icon::Wrench)
                            .on_click(move |_, _, cx| {
                                edited.update(cx, |bench, cx| bench.open_space_editor(cx));
                            }),
                    )
            }
        }))
        .child(chip(None, Some(Icon::Stack), "All".into()))
        .children(spaces.iter().map(|space| {
            chip(
                Some(space.id.clone()),
                Icon::named(&space.icon),
                space.name.clone().into(),
            )
        }));
    let width = bench.rail_width(Edge::Start) - 20.;
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
        .item(item("Rename…", Icon::Pencil, |bench, space, cx| {
            bench.rename_space(space, cx)
        }))
        .submenu_with_icon(
            Some(Icon::Pencil.into()),
            "Change Icon…",
            window,
            cx,
            ui::menu(icons),
        );
    if !icon.is_empty() {
        popup = popup.item(item("Remove Icon", Icon::Trash, |bench, space, _| {
            bench.set_space_icon(&space, "")
        }));
    }
    popup
        .separator()
        .item(item("New Space…", Icon::Plus, |bench, _, cx| {
            bench.new_space(None, cx)
        }))
        .item(item("Edit Spaces…", Icon::Wrench, |bench, _, cx| {
            bench.open_space_editor(cx)
        }))
        .separator()
        .item(
            ui::MenuRow::new("Delete Space…")
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
        return div()
            .flex_none()
            .flex()
            .justify_end()
            .px(SPACE_2H)
            .py(SPACE_1H)
            .border_t_1()
            .border_color(t.border)
            .child(settings_button())
            .into_any_element();
    };
    let Some(account) = view.account.as_deref() else {
        return sign_in(bench, cx).into_any_element();
    };
    let who = account.name.clone().unwrap_or_else(|| "Signed in".into());
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
            "Your account",
            Icon::User,
            None,
            act(|bench, window, cx| bench.open_settings(Page::Account, window, cx)),
        ),
        (
            "Devices",
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
            "Connectors",
            Icon::Plug,
            None,
            act(|bench, window, cx| bench.open_settings(Page::Connectors, window, cx)),
        ),
        (
            "Settings",
            Icon::Settings,
            settings_keys,
            act(|bench, window, cx| {
                keymap::count(actions::ToggleSettings::ID.name(), cx);
                bench.open_settings(Page::default(), window, cx)
            }),
        ),
    ];
    let sign_out = act(|bench, _, _| bench.sign_out());
    let menu_width = px(bench.rail_width(Edge::Start)) - SPACE_2H * 2. - px(2.);
    let menu = ui::menu_button(
        "account-menu",
        Anchor::BottomLeft,
        row,
        move |mut popup, _, _| {
            popup = popup.min_w(menu_width).max_w(menu_width);
            for (label, icon, hint, act) in items.iter().cloned() {
                popup = popup.item(
                    account_item(label, icon, hint)
                        .on_click(move |event, window, cx| act(event, window, cx)),
                );
            }
            let sign_out = sign_out.clone();
            popup.separator().item(
                account_item("Sign out", Icon::LogOut, None)
                    .on_click(move |event, window, cx| sign_out(event, window, cx)),
            )
        },
    );
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .px(SPACE_2H)
        .py(SPACE_1H)
        .border_t_1()
        .border_color(t.border)
        .child(div().flex_1().min_w_0().child(menu))
        .into_any_element()
}

/// A row of the account menu, roomier than other menus' rows since it opens the width of the rail.
fn account_item(label: &'static str, icon: Icon, hint: Option<SharedString>) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        let t = Tokens::get(cx);
        div()
            .id(label)
            .aria_label(label)
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .py(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_SM))
            .child(ui::icon(icon, 16.).text_color(t.text_secondary))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .children(hint.clone().map(|hint| {
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(hint)
            }))
    })
}

/// Settings, in the rail's foot until the person signs in and it moves into the account menu.
fn settings_button() -> IconButton {
    IconButton::new("settings", Icon::Settings, "Settings")
        .glyph(14.)
        .width(28.)
        .on_click(|_, window, cx| keymap::dispatch(actions::ToggleSettings::ID, window, cx))
}

/// The account's picture in a circle, else its initials.
pub(crate) fn avatar(bench: &Workbench, account: &AccountInfo, size: f32, t: &Tokens) -> Div {
    let initials = initials(account.name.as_deref(), account.email.as_deref());
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.375))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_secondary)
        .map(|face| match bench.picture() {
            Some(picture) => face.child(gpui_kit::img(picture).size_full().rounded_full()),
            None => face.child(initials),
        })
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

fn sign_in(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx);
    let (copy, button) = if bench.signing_in {
        (
            "Finish signing in in your browser.",
            Button::new("sign-in", "Cancel", Variant::Outline).on_click(cx.listener(
                |bench, _, _, cx| {
                    bench.cancel_sign_in();
                    cx.notify();
                },
            )),
        )
    } else {
        (
            "Sign in to reach this host from your phone and other devices.",
            Button::new("sign-in", "Sign in", Variant::Primary)
                .icon(Icon::LogIn)
                .on_click(cx.listener(|bench, _, _, cx| {
                    bench.sign_in();
                    cx.notify();
                })),
        )
    };
    div()
        .flex_none()
        .mx(SPACE_2H)
        .mb(SPACE_2H)
        .p(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .bg(gpui_kit::Hsla {
            a: 0.04,
            ..t.text_primary
        })
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(crate::theme::ui_text(12. * 1.4))
                .text_color(t.text_secondary)
                .child(copy),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .child(div().flex_1().min_w_0().flex().flex_col().child(button))
                .child(settings_button()),
        )
}

impl Workbench {
    /// Keeps a rail open, or closes it; a peeking rail that is kept stays where it is.
    pub fn toggle_rail(&mut self, edge: Edge, cx: &mut Context<Self>) {
        let open = match edge {
            Edge::Start => &mut self.rails.side_open,
            Edge::End => &mut self.rails.tab_open,
        };
        *open = !*open;
        self.rails.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    pub fn rail_width(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Start => self.rails.side_width,
            Edge::End => self.rails.tab_width,
        }
    }

    /// Sets a rail's width, within what that rail allows, and draws it at once.
    pub(crate) fn resize_rail(
        &mut self,
        edge: Edge,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kept, range) = match edge {
            Edge::Start => (&mut self.rails.side_width, rail::SIDE_RAIL),
            Edge::End => (&mut self.rails.tab_width, agents::TAB_RAIL),
        };
        let width = width.clamp(range.min, range.max).round();
        if *kept != width {
            *kept = width;
            window.refresh();
            self.keep_rails(cx);
        }
    }

    /// Moves the edge between a rail and the stage `by` points to the right.
    pub(crate) fn nudge_rail(
        &mut self,
        edge: Edge,
        by: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outward = match edge {
            Edge::Start => by,
            Edge::End => -by,
        };
        self.resize_rail(edge, self.rail_width(edge) + outward, window, cx);
    }

    /// The handle between a rail and the stage, dragged or moved by arrow keys to resize the rail.
    pub(crate) fn rail_resizer(&self, edge: Edge, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::keymap::{EdgeFarLeft, EdgeFarRight, EdgeLeft, EdgeRight};
        div()
            .id(match edge {
                Edge::Start => "resize-side-rail",
                Edge::End => "resize-tab-rail",
            })
            .flex_none()
            .w(px(RESIZER_WIDTH))
            .h_full()
            .cursor_col_resize()
            .key_context("RailResizer")
            .track_focus(&self.rails.resizers[edge as usize])
            .on_click(cx.listener(move |bench, _, window, cx| {
                bench.rails.resizers[edge as usize].focus(window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeLeft, window, cx| {
                bench.nudge_rail(edge, -16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeRight, window, cx| {
                bench.nudge_rail(edge, 16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarLeft, window, cx| {
                bench.nudge_rail(edge, -40., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarRight, window, cx| {
                bench.nudge_rail(edge, 40., window, cx)
            }))
            .on_drag(DraggedRail(edge), |rail, _, _, cx| {
                cx.new(|_| DraggedRail(rail.0))
            })
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
    pub(crate) fn peek(&mut self, edge: Edge, inside: bool, cx: &mut Context<Self>) {
        if inside {
            self.rails.unpeek = None;
            if self.rails.peeking != Some(edge) {
                self.rails.peeking = Some(edge);
                cx.notify();
            }
            return;
        }
        self.rails.unpeek = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PEEK_CLOSE).await;
            let _ = this.update(cx, |bench, cx| {
                if bench.rails.peeking == Some(edge) {
                    bench.rails.peeking = None;
                    cx.notify();
                }
            });
        }));
    }

    /// A docked rail and its handle, `tuck` of the way under the stage. It keeps its width as
    /// it goes, so nothing in it wraps again; the stage takes the room it gives up.
    pub(crate) fn docked_rail(
        &self,
        edge: Edge,
        tuck: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (part, width) = match edge {
            Edge::Start => (&self.parts.rail, self.rails.side_width),
            Edge::End => (&self.parts.tabs, self.rails.tab_width),
        };
        let rail = crate::settings::leaves(div().h_full().flex_none(), cx).child(
            part.clone()
                .cached(StyleRefinement::default().w(px(width)).h_full().flex_none()),
        );
        let drawer = div()
            .flex_none()
            .h_full()
            .w(px((width + RESIZER_WIDTH) * (1. - tuck)))
            .overflow_hidden()
            .flex()
            .opacity(1. - tuck);
        match edge {
            Edge::Start => drawer.child(rail).child(self.rail_resizer(edge, cx)),
            Edge::End => drawer
                .justify_end()
                .child(self.rail_resizer(edge, cx))
                .child(rail),
        }
    }

    /// A closed rail's edge of the window, which brings the rail out over the stage.
    pub(crate) fn rail_edge(
        &self,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (part, width) = match edge {
            Edge::Start => (&self.parts.rail, self.rails.side_width),
            Edge::End => (&self.parts.tabs, self.rails.tab_width),
        };
        let peeking = self.rails.peeking == Some(edge);
        let shown = motion::follow(
            match edge {
                Edge::Start => "peek-start-shown",
                Edge::End => "peek-end-shown",
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
            .id(match edge {
                Edge::Start => "peek-start",
                Edge::End => "peek-end",
            })
            .absolute()
            .top(px(tabs::top_height(self) + 4.))
            .bottom(px(10.))
            .w(px(if shown > 0. { width + 20. } else { PEEK_REACH }))
            .px(SPACE_2H)
            .on_hover(cx.listener(move |bench, inside: &bool, _, cx| bench.peek(edge, *inside, cx)))
            .when(shown > 0., |zone| {
                zone.child(
                    div()
                        .h_full()
                        .relative()
                        .left(px(match edge {
                            Edge::Start => -away,
                            Edge::End => away,
                        }))
                        .opacity(shown)
                        .child(
                            crate::settings::leaves(div().h_full(), cx).child(
                                part.clone()
                                    .cached(StyleRefinement::default().w(px(width)).h_full()),
                            ),
                        ),
                )
            });
        match edge {
            Edge::Start => zone.left_0(),
            Edge::End => zone.right_0(),
        }
    }
}
