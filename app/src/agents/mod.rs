use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, fill, point, px, radians, relative, size, Anchor, AnyElement, App, Bounds,
    ClickEvent, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Transformation, Window,
};
use sikemux_core::agent_accounts::usage::{Usage, UsageWindow};
use sikemux_core::agent_accounts::Profile;
use sikemux_core::server::AccountUsage;
use sikemux_wire::protocol::{LauncherInfo, PaneKind, RecentInfo};

use crate::agents::menu::AgentMenu;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_6, TEXT_2XS,
    TEXT_3XS, TEXT_3XS_H, TEXT_MD, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, Field, Group, IconButton, Row, Segmented};
use crate::workbench::{AgentRow, Edge, RailPage, RailWidth, Strip, TeamLink, Workbench};
use sikemux_wire::inbox::RemoteAgent;

pub const TAB_RAIL: RailWidth = RailWidth {
    min: 240.,
    max: 560.,
    initial: 288.,
};

pub mod card;
pub mod menu;
pub mod palette;
pub mod review;
pub mod setup;

pub fn tab_rail(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let frame = bench.rails.frame.clone();
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
        .filter(|_| !bench.rails.all_projects)
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
    let mut recent: Vec<_> = project
        .map(|project| bench.recent_of(&project.id))
        .unwrap_or_default()
        .into_iter()
        .filter(|recent| !recent.pinned || bench.rails.all_projects)
        .map(|recent| recent_row(bench, recent, cx))
        .collect();
    if bench.has_older() && !recent.is_empty() {
        recent.push(older_when_seen(bench, cx));
    }
    let empty = (rows.is_empty() && pinned.is_empty() && recent.is_empty()).then(|| {
        let shown = (project.zip(other_host(bench)))
            .filter(|_| bench.filter_text().is_empty())
            .map(|(project, host)| match bench.rails.all_projects {
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
                            bench.open_settings(crate::settings::Page::Installed, window, cx)
                        }),
                    ),
                )
                .into_any_element()
        } else if !bench.filter_text().is_empty() {
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
                .mt(SPACE_2H)
                .mx(SPACE_3)
                .mb(SPACE_0)
                .child(Field::new(&filter.input).icon(Icon::Search))
        }))
        .child(
            div()
                .flex_none()
                .pt(SPACE_1)
                .px(SPACE_2H)
                .children(
                    other_host(bench)
                        .map(|host| ui::meta(format!("on {host}"), cx).px(SPACE_2).pt(SPACE_1)),
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
                .child(div().mt(SPACE_2).child(scope_switch(bench, cx))),
        )
        .child(faded(
            div()
                .id("tab-rail-scroll")
                .track_scroll(&bench.rails.scroll)
                .on_scroll(cx.listener(|bench, _, _, cx| bench.hold_card(cx)))
                .size_full()
                .pt(px(EDGE_FADE))
                .px(SPACE_2H)
                .pb(px(EDGE_FADE))
                .overflow_y_scroll()
                .children(empty)
                .child({
                    let glide = bench.rails.glide.borrow();
                    let skip = usize::from(copy.is_some());
                    div().on_children_prepainted(glide.keep_origin()).child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap(SPACE_0)
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
            cx,
        ))
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
        .pt(SPACE_1H)
        .pr(SPACE_1H)
        .pb(SPACE_1H)
        .pl(SPACE_2H)
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .min_h(px(28.))
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .child(title)
                .child(
                    div().flex().gap(SPACE_0).children(buttons).child(
                        IconButton::new(
                            "hide-tab-rail",
                            Icon::PanelRight,
                            if bench.rails.tab_open {
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
        _ if bench.rails.all_harnesses => (
            ui::icon(Icon::Inbox, 16.)
                .text_color(t.text_secondary)
                .into_any_element(),
            "All harnesses".to_owned(),
        ),
        Some(launcher) => (
            ui::mark(&launcher.provider, 16.),
            harness_label(bench, launcher),
        ),
        None => {
            return ui::text("Agents", TEXT_SM, FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .into_any_element()
        }
    };
    let name = ui::text(label, TEXT_SM, FontWeight::SEMIBOLD)
        .letter_spacing(-0.015)
        .text_color(t.text_primary);
    let mut options: Vec<(String, String, Option<f64>)> = Vec::new();
    for launcher in bench.launchers() {
        if options.iter().all(|(kind, ..)| *kind != launcher.provider) {
            let current = bench
                .launcher_of_kind(&launcher.provider)
                .unwrap_or(launcher);
            options.push((
                launcher.provider.clone(),
                harness_label(bench, current),
                bench.usage_peak(&current.id),
            ));
        }
    }
    if options.len() < 2 {
        return div()
            .flex()
            .items_center()
            .gap(px(7.))
            .child(mark)
            .child(name)
            .into_any_element();
    }
    let chosen = if bench.rails.all_harnesses {
        None
    } else {
        launcher.map(|launcher| launcher.provider.clone())
    };
    let choose = Rc::new(cx.listener(|bench, choice: &Option<String>, _, cx| {
        bench.rails.all_harnesses = choice.is_none();
        if choice.is_some() {
            bench.launcher_choice = choice.clone();
        }
        bench.keep_rails(cx);
        cx.notify();
    }));
    let glyph_ink = t.text_tertiary;
    ui::menu_button(
        "agent-picker",
        Anchor::TopLeft,
        BaseButton::new("agent-picker-button")
            .accessibility_label("Agent provider")
            .h(px(28.))
            .ml(px(-6.))
            .px(SPACE_1H)
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
        move |mut popup, _, cx| {
            let item = |row: ui::MenuRow, choice: Option<String>, cx: &App| {
                let choose = choose.clone();
                row.item(cx)
                    .checked(choice == chosen)
                    .on_click(move |_, window, cx| choose(&choice, window, cx))
            };
            popup = popup.item(item(
                ui::MenuRow::new("All harnesses")
                    .mark(move || ui::icon(Icon::Inbox, 13.).text_color(glyph_ink)),
                None,
                cx,
            ));
            for (kind, label, peak) in &options {
                let provider = kind.clone();
                popup = popup.item(item(
                    ui::MenuRow::new(label.clone())
                        .mark(move || ui::mark(&provider, 14.))
                        .when_some(*peak, |row, peak| row.hint(format!("{}%", peak.round()))),
                    Some(kind.clone()),
                    cx,
                ));
            }
            popup
        },
    )
    .into_any_element()
}

/// The agent's own name, such as "Claude", whichever account it starts on.
fn harness_label(bench: &Workbench, launcher: &LauncherInfo) -> String {
    bench.agent_name(&launcher.provider).to_owned()
}

/// A saved chat no agent holds, to take up again: how long ago it ran, or a pin when it is
/// pinned, and Pin or Unpin on a right-click.
fn recent_row(bench: &Workbench, recent: &RecentInfo, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tail = match recent.pinned {
        true => ui::gives_way(
            RECENT_ROW,
            ui::icon(Icon::Pin, 11.).text_color(t.text_tertiary),
        )
        .into_any_element(),
        false => ui::mono(ui::time::age(recent.active_at), TEXT_2XS, cx).into_any_element(),
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
        move |bench, _, _, cx| bench.resume_chat(&recent, cx)
    }));
    let pin = pin_chat(&cx.entity(), recent);
    let unpin = recent.pinned.then(|| {
        let unpin = pin.1.clone();
        ui::hover_actions(RECENT_ROW)
            .top_0()
            .h(px(if bench.rail_compact() { 27. } else { 28. }))
            .child(
                IconButton::new(
                    SharedString::from(format!("unpin-{}", recent.id)),
                    Icon::Pin,
                    format!("Unpin {}", recent.title),
                )
                .small()
                .pressed(true)
                .on_click(move |_, window, cx| unpin(window, cx)),
            )
    });
    let deletable = bench
        .reads
        .catalog
        .kind(&recent.provider)
        .is_some_and(|kind| kind.deletable_chats);
    let (this, chat) = (cx.weak_entity(), recent.clone());
    div()
        .id(SharedString::from(format!("recent-menu-{}", recent.id)))
        .group(RECENT_ROW)
        .relative()
        .child(row)
        .children(unpin)
        .context_menu(ui::menu(move |popup, _, cx| {
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
                        .on_click(on(|bench, chat, cx| bench.resume_chat(chat, cx))),
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
                ui::MenuRow::new("Delete…")
                    .icon(Icon::Trash)
                    .danger(true)
                    .item(cx)
                    .on_click(on(delete_recent)),
            )
        }))
        .into_any_element()
}

/// The foot of Recent, which brings the next page of older chats once it scrolls into view.
fn older_when_seen(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let (scroll, this) = (bench.rails.scroll.clone(), cx.weak_entity());
    canvas(
        move |bounds, _, cx| {
            if bounds.top() <= scroll.bounds().bottom() {
                cx.defer(move |cx| {
                    let _ = this.update(cx, |bench, cx| {
                        bench.look_for_older(true, crate::recent::PAGE, cx)
                    });
                });
            }
        },
        |_, _, _, _| {},
    )
    .w_full()
    .h(px(1.))
    .into_any_element()
}

/// Asks for a recent chat's new title, and renames it in its CLI's own history.
fn rename_recent(bench: &mut Workbench, chat: &RecentInfo, cx: &mut Context<Workbench>) {
    let (recent, cwd) = (chat.id.clone(), chat.cwd.clone());
    bench.prompt(
        "Rename chat",
        chat.title.clone(),
        move |bench, title, _, _| {
            if !title.trim().is_empty() {
                bench.call(
                    "renameRecent",
                    serde_json::json!({ "recent": recent, "title": title.trim(), "cwd": cwd }),
                );
                bench.retitle_older(&recent, title.trim());
            }
        },
        cx,
    );
}

/// Deletes a recent chat from its CLI's own history, once the person says so.
fn delete_recent(bench: &mut Workbench, chat: &RecentInfo, cx: &mut Context<Workbench>) {
    let (recent, cwd) = (chat.id.clone(), chat.cwd.clone());
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
                bench.call(
                    "deleteRecent",
                    serde_json::json!({ "recent": recent, "cwd": cwd }),
                );
                bench.forget_older(&recent);
            }
        },
        cx,
    );
}

/// Something a click on the rail does.
type Click = Rc<dyn Fn(&mut Window, &mut App)>;

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
        .rails
        .glide
        .borrow_mut()
        .choose(chosen, cx.reduce_motion());
    let teams = bench.filter_text().is_empty();
    let remote = if teams {
        remote_workers(bench)
    } else {
        Vec::new()
    };
    let remote_of = |members: &[&AgentRow]| -> Vec<&RemoteAgent> {
        remote
            .iter()
            .copied()
            .filter(|worker| {
                members
                    .iter()
                    .any(|member| worker.lead.as_deref() == Some(member.id.as_str()))
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
                folded = bench.rails.folded_teams.contains(&agent.id);
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
        bench.rails.all_projects,
        bench.project().map_or("", |project| project.id.as_str()),
        bench.filter_text()
    );
    let lasting = crate::motion::ROW_DEPARTURE
        .close
        .duration
        .max(crate::motion::ARRIVE_SLIDE.duration);
    let mut seen = bench.rails.seen.borrow_mut();
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
    let mut glide = bench.rails.glide.borrow_mut();
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
fn remote_workers(bench: &Workbench) -> Vec<&RemoteAgent> {
    bench
        .shown()
        .into_iter()
        .flat_map(|view| &view.inbox.remote_agents)
        .filter(|worker| worker.team.is_some())
        .collect()
}

/// A worker on a linked host, in its lead's team. It has no chat here to open.
fn remote_row(worker: &RemoteAgent, cx: &mut Context<Workbench>) -> AnyElement {
    let team = match worker.team {
        Some(TeamLink::Last) => Some(TeamLink::Last),
        _ => Some(TeamLink::Member),
    };
    let reachable = worker.reachable;
    let host = &worker.host;
    let title = format!(
        "@{} on {host}{}",
        worker.name,
        if reachable { "" } else { ", out of reach" }
    );
    let status = reachable.then_some(worker.status);
    let tip = if reachable {
        format!("Runs on {host}")
    } else {
        format!("{host} is out of reach")
    };
    div()
        .id(SharedString::from(format!("remote-{}", worker.agent_id)))
        .relative()
        .pl(px(18.))
        .tooltip(ui::tooltip(tip))
        .child(
            Row::new(
                SharedString::from(format!("remote-row-{}", worker.agent_id)),
                title,
            )
            .mark(22., ui::mark(&worker.cli, 20.))
            .quiet()
            .when_some(status, |row, status| {
                row.status(ui::status_mark(&worker.agent_id, status, cx))
            }),
        )
        .children(team_lines(team, false, cx))
        .into_any_element()
}

impl Workbench {
    /// Folds a lead's team away, or brings it back.
    pub fn fold_team(&mut self, lead: &str, cx: &mut Context<Self>) {
        if !self.rails.folded_teams.remove(lead) {
            self.rails.folded_teams.insert(lead.to_owned());
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
        .rails
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
                    .child(agent.mark(20., cx)),
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
        None => agent.mark(20., cx),
    };
    let status = ui::status_mark(&agent.id, agent.status, cx);
    let fades = status.is_none();
    let pin = bench
        .chat_of(&agent.id)
        .map(|chat| (chat.pinned, pin_chat(&cx.entity(), chat).1));
    let editing = bench
        .overlays
        .retitling
        .as_ref()
        .filter(|retitling| retitling.agent == agent.id && retitling.in_rail)
        .map(|retitling| crate::home::retitle_field(retitling, 13., cx).w_full());
    let row = Row::new(
        SharedString::from(format!("agent-{}", agent.id)),
        agent.title.clone(),
    )
    .title_view(editing)
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
                .line_height(crate::theme::ui_text(TEXT_MD))
                .child(ui::icon(Icon::Folder, 11.).text_color(t.live))
                .child(ui::meta(project, cx)),
        )
    })
    .status(status)
    .on_click(cx.listener({
        let id = id.clone();
        move |bench, event: &ClickEvent, window, cx| {
            if bench
                .overlays
                .retitling
                .as_ref()
                .is_some_and(|retitling| retitling.agent == id)
            {
                return;
            }
            match event.click_count() {
                2 => bench.retitle(&id, true, window, cx),
                _ => bench.open_agent(&id),
            }
        }
    }))
    .children(agent.name.clone().map(|name| {
        let name = ui::meta(format!("@{name}"), cx).pl(SPACE_2).pr(SPACE_1);
        match fades {
            true => ui::gives_way(AGENT_ROW, name).into_any_element(),
            false => name.into_any_element(),
        }
    }))
    .children(fold.as_ref().filter(|fold| !fold.open).map(|fold| {
        div()
            .flex_none()
            .px(px(5.))
            .rounded_full()
            .bg(t.surface_hover)
            .text_color(t.text_tertiary)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .line_height(crate::theme::ui_text(15.))
            .font_features(crate::theme::tabular())
            .child(format!("+{}", fold.count))
    }))
    .children(counts(agent, cx).map(|counts| match fades {
        true => ui::gives_way(AGENT_ROW, counts).into_any_element(),
        false => counts.into_any_element(),
    }));
    let lines = team_lines(team, matches!(fold, Some(Fold { open: true, .. })), cx);
    let card = crate::agents::card::view(bench, agent, project.as_deref(), cx);
    let close = agent.id.clone();
    let actions = ui::hover_actions(AGENT_ROW)
        .top_0()
        .h(px(if bench.rail_compact() { 27. } else { 28. }))
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
            cx.listener(move |bench, _, _, cx| bench.close_agents(vec![close.clone()], cx)),
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
        .on_hover({
            let id = id.clone();
            cx.listener(move |bench, inside: &bool, _, cx| bench.rest_on_agent(&id, *inside, cx))
        })
        .on_mouse_move(cx.listener(move |bench, _, _, cx| bench.move_on_agent(&id, cx)))
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            cx.listener(|bench, _, _, cx| bench.close_card(cx)),
        )
        .child(row)
        .children(called(agent, cx))
        .child(actions)
        .children(lines)
        .when(card.is_some(), |wrap| {
            wrap.child(crate::agents::card::follow_row(cx.entity()))
        })
        .children(card.map(|card| div().absolute().top_0().left_0().child(card)))
        .context_menu(ui::menu(move |popup, _, _| match &menu {
            Some(menu) => menu.build(popup),
            None => popup,
        }))
}

/// A band of light that passes once over the row each time the agent is named with `@` or
/// messaged by another agent.
pub fn called(agent: &AgentRow, cx: &App) -> Option<AnyElement> {
    let called_at = agent.called_at?;
    let lit = Hsla {
        a: 0.09,
        ..Tokens::get(cx).text_primary
    };
    let clear = Hsla { a: 0., ..lit };
    Some(
        motion::replay(
            format!("called:{}", agent.id),
            called_at,
            motion::CALLED,
            move |through| {
                let band = |from: Hsla, to: Hsla| {
                    div()
                        .w(relative(0.5))
                        .h_full()
                        .bg(gpui_kit::linear_gradient(
                            90.,
                            gpui_kit::linear_color_stop(from, 0.),
                            gpui_kit::linear_color_stop(to, 1.),
                        ))
                };
                div()
                    .absolute()
                    .inset_0()
                    .rounded(theme::RADIUS_2)
                    .overflow_hidden()
                    .when(through > 0. && through < 1., |light| {
                        light.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(relative(through * 1.6 - 0.6))
                                .w(relative(0.6))
                                .flex()
                                .child(band(clear, lit))
                                .child(band(lit, clear)),
                        )
                    })
                    .into_any_element()
            },
        )
        .into_any_element(),
    )
}

/// The hover group of an agent's rail row, whose pin and close buttons show on hover.
const AGENT_ROW: &str = "agent-row";

/// The hover group of a saved chat's rail row, whose unpin button shows on hover.
const RECENT_ROW: &str = "recent-row";

pub fn counts(agent: &AgentRow, cx: &App) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    let waits = !matches!(
        agent.status,
        sikemux_wire::protocol::AgentStatus::Working
            | sikemux_wire::protocol::AgentStatus::Blocked
            | sikemux_wire::protocol::AgentStatus::Starting
    );
    let trigger = agent.next_trigger.filter(|_| waits);
    (agent.subagents > 0 || agent.tasks > 0 || trigger.is_some()).then(|| {
        let count = |glyph: Icon, size: f32, color, value: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .child(ui::icon(glyph, size).text_color(color))
                .child(
                    ui::text(value.to_string(), TEXT_2XS, FontWeight::MEDIUM)
                        .font_features(crate::theme::tabular()),
                )
        };
        div()
            .flex()
            .gap(SPACE_1H)
            .text_color(t.text_secondary)
            .when(agent.subagents > 0, |row| {
                row.child(count(Icon::Agent, 10., t.live, agent.subagents))
            })
            .when(agent.tasks > 0, |row| {
                row.child(count(Icon::Command, 11., t.warn, agent.tasks))
            })
            .children(trigger.map(|at| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(ui::icon(Icon::Timer, 11.).text_color(t.cmd))
                    .child(
                        ui::text(ui::time::until(at), TEXT_2XS, FontWeight::MEDIUM)
                            .font_features(crate::theme::tabular()),
                    )
            }))
    })
}

/// The account new chats here start with, and the person's other accounts with the agent, which
/// open below it while the pointer rests on the panel.
fn limits(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    if bench.rails.all_harnesses {
        return None;
    }
    let launcher = bench.launcher()?;
    let compact = bench.rail_compact();
    let provider = launcher.provider.clone();
    let agent = bench.agent_name(&provider).to_owned();
    let current = bench.accounts.current(&provider);
    let seen = |profile: &Profile| {
        let status =
            crate::accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
        Shown {
            id: Some(profile.id.clone()),
            profile: Some(profile.clone()),
            name: crate::accounts::name(bench, profile, status),
            usage: crate::accounts::usage(bench, profile).cloned(),
            signed_out: status.is_some_and(|status| !status.signed_in),
        }
    };
    let top = match current {
        Some(profile) => seen(profile),
        None => Shown {
            id: None,
            profile: None,
            name: agent.clone(),
            usage: bench.usage_of(&launcher.id).cloned(),
            signed_out: launcher.status.as_deref() == Some("signedOut"),
        },
    };
    let others: Vec<Shown> = bench
        .accounts
        .of(&provider)
        .into_iter()
        .filter(|profile| Some(&profile.id) != current.map(|current| &current.id))
        .map(seen)
        .collect();
    let open = bench.accounts.limits_open && !others.is_empty();
    let (top_label, top_signed_out) = (top.name.clone(), top.signed_out);
    let top = if top.signed_out {
        let title = match top.id {
            Some(_) => format!("Sign in to {}", top.name),
            None => format!("Sign in to {agent}"),
        };
        let line = match top.id {
            Some(_) => "This account is signed out",
            None => "See your 5-hour and weekly limits",
        };
        let (provider, id) = (provider.clone(), top.id.clone());
        sign_in_card(&launcher.provider, title, line, cx)
            .on_click(cx.listener(move |bench, _, window, cx| {
                crate::accounts::open_sign_in(bench, &provider, id.as_deref(), window, cx)
            }))
            .into_any_element()
    } else {
        limit_rows(&top, compact, cx)
    };
    let top_name = (!top_signed_out).then(|| {
        ui::text(top_label.clone(), TEXT_2XS, FontWeight::MEDIUM)
            .px(px(3.))
            .truncate()
            .text_color(t.text_primary)
    });
    let more = (!others.is_empty()).then(|| {
        let tall: f32 = others
            .iter()
            .map(|shown| shown.height(compact) + BLOCK_GAP)
            .sum();
        let blocks: Vec<AnyElement> = others
            .iter()
            .map(|shown| account_block(shown, &provider, compact, cx))
            .collect();
        let reveal = motion::animated(
            "limits-others",
            if open { 1. } else { 0. },
            motion::UNFOLD,
            move |share| {
                div()
                    .h(px(tall * share))
                    .overflow_hidden()
                    .opacity(share)
                    .flex()
                    .flex_col()
                    .pt(px(BLOCK_GAP))
                    .gap(px(BLOCK_GAP))
                    .children(blocks)
                    .into_any_element()
            },
        );
        let chevron = motion::turn(
            "limits-more",
            if open { -FRAC_PI_2 } else { FRAC_PI_2 },
            motion::LIST_FOLD,
            ui::icon(Icon::Chevron, 9.).text_color(t.text_tertiary),
        );
        (
            reveal,
            div()
                .h(px(14.))
                .mt(SPACE_0)
                .flex()
                .justify_center()
                .items_center()
                .child(chevron),
        )
    });
    let (reveal, chevron) = more.unzip();
    let bottom = match (chevron.is_some(), compact) {
        (true, true) => 4.,
        (true, false) => 6.,
        (false, true) => 12.,
        (false, false) => 16.,
    };
    let header = div()
        .h(px(18.))
        .mb(px(if compact { 8. } else { 9. }))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text("LIMITS", TEXT_2XS, FontWeight::MEDIUM)
                .letter_spacing(0.08)
                .text_color(t.text_tertiary),
        )
        .child(div().flex_1().h(px(1.)).bg(t.border))
        .children(top_name)
        .child({
            let provider = provider.clone();
            IconButton::new(
                "refresh-limits",
                Icon::Refresh,
                format!("Refresh {agent} plan limits"),
            )
            .small()
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.read_usage();
                crate::accounts::ask_again(bench, &provider, cx);
            }))
        })
        .child({
            let provider = provider.clone();
            IconButton::new("add-account", Icon::Plus, format!("Add a {agent} account"))
                .small()
                .on_click(cx.listener(move |bench, _, window, cx| {
                    crate::accounts::open_sign_in(bench, &provider, None, window, cx)
                }))
        });
    Some(
        div()
            .flex_none()
            .mt(px(if compact { 10. } else { 12. }))
            .mx(SPACE_2H)
            .mb(px(bottom))
            .child(header)
            .child(
                div()
                    .id("limits-content")
                    .on_hover(cx.listener(|bench, inside: &bool, _, cx| {
                        if bench.accounts.limits_open != *inside {
                            bench.accounts.limits_open = *inside;
                            cx.notify();
                        }
                    }))
                    .child(top)
                    .children(reveal)
                    .children(chevron),
            ),
    )
}

const BLOCK_GAP: f32 = 10.;
const BLOCK_PAD_TOP: f32 = 7.;
const BLOCK_PAD_BOTTOM: f32 = 9.;
const NAME_LINE: f32 = 16.;
const NAME_GAP: f32 = 6.;
const LIMIT_ROW: f32 = 18.;
const SIGNED_OUT_LINE: f32 = 30.;

fn row_gap(compact: bool) -> f32 {
    if compact {
        2.
    } else {
        3.
    }
}

/// One account as the Limits panel shows it.
struct Shown {
    /// The account, or none for an agent the person never added an account to.
    id: Option<String>,
    profile: Option<Profile>,
    name: String,
    usage: Option<AccountUsage>,
    signed_out: bool,
}

impl Shown {
    fn reading(&self) -> Option<&Usage> {
        self.usage.as_ref()?.usage.as_ref()
    }

    fn windows(&self) -> &[UsageWindow] {
        self.reading()
            .map_or(&[][..], |usage| usage.windows.as_slice())
    }

    /// How tall its block draws, so the panel can open onto every account at its full height.
    fn height(&self, compact: bool) -> f32 {
        if self.signed_out {
            return SIGNED_OUT_LINE;
        }
        let rows = self.windows().len().max(1) as f32;
        BLOCK_PAD_TOP
            + NAME_LINE
            + NAME_GAP
            + rows * LIMIT_ROW
            + (rows - 1.) * row_gap(compact)
            + BLOCK_PAD_BOTTOM
    }

    /// When a used-up window of the account's plan is back.
    fn back(&self) -> Option<String> {
        let full = self
            .windows()
            .iter()
            .find(|window| window.used_percent >= 100.)?;
        let label = &full.label;
        let back = reset_at(full).map(day_and_time)?;
        Some(format!("{label} full · back {back}"))
    }
}

/// What an account's plan reports: a row for each window, or why there are none.
fn limit_rows(shown: &Shown, compact: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let problem = shown
        .usage
        .as_ref()
        .and_then(|usage| usage.problem.as_ref());
    let windows = shown.windows();
    let quiet = |text: String| {
        ui::text(text, TEXT_3XS_H, FontWeight::NORMAL)
            .h(px(LIMIT_ROW))
            .px(SPACE_2)
            .truncate()
            .italic()
            .text_color(t.text_tertiary)
    };
    if shown.reading().is_none() && problem.is_none() {
        quiet("Refresh to read plan limits.".into()).into_any_element()
    } else if problem.is_some() {
        quiet("Could not read plan limits. Refresh to try again.".into()).into_any_element()
    } else if windows.is_empty() {
        let reason = (shown.reading())
            .and_then(|usage| usage.unavailable_reason.as_deref())
            .unwrap_or("This account did not report plan limits.");
        quiet(reason.to_owned()).into_any_element()
    } else {
        let key = shown.id.clone().unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(row_gap(compact)))
            .children(windows.iter().map(|window| limit_row(window, &key, cx)))
            .into_any_element()
    }
}

/// Another of the person's accounts: its name, when a full window is back, and its rows; a click
/// starts new chats on it.
fn account_block(
    shown: &Shown,
    provider: &str,
    compact: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = shown.id.clone().unwrap_or_else(|| provider.to_owned());
    let group = SharedString::from(format!("limits-account-{key}"));
    let id = SharedString::from(format!("limits-block-{key}"));
    if shown.signed_out {
        let (provider, account) = (provider.to_owned(), shown.id.clone());
        return div()
            .id(id)
            .h(px(SIGNED_OUT_LINE))
            .px(SPACE_2)
            .flex()
            .items_center()
            .child(
                ui::text(shown.name.clone(), TEXT_2XS, FontWeight::MEDIUM)
                    .flex_1()
                    .truncate()
                    .text_color(t.text_tertiary),
            )
            .child(
                ui::text("Sign in", TEXT_3XS_H, FontWeight::NORMAL)
                    .id(SharedString::from(format!("limits-sign-in-{key}")))
                    .cursor_pointer()
                    .text_color(t.text_secondary)
                    .underline()
                    .hover(|link| link.text_color(t.text_primary))
                    .on_click(cx.listener(move |bench, _, window, cx| {
                        crate::accounts::open_sign_in(
                            bench,
                            &provider,
                            account.as_deref(),
                            window,
                            cx,
                        )
                    })),
            )
            .into_any_element();
    }
    let line = div()
        .h(px(NAME_LINE))
        .mb(px(NAME_GAP))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text(shown.name.clone(), TEXT_2XS, FontWeight::MEDIUM)
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .children(shown.back().map(|back| {
                    ui::text(back, TEXT_3XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.danger)
                        .group_hover(group.clone(), |note| note.opacity(0.))
                }))
                .child(
                    ui::text("Use for new chats", TEXT_3XS, FontWeight::NORMAL)
                        .absolute()
                        .top_0()
                        .right_0()
                        .whitespace_nowrap()
                        .opacity(0.)
                        .text_color(t.text_primary)
                        .group_hover(group.clone(), |text| text.opacity(1.)),
                ),
        );
    let profile = shown.profile.clone();
    div()
        .id(id)
        .group(group.clone())
        .h(px(shown.height(compact)))
        .overflow_hidden()
        .pt(px(BLOCK_PAD_TOP))
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(|block| block.bg(t.surface_hover))
        .on_click(cx.listener(move |bench, _, _, cx| {
            if let Some(profile) = profile.as_ref() {
                crate::accounts::select(bench, profile, cx);
            }
        }))
        .child(line)
        .child(limit_rows(shown, compact, cx))
        .into_any_element()
}

/// One of a plan's rolling windows: how much of it is used, and when it starts again.
fn limit_row(window: &UsageWindow, key: &str, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let label = window.label.clone();
    let percent = window.used_percent.clamp(0., 100.);
    let signal = signal(percent, &t);
    let reset = reset_at(window);
    let note = match reset {
        Some(at) => format!(
            "Resets {}",
            at.with_timezone(&chrono::Local)
                .format("%-m/%-d/%Y, %-I:%M:%S %p")
        ),
        None => "Reset time unavailable".into(),
    };
    div()
        .id(SharedString::from(format!("limit-{key}-{label}")))
        .h(px(LIMIT_ROW))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2H)
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
        .child(ticks(percent, signal, &t).flex_1().h(px(10.)))
        .child(
            ui::text(
                format!("{}%", percent.round()),
                TEXT_2XS,
                FontWeight::NORMAL,
            )
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
            ui::text(ui::time::countdown(reset), TEXT_3XS, FontWeight::NORMAL)
                .w(px(42.))
                .flex_none()
                .text_right()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary),
        )
}

/// A bar of thin ticks, lit as far as `percent`.
fn ticks(percent: f64, signal: Hsla, t: &Tokens) -> gpui_kit::Canvas<()> {
    let unlit = Hsla {
        a: 0.10,
        ..t.text_primary
    };
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
}

/// When a window resets, as a person would say it: today, tomorrow, a weekday or a date.
pub(crate) fn day_and_time(at: chrono::DateTime<chrono::Utc>) -> String {
    let at = at.with_timezone(&chrono::Local);
    let days = (at.date_naive() - chrono::Local::now().date_naive()).num_days();
    let time = at.format("%-I:%M %p");
    match days {
        ..=0 => format!("today {time}"),
        1 => format!("tomorrow {time}"),
        2..=6 => format!("{} {time}", at.format("%a")),
        _ => format!("{}, {time}", at.format("%b %-d")),
    }
}

/// The call to sign in, in the agent's own colours: its mark, what signing in shows, and a button.
fn sign_in_card(
    provider: &str,
    title: String,
    line: &'static str,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    let brand = theme::brand(provider);
    let tint = |a: f32| Hsla { a, ..brand };
    div()
        .id("limits-sign-in")
        .mx(SPACE_0)
        .py(SPACE_2H)
        .px(px(11.))
        .flex()
        .items_center()
        .gap(px(11.))
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(tint(0.22))
        .bg(gpui_kit::linear_gradient(
            100.,
            gpui_kit::linear_color_stop(tint(0.13), 0.),
            gpui_kit::linear_color_stop(
                Hsla {
                    a: 0.02,
                    ..t.text_primary
                },
                0.7,
            ),
        ))
        .cursor_pointer()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_2)
                .bg(tint(0.16))
                .child(ui::mark(provider, 18.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    ui::text(title, TEXT_XS, FontWeight::SEMIBOLD)
                        .truncate()
                        .text_color(t.text_primary),
                )
                .child(
                    ui::text(line, TEXT_3XS_H, FontWeight::NORMAL)
                        .truncate()
                        .text_color(t.text_tertiary),
                ),
        )
        .child(
            ui::text("Sign in", TEXT_2XS, FontWeight::SEMIBOLD)
                .flex_none()
                .h(px(24.))
                .px(px(11.))
                .flex()
                .items_center()
                .rounded(theme::RADIUS_2)
                .bg(brand)
                .text_color(if brand.l > 0.7 {
                    gpui_kit::black()
                } else {
                    gpui_kit::white()
                }),
        )
}

/// The colour of a window used `percent` full: quiet until it nears its limit.
fn signal(percent: f64, t: &Tokens) -> Hsla {
    if percent >= 90. {
        t.danger
    } else if percent >= 70. {
        t.warn
    } else {
        Hsla {
            a: 0.62,
            ..t.text_primary
        }
    }
}

/// When a window starts again: Unix seconds, as a number or digits, or an ISO time.
fn reset_at(window: &UsageWindow) -> Option<chrono::DateTime<chrono::Utc>> {
    let at = window.resets_at.as_ref()?.unix_ms()?;
    chrono::DateTime::from_timestamp_millis(i64::try_from(at).ok()?)
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
            .pressed(bench.rails.page == shown)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.show_rail_page(shown, cx);
            }))
    };
    div()
        .flex_none()
        .h(px(35.))
        .px(SPACE_2)
        .py(SPACE_1)
        .flex()
        .justify_center()
        .gap(SPACE_1)
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
    let title = ui::text(title, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_primary);
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
                .gap(SPACE_1)
                .text_size(crate::theme::ui_text(TEXT_XS))
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
        .gap(SPACE_2)
        .pt(px(40.))
        .pb(SPACE_6)
        .child(ui::icon(glyph, 20.).text_color(t.text_tertiary))
        .child(ui::text(title, TEXT_SM, FontWeight::MEDIUM).text_color(t.text_secondary))
        .children(press)
        .into_any_element()
}

/// The project's terminal tabs, or every project's, to bring forward or start.
fn terminals_page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let strips: Vec<Strip> = if bench.rails.all_projects {
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
            let title = match (bench.rails.all_projects, bench.project(), other_host(bench)) {
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
                .pt(SPACE_1)
                .px(SPACE_2H)
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
                .child(div().mt(SPACE_2).child(scope_switch(bench, cx))),
        )
        .child(faded(
            div()
                .id("terminals-scroll")
                .size_full()
                .pt(px(EDGE_FADE))
                .px(SPACE_2H)
                .pb(px(EDGE_FADE))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(SPACE_0).children(rows)),
            cx,
        ))
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
            Strip::Project(id) if bench.rails.all_projects => bench
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
                    .context_menu(ui::menu(crate::tabs::tab_menu(bench, strip, tab, cx)))
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
                .pt(SPACE_1)
                .px(SPACE_2H)
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
        .child(faded(
            div()
                .id("ssh-terminals-scroll")
                .size_full()
                .pt(px(EDGE_FADE))
                .px(SPACE_2H)
                .pb(px(EDGE_FADE))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(SPACE_0).children(rows)),
            cx,
        ))
        .into_any_element()
}

/// How deep a rail list's ends fade; the list is padded as deep, so rows at rest sit clear of it.
const EDGE_FADE: f32 = 14.;

/// A rail list whose top and bottom fade into the panel as rows scroll under them.
fn faded(list: impl IntoElement, cx: &App) -> gpui_kit::Div {
    let wrap = div().relative().flex_1().min_h_0().child(list);
    if crate::see_through::clear(cx) {
        return wrap;
    }
    let surface = Tokens::get(cx).surface;
    let edge = |angle: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .h(px(EDGE_FADE))
            .bg(gpui_kit::linear_gradient(
                angle,
                gpui_kit::linear_color_stop(surface, 0.),
                gpui_kit::linear_color_stop(Hsla { a: 0., ..surface }, 1.),
            ))
    };
    wrap.child(edge(180.).top_0()).child(edge(0.).bottom_0())
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
    Segmented::new("scope", scope, usize::from(bench.rails.all_projects))
        .waiting(1, waiting_elsewhere(bench))
        .on_select(cx.listener(|bench, all: &usize, _, cx| {
            bench.rails.all_projects = *all == 1;
            bench.keep_rails(cx);
            cx.notify();
        }))
}

/// How many things in other projects wait on the person.
fn waiting_elsewhere(bench: &Workbench) -> usize {
    let (Some(view), Some(here)) = (bench.shown(), bench.project()) else {
        return 0;
    };
    let path = here.path.to_string_lossy();
    (view.inbox.needs.iter())
        .filter(|(project, _)| **project != here.id && **project != path)
        .map(|(_, count)| count)
        .sum()
}
