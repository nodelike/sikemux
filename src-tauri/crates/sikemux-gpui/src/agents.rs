use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Popover};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, fill, point, px, radians, size, Anchor, AnyElement, App, Bounds, ClickEvent,
    Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Transformation, Window,
};
use serde_json::Value;
use sikemux_core::agent_accounts::Profile;
use sikemux_wire::protocol::{PaneKind, RecentInfo};

use crate::agent_menu::AgentMenu;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{self, Tokens};
use crate::ui::{self, Field, Group, IconButton, Menu, MenuItem, Row, Segmented};
use crate::workbench::{AgentRow, Edge, RailPage, RailWidth, Strip, TeamLink, Workbench};

pub const TAB_RAIL: RailWidth = RailWidth {
    min: 240.,
    max: 560.,
    initial: 288.,
};

pub fn tab_rail(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let frame = bench.rail_frame.clone();
    ui::panel(cx)
        .relative()
        .w(px(bench.rail_width(Edge::End)))
        .flex_none()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, _, _| {
                    frame.set(
                        [
                            bounds.left(),
                            bounds.top(),
                            bounds.size.width,
                            bounds.size.height,
                        ]
                        .map(f32::from),
                    )
                },
            )
            .absolute()
            .size_full(),
        )
        .child(crate::track::rail(
            bench,
            |page, cx| page_panel(bench, page, cx),
            cx,
        ))
        .child(pages(bench, cx))
        .into_any_element()
}

/// What one of the rail's pages holds; the rail's frame and its page switch stay put while
/// the pages slide inside it.
fn page_body() -> gpui_kit::Div {
    div().size_full().flex().flex_col().overflow_hidden()
}

/// One of the tab rail's pages, whole.
fn page_panel(bench: &Workbench, page: RailPage, cx: &mut Context<Workbench>) -> AnyElement {
    match page {
        RailPage::Files => {
            return page_body()
                .child(page_header("Files", bench, cx))
                .child(crate::files::page(bench, cx))
                .into_any_element();
        }
        RailPage::Terminals => return terminals_page(bench, cx),
        RailPage::Agents => {}
    }
    if let crate::hosts::HostChoice::Ssh(alias) = bench.chosen_host() {
        return ssh_page(bench, alias, cx);
    }
    let project = bench.project();
    let pins = project
        .filter(|_| !bench.all_projects)
        .map(|project| bench.pinned_of(&project.id))
        .unwrap_or_default();
    let (held, agents): (Vec<_>, Vec<_>) = bench.listed_agents().into_iter().partition(|agent| {
        pins.iter()
            .any(|pin| pin.agent_id.as_deref() == Some(agent.id.as_str()))
    });
    let (copy, rows) = team_rows(bench, &agents, cx);
    let pinned: Vec<_> = pins
        .iter()
        .map(|pin| {
            match held
                .iter()
                .find(|agent| pin.agent_id.as_deref() == Some(agent.id.as_str()))
            {
                Some(agent) => agent_row(bench, agent, None, None, false, cx).into_any_element(),
                None => recent_row(bench, pin, cx),
            }
        })
        .collect();
    let recent: Vec<_> = project
        .map(|project| bench.recent_of(&project.id))
        .unwrap_or_default()
        .into_iter()
        .filter(|recent| !recent.pinned || bench.all_projects)
        .map(|recent| recent_row(bench, recent, cx))
        .collect();
    let empty = (rows.is_empty() && pinned.is_empty() && recent.is_empty()).then(|| {
        let shown = (project.zip(other_host(bench)))
            .filter(|_| bench.filter_text.is_empty())
            .map(|(project, host)| match bench.all_projects {
                true => format!("No chats on {host}"),
                false => format!("No chats in {} on {host}", project.name),
            });
        if let Some(title) = shown {
            rail_empty(Icon::Agent, title, Some(("agent.new", "start one")), cx)
        } else if project.is_none() {
            rail_empty(
                Icon::Agent,
                "No project open",
                Some(("project.open", "open one")),
                cx,
            )
        } else if bench.launchers().is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(rail_empty(Icon::Agent, "No agents set up", None, cx))
                .child(
                    ui::Button::new("set-up-agent", "Set one up", ui::Variant::Outline).on_click(
                        cx.listener(|bench, _, window, cx| {
                            bench.open_settings(crate::settings::Page::Agents, window, cx)
                        }),
                    ),
                )
                .into_any_element()
        } else if !bench.filter_text.is_empty() {
            rail_empty(Icon::Search, "No chats match", None, cx)
        } else {
            rail_empty(
                Icon::Agent,
                "No agents yet",
                Some(("agent.new", "start one")),
                cx,
            )
        }
    });
    page_body()
        .child(header(bench, cx))
        .children(bench.filter.as_ref().map(|filter| {
            div()
                .flex_none()
                .mt(px(10.))
                .mx(px(12.))
                .mb(px(2.))
                .child(Field::new(filter).icon(Icon::Search))
        }))
        .child(
            div()
                .flex_none()
                .pt(px(4.))
                .px(px(10.))
                .children(
                    other_host(bench)
                        .map(|host| ui::meta(format!("on {host}"), cx).px(px(8.)).pt(px(4.))),
                )
                .child(
                    new_row("new-chat", Icon::Plus, "New chat", "agent.new", cx)
                        .on_click(cx.listener(|bench, _, window, cx| bench.new_chat(window, cx))),
                )
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Terminal,
                        "New terminal",
                        "terminal.new",
                        cx,
                    )
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    new_row(
                        "new-browser",
                        Icon::Globe,
                        "New browser",
                        "browser.tabNew",
                        cx,
                    )
                    .on_click(|_, window, cx| {
                        crate::keymap::dispatch("browser.tabNew", window, cx)
                    }),
                )
                .child(div().mt(px(8.)).child(scope_switch(bench, cx))),
        )
        .child(
            div()
                .id("tab-rail-scroll")
                .flex_1()
                .min_h_0()
                .pt(px(8.))
                .px(px(10.))
                .pb(px(16.))
                .overflow_y_scroll()
                .children(empty)
                .child({
                    let glide = bench.rail_glide.borrow();
                    let skip = usize::from(copy.is_some());
                    div().on_children_prepainted(glide.keep_origin()).child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(copy)
                            .children(rows)
                            .on_children_prepainted(glide.keep_items(skip)),
                    )
                })
                .when(!pinned.is_empty(), |scroll| {
                    scroll.child(Group::new("Pinned", "").children(pinned))
                })
                .when(!recent.is_empty(), |scroll| {
                    scroll.child(Group::new("Recent", "").children(recent))
                }),
        )
        .children(limits(bench, cx))
        .into_any_element()
}

fn header(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    header_frame(
        picker(bench, cx),
        [
            IconButton::new("search", Icon::Search, "Filter recent chats")
                .pressed(bench.filter.is_some())
                .on_click(cx.listener(|bench, _, window, cx| {
                    bench.toggle_filter(window, cx);
                    cx.notify();
                }))
                .into_any_element(),
            IconButton::new("start-agent", Icon::Plus, "Choose agent")
                .on_click(|_, window, cx| crate::keymap::dispatch("agent.choose", window, cx))
                .into_any_element(),
        ],
        bench,
        cx,
    )
}

/// A rail page's head: what the page is, its own buttons, then the one that hides the rail.
fn header_frame(
    title: impl IntoElement,
    buttons: impl IntoIterator<Item = AnyElement>,
    bench: &Workbench,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex_none()
        .pt(px(6.))
        .pr(px(6.))
        .pb(px(6.))
        .pl(px(10.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .min_h(px(28.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(title)
                .child(
                    div().flex().gap(px(2.)).children(buttons).child(
                        IconButton::new(
                            "hide-tab-rail",
                            Icon::PanelRight,
                            if bench.tab_rail_open {
                                "Hide agents rail"
                            } else {
                                "Keep agents rail open"
                            },
                        )
                        .on_click(cx.listener(|bench, _, _, cx| bench.toggle_rail(Edge::End, cx))),
                    ),
                ),
        )
}

/// Which agent the rail is for, and a menu to switch it or show every agent's chats.
fn picker(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let launcher = bench.launcher();
    let (mark, label) = match launcher {
        _ if bench.all_harnesses => (
            ui::icon(Icon::Inbox, 16.)
                .text_color(t.text_secondary)
                .into_any_element(),
            "All harnesses".to_owned(),
        ),
        Some(launcher) => (ui::mark(&launcher.provider, 16.), launcher.label.clone()),
        None => {
            return ui::text("Agents", 13., FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .into_any_element()
        }
    };
    let name = ui::text(label, 13., FontWeight::SEMIBOLD)
        .letter_spacing(-0.015)
        .text_color(t.text_primary);
    if bench.launchers().len() < 2 {
        return div()
            .flex()
            .items_center()
            .gap(px(7.))
            .child(mark)
            .child(name)
            .into_any_element();
    }
    let chosen = if bench.all_harnesses {
        None
    } else {
        launcher.map(|launcher| launcher.id.clone())
    };
    let options: Vec<_> = bench
        .launchers()
        .iter()
        .map(|launcher| {
            (
                launcher.id.clone(),
                launcher.provider.clone(),
                launcher.label.clone(),
                bench.usage_peak(&launcher.id),
            )
        })
        .collect();
    let choose = Rc::new(cx.listener(|bench, choice: &Option<String>, _, cx| {
        bench.all_harnesses = choice.is_none();
        if choice.is_some() {
            bench.launcher_choice = choice.clone();
        }
        cx.notify();
    }));
    Popover::new("agent-picker")
        .anchor(Anchor::TopLeft)
        .offset(px(4.))
        .trigger(
            BaseButton::new("agent-picker-button")
                .accessibility_label("Agent provider")
                .h(px(28.))
                .ml(px(-6.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(7.))
                .rounded(theme::RADIUS_2)
                .hover(move |style| style.bg(t.surface_hover))
                .child(mark)
                .child(name)
                .child(
                    ui::icon(Icon::Chevron, 9.)
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_tertiary),
                ),
        )
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let item = |id: SharedString, label: String, choice: Option<String>| {
                let (choose, popover) = (choose.clone(), popover.clone());
                MenuItem::new(id, label)
                    .checked(choice == chosen)
                    .on_click(move |_, window, cx| {
                        choose(&choice, window, cx);
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                    })
            };
            div().w(px(200.)).child(
                Menu::new()
                    .child(
                        item("all-harnesses".into(), "All harnesses".into(), None)
                            .icon(Icon::Inbox),
                    )
                    .children(options.iter().map(|(id, provider, label, peak)| {
                        item(
                            SharedString::from(format!("launcher-{id}")),
                            label.clone(),
                            Some(id.clone()),
                        )
                        .mark(ui::mark(provider, 14.))
                        .when_some(*peak, |item, peak| item.hint(format!("{}%", peak.round())))
                    })),
            )
        })
        .into_any_element()
}

/// A saved chat no agent holds, to take up again: how long ago it ran, or a pin when it is
/// pinned, and Pin or Unpin on a right-click.
fn recent_row(bench: &Workbench, recent: &RecentInfo, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tail = match recent.pinned {
        true => ui::icon(Icon::Pin, 11.)
            .text_color(t.text_tertiary)
            .into_any_element(),
        false => ui::mono(age(recent.active_at), 11., cx).into_any_element(),
    };
    let row = Row::new(
        SharedString::from(format!("recent-{}", recent.id)),
        recent.title.clone(),
    )
    .mark(22., ui::mark(&recent.provider, 20.))
    .compact(bench.rail_compact())
    .child(tail)
    .on_click(cx.listener({
        let recent = recent.clone();
        move |bench, _, _, _| bench.resume_chat(&recent)
    }));
    let pin = pin_chat(&cx.entity(), recent);
    let deletable = bench
        .read("catalog")
        .and_then(Value::as_array)
        .and_then(|kinds| {
            kinds
                .iter()
                .find(|kind| kind["type"] == recent.provider.as_str())
        })
        .is_some_and(|kind| kind["deletableChats"] == true);
    let (this, chat) = (cx.weak_entity(), recent.clone());
    div()
        .id(SharedString::from(format!("recent-menu-{}", recent.id)))
        .child(row)
        .context_menu(move |popup, _, cx| {
            let (label, pin) = pin.clone();
            let pin = PopupMenuItem::new(label)
                .icon(Icon::Pin)
                .on_click(move |_, window, cx| pin(window, cx));
            if chat.pinned {
                return popup.item(pin);
            }
            let on = |work: fn(&mut Workbench, &RecentInfo, &mut Context<Workbench>)| {
                let (this, chat) = (this.clone(), chat.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |bench, cx| work(bench, &chat, cx));
                }
            };
            let popup = popup
                .item(
                    PopupMenuItem::new("Open")
                        .icon(Icon::Message)
                        .on_click(on(|bench, chat, _| bench.resume_chat(chat))),
                )
                .item(
                    PopupMenuItem::new("Rename…")
                        .icon(Icon::Pencil)
                        .on_click(on(rename_recent)),
                )
                .item(pin);
            if !deletable {
                return popup;
            }
            popup.separator().item(
                ui::danger_popup_item("Delete…", Some(Icon::Trash), cx).on_click(on(delete_recent)),
            )
        })
        .into_any_element()
}

/// Asks for a recent chat's new title, and renames it in its CLI's own history.
fn rename_recent(bench: &mut Workbench, chat: &RecentInfo, cx: &mut Context<Workbench>) {
    let recent = chat.id.clone();
    bench.prompt(
        "Rename chat",
        chat.title.clone(),
        move |bench, title, _, _| {
            if !title.trim().is_empty() {
                bench.call(
                    "renameRecent",
                    serde_json::json!({ "recent": recent, "title": title.trim() }),
                );
            }
        },
        cx,
    );
}

/// Deletes a recent chat from its CLI's own history, once the person says so.
fn delete_recent(bench: &mut Workbench, chat: &RecentInfo, cx: &mut Context<Workbench>) {
    let recent = chat.id.clone();
    bench.confirm(
        "Delete this chat?",
        format!(
            "“{}” will be removed from disk for good and can no longer be resumed.",
            chat.title
        ),
        "Delete",
        true,
        move |bench, yes, _, _| {
            if yes {
                bench.call("deleteRecent", serde_json::json!({ "recent": recent }));
            }
        },
        cx,
    );
}

/// Something a click on the rail does.
type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a menu item of the rail does to the workbench.
type BenchAction = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

/// "Pin" or "Unpin" for the saved chat, and what does it.
fn pin_chat(this: &Entity<Workbench>, recent: &RecentInfo) -> (&'static str, Click) {
    let (this, id, pinned) = (this.clone(), recent.id.clone(), recent.pinned);
    let label = if pinned { "Unpin" } else { "Pin" };
    let pin = move |_: &mut Window, cx: &mut App| {
        let params = serde_json::json!({ "recent": id, "pinned": !pinned });
        this.update(cx, |bench, _| bench.call("pinChat", params));
    };
    (label, Rc::new(pin))
}

fn new_row(id: &'static str, glyph: Icon, label: &'static str, command: &str, cx: &App) -> Row {
    let t = Tokens::get(cx).clone();
    Row::new(id, label)
        .mark(
            22.,
            ui::icon(glyph, 15.)
                .text_color(t.text_secondary)
                .group_hover(ui::ROW, move |style| style.text_color(t.text_primary)),
        )
        .quiet()
        .children(crate::keymap::hint(command, cx).map(|keys| ui::kbd(keys, cx)))
}

/// The agent rows the rail drew last, for telling those that have just come and gone.
pub type RailSeen = crate::motion::ListMemory<AgentRow>;

/// The rows, each lead's workers under it on a tree the lead folds, unless the list is
/// filtered and a team may have lost its lead.
fn team_rows(
    bench: &Workbench,
    agents: &[AgentRow],
    cx: &mut Context<Workbench>,
) -> (Option<AnyElement>, Vec<AnyElement>) {
    let chosen = bench.agent().map(|agent| agent.id.clone());
    let glided = bench
        .rail_glide
        .borrow_mut()
        .choose(chosen, cx.reduce_motion());
    let teams = bench.filter_text.is_empty();
    let remote = if teams {
        remote_workers(bench)
    } else {
        Vec::new()
    };
    let remote_of = |members: &[&AgentRow]| -> Vec<&serde_json::Value> {
        remote
            .iter()
            .copied()
            .filter(|worker| {
                members
                    .iter()
                    .any(|member| worker["lead"] == member.id.as_str())
            })
            .collect()
    };
    let mut folded = false;
    let mut team_members: Vec<&AgentRow> = Vec::new();
    let mut drawn: Vec<(String, AnyElement)> = Vec::with_capacity(agents.len());
    let mut shown = Vec::new();
    for (ix, agent) in agents.iter().enumerate() {
        let team = agent.team.filter(|_| teams);
        let continues = |at: usize| {
            agents
                .get(at)
                .is_some_and(|next| matches!(next.team, Some(TeamLink::Member | TeamLink::Last)))
        };
        let fold = match team {
            Some(TeamLink::Head) => {
                let members: Vec<&AgentRow> = std::iter::once(agent)
                    .chain(agents[ix + 1..].iter().take_while(|worker| {
                        matches!(worker.team, Some(TeamLink::Member | TeamLink::Last))
                    }))
                    .collect();
                let count = members.len() - 1 + remote_of(&members).len();
                folded = bench.folded_teams.contains(&agent.id);
                team_members = members;
                Some(Fold {
                    open: !folded,
                    count,
                })
            }
            Some(_) if folded => continue,
            Some(_) => None,
            None => {
                folded = false;
                None
            }
        };
        shown.push((agent.id.clone(), agent.clone()));
        let row = agent_row(bench, agent, team, fold, glided, cx);
        drawn.push((agent.id.clone(), row.into_any_element()));
        if team.is_some() && !folded && !continues(ix + 1) {
            for worker in remote_of(&team_members) {
                drawn.push((String::new(), remote_row(worker, cx)));
            }
        }
    }
    let view = format!(
        "{}:{}:{}",
        bench.all_projects,
        bench.project().map_or("", |project| project.id.as_str()),
        bench.filter_text
    );
    let lasting = crate::motion::ROW_DEPARTURE
        .close
        .duration
        .max(crate::motion::ARRIVE_SLIDE.duration);
    let mut seen = bench.rail_seen.borrow_mut();
    let (arriving, left) = seen.update(view, shown, usize::MAX, lasting);
    let mut order: Vec<String> = drawn.iter().map(|(id, _)| id.clone()).collect();
    let mut rows: Vec<AnyElement> = drawn
        .into_iter()
        .map(|(id, row)| match arriving.contains(&id) {
            true => {
                let key = SharedString::from(format!("arrive:{id}"));
                crate::motion::arriving(key, crate::motion::ROW_ARRIVAL, row).into_any_element()
            }
            false => row,
        })
        .collect();
    for gone in left {
        let row = agent_row(bench, &gone.item, None, None, false, cx);
        let key = SharedString::from(format!("depart:{}", gone.id));
        let row = crate::motion::departing(key, gone.since, crate::motion::ROW_DEPARTURE, row);
        rows.insert(gone.at.min(rows.len()), row.into_any_element());
        order.insert(gone.at.min(order.len()), gone.id.clone());
    }
    let mut glide = bench.rail_glide.borrow_mut();
    glide.set_order(order);
    let chosen = Tokens::get(cx).surface_chosen;
    let copy = glide.copy(move |copy| copy.rounded(theme::RADIUS_2).bg(chosen));
    (copy, rows)
}

/// The spine and tick that hang a team's rows under its lead.
fn team_lines(team: Option<TeamLink>, open_head: bool, cx: &App) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let spine = |line: gpui_kit::Div| line.absolute().left(px(14.)).w(px(1.)).bg(t.tree_spine);
    match (team, open_head) {
        (Some(TeamLink::Head), true) => {
            vec![spine(div()).top(px(26.)).bottom(px(-2.)).into_any_element()]
        }
        (Some(TeamLink::Member), _) => vec![
            spine(div()).top(px(-2.)).bottom(px(-2.)).into_any_element(),
            div()
                .absolute()
                .top(px(14.))
                .left(px(14.))
                .w(px(7.))
                .h(px(1.))
                .bg(t.tree_tick)
                .into_any_element(),
        ],
        (Some(TeamLink::Last), _) => vec![div()
            .absolute()
            .top(px(-2.))
            .left(px(14.))
            .w(px(8.))
            .h(px(17.))
            .border_l_1()
            .border_b_1()
            .border_color(t.tree_spine)
            .rounded_bl(px(4.))
            .into_any_element()],
        _ => Vec::new(),
    }
}

/// The workers agents here started on linked hosts, as the core lists them in team order.
fn remote_workers(bench: &Workbench) -> Vec<&serde_json::Value> {
    bench
        .shown()
        .and_then(|view| view.inbox["remoteAgents"].as_array())
        .into_iter()
        .flatten()
        .filter(|worker| worker["team"].is_string())
        .collect()
}

/// A worker on a linked host, in its lead's team. It has no chat here to open.
fn remote_row(worker: &serde_json::Value, cx: &mut Context<Workbench>) -> AnyElement {
    let team = match worker["team"].as_str() {
        Some("last") => Some(TeamLink::Last),
        _ => Some(TeamLink::Member),
    };
    let reachable = worker["reachable"] == true;
    let host = worker["host"].as_str().unwrap_or_default();
    let title = format!(
        "@{} on {host}{}",
        worker["name"].as_str().unwrap_or_default(),
        if reachable { "" } else { ", out of reach" }
    );
    let status = reachable
        .then(|| {
            serde_json::from_value::<sikemux_wire::protocol::AgentStatus>(worker["status"].clone())
                .ok()
        })
        .flatten();
    let tip = if reachable {
        format!("Runs on {host}")
    } else {
        format!("{host} is out of reach")
    };
    div()
        .id(SharedString::from(format!(
            "remote-{}",
            worker["agentId"].as_str().unwrap_or_default()
        )))
        .relative()
        .pl(px(18.))
        .tooltip(ui::tooltip(tip))
        .child(
            Row::new(
                SharedString::from(format!(
                    "remote-row-{}",
                    worker["agentId"].as_str().unwrap_or_default()
                )),
                title,
            )
            .mark(
                22.,
                ui::mark(worker["cli"].as_str().unwrap_or_default(), 20.),
            )
            .quiet()
            .when_some(status, |row, status| {
                let agent = worker["agentId"].as_str().unwrap_or_default();
                row.status(ui::status_mark(agent, status, cx))
            }),
        )
        .children(team_lines(team, false, cx))
        .into_any_element()
}

impl Workbench {
    /// Folds a lead's team away, or brings it back.
    pub fn fold_team(&mut self, lead: &str, cx: &mut Context<Self>) {
        if !self.folded_teams.remove(lead) {
            self.folded_teams.insert(lead.to_owned());
        }
        cx.notify();
    }
}

/// Whether a lead's team shows, and how many workers it has.
struct Fold {
    open: bool,
    count: usize,
}

fn agent_row(
    bench: &Workbench,
    agent: &AgentRow,
    team: Option<TeamLink>,
    fold: Option<Fold>,
    glided: bool,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let project = bench
        .all_projects
        .then(|| {
            bench
                .projects()
                .iter()
                .find(|project| project.id == agent.project)
        })
        .flatten()
        .map(|project| project.name.clone());
    let chosen = bench.agent().is_some_and(|front| front.id == agent.id);
    let id = agent.id.clone();
    let menu = AgentMenu::new(bench, &agent.id, cx);
    let mark = match &fold {
        Some(fold) => div()
            .relative()
            .size(px(20.))
            .child(
                div()
                    .group_hover(ui::ROW, |mark| mark.opacity(0.))
                    .child(ui::agent_mark(agent, 20., cx)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("team-fold:{}", agent.id)))
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.)
                    .group_hover(ui::ROW, |chevron| chevron.opacity(1.))
                    .on_click(cx.listener({
                        let id = agent.id.clone();
                        move |bench, _, _, cx| {
                            cx.stop_propagation();
                            bench.fold_team(&id, cx);
                        }
                    }))
                    .child(motion::turn(
                        format!("team-chevron:{}", agent.id),
                        motion::fold_angle(fold.open),
                        motion::LANE_FOLD,
                        ui::icon(Icon::Chevron, 11.).text_color(t.text_secondary),
                    )),
            )
            .into_any_element(),
        None => ui::agent_mark(agent, 20., cx),
    };
    let status = ui::status_mark(&agent.id, agent.status, cx);
    let fades = status.is_none();
    let pin = bench
        .chat_of(&agent.id)
        .map(|chat| (chat.pinned, pin_chat(&cx.entity(), chat).1));
    let row = Row::new(
        SharedString::from(format!("agent-{}", agent.id)),
        agent.title.clone(),
    )
    .mark(22., mark)
    .hover_padding(AGENT_ROW, if pin.is_some() { 48. } else { 28. })
    .chosen(chosen)
    .glided(glided && chosen)
    .compact(bench.rail_compact())
    .when_some(project.clone(), |row, project| {
        row.subtitle(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .line_height(crate::theme::ui_text(14.))
                .child(ui::icon(Icon::Folder, 11.).text_color(t.live))
                .child(ui::meta(project, cx)),
        )
    })
    .status(status.map(|mark| {
        div()
            .group_hover(AGENT_ROW, |mark| mark.opacity(0.))
            .child(mark)
    }))
    .on_click(cx.listener({
        let id = id.clone();
        move |bench, _, _, _| bench.open_agent(&id)
    }))
    .children(agent.name.clone().map(|name| {
        ui::meta(format!("@{name}"), cx)
            .pl(px(8.))
            .pr(px(4.))
            .when(fades, |name| {
                name.group_hover(AGENT_ROW, |name| name.opacity(0.))
            })
    }))
    .children(fold.as_ref().filter(|fold| !fold.open).map(|fold| {
        div()
            .flex_none()
            .px(px(5.))
            .rounded_full()
            .bg(t.surface_hover)
            .text_color(t.text_tertiary)
            .text_size(crate::theme::ui_text(10.))
            .line_height(crate::theme::ui_text(15.))
            .font_features(crate::theme::tabular())
            .child(format!("+{}", fold.count))
    }))
    .children(counts(agent, cx).map(|counts| {
        div()
            .when(fades, |counts| {
                counts.group_hover(AGENT_ROW, |counts| counts.opacity(0.))
            })
            .child(counts)
    }));
    let lines = team_lines(team, matches!(fold, Some(Fold { open: true, .. })), cx);
    let card = crate::agent_card::view(bench, agent, project.as_deref(), cx);
    let close = agent.id.clone();
    let actions = div()
        .absolute()
        .top_0()
        .right(px(4.))
        .h(px(if bench.rail_compact() { 27. } else { 28. }))
        .flex()
        .items_center()
        .opacity(0.)
        .group_hover(AGENT_ROW, |actions| actions.opacity(1.))
        .children(pin.map(|(pinned, pin)| {
            let label = format!("{} {}", if pinned { "Unpin" } else { "Pin" }, agent.title);
            IconButton::new(
                SharedString::from(format!("pin-{}", agent.id)),
                Icon::Pin,
                label,
            )
            .small()
            .pressed(pinned)
            .on_click(move |_, window, cx| pin(window, cx))
        }))
        .child(ui::close_button(
            SharedString::from(format!("close-{}", agent.id)),
            format!("Close {}", agent.title),
            cx.listener(move |bench, _, _, _| {
                bench.call("closeAgents", serde_json::json!({ "agentIds": [close] }))
            }),
            cx,
        ));
    div()
        .id(SharedString::from(format!("agent-menu-{}", agent.id)))
        .group(AGENT_ROW)
        .relative()
        .when(
            matches!(team, Some(TeamLink::Member | TeamLink::Last)),
            |wrap| wrap.pl(px(18.)),
        )
        .on_hover(
            cx.listener(move |bench, inside: &bool, _, cx| bench.rest_on_agent(&id, *inside, cx)),
        )
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            cx.listener(|bench, _, _, cx| bench.close_card(cx)),
        )
        .child(row)
        .child(actions)
        .children(lines)
        .children(card.map(|card| div().absolute().top_0().left_0().child(card)))
        .context_menu(move |popup, _, _| match &menu {
            Some(menu) => menu.build(popup),
            None => popup,
        })
}

/// The hover group of an agent's rail row, whose pin and close buttons show on hover.
const AGENT_ROW: &str = "agent-row";

fn counts(agent: &AgentRow, cx: &App) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    (agent.subagents > 0 || agent.tasks > 0).then(|| {
        let count = |glyph: Icon, size: f32, color, value: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .child(ui::icon(glyph, size).text_color(color))
                .child(
                    ui::text(value.to_string(), 11., FontWeight::MEDIUM)
                        .font_features(crate::theme::tabular()),
                )
        };
        div()
            .flex()
            .gap(px(6.))
            .text_color(t.text_secondary)
            .when(agent.subagents > 0, |row| {
                row.child(count(Icon::Agent, 10., t.live, agent.subagents))
            })
            .when(agent.tasks > 0, |row| {
                row.child(count(Icon::Command, 11., t.warn, agent.tasks))
            })
    })
}

/// How long ago, in a tight column: `now`, `5m`, `3h`, `4d`, `2mo`, `1y`.
pub fn age(at: u64) -> String {
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let seconds = now.saturating_sub(at) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        2_592_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// The plan limits of the account the person chose for the agent new chats here start with, as
/// its agent last reported them.
fn limits(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    if bench.all_harnesses {
        return None;
    }
    let launcher = bench.launcher()?;
    let compact = bench.rail_compact();
    let chosen = bench.accounts.current(&launcher.provider);
    let status = chosen
        .and_then(|profile| crate::accounts::status(bench, profile))
        .and_then(|status| status.as_ref().ok());
    let agent = bench
        .agent_label(&launcher.provider)
        .unwrap_or(&launcher.label)
        .to_owned();
    let (name, account, signed_out) = match chosen {
        Some(profile) => (
            crate::accounts::name(bench, profile, status),
            crate::accounts::usage(bench, profile),
            status.is_some_and(|status| !status.signed_in),
        ),
        None => (
            agent.clone(),
            bench.usage_of(&launcher.id),
            launcher.status.as_deref() == Some("signedOut"),
        ),
    };
    let account = account.unwrap_or(&Value::Null);
    let unread = account["usage"].is_null() && account["problem"].is_null();
    let refresh = format!("Refresh {agent} plan limits");
    let windows = account["usage"]["windows"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let quiet = |text: String| {
        ui::text(text, 10.5, FontWeight::NORMAL)
            .italic()
            .text_color(t.text_tertiary)
            .px(px(8.))
    };
    let provider = launcher.provider.clone();
    let current = chosen.map(|profile| profile.id.clone());
    let body = if signed_out {
        let (provider, current) = (provider.clone(), current.clone());
        div()
            .flex()
            .gap(px(4.))
            .child(quiet(format!("{name} is signed out.")).pr_0())
            .child(
                div()
                    .id("limits-sign-in")
                    .cursor_pointer()
                    .child(
                        ui::text("Sign in", 10.5, FontWeight::NORMAL)
                            .text_color(t.text_secondary)
                            .underline(),
                    )
                    .on_click(cx.listener(move |bench, _, window, cx| {
                        crate::accounts::open_sign_in(
                            bench,
                            &provider,
                            current.as_deref(),
                            window,
                            cx,
                        )
                    })),
            )
            .into_any_element()
    } else if unread {
        quiet("Refresh to read plan limits.".into()).into_any_element()
    } else if !account["problem"].is_null() {
        quiet("Could not read plan limits. Refresh to try again.".into()).into_any_element()
    } else if windows.is_empty() {
        let reason = account["usage"]["unavailableReason"]
            .as_str()
            .unwrap_or("This account did not report plan limits.");
        quiet(reason.to_owned()).into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap(px(if compact { 4. } else { 6. }))
            .children(windows.iter().map(|window| limit_row(window, cx)))
            .into_any_element()
    };
    Some(
        div()
            .flex_none()
            .mt(px(if compact { 10. } else { 12. }))
            .mx(px(10.))
            .mb(px(if compact { 12. } else { 16. }))
            .child(
                div()
                    .h(px(18.))
                    .mb(px(if compact { 8. } else { 9. }))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        ui::text("LIMITS", 11., FontWeight::MEDIUM)
                            .letter_spacing(0.08)
                            .text_color(t.text_tertiary),
                    )
                    .child(div().flex_1().h(px(1.)).bg(t.border))
                    .children(account["readAt"].as_u64().map(|read_at| {
                        ui::text(age(read_at), 10.5, FontWeight::NORMAL)
                            .text_color(t.text_tertiary)
                            .font_features(crate::theme::tabular())
                    }))
                    .child(account_menu(bench, &provider, name, cx))
                    .child(
                        IconButton::new("refresh-limits", Icon::Refresh, refresh)
                            .small()
                            .on_click(cx.listener(move |bench, _, _, cx| {
                                bench.read_usage();
                                crate::accounts::ask_again(bench, &provider, cx);
                            })),
                    ),
            )
            .child(body),
    )
}

/// The account new chats start on, opening the person's other accounts with the agent: which
/// to start on, signing out or in, and adding one. Accounts signed out of wait in Settings.
fn account_menu(
    bench: &Workbench,
    provider: &str,
    name: String,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let status = |profile: &Profile| {
        crate::accounts::status(bench, profile).and_then(|status| status.as_ref().ok())
    };
    let current = bench
        .accounts
        .current(provider)
        .map(|profile| profile.id.clone());
    let rows: Vec<(Profile, String, Option<String>, bool)> = bench
        .accounts
        .of(provider)
        .into_iter()
        .filter(|profile| {
            Some(&profile.id) == current.as_ref()
                || status(profile).is_none_or(|status| status.signed_in)
        })
        .map(|profile| {
            let status = status(profile);
            let signed_out = status.is_some_and(|status| !status.signed_in);
            let hint = if signed_out {
                vec!["Signed out".to_owned()]
            } else {
                status
                    .and_then(|status| status.email.clone().or_else(|| status.organization.clone()))
                    .into_iter()
                    .chain(
                        crate::accounts::used(bench, profile)
                            .map(|used| format!("{}%", used.round())),
                    )
                    .collect()
            };
            let hint = (!hint.is_empty()).then(|| hint.join(" · "));
            let name = crate::accounts::name(bench, profile, status);
            (profile.clone(), name, hint, signed_out)
        })
        .collect();
    let chosen = rows
        .iter()
        .find(|(profile, ..)| Some(&profile.id) == current.as_ref())
        .map(|(profile, name, _, signed_out)| (profile.id.clone(), name.clone(), *signed_out));
    let provider = provider.to_owned();
    Popover::new("limits-account-menu")
        .anchor(Anchor::BottomRight)
        .offset(px(4.))
        .trigger(
            BaseButton::new("limits-account")
                .accessibility_label("Account")
                .h(px(20.))
                .pl(px(3.))
                .pr(px(5.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(theme::RADIUS_2)
                .hover(move |style| style.bg(t.surface_hover))
                .child(ui::text(name, 11., FontWeight::MEDIUM).text_color(t.text_primary))
                .child(
                    ui::icon(Icon::Chevron, 9.)
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_tertiary),
                ),
        )
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let act = |id: SharedString, label: String, run: BenchAction| {
                let (this, popover) = (this.clone(), popover.clone());
                MenuItem::new(id, label).on_click(move |_, window, cx| {
                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                    this.update(cx, |bench, cx| run(bench, window, cx));
                })
            };
            let mut menu = Menu::new().children(rows.iter().map(|(profile, name, hint, _)| {
                let here = Some(&profile.id) == current.as_ref();
                let chosen = profile.clone();
                act(
                    SharedString::from(format!("limits-account-{}", profile.id)),
                    name.clone(),
                    Rc::new(move |bench, _, cx| {
                        if !here {
                            crate::accounts::select(bench, &chosen, cx);
                        }
                    }),
                )
                .mark(crate::accounts::avatar(profile, name, 16.))
                .checked(here)
                .when_some(hint.clone(), MenuItem::hint)
            }));
            menu = menu.separator(cx);
            if let Some((id, name, signed_out)) = chosen.clone() {
                let provider = provider.clone();
                menu = menu.child(match signed_out {
                    true => act(
                        "limits-sign-in-account".into(),
                        format!("Sign in to {name}"),
                        Rc::new(move |bench, window, cx| {
                            crate::accounts::open_sign_in(bench, &provider, Some(&id), window, cx)
                        }),
                    )
                    .icon(Icon::User),
                    false => act(
                        "limits-sign-out".into(),
                        format!("Sign out of {name}"),
                        Rc::new(move |bench, _, cx| crate::accounts::sign_out(bench, &id, cx)),
                    )
                    .icon(Icon::LogOut),
                });
            }
            let provider = provider.clone();
            menu.child(
                act(
                    "limits-add-account".into(),
                    "Add account…".into(),
                    Rc::new(move |bench, window, cx| {
                        crate::accounts::open_sign_in(bench, &provider, None, window, cx)
                    }),
                )
                .icon(Icon::Plus),
            )
        })
        .into_any_element()
}

/// One of a plan's rolling windows: how much of it is used, and when it starts again.
fn limit_row(window: &Value, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let label = window["label"].as_str().unwrap_or_default().to_owned();
    let percent = window["usedPercent"].as_f64().unwrap_or(0.).clamp(0., 100.);
    let signal = if percent >= 90. {
        t.danger
    } else if percent >= 70. {
        t.warn
    } else {
        Hsla {
            a: 0.62,
            ..t.text_primary
        }
    };
    let reset = reset_at(&window["resetsAt"]);
    let note = match reset {
        Some(at) => format!(
            "Resets {}",
            at.with_timezone(&chrono::Local)
                .format("%-m/%-d/%Y, %-I:%M:%S %p")
        ),
        None => "Reset time unavailable".into(),
    };
    let unlit = Hsla {
        a: 0.10,
        ..t.text_primary
    };
    div()
        .id(SharedString::from(format!("limit-{label}")))
        .h(px(18.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(10.))
        .tooltip(ui::tooltip(format!(
            "{label}: {}% used. {note}",
            percent.round()
        )))
        .child(
            ui::meta(label, cx)
                .w(px(58.))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        )
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let lit = bounds.size.width * (percent / 100.) as f32;
                    let mut x = px(0.);
                    while x < bounds.size.width {
                        let tick = Bounds::new(
                            point(bounds.origin.x + x, bounds.origin.y),
                            size(px(1.5), bounds.size.height),
                        );
                        window.paint_quad(fill(tick, if x < lit { signal } else { unlit }));
                        x += px(4.);
                    }
                },
            )
            .flex_1()
            .h(px(10.)),
        )
        .child(
            ui::text(format!("{}%", percent.round()), 11., FontWeight::NORMAL)
                .w(px(28.))
                .flex_none()
                .text_right()
                .font_features(theme::tabular())
                .text_color(if percent < 70. {
                    t.text_primary
                } else {
                    signal
                }),
        )
        .child(
            ui::text(countdown(reset), 10., FontWeight::NORMAL)
                .w(px(42.))
                .flex_none()
                .text_right()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary),
        )
}

/// When a window starts again: Unix seconds, as a number or digits, or an ISO time.
fn reset_at(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = value.as_i64().or_else(|| {
        value
            .as_str()
            .filter(|text| text.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|text| text.parse().ok())
    });
    match seconds {
        Some(seconds) => chrono::DateTime::from_timestamp(seconds, 0),
        None => chrono::DateTime::parse_from_rfc3339(value.as_str()?)
            .ok()
            .map(|at| at.to_utc()),
    }
}

fn countdown(reset: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(reset) = reset else {
        return "unknown".into();
    };
    let minutes = ((reset - chrono::Utc::now()).num_seconds().max(0) as f64 / 60.).ceil() as i64;
    let (hours, days) = (minutes / 60, minutes / 1440);
    let with = |big: i64, unit: &str, small: i64, small_unit: &str| match small {
        0 => format!("{big}{unit}"),
        small => format!("{big}{unit} {small}{small_unit}"),
    };
    match minutes {
        0 => "resetting now".into(),
        1..=59 => format!("{minutes}m"),
        _ if hours < 24 => with(hours, "h", minutes % 60, "m"),
        _ if days < 7 => with(days, "d", hours % 24, "h"),
        _ => reset
            .with_timezone(&chrono::Local)
            .format("%b %-d")
            .to_string(),
    }
}

fn pages(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let border = Tokens::get(cx).border;
    let page = |id: &'static str,
                glyph: Icon,
                label: &'static str,
                shown: RailPage,
                cx: &mut Context<Workbench>| {
        IconButton::new(id, glyph, label)
            .width(32.)
            .pressed(bench.rail_page == shown)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.show_rail_page(shown, cx);
            }))
    };
    div()
        .flex_none()
        .h(px(35.))
        .px(px(8.))
        .py(px(4.))
        .flex()
        .justify_center()
        .gap(px(4.))
        .border_t_1()
        .border_color(border)
        .child(page(
            "page-agents",
            Icon::Agents,
            "Agents",
            RailPage::Agents,
            cx,
        ))
        .child(page(
            "page-terminals",
            Icon::Terminal,
            "Terminals",
            RailPage::Terminals,
            cx,
        ))
        .child(page("page-files", Icon::File, "Files", RailPage::Files, cx))
}

/// A rail page's title bar.
fn page_header(
    title: impl Into<SharedString>,
    bench: &Workbench,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let title = ui::text(title, 12., FontWeight::MEDIUM).text_color(t.text_primary);
    header_frame(title, [], bench, cx)
}

/// A rail page with nothing to list: a faint mark, what is missing, and the key that fills it.
/// The account host chosen in front, by its name; none for this host or an SSH host.
fn other_host(bench: &Workbench) -> Option<SharedString> {
    match bench.chosen_host() {
        crate::hosts::HostChoice::Account(key) => Some(bench.host_name(&key)),
        _ => None,
    }
}

fn rail_empty(
    glyph: Icon,
    title: impl Into<SharedString>,
    press: Option<(&str, &'static str)>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let press = press.and_then(|(command, doing)| {
        let keys = crate::keymap::hint(command, cx)?;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_tertiary)
                .child("Press")
                .child(ui::kbd(keys, cx))
                .child(format!("to {doing}")),
        )
    });
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .pt(px(40.))
        .pb(px(24.))
        .child(ui::icon(glyph, 20.).text_color(t.text_tertiary))
        .child(ui::text(title, 13., FontWeight::MEDIUM).text_color(t.text_secondary))
        .children(press)
        .into_any_element()
}

/// The project's terminal tabs, or every project's, to bring forward or start.
fn terminals_page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let strips: Vec<Strip> = if bench.all_projects {
        bench
            .projects()
            .iter()
            .map(|project| Strip::Project(project.id.clone()))
            .collect()
    } else {
        bench.strip().into_iter().collect()
    };
    let rows = terminal_rows(bench, &strips, cx);
    let empty = rows.is_empty().then(|| {
        if strips.is_empty() {
            rail_empty(
                Icon::Terminal,
                "No project open",
                Some(("project.open", "open one")),
                cx,
            )
        } else {
            let title = match (bench.all_projects, bench.project(), other_host(bench)) {
                (false, Some(project), Some(host)) => {
                    format!("No terminals in {} on {host}", project.name)
                }
                _ => "No terminals open".to_owned(),
            };
            rail_empty(
                Icon::Terminal,
                title,
                Some(("terminal.new", "start one")),
                cx,
            )
        }
    });
    page_body()
        .child(page_header("Terminals", bench, cx))
        .child(
            div()
                .flex_none()
                .pt(px(4.))
                .px(px(10.))
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Plus,
                        "New terminal",
                        "terminal.new",
                        cx,
                    )
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(div().mt(px(8.)).child(scope_switch(bench, cx))),
        )
        .child(
            div()
                .id("terminals-scroll")
                .flex_1()
                .min_h_0()
                .pt(px(8.))
                .px(px(10.))
                .pb(px(16.))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(px(2.)).children(rows)),
        )
        .into_any_element()
}

/// A row for each terminal tab of `strips`, which brings it to the front.
fn terminal_rows(
    bench: &Workbench,
    strips: &[Strip],
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let front = bench.tab().map(|tab| tab.id.clone());
    let mut rows = Vec::new();
    for strip in strips {
        let project = match strip {
            Strip::Project(id) if bench.all_projects => bench
                .projects()
                .iter()
                .find(|project| &project.id == id)
                .map(|project| project.name.clone()),
            _ => None,
        };
        for tab in bench.tabs(strip) {
            if !matches!(
                crate::tabs::first_view(&tab.pane),
                Some((PaneKind::Terminal, _))
            ) {
                continue;
            }
            let (shown, id) = (strip.clone(), tab.id.clone());
            let row = Row::new(
                SharedString::from(format!("terminal-{}", tab.id)),
                crate::tabs::label(bench, tab),
            )
            .mark(22., ui::icon(Icon::Terminal, 15.).text_color(t.live))
            .chosen(front.as_deref() == Some(tab.id.as_str()))
            .when_some(project.clone(), |row, project| {
                row.subtitle(ui::meta(project, cx))
            })
            .on_click(cx.listener(move |bench, _, _, _| bench.activate_tab(&shown, &id)));
            rows.push(
                div()
                    .id(SharedString::from(format!("terminal-menu-{}", tab.id)))
                    .child(row)
                    .context_menu(crate::tabs::tab_menu(bench, strip, tab, cx))
                    .into_any_element(),
            );
        }
    }
    rows
}

/// What an SSH host without Sikemux offers: a login there, or installing Sikemux, over its
/// terminals (ADR 0019).
fn ssh_page(bench: &Workbench, alias: String, cx: &mut Context<Workbench>) -> AnyElement {
    let rows = terminal_rows(bench, &[Strip::Host(alias.clone())], cx);
    let empty = rows.is_empty().then(|| {
        rail_empty(
            Icon::Terminal,
            "No terminals open",
            Some(("terminal.new", "start one")),
            cx,
        )
    });
    page_body()
        .child(page_header(alias.clone(), bench, cx))
        .child(
            div()
                .flex_none()
                .pt(px(4.))
                .px(px(10.))
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Terminal,
                        "New terminal",
                        "terminal.new",
                        cx,
                    )
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    new_row(
                        "install-sikemux",
                        Icon::Download,
                        "Install Sikemux",
                        "agent.new",
                        cx,
                    )
                    .on_click(cx.listener(move |bench, _, _, _| bench.open_install_pane(&alias))),
                ),
        )
        .child(
            div()
                .id("ssh-terminals-scroll")
                .flex_1()
                .min_h_0()
                .pt(px(8.))
                .px(px(10.))
                .pb(px(16.))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(px(2.)).children(rows)),
        )
        .into_any_element()
}

/// This project's things, or every project's.
fn scope_switch(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let scope = vec![
        (
            Icon::Folder,
            SharedString::from(
                bench
                    .project()
                    .map(|project| project.name.clone())
                    .unwrap_or_default(),
            ),
        ),
        (Icon::Stack, SharedString::from("All projects")),
    ];
    Segmented::new("scope", scope, usize::from(bench.all_projects)).on_select(cx.listener(
        |bench, all: &usize, _, cx| {
            bench.all_projects = *all == 1;
            bench.keep_rails(cx);
            cx.notify();
        },
    ))
}
