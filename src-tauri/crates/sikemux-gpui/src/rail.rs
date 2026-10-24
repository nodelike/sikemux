use std::rc::Rc;

use gpui_kit::base::Popover;
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::{AccountInfo, PaneKind, ProjectInfo};

use crate::codehost::{Area, OpenArea};
use crate::hosts::HostChoice;
use crate::icons::Icon;
use crate::keymap;
use crate::tabs;
use crate::theme::{Tokens, RADIUS_1, RADIUS_2};
use crate::ui::{self, Button, Group, IconButton, Menu, MenuItem, Row, Tone, Variant};
use crate::workbench::{AgentRow, Edge, RailWidth, Strip, Workbench};

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
                .pt(px(10.))
                .px(px(10.))
                .pb(px(16.))
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
        .chosen(bench.settings.is_none() && bench.chosen_host() == crate::hosts::HostChoice::This)
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
    let (bench_entity, approval) = (cx.weak_entity(), bench.approvals.get(&None).copied());
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
        .context_menu(move |popup, window, cx| menu(popup, window, cx))
        .child(row)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(4.))
                .flex()
                .items_center()
                .opacity(0.)
                .group_hover(HOST_ROW, |more| more.opacity(1.))
                .child(
                    KitButton::new(SharedString::from(format!("{id}-more")))
                        .ghost()
                        .compact()
                        .rounded(RADIUS_2)
                        .bg(t.surface_hover)
                        .child(ui::icon(Icon::More, 12.).text_color(t.text_secondary))
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, window, cx| {
                            more(popup, window, cx)
                        }),
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
    rows(bench.read("hosts"))
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
                .and_then(|id| bench.host_views.get(id))
                .map_or(0, |view| view.workspace.agents.len());
            let load = host["id"]
                .as_str()
                .and_then(|id| bench.host_views.get(id))
                .and_then(|view| view.load.as_ref())
                .filter(|_| reach == "connected")
                .map(crate::hosts::load_label);
            let detail = match reach {
                "away" if linked => Some("out of reach".to_owned()),
                "connecting" if linked => Some("connecting".to_owned()),
                _ if linked && bench.hosts_unavailable.is_some() => {
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
            let chosen = bench.settings.is_none()
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
                    match (load, detail) {
                        (Some(load), Some(detail)) => Some(format!("{load} · {detail}")),
                        (Some(load), None) => Some(load.to_string()),
                        (None, detail) => detail,
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
                .host_views
                .get(&key)
                .is_some_and(|view| !view.workspace.projects.is_empty());
            let approval = bench.approvals.get(&Some(key.clone())).copied();
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
                    popup
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
                                    bench.show_rail_page(crate::workbench::RailPage::Files, cx);
                                })),
                        )
                        .separator()
                        .item(approval_item(bench.clone(), Some(key.clone()), approval))
                        .item(PopupMenuItem::new("Rename").icon(Icon::Pencil).on_click({
                            let (bench, key, named) = (bench.clone(), key.clone(), named.clone());
                            move |_, _, cx| {
                                let _ = bench
                                    .update(cx, |bench, cx| bench.rename_host(&key, &named, cx));
                            }
                        }))
                        .item(
                            ui::danger_popup_item("Remove from account", Some(Icon::Trash), cx)
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
    rows(bench.read("plugins"))
        .filter(|plugin| plugin["enabled"].as_bool() == Some(true))
        .filter_map(|plugin| {
            let mark = plugin_mark(plugin["id"].as_str()?)?;
            let tone =
                plugin["signedIn"]
                    .as_bool()
                    .map(|signed_in| if signed_in { Tone::Ok } else { Tone::Idle });
            let id = plugin["id"].as_str()?;
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
                    plugin["name"].as_str()?.to_owned(),
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
            .chosen(bench.settings.is_none() && front.as_ref() == Some(&id))
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
    let (release, channel) = split_version(&bench.version);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(40.))
        .pl(px(18.))
        .pr(px(8.))
        .border_b_1()
        .border_color(t.border)
        .child(ui::icon(Icon::Logo, 14.).text_color(t.accent))
        .child(
            ui::text("Sikemux", 13., FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .ml(px(1.)),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(4.))
                .pt(px(1.))
                .child(ui::mono(format!("v{release}"), 10., cx))
                .children(channel.map(|channel| {
                    ui::text(channel, 10., FontWeight::MEDIUM)
                        .px(px(4.))
                        .py(px(1.))
                        .line_height(crate::theme::ui_text(12.))
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
                    if bench.side_rail_open {
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
        .gap(px(6.))
        .children(keys.map(|keys| ui::kbd(keys, cx)))
        .children(buttons.map(IconButton::small))
}

fn project_tree(
    bench: &Workbench,
    project: &ProjectInfo,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let active =
        bench.settings.is_none() && bench.strip() == Some(Strip::Project(project.id.clone()));
    let agents = bench.agents_of(&project.id);
    let id = project.id.clone();
    let menu = project_menu(bench, project, cx);
    let name = project.name.clone();
    let row = div()
        .id(SharedString::from(format!("project-menu-{}", project.id)))
        .relative()
        .children(ui::spot(|| format!("project-row-{name}")))
        .context_menu(menu)
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
            .on_click(cx.listener(move |bench, _, _, _| bench.activate_project(&id)))
            .child(faces(&agents, cx))
            .into_any_element(),
            crate::drag::DraggedProject {
                id: project.id.clone(),
                label: project.name.clone().into(),
            },
            &cx.entity(),
            cx,
        ));
    div().flex().flex_col().child(row).when(active, |tree| {
        tree.pb(px(4.)).child(surfaces(
            project,
            bench
                .tab()
                .and_then(|tab| tabs::first_view(&tab.pane))
                .map(|(kind, _)| kind),
            bench.area,
            cx,
        ))
    })
}

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

/// Up to five agents' faces, each overlapping the one before by 6px.
fn faces(agents: &[AgentRow], cx: &App) -> impl IntoElement {
    let shown = agents.len().min(5);
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
            ui::mark_box(20., ui::agent_mark(agent, 20., cx))
                .absolute()
                .left(px(14. * index as f32))
        }))
}

/// What a project opens, in the order the rail lists them: its own surfaces,
/// with its code host's sections after Git, each an area of the Git tab.
fn surface_rows(
    project: &ProjectInfo,
) -> Vec<(Icon, SharedString, &'static str, Option<PaneKind>, Area)> {
    let mut rows = vec![
        (
            Icon::Editor,
            "Files".into(),
            "window.files",
            Some(PaneKind::Files),
            Area::Local,
        ),
        (
            Icon::Terminal,
            "Terminal".into(),
            "window.terminal",
            Some(PaneKind::Terminal),
            Area::Local,
        ),
        (
            Icon::Agent,
            "Agents".into(),
            "window.agents",
            Some(PaneKind::Agent),
            Area::Local,
        ),
        (
            Icon::Git,
            "Git".into(),
            "window.git",
            Some(PaneKind::Git),
            Area::Local,
        ),
    ];
    if let Some(host) = &project.code_host {
        rows.extend(host.sections.iter().filter_map(|section| {
            let area = Area::of(section)?;
            Some((area.icon(), area.label(host), "", Some(PaneKind::Git), area))
        }));
    }
    rows.extend([
        (
            Icon::Search,
            "Search".into(),
            "search.global",
            Some(PaneKind::Search),
            Area::Local,
        ),
        (
            Icon::Board,
            "Agent board".into(),
            "window.board",
            Some(PaneKind::Board),
            Area::Local,
        ),
    ]);
    rows
}

/// The project's surfaces, hung off a spine with a tick to each, the one in front lit.
fn surfaces(
    project: &ProjectInfo,
    front: Option<PaneKind>,
    area: Area,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let rows = surface_rows(project);
    let last = rows.len() - 1;
    div()
        .flex()
        .flex_col()
        .ml(px(20.))
        .py(px(2.))
        .pr(px(6.))
        .children(rows.into_iter().enumerate().map(
            move |(index, (glyph, label, command, kind, opens))| {
                let tail = index == last;
                let in_git = kind == Some(PaneKind::Git);
                let lit = kind.is_some() && kind == front && (!in_git || opens == area);
                let spine = div().absolute().left_0().top_0();
                div()
                    .id(SharedString::from(format!("surface-{index}")))
                    .relative()
                    .h(px(25.))
                    .on_click(move |_, window, cx| {
                        if in_git {
                            window.dispatch_action(Box::new(OpenArea { area: opens }), cx);
                        } else {
                            keymap::dispatch(command, window, cx);
                        }
                    })
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
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
                    .child(ui::text(label, 12.5, FontWeight::MEDIUM).letter_spacing(0.01))
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
            .mb(px(6.))
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
            .px(px(8.))
            .rounded(RADIUS_2)
            .text_size(crate::theme::ui_text(11.))
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
                chip.context_menu(move |popup, window, cx| {
                    space_menu(popup, window, cx, &bench, &space, &icon)
                })
                .into_any_element()
            }
            None => chip.into_any_element(),
        }
    };
    let track = div()
        .id("space-track")
        .flex()
        .gap(px(2.))
        .p(px(2.))
        .overflow_x_scroll()
        .border_1()
        .border_color(t.border)
        .rounded(RADIUS_2 + px(3.))
        .context_menu({
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
        })
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
        .mb(px(6.))
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
        .submenu_with_icon(Some(Icon::Pencil.into()), "Change Icon…", window, cx, icons);
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
            ui::danger_popup_item("Delete Space…", Some(Icon::Trash), cx).on_click({
                let (bench, space) = (bench.clone(), space.to_owned());
                move |_, _, cx| bench.update(cx, |bench, cx| bench.delete_space(space.clone(), cx))
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
            .px(px(10.))
            .py(px(6.))
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
            "your-account",
            "Your account",
            Icon::User,
            None,
            act(|bench, window, cx| {
                crate::settings::open_at(bench, Page::Devices, "Your account", window, cx)
            }),
        ),
        (
            "devices",
            "Devices",
            Icon::Laptop,
            None,
            act(|bench, window, cx| bench.open_settings(Page::Devices, window, cx)),
        ),
        (
            "settings",
            "Settings",
            Icon::Settings,
            settings_keys,
            act(|bench, window, cx| bench.open_settings(Page::default(), window, cx)),
        ),
    ];
    let sign_out = act(|bench, _, _| bench.sign_out());
    let menu = Popover::new("account-menu")
        .anchor(Anchor::BottomLeft)
        .offset(px(4.))
        .trigger(row)
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let item = |id: &'static str, label: &'static str, icon: Icon, act: ui::OnClick| {
                let popover = popover.clone();
                MenuItem::new(id, label)
                    .icon(icon)
                    .on_click(move |event, window, cx| {
                        act(event, window, cx);
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                    })
            };
            let mut menu = Menu::new();
            for (id, label, icon, hint, act) in items.iter().cloned() {
                menu = menu.child(
                    item(id, label, icon, act).when_some(hint, |item, keys| item.hint(keys)),
                );
            }
            menu.separator(cx)
                .child(item("sign-out", "Sign out", Icon::LogOut, sign_out.clone()))
        });
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(10.))
        .py(px(6.))
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
            Some(picture) => face.child(gpui_kit::img(picture).size_full()),
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
        .mx(px(10.))
        .mb(px(10.))
        .p(px(8.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .rounded(RADIUS_2)
        .bg(gpui_kit::Hsla {
            a: 0.04,
            ..t.text_primary
        })
        .child(
            div()
                .text_size(crate::theme::ui_text(12.))
                .line_height(crate::theme::ui_text(12. * 1.4))
                .text_color(t.text_secondary)
                .child(copy),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().flex_1().min_w_0().flex().flex_col().child(button))
                .child(settings_button()),
        )
}
