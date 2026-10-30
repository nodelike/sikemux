use std::rc::Rc;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::{
    AccountInfo, AgentGroup, AgentStatus, PaneKind, Preferences, ProjectInfo, TabRailPage,
    TabRailScope,
};

use crate::codehost::{Area, OpenArea};
use crate::hosts::HostChoice;
use crate::icons::Icon;
use crate::keymap;
use crate::tabs;
use crate::theme::{
    Tokens, RADIUS_1, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS,
    TEXT_3XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, Group, IconButton, Row, Tone, Variant};
use crate::workbench::{AgentRow, Edge, RailPage, RailWidth, Strip, Workbench};

impl Rails {
    /// Opens, sizes and pages the rails as the person's preferences keep them.
    pub fn follow(&mut self, preferences: &Preferences) {
        self.side_open = preferences.side_rail_open;
        self.tab_open = preferences.tab_rail_open;
        let width =
            |width: f64, range: RailWidth| (width as f32).clamp(range.min, range.max).round();
        self.side_width = width(preferences.side_rail_width, SIDE_RAIL);
        self.tab_width = width(preferences.tab_rail_width, crate::agents::TAB_RAIL);
        self.page = match preferences.tab_rail_page {
            TabRailPage::Agents => RailPage::Agents,
            TabRailPage::Terminals => RailPage::Terminals,
            TabRailPage::Files => RailPage::Files,
        };
        self.all_projects = preferences.tab_rail_scope == TabRailScope::All;
        self.all_harnesses = preferences.tab_rail_all_agents;
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
    pub page: RailPage,
    /// The rails' state waiting to be kept in the core's preferences, once a drag settles.
    pub(crate) keeping: Option<gpui_kit::Task<()>>,
    pub(crate) all_projects: bool,
    /// The rail lists the recent chats of every agent, not one's.
    pub(crate) all_harnesses: bool,
    /// The leads whose teams the agent rail folds away.
    pub(crate) folded_teams: std::collections::HashSet<String>,
    /// The agent rows the rail drew last, for telling the rows that have just arrived.
    pub(crate) seen: std::cell::RefCell<crate::agents::RailSeen>,
    /// The tab rail's list of agents and chats, whose foot brings older chats into view.
    pub(crate) scroll: gpui_kit::ScrollHandle,
    /// Where the rail's agent rows were drawn, for the chosen surface to glide between them.
    pub(crate) glide: std::cell::RefCell<crate::motion::GlideTrack>,
    /// Where the tab rail's frame was last drawn, left, top, width and height, for the driver.
    pub(crate) frame: Rc<std::cell::Cell<[f32; 4]>>,
    /// The tab rail's pages while they move between one another.
    pub(crate) slide: crate::track::Slide<RailPage>,
}

pub const SIDE_RAIL: RailWidth = RailWidth {
    min: 180.,
    max: 480.,
    initial: 258.,
};

pub fn side_rail(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let space = bench.space();
    let projects: Vec<_> = bench
        .projects()
        .iter()
        .filter(|project| space.is_none_or(|space| project.space.as_deref() == Some(&space.id)))
        .map(|project| project_tree(bench, project, cx).into_any_element())
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
                        .actions(actions(
                            keymap::hint("project.open", cx),
                            [
                                IconButton::new("add-project", Icon::Plus, "Open project")
                                    .on_click(|_, window, cx| {
                                        keymap::dispatch("project.open", window, cx)
                                    }),
                            ],
                            cx,
                        ))
                        .toolbar((!bench.projects().is_empty()).then(|| space_switch(bench, cx)))
                        .children(projects),
                )
                .child(
                    Group::new("Hosts", "no hosts")
                        .actions(actions(
                            keymap::hint("ssh.open", cx),
                            [
                                IconButton::new("edit-ssh", Icon::Pencil, "Edit ~/.ssh/config")
                                    .on_click(cx.listener(|bench, _, _, cx| {
                                        bench.edit_ssh_config(None, cx)
                                    })),
                                IconButton::new("add-host", Icon::Plus, "Find a host").on_click(
                                    |_, window, cx| keymap::dispatch("ssh.open", window, cx),
                                ),
                            ],
                            cx,
                        ))
                        .child(this_host(bench, cx))
                        .children(hosts(bench, cx)),
                )
                .child(Group::new("Plugins", "no plugins").children(plugins(bench, cx)))
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
        .children(crate::whats_new::rail_chip(bench, cx))
        .child(account(bench, cx))
}

/// The host this window runs on, where new work starts.
fn this_host(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let agents = bench
        .view
        .as_ref()
        .map_or(0, |view| view.workspace.agents.len());
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
        .status(Some(ui::dot(Tone::Ok, cx)));
    let (bench_entity, approval) = (cx.weak_entity(), bench.hosts.approvals.get(&None).copied());
    with_menu(
        "this-host-menu".into(),
        move |popup, _, _| {
            let bench = bench_entity.clone();
            popup
                .item(
                    PopupMenuItem::new("New terminal here")
                        .icon(Icon::Terminal)
                        .on_click(move |_, _, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                bench.choose_host(HostChoice::This, cx);
                                bench.new_loose_terminal(cx);
                            });
                        }),
                )
                .separator()
                .item(approval_item(bench_entity.clone(), None, approval))
        },
        row,
        cx,
    )
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
    rows(bench.reads.hosts.as_ref())
        .map(|host| {
            let name = host["name"].as_str().unwrap_or_default().to_owned();
            let linked = !host["id"].is_null();
            let reach = host["reach"].as_str().unwrap_or_default();
            let tone = match reach {
                "connected" => Tone::Ok,
                "connecting" => Tone::Pending,
                "failed" => Tone::Failed,
                _ => Tone::Idle,
            };
            let agents = host["id"]
                .as_str()
                .and_then(|id| bench.hosts.views.get(id))
                .map_or(0, |view| view.workspace.agents.len());
            let load = host["id"]
                .as_str()
                .and_then(|id| bench.hosts.views.get(id))
                .and_then(|view| view.load.as_ref())
                .filter(|_| reach == "connected")
                .map(crate::hosts::load_label);
            let updates = host["id"]
                .as_str()
                .and_then(|id| bench.hosts.views.get(id))
                .and_then(crate::hosts::host_updates)
                .filter(|_| reach == "connected")
                .cloned();
            let detail = match reach {
                "away" if linked => Some("out of reach".to_owned()),
                "connecting" if linked => Some("connecting".to_owned()),
                _ if linked && bench.hosts.unavailable.is_some() => {
                    Some("needs the signed app".to_owned())
                }
                _ if agents > 0 => Some(match agents {
                    1 => "1 agent".to_owned(),
                    count => format!("{count} agents"),
                }),
                _ => None,
            };
            let setup = &host["setup"];
            let found = match host["found"].as_str() {
                Some("signedOut") => Some("not signed in".to_owned()),
                Some("other") => Some("on another account".to_owned()),
                Some("stopped") => Some("stopped".to_owned()),
                Some("outdated") => Some("older release".to_owned()),
                _ => None,
            };
            let detail = match (setup["work"].as_str(), setup["problem"].as_str()) {
                (_, Some(problem)) => Some(format!("not set up: {problem}")),
                (Some("install"), None) => Some("installing".to_owned()),
                (Some("update"), None) => Some("updating".to_owned()),
                (Some("start"), None) => Some("starting".to_owned()),
                (Some(_), None) => Some("signing in".to_owned()),
                (None, _) => found.or(detail),
            };
            let setting_up = setup.is_object() && setup["problem"].is_null();
            let alias = host["ssh"].as_str().filter(|_| !linked).map(str::to_owned);
            let front = alias.is_some() && bench.strip() == alias.clone().map(Strip::Host);
            let key = host["id"].as_str().map(str::to_owned);
            let chosen = !bench.settings.read(cx).is_open()
                && (front
                    || key.as_ref().is_some_and(|key| {
                        bench.chosen_host() == crate::hosts::HostChoice::Account(key.clone())
                    }));
            let row = Row::new(SharedString::from(format!("host-{name}")), name.clone())
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
                            load.map(|load| load.to_string()),
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
                .status(Some(ui::dot(tone, cx)));
            let bench_entity = cx.weak_entity();
            let set_up_alias = host["ssh"].as_str().map(str::to_owned);
            let set_up = set_up_alias
                .as_deref()
                .and_then(|alias| bench.set_up_of(alias))
                .zip(set_up_alias);
            let set_up_item = move |bench: WeakEntity<Workbench>, here: bool| {
                set_up.clone().map(|(set_up, alias)| {
                    let label = match set_up {
                        crate::install_pane::SetUp::Install if here => "Install Sikemux here",
                        _ => set_up.menu_label(),
                    };
                    PopupMenuItem::new(label)
                        .icon(set_up.icon())
                        .disabled(setting_up)
                        .on_click(move |_, _, cx| {
                            let _ =
                                bench.update(cx, |bench, cx| bench.set_up_host(&alias, set_up, cx));
                        })
                })
            };
            let menu_id = SharedString::from(format!("host-menu-{name}"));
            let Some(key) = host["id"].as_str().map(str::to_owned) else {
                let Some(alias) = alias else {
                    return row.into_any_element();
                };
                return with_menu(
                    menu_id,
                    move |popup, _, _| {
                        let (bench, alias) = (bench_entity.clone(), alias.clone());
                        let edited = (bench.clone(), alias.clone());
                        let popup = popup.item(
                            PopupMenuItem::new("New SSH terminal")
                                .icon(Icon::Terminal)
                                .on_click(move |_, _, cx| {
                                    let _ = bench.update(cx, |bench, cx| {
                                        bench.open_ssh_terminal(&alias, cx)
                                    });
                                }),
                        );
                        let popup = match set_up_item(bench_entity.clone(), true) {
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
                );
            };
            let has_project = bench
                .hosts
                .views
                .get(&key)
                .is_some_and(|view| !view.workspace.projects.is_empty());
            let approval = bench.hosts.approvals.get(&Some(key.clone())).copied();
            with_menu(
                menu_id,
                move |popup, _, cx| {
                    let bench = bench_entity.clone();
                    let mut popup = popup;
                    if let Some(item) = set_up_item(bench.clone(), false) {
                        popup = popup.item(item).separator();
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
                                    bench.show_rail_page(RailPage::Files, cx);
                                })),
                        )
                        .separator()
                        .item(approval_item(bench.clone(), Some(key.clone()), approval));
                    let popup = match &updates {
                        Some(updates) => {
                            let (bench, key) = (bench.clone(), key.clone());
                            crate::hosts::updates_items(popup, updates, move |on, cx| {
                                let host = key.clone();
                                let _ = bench
                                    .update(cx, |bench, cx| bench.set_host_updates(host, on, cx));
                            })
                        }
                        None => popup,
                    };
                    popup
                        .item(PopupMenuItem::new("Rename").icon(Icon::Pencil).on_click({
                            let (bench, key, named) = (bench.clone(), key.clone(), named.clone());
                            move |_, _, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.rename_host(&key, &named, cx));
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
                                        let _ = bench.update(cx, |bench, cx| {
                                            bench.remove_host(&key, &named, cx)
                                        });
                                    }
                                }),
                        )
                },
                row,
                cx,
            )
        })
        .collect()
}

/// The plugins that are on and draw in the rail, signed in or not.
fn plugins(bench: &Workbench, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let strip = bench.strip();
    bench
        .reads
        .plugins
        .on()
        .filter_map(|plugin| {
            let mark = plugin_mark(&plugin.id)?;
            let tone = plugin
                .signed_in
                .map(|signed_in| if signed_in { Tone::Ok } else { Tone::Idle });
            let id = plugin.id.as_str();
            let opens =
                crate::plugin_pane::surface(id)
                    .zip(strip.clone())
                    .map(|(surface, strip)| {
                        let target = format!("{id}:{surface}");
                        cx.listener(move |bench: &mut Workbench, _, _, _| {
                            let params = json!({ "kind": PaneKind::Plugin, "target": target });
                            bench.call_in_front("openTab", strip.with(params));
                        })
                    });
            Some(
                Row::new(
                    SharedString::from(format!("plugin-{id}")),
                    plugin.name.clone(),
                )
                .mark(16., ui::picture(mark, 13., ink))
                .quiet()
                .status(tone.map(|tone| ui::dot(tone, cx)))
                .when_some(opens, |row, opens| row.on_click(opens))
                .into_any_element(),
            )
        })
        .collect()
}

/// The mark of a plugin that has a place of its own in the rail; code hosts show
/// under their projects instead.
pub(crate) fn plugin_mark(id: &str) -> Option<Icon> {
    Some(match id.strip_prefix("sikemux.")? {
        "aws" => Icon::Aws,
        "bruno" => Icon::Bruno,
        "database" => Icon::Database,
        "jira" => Icon::Jira,
        "linear" => Icon::Linear,
        "rundeck" => Icon::Rundeck,
        "signoz" => Icon::Signoz,
        "slack" => Icon::Slack,
        "vercel" => Icon::Vercel,
        _ => return None,
    })
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
                let params = json!({ "kind": PaneKind::Terminal, "target": id });
                bench.call("openTab", Strip::Terminals.with(params));
            }))
            .mark(16., ui::icon(Icon::Terminal, 13.).text_color(live))
            .quiet()
            .into_any_element()
        })
        .collect()
}

fn rows(read: Option<&Value>) -> impl Iterator<Item = &Value> {
    read.and_then(Value::as_array).into_iter().flatten()
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
    project: &ProjectInfo,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let active = !bench.settings.read(cx).is_open()
        && bench.strip() == Some(Strip::Project(project.id.clone()));
    let agents = bench.agents_of(&project.id);
    let id = project.id.clone();
    let menu = project_menu(bench, project, cx);
    let name = project.name.clone();
    let closable = bench.projects().len() > 1;
    let close = closable.then(|| {
        let project = project.clone();
        ui::hover_actions(PROJECT_ROW)
            .top_0()
            .bottom_0()
            .child(ui::close_button(
                SharedString::from(format!("close-project-{}", project.id)),
                format!("Close {}", project.name),
                cx.listener(move |bench, _, _, cx| bench.close_project(project.clone(), cx)),
                cx,
            ))
    });
    let row = div()
        .id(SharedString::from(format!("project-menu-{}", project.id)))
        .group(PROJECT_ROW)
        .relative()
        .children(ui::spot(|| format!("project-row-{name}")))
        .context_menu(ui::menu(menu))
        .child(crate::drag::project(
            Row::new(
                SharedString::from(format!("project-{}", project.id)),
                project.name.clone(),
            )
            .mark(16., ui::icon(Icon::Folder, 13.).text_color(t.live))
            .quiet()
            .bold(active)
            .chosen(active)
            .padding_right(4.)
            .when(closable, |row| row.hover_padding(PROJECT_ROW, 28.))
            .on_click(cx.listener(move |bench, _, _, _| bench.activate_project(&id)))
            .child(faces(&agents, cx))
            .status(rollup(bench, &project.id, cx))
            .into_any_element(),
            crate::drag::DraggedProject {
                id: project.id.clone(),
                label: project.name.clone().into(),
            },
            &cx.entity(),
            cx,
        ))
        .children(close);
    div().flex().flex_col().child(row).when(active, |tree| {
        tree.pb(SPACE_1)
            .child(surfaces(project, lit_surface(bench), bench.git.area, cx))
    })
}

/// The surface the project tree lights: the tab rail's page while it is open, unless the
/// tab in front is a surface the tab rail has no page for, such as Git.
fn lit_surface(bench: &Workbench) -> Option<PaneKind> {
    let front = bench
        .tab()
        .and_then(|tab| tabs::first_view(&tab.pane))
        .map(|(kind, _)| kind);
    let paged = front.is_none_or(|kind| page_of(kind).is_some());
    if !(bench.rails.tab_open && paged) {
        return front;
    }
    Some(match bench.rails.page {
        RailPage::Agents => PaneKind::Agent,
        RailPage::Terminals => PaneKind::Terminal,
        RailPage::Files => PaneKind::Files,
    })
}

/// The tab rail's page that lists a surface's tabs.
fn page_of(kind: PaneKind) -> Option<RailPage> {
    crate::pane_kind::of(kind).page
}

/// The hover group of a project's row, whose × shows on hover.
const PROJECT_ROW: &str = "project-row";

/// What right-clicking a project offers: its settings, and the space the rail lists it under.
fn project_menu(
    bench: &Workbench,
    project: &ProjectInfo,
    cx: &Context<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let bench_entity = cx.weak_entity();
    let (id, current) = (project.id.clone(), project.space.clone());
    let path = project.path.clone();
    let others = bench.other_hosts();
    let spaces: Vec<(String, String, Option<Icon>)> =
        bench.view.as_ref().map_or(Vec::new(), |view| {
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
    move |popup, _, _| {
        let act = |work: Rc<dyn Fn(&mut Workbench)>| {
            let bench = bench_entity.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = bench.update(cx, |bench, _| work(bench));
            }
        };
        let move_to = |space: Option<String>| {
            let id = id.clone();
            act(Rc::new(move |bench: &mut Workbench| {
                bench.call(
                    "updateProject",
                    json!({ "id": id, "patch": { "space": space } }),
                )
            }))
        };
        let settings = {
            let id = id.clone();
            act(Rc::new(move |bench: &mut Workbench| {
                bench.open_project_settings(&id)
            }))
        };
        let mut popup = popup.item(
            PopupMenuItem::new("Project settings")
                .icon(Icon::Settings)
                .on_click(settings),
        );
        for (host, label, reached, after) in &others {
            let (bench, path, host, label, after) = (
                bench_entity.clone(),
                path.clone(),
                host.clone(),
                label.clone(),
                after.clone(),
            );
            popup = popup.item(
                PopupMenuItem::new(format!("Open on {label}"))
                    .icon(Icon::Server)
                    .disabled(!reached)
                    .on_click(move |_, _, cx| {
                        let _ = bench.update(cx, |bench, cx| {
                            bench.open_on(&path, &host, label.clone(), after.clone(), cx)
                        });
                    }),
            );
        }
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

/// How a project's open agents stand together, as the core rolls them up; none while all are idle.
fn rollup(bench: &Workbench, project: &str, cx: &App) -> Option<gpui_kit::AnyElement> {
    let status = match bench.shown()?.inbox.projects.get(project)?.group {
        AgentGroup::Blocked => AgentStatus::Blocked,
        AgentGroup::Working => AgentStatus::Working,
        AgentGroup::Done => AgentStatus::Done,
        AgentGroup::Idle => return None,
    };
    ui::status_mark(&format!("project:{project}"), status, cx)
}

/// What a project opens, in the order the rail lists them: its own surfaces,
/// with its code host's sections after Git, each an area of the Git tab.
fn surface_rows(
    project: &ProjectInfo,
) -> Vec<(Icon, SharedString, &'static str, Option<PaneKind>, Area)> {
    let surface = |kind: PaneKind| {
        let shown = crate::pane_kind::of(kind);
        let (label, command) = shown.surface.unwrap_or_default();
        (shown.icon, label.into(), command, Some(kind), Area::Local)
    };
    let mut rows: Vec<_> = [
        PaneKind::Files,
        PaneKind::Terminal,
        PaneKind::Agent,
        PaneKind::Git,
    ]
    .map(surface)
    .into();
    if let Some(host) = &project.code_host {
        rows.extend(host.sections.iter().filter_map(|section| {
            let area = Area::of(section)?;
            Some((area.icon(), area.label(host), "", Some(PaneKind::Git), area))
        }));
    }
    rows.extend([PaneKind::Search, PaneKind::Board].map(surface));
    rows
}

/// The project's surfaces, hung off a spine with a tick to each, the one in front lit.
fn surfaces(
    project: &ProjectInfo,
    front: Option<PaneKind>,
    area: Area,
    cx: &Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let bench = cx.entity().downgrade();
    let rows = surface_rows(project);
    let last = rows.len() - 1;
    div()
        .flex()
        .flex_col()
        .ml(px(13.5))
        .py(SPACE_0)
        .pr(SPACE_1H)
        .children(rows.into_iter().enumerate().map(
            move |(index, (glyph, label, command, kind, opens))| {
                let tail = index == last;
                let in_git = kind == Some(PaneKind::Git);
                let lit = kind.is_some() && kind == front && (!in_git || opens == area);
                let spine = div().absolute().left_0().top_0();
                let bench = bench.clone();
                div()
                    .id(SharedString::from(format!("surface-{index}")))
                    .relative()
                    .h(px(25.))
                    .on_click(move |_, window, cx| {
                        if let Some(page) = kind.and_then(page_of) {
                            let _ = bench.update(cx, |bench, cx| {
                                if bench.rails.tab_open {
                                    bench.show_rail_page(page, cx);
                                }
                            });
                        }
                        if in_git {
                            window.dispatch_action(Box::new(OpenArea { area: opens }), cx);
                        } else {
                            keymap::dispatch(command, window, cx);
                        }
                    })
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
                    .when_some(keymap::hint(command, cx), |row, keys| {
                        let keys = ui::kbd(keys, cx).when(lit, |keys| keys.text_color(t.accent));
                        row.child(
                            div()
                                .ml_auto()
                                .when(!lit, |keys| keys.opacity(0.5))
                                .child(keys),
                        )
                    })
            },
        ))
}

/// The spaces the person sorts projects into, which filter the projects below them.
fn space_switch(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let spaces = bench
        .view
        .as_ref()
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
    let Some(view) = bench.view.as_ref() else {
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
    let who = account
        .name
        .clone()
        .or_else(|| account.email.clone())
        .unwrap_or_else(|| "Signed in".into());
    let row = Row::new("account", who)
        .mark(24., avatar(bench, account, 24., &t))
        .centered()
        .when_some(
            account.name.as_ref().and(account.email.clone()),
            |row, email| row.subtitle(ui::meta(email, cx)),
        );
    use crate::settings::Page;
    let settings_keys = keymap::hint("settings.toggle", cx);
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
            act(|bench, window, cx| bench.open_settings(Page::Devices, window, cx)),
        ),
        (
            "Settings",
            Icon::Settings,
            settings_keys,
            act(|bench, window, cx| bench.open_settings(Page::default(), window, cx)),
        ),
    ];
    let sign_out = act(|bench, _, _| bench.sign_out());
    let menu = ui::menu_button(
        "account-menu",
        Anchor::BottomLeft,
        row,
        move |mut popup, _, cx| {
            for (label, icon, hint, act) in items.iter().cloned() {
                popup = popup.item(
                    ui::MenuRow::new(label)
                        .icon(icon)
                        .when_some(hint, ui::MenuRow::hint)
                        .item(cx)
                        .on_click(move |event, window, cx| act(event, window, cx)),
                );
            }
            let sign_out = sign_out.clone();
            popup.separator().item(
                PopupMenuItem::new("Sign out")
                    .icon(Icon::LogOut)
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
        .child(settings_button())
        .into_any_element()
}

/// Settings, kept in the rail's foot whether or not the person is signed in.
fn settings_button() -> IconButton {
    IconButton::new("settings", Icon::Settings, "Settings")
        .glyph(14.)
        .width(28.)
        .on_click(|_, window, cx| keymap::dispatch("settings.toggle", window, cx))
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
            Button::new("sign-in", "Cancel", Variant::Outline)
                .on_click(cx.listener(|bench, _, _, _| bench.cancel_sign_in())),
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
