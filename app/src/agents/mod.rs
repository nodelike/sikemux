use crate::agents::limits::limits;
use crate::agents::terminal_pages::ssh_page;
use crate::agents::terminal_pages::terminals_page;
use crate::core::Target;
use crate::hosts::Place;
use crate::ui::Clicks;
use crate::workbench::actions;
use crate::workbench::actions::CommandId;
use sikemux_calls::agent_view::AgentViewKind;
use sikemux_calls::agent_view::AgentViewParams;
use sikemux_calls::call;
use sikemux_calls::saved_sessions::RecentChat;
use sikemux_calls::saved_sessions::RecentRenamed;
use sikemux_calls::workspace::PinChatParams;
use sikemux_wire::protocol::{AgentGroup, SessionId};
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, px, radians, relative, Anchor, AnyElement, App, Bounds, ClickEvent, Context,
    Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Transformation, Window,
};
use sikemux_view::device::RecentInfo;
use sikemux_view::workspace::{LauncherInfo, ProjectInfo, ProjectStanding};

use crate::agents::menu::AgentMenu;
use crate::agents::rows::AgentRow;
use crate::icons::Icon;
use crate::motion;
use crate::rail::{Rail, RailWidth};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, SPACE_6,
    TEXT_2XS, TEXT_3XS, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, Field, Group, IconButton, Row, Segmented};
use crate::workbench::Workbench;
use sikemux_view::inbox::RemoteAgent;
use sikemux_view::prefs::TabRailPage;

pub const TAB_RAIL: RailWidth = RailWidth {
    min: 240.,
    max: 560.,
    initial: 288.,
};

pub mod card;
pub(crate) mod limits;
pub(crate) mod marble;
pub mod menu;
pub mod palette;
pub(crate) mod retitle;
pub mod review;
pub mod rows;
pub mod settled;
pub mod setup;
pub(crate) mod tags;
mod terminal_pages;
pub(crate) mod tree;

pub fn tab_rail(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let frame = bench.rails.frame.clone();
    let rail = ui::panel(cx)
        .relative()
        .w(px(bench.rail_width(Rail::Agents)))
        .flex_none();
    crate::rail::keyed_rail(rail, Rail::Agents, bench, cx)
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
pub(crate) fn page_body() -> gpui_kit::Div {
    div().size_full().flex().flex_col().overflow_hidden()
}

fn page_panel(bench: &Workbench, page: TabRailPage, cx: &mut Context<Workbench>) -> AnyElement {
    match page {
        TabRailPage::Files => {
            return page_body()
                .child(files_header(bench, cx))
                .child(crate::files::page(bench, cx))
                .into_any_element();
        }
        TabRailPage::Terminals => return terminals_page(bench, cx),
        TabRailPage::Agents => {}
    }
    if let crate::hosts::HostChoice::Ssh(alias) = bench.chosen_host() {
        return ssh_page(bench, alias, cx);
    }
    let project = bench.project();
    let pins = project
        .filter(|_| !bench.rails.all_projects)
        .map(|project| bench.pinned_of(&project.id))
        .unwrap_or_default();
    let mut listed = bench.listed_agents();
    let chiefs: HashSet<String> = (listed.iter())
        .filter(|agent| agent.chief)
        .map(|agent| agent.id.clone())
        .collect();
    let mut chief_led = HashSet::new();
    for agent in &mut listed {
        if (agent.lead.as_ref()).is_some_and(|lead| chiefs.contains(lead)) {
            agent.lead = None;
            chief_led.insert(agent.id.clone());
        }
    }
    let pinned_ids: HashSet<&str> = (pins.iter())
        .filter_map(|pin| pin.agent_id.as_deref())
        .collect();
    let held_ids = tree::under(&listed, &pinned_ids);
    let (held, agents): (Vec<_>, Vec<_>) =
        (listed.into_iter()).partition(|agent| held_ids.contains(&agent.id));
    let away = |agent: &AgentRow| agent.snoozed_until.is_some() || agent.settled_at.is_some();
    let working: HashSet<&str> = (agents.iter())
        .filter(|agent| !away(agent))
        .map(|agent| agent.id.as_str())
        .collect();
    let leading = tree::above(&agents, &working);
    let (gone, agents): (Vec<_>, Vec<_>) =
        (agents.into_iter()).partition(|agent| away(agent) && !leading.contains(&agent.id));
    let snoozed: Vec<_> = (gone.into_iter())
        .filter(|agent| agent.settled_at.is_none())
        .collect();
    let away_rows = |agents: &[AgentRow], cx: &mut Context<Workbench>| -> Vec<AnyElement> {
        (tree::tree(agents, 0, &HashSet::new()).into_iter())
            .map(|placed| agent_row(bench, &placed.row, &placed.hang, None, false, cx))
            .collect()
    };
    let unseen = out_of_sight(bench, &agents);
    let snoozed = away_rows(&snoozed, cx);
    let homes = home_projects(bench);
    let place = bench.place();
    let mut projects: Vec<ProjectInfo> = match bench.rails.all_projects {
        true => ranked_projects(bench),
        false => bench.project().cloned().into_iter().collect(),
    };
    for project in &mut projects {
        if let Some(home) = home_project(bench, &homes, &place, &project.id) {
            *project = home.clone();
        }
    }
    let mut seen = HashSet::new();
    projects.retain(|project| seen.insert(project.id.clone()));
    let heads: Vec<(String, AnyElement)> = (projects.iter())
        .map(|project| {
            let chief = chief_of(project, &agents);
            let row = chief_row(bench, project, chief, bench.rails.all_projects, cx);
            (project.id.clone(), row)
        })
        .collect();
    let leading: Vec<String> = (projects.iter())
        .filter_map(|project| chief_of(project, &agents))
        .map(|chief| chief.id.clone())
        .collect();
    let mut agents: Vec<AgentRow> = (agents.into_iter())
        .filter(|agent| {
            !agent.chief || (agent.place != Place::This && !leading.contains(&agent.id))
        })
        .collect();
    if bench.rails.all_projects {
        let at = |project: &str| heads.iter().position(|(id, _)| id == project);
        agents.sort_by_key(|agent| at(home_of(&homes, agent)).unwrap_or(usize::MAX));
    }
    let (copy, rows) = team_rows(bench, &agents, &homes, heads, (&chiefs, chief_led), cx);
    let moving = bench.pin_move.as_ref().filter(|_| cx.has_active_drag());
    let tagged = bench.rails.tag.is_some();
    let pinned: Vec<_> = pins
        .iter()
        .enumerate()
        .filter(|(_, pin)| {
            !tagged
                || held
                    .iter()
                    .any(|agent| pin.agent_id.as_deref() == Some(agent.id.as_str()))
        })
        .filter_map(|(index, pin)| {
            let row = match pin.agent_id.as_deref() {
                Some(id) if held.iter().any(|agent| agent.id == id) => {
                    let mine = tree::under(&held, &HashSet::from([id]));
                    let team: Vec<AgentRow> = (held.iter())
                        .filter(|agent| mine.contains(&agent.id))
                        .cloned()
                        .collect();
                    let placed = tree::tree(&team, 0, &bench.rails.folded_teams);
                    if placed.first().is_none_or(|top| top.row.id != id) {
                        return None;
                    }
                    let rows = (placed.into_iter()).map(|placed| {
                        let fold = (placed.below > 0).then(|| Fold {
                            open: !bench.rails.folded_teams.contains(&placed.row.id),
                            count: placed.below,
                        });
                        agent_row(bench, &placed.row, &placed.hang, fold, false, cx)
                    });
                    div().flex().flex_col().children(rows).into_any_element()
                }
                _ => recent_row(bench, pin, cx),
            };
            Some(crate::drag::pinned(row, &pin.id, index, moving, &cx.entity()).into_any_element())
        })
        .collect();
    let mut recent: Vec<_> = project
        .map(|project| bench.recent_of(&project.id))
        .unwrap_or_default()
        .into_iter()
        .filter(|recent| !tagged && (!recent.pinned || bench.rails.all_projects))
        .map(|recent| recent_row(bench, recent, cx))
        .collect();
    if bench.has_older() && !recent.is_empty() && !bench.rails.folded(RECENT) {
        recent.push(older_when_seen(bench, cx));
    }
    if !tagged {
        recent.extend(other_hosts_line(bench, cx));
    }
    let quiet = rows.is_empty() && snoozed.is_empty();
    let empty = (quiet && pinned.is_empty() && recent.is_empty()).then(|| {
        let shown = (project.zip(other_host(bench)))
            .filter(|_| bench.filter_text().is_empty())
            .map(|(project, host)| match bench.rails.all_projects {
                true => t!("No chats on {host}", host = host),
                false => t!(
                    "No chats in {project} on {host}",
                    project = project.name,
                    host = host
                ),
            });
        if let Some(title) = shown {
            rail_empty(
                Icon::Agent,
                title,
                Some((actions::NewChat::ID, t!("to start one"))),
                cx,
            )
        } else if project.is_none() {
            rail_empty(
                Icon::Agent,
                t!("No project open"),
                Some((actions::OpenProject::ID, t!("to open one"))),
                cx,
            )
        } else if bench.launchers().is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(rail_empty(Icon::Agent, t!("No agents set up"), None, cx))
                .child(
                    ui::Button::new("set-up-agent", t!("Set one up"), ui::Variant::Outline)
                        .on_click(cx.listener(|bench, _, window, cx| {
                            bench.open_settings(crate::settings::Page::Installed, window, cx)
                        })),
                )
                .into_any_element()
        } else if !bench.filter_text().is_empty() || tagged {
            rail_empty(Icon::Search, t!("No chats match"), None, cx)
        } else {
            rail_empty(
                Icon::Agent,
                t!("No agents yet"),
                Some((actions::NewChat::ID, t!("to start one"))),
                cx,
            )
        }
    });
    page_body()
        .child(header(bench, cx))
        .children(tags::pill(bench, cx))
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
                .child(
                    new_row(
                        "new-chat",
                        Icon::Plus,
                        t!("New chat"),
                        actions::NewChat::ID,
                        cx,
                    )
                    .children(elsewhere_chip("new-chat-copy", bench, cx))
                    .on_click(cx.listener(|bench, _, window, cx| bench.new_chat(window, cx))),
                )
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Terminal,
                        t!("New terminal"),
                        actions::NewTerminal::ID,
                        cx,
                    )
                    .children(elsewhere_chip("new-terminal-copy", bench, cx))
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    new_row(
                        "new-browser",
                        Icon::Globe,
                        t!("New browser"),
                        actions::NewPage::ID,
                        cx,
                    )
                    .on_click(|_, window, cx| {
                        crate::keymap::dispatch(actions::NewPage::ID, window, cx)
                    }),
                )
                .child(div().mt(SPACE_2).child(scope_switch(bench, cx))),
        )
        .child(
            faded(
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
                    .when(!snoozed.is_empty(), |scroll| {
                        scroll.child(
                            folding(Group::new(t!("Snoozed"), ""), SNOOZED, bench, cx)
                                .children(snoozed),
                        )
                    })
                    .when(!pinned.is_empty(), |scroll| {
                        scroll.child(
                            folding(Group::new(t!("Pinned"), ""), PINNED, bench, cx)
                                .children(pinned),
                        )
                    })
                    .when(!recent.is_empty(), |scroll| {
                        scroll.child(
                            folding(Group::new(t!("Recent"), ""), RECENT, bench, cx)
                                .total(project.and_then(|project| bench.recent_total(project)))
                                .children(recent),
                        )
                    }),
                cx,
            )
            .children(unseen.chip(true, cx))
            .children(unseen.chip(false, cx)),
        )
        .children(limits(bench, cx))
        .into_any_element()
}

/// The keys the agent rail's groups are kept folded under.
const PINNED: &str = "pinned";
const RECENT: &str = "recent";
const SNOOZED: &str = "snoozed";

/// `group` folding on a click on its heading, and staying folded under `key` across restarts.
pub(crate) fn folding(
    group: Group,
    key: &'static str,
    bench: &Workbench,
    cx: &mut Context<Workbench>,
) -> Group {
    group.folds(
        bench.rails.folded(key),
        cx.listener(move |bench, _, _, cx| bench.fold_group(key, cx)),
    )
}

fn header(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    header_frame(
        picker(bench, cx),
        tags::button(bench, cx).into_iter().chain([
            IconButton::new("search", Icon::Search, t!("Filter recent chats"))
                .pressed(bench.filter.is_some())
                .on_click(cx.listener(|bench, _, window, cx| {
                    bench.toggle_filter(window, cx);
                    cx.notify();
                }))
                .into_any_element(),
            IconButton::new("start-agent", Icon::Plus, t!("Choose agent"))
                .on_click(|_, window, cx| {
                    crate::keymap::dispatch(actions::ChooseAgent::ID, window, cx)
                })
                .into_any_element(),
        ]),
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
        .border_b(crate::theme::border())
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
                                t!("Hide agents rail")
                            } else {
                                t!("Keep agents rail open")
                            },
                        )
                        .on_click(
                            cx.listener(|bench, _, _, cx| bench.toggle_rail(Rail::Agents, cx)),
                        ),
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
            t!("All harnesses").to_owned(),
        ),
        Some(launcher) => (
            ui::mark(&launcher.provider, 16.),
            harness_label(bench, launcher),
        ),
        None => {
            return ui::text(t!("Agents"), TEXT_SM, FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .into_any_element();
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
            .accessibility_label(t!("Agent provider"))
            .h(px(28.))
            .ml(px(-6.))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .gap(px(7.))
            .rounded(theme::radius_2())
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
                ui::MenuRow::new(t!("All harnesses"))
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

/// A saved chat no agent holds, to take up again: how many triggers wait for it, else how long
/// ago it ran, or a pin when it is pinned, and Pin or Unpin on a right-click. Resting on a chat
/// whose triggers wait shows them in a card.
fn recent_row(bench: &Workbench, recent: &RecentInfo, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let waiting = recent.triggers.len();
    let tail = match recent.pinned {
        _ if waiting > 0 => div()
            .flex()
            .items_center()
            .gap(px(3.))
            .text_color(t.text_tertiary)
            .child(ui::icon(Icon::Timer, 11.).text_color(t.text_tertiary))
            .child(ui::mono(waiting.to_string(), TEXT_2XS, cx))
            .into_any_element(),
        true => ui::gives_way(
            RECENT_ROW,
            ui::icon(Icon::Pin, 11.).text_color(t.text_tertiary),
        )
        .into_any_element(),
        false => ui::mono(ui::time::age(recent.active_at), TEXT_2XS, cx).into_any_element(),
    };
    let elsewhere = bench.found_host(&recent.id).map(|key| {
        let said = t!("On {host}", host = bench.host_name(key));
        host_mark(format!("host-of-{}", recent.id), said.into(), cx)
    });
    let row = Row::new(
        SharedString::from(format!("recent-{}", recent.id)),
        recent.title.clone(),
    )
    .mark(22., ui::mark(&recent.provider, 20.))
    .compact(bench.rail_compact())
    .when_some(bench.said_in(&recent.id), |row, said| {
        row.subtitle(crate::pickers::recent::said_line(said, cx))
    })
    .children(elsewhere)
    .child(tail)
    .on_click(cx.listener({
        let recent = recent.clone();
        move |bench, _, _, cx| bench.take_up_chat(&recent, cx)
    }));
    if bench.found_host(&recent.id).is_some() {
        return div()
            .id(SharedString::from(format!("recent-menu-{}", recent.id)))
            .child(row)
            .into_any_element();
    }
    let pin = pin_chat(&cx.entity(), recent);
    let unpin = recent.pinned.then(|| {
        let unpin = pin.1.clone();
        ui::hover_actions(RECENT_ROW)
            .top_0()
            .h(px(crate::theme::dense(if bench.rail_compact() {
                27.
            } else {
                28.
            })))
            .child(
                IconButton::new(
                    SharedString::from(format!("unpin-{}", recent.id)),
                    Icon::Pin,
                    t!("Unpin {title}", title = recent.title),
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
    let key = crate::agents::card::paused_key(&recent.id);
    let card = crate::agents::card::paused(bench, recent, cx);
    let dragged = crate::drag::DraggedChat {
        agent: None,
        recent: Some(recent.id.clone()),
        pinned: recent.pinned,
        label: recent.title.clone().into(),
    };
    let wrap = div()
        .id(SharedString::from(format!("recent-menu-{}", recent.id)))
        .group(RECENT_ROW)
        .relative();
    let (keyed_id, label) = (format!("recent-{}", recent.id), recent.title.clone());
    let row = crate::drag::chat(wrap, dragged, cx.weak_entity())
        .when(waiting > 0, |wrap| {
            let rest = key.clone();
            wrap.on_hover(cx.listener(move |bench, inside: &bool, _, cx| {
                bench.rest_on_agent(&rest, *inside, cx)
            }))
            .on_mouse_move(cx.listener(move |bench, _, _, cx| bench.move_on_agent(&key, cx)))
        })
        .child(row)
        .children(unpin)
        .when(card.is_some(), |wrap| {
            wrap.child(crate::agents::card::follow_row(cx.entity()))
        })
        .children(card.map(|card| div().absolute().top_0().left_0().child(card)))
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
                    PopupMenuItem::new(t!("Open"))
                        .icon(Icon::Message)
                        .on_click(on(|bench, chat, cx| bench.resume_chat(chat, cx))),
                )
                .item(
                    PopupMenuItem::new(t!("Rename…"))
                        .icon(Icon::Pencil)
                        .on_click(on(rename_recent)),
                )
                .item(pin);
            if !deletable {
                return popup;
            }
            popup.separator().item(
                ui::MenuRow::new(t!("Delete…"))
                    .icon(Icon::Trash)
                    .danger(true)
                    .item(cx)
                    .on_click(on(delete_recent)),
            )
        }));
    (bench.rails.keyed(Rail::Agents)).row(keyed_id, label, row, cx)
}

/// The foot of Recent, which brings the next page of older chats once it scrolls into view.
fn older_when_seen(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let (scroll, this) = (bench.rails.scroll.clone(), cx.weak_entity());
    canvas(
        move |bounds, _, cx| {
            if bounds.top() <= scroll.bounds().bottom() {
                cx.defer(move |cx| {
                    let _ = this.update(cx, |bench, cx| {
                        bench.look_for_older(true, crate::pickers::recent::PAGE, cx)
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

/// The quiet last line of Recent while the filter searches other hosts: how many are still
/// being asked, then which did not answer, with Try again for those alone.
fn other_hosts_line(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let older = bench
        .older
        .as_ref()
        .filter(|_| !bench.filter_text().is_empty())?;
    if !bench.searches_hosts() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let line = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .pt(SPACE_2)
        .pb(SPACE_1)
        .pl(px(46.))
        .pr(SPACE_4)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary);
    if !older.asking.is_empty() {
        let asking = match older.asking.len() {
            1 => SharedString::from(t!("Asking 1 more host…")),
            count => t!("Asking {count} more hosts…", count = count).into(),
        };
        return Some(
            line.child(ui::working(9., t.text_tertiary))
                .child(asking)
                .into_any_element(),
        );
    }
    if !bench.may_search_hosts() {
        let upgrade = div()
            .id("search-hosts-pro")
            .hover(|upgrade| upgrade.text_color(t.text_secondary))
            .child(t!("Pro searches every host's chats ›"))
            .clicks(cx.listener(|bench, _, window, cx| {
                bench.open_settings(crate::settings::Page::Account, window, cx)
            }));
        return Some(line.child(upgrade).into_any_element());
    }
    if older.unanswered.is_empty() {
        return bench
            .pro_free()
            .map(|free| line.child(free).into_any_element());
    }
    let names: Vec<SharedString> = (older.unanswered.iter())
        .map(|key| bench.host_name(key))
        .collect();
    let silent = t!("{hosts} didn't answer", hosts = names.join(", "));
    let unanswered = older.unanswered.clone();
    let again = div()
        .id("search-hosts-again")
        .text_color(t.text_secondary)
        .hover(|again| again.text_color(t.text_primary))
        .child(t!("Try again"))
        .clicks(cx.listener(move |bench, _, _, cx| bench.search_hosts(unanswered.clone(), cx)));
    Some(
        line.child(silent)
            .child("·")
            .child(again)
            .into_any_element(),
    )
}

/// Asks for a recent chat's new title, and renames it in its CLI's own history.
fn rename_recent(bench: &mut Workbench, chat: &RecentInfo, cx: &mut Context<Workbench>) {
    let (recent, cwd) = (chat.id.clone(), chat.cwd.clone());
    bench.prompt(
        t!("Rename chat"),
        chat.title.clone(),
        move |bench, title, _, _| {
            if !title.trim().is_empty() {
                bench
                    .target()
                    .call_typed::<call::RenameRecent>(RecentRenamed {
                        recent: recent.clone(),
                        title: title.trim().to_owned(),
                        cwd: Some(cwd.clone()),
                    });
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
        t!("Delete this chat?"),
        t!(
            "“{title}” will be removed from disk for good and can no longer be resumed.",
            title = chat.title
        ),
        t!("Delete"),
        true,
        move |bench, yes, _, _| {
            if yes {
                bench.target().call_typed::<call::DeleteRecent>(RecentChat {
                    recent: recent.clone(),
                    cwd: Some(cwd.clone()),
                });
                bench.forget_older(&recent);
            }
        },
        cx,
    );
}

type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// "Pin" or "Unpin" for the saved chat, and what does it.
fn pin_chat(this: &Entity<Workbench>, recent: &RecentInfo) -> (&'static str, Click) {
    let (this, id, pinned) = (this.clone(), recent.id.clone(), recent.pinned);
    let label = if pinned { t!("Unpin") } else { t!("Pin") };
    let pin = move |_: &mut Window, cx: &mut App| {
        let params = PinChatParams {
            recent: id.clone(),
            pinned: !pinned,
        };
        this.update(cx, |bench, _| {
            bench.target().call_typed::<call::PinChat>(params)
        });
    };
    (label, Rc::new(pin))
}

/// The copy switch, while the copy in front is on another host.
fn elsewhere_chip(
    id: &'static str,
    bench: &Workbench,
    cx: &Context<Workbench>,
) -> Option<AnyElement> {
    if bench.place() == Place::This {
        return None;
    }
    let folded = bench.folded_in_front()?;
    crate::copies::copy_chip(id, bench, &folded, cx)
        .map(|chip| div().pr(SPACE_1).child(chip).into_any_element())
}

fn new_row(
    id: &'static str,
    glyph: Icon,
    label: &'static str,
    command: CommandId,
    cx: &App,
) -> Row {
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
/// filtered and a team may have lost its lead. Only the agents a chief started hang under its
/// head; those the person started stand at the root below it.
fn team_rows(
    bench: &Workbench,
    agents: &[AgentRow],
    homes: &HashMap<(Place, String), String>,
    heads: Vec<(String, AnyElement)>,
    (chiefs, mut chief_led): (&HashSet<String>, HashSet<String>),
    cx: &mut Context<Workbench>,
) -> (Option<AnyElement>, Vec<AnyElement>) {
    let chosen = bench.agent().map(|agent| agent.id);
    let glided = bench
        .rails
        .glide
        .borrow_mut()
        .choose(chosen, cx.reduce_motion());
    let teams = bench.filter_text().is_empty();
    let mut rows: Vec<AgentRow> = agents.to_vec();
    match teams {
        true => {
            let workers: Vec<AgentRow> = (remote_workers(bench, agents).into_iter())
                .filter_map(|worker| {
                    let mut row = (agents.iter())
                        .find(|agent| agent.id == worker.shown.agent_id)
                        .cloned()
                        .or_else(|| crate::agents::rows::worker(worker))?;
                    row.lead = worker.shown.lead.clone();
                    if (row.lead.as_ref()).is_some_and(|lead| chiefs.contains(lead)) {
                        row.lead = None;
                        chief_led.insert(row.id.clone());
                    }
                    Some(row)
                })
                .collect();
            rows.retain(|row| !workers.iter().any(|worker| worker.id == row.id));
            rows.extend(workers);
        }
        false => rows.iter_mut().for_each(|row| row.lead = None),
    }
    let home: Vec<(String, bool)> = {
        let at: HashMap<&str, &AgentRow> = rows.iter().map(|row| (row.id.as_str(), row)).collect();
        (rows.iter())
            .map(|row| {
                let mut top = row;
                for _ in 0..rows.len() {
                    match top.lead.as_deref().and_then(|lead| at.get(lead)) {
                        Some(lead) => top = lead,
                        None => break,
                    }
                }
                (home_of(homes, top).to_owned(), chief_led.contains(&top.id))
            })
            .collect()
    };
    let folded = &bench.rails.folded_teams;
    let mut drawn: Vec<(String, AnyElement)> = Vec::with_capacity(rows.len() + heads.len());
    let mut shown = Vec::new();
    let mut place = |placed: Vec<tree::Placed>,
                     drawn: &mut Vec<(String, AnyElement)>,
                     cx: &mut Context<Workbench>| {
        for placed in placed {
            let fold = (teams && placed.below > 0).then(|| Fold {
                open: !folded.contains(&placed.row.id),
                count: placed.below,
            });
            let row = agent_row(bench, &placed.row, &placed.hang, fold, glided, cx);
            drawn.push((placed.row.id.clone(), row));
            shown.push((placed.row.id.clone(), placed.row));
        }
    };
    let mut headed = HashSet::new();
    for (project, head) in heads {
        let (team, own): (Vec<_>, Vec<_>) = (rows.iter().zip(&home))
            .filter(|(_, (home, _))| *home == project)
            .partition(|(_, (_, chief_led))| *chief_led);
        let team: Vec<AgentRow> = team.into_iter().map(|(row, _)| row.clone()).collect();
        let own: Vec<AgentRow> = own.into_iter().map(|(row, _)| row.clone()).collect();
        let placed = tree::tree(&team, 1, folded);
        let head = match placed.is_empty() {
            true => head,
            false => div()
                .relative()
                .child(head)
                .children(hang_lines(
                    &tree::Hang {
                        open: true,
                        ..Default::default()
                    },
                    cx,
                ))
                .into_any_element(),
        };
        drawn.push((format!("chief:{project}"), head));
        place(placed, &mut drawn, cx);
        place(tree::tree(&own, 0, folded), &mut drawn, cx);
        headed.insert(project);
    }
    let loose: Vec<AgentRow> = (rows.iter().zip(&home))
        .filter(|(_, (home, _))| !headed.contains(home))
        .map(|(row, _)| row.clone())
        .collect();
    place(tree::tree(&loose, 0, folded), &mut drawn, cx);
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
        let row = agent_row(bench, &gone.item, &tree::Hang::default(), None, false, cx);
        let key = SharedString::from(format!("depart:{}", gone.id));
        let row = crate::motion::departing(key, gone.since, crate::motion::ROW_DEPARTURE, row);
        rows.insert(gone.at.min(rows.len()), row.into_any_element());
        order.insert(gone.at.min(order.len()), gone.id.clone());
    }
    let mut glide = bench.rails.glide.borrow_mut();
    glide.set_order(order);
    let chosen = Tokens::get(cx).surface_chosen;
    let copy = glide.copy(move |copy| copy.rounded(theme::radius_2()).bg(chosen));
    (copy, rows)
}

/// The spines, ticks and corners that hang a row under its lead, level by level, and the line
/// dropping from its mark to the rows under it.
fn hang_lines(hang: &tree::Hang, cx: &App) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let at = |level: usize| px(14. + INDENT * level as f32);
    let spine = |level: usize| div().absolute().left(at(level)).w(px(1.)).bg(t.tree_spine);
    let mut lines: Vec<AnyElement> = Vec::new();
    for (level, line) in hang.lines.iter().enumerate() {
        match line {
            tree::Line::Blank => {}
            tree::Line::Through => {
                lines.push(spine(level).top(px(-2.)).bottom(px(-2.)).into_any_element())
            }
            tree::Line::Tick => {
                lines.push(spine(level).top(px(-2.)).bottom(px(-2.)).into_any_element());
                lines.push(
                    div()
                        .absolute()
                        .top(px(14.))
                        .left(at(level))
                        .w(px(7.))
                        .h(px(1.))
                        .bg(t.tree_tick)
                        .into_any_element(),
                );
            }
            tree::Line::Corner => lines.push(
                div()
                    .absolute()
                    .top(px(-2.))
                    .left(at(level))
                    .w(px(8.))
                    .h(px(17.))
                    .border_l(crate::theme::border())
                    .border_b(crate::theme::border())
                    .border_color(t.tree_spine)
                    .rounded_bl(px(4.))
                    .into_any_element(),
            ),
        }
    }
    if hang.open {
        lines.push(
            spine(hang.depth)
                .top(px(26.))
                .bottom(px(-2.))
                .into_any_element(),
        );
    }
    lines
}

/// How far each level of a tree sits to the right of the one above.
const INDENT: f32 = 18.;

/// The workers the agents listed started on linked hosts, as each lead's core lists them in team
/// order.
fn remote_workers<'a>(bench: &'a Workbench, agents: &[AgentRow]) -> Vec<&'a RemoteAgent> {
    let mut places: Vec<&Place> = Vec::new();
    for agent in agents {
        if !places.contains(&&agent.place) {
            places.push(&agent.place);
        }
    }
    (places.into_iter())
        .filter_map(|place| bench.view_at(place))
        .flat_map(|view| &view.inbox.remote_agents)
        .filter(|worker| worker.shown.lead.is_some())
        .collect()
}

impl Workbench {
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
    hang: &tree::Hang,
    fold: Option<Fold>,
    glided: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let elsewhere = agent.place != bench.place();
    let project = bench
        .rails
        .all_projects
        .then(|| bench.project_on(&agent.place, &agent.project))
        .flatten()
        .map(|project| project.name.clone());
    let chosen = bench.agent().is_some_and(|front| front.id == agent.id);
    let id = agent.id.clone();
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
                    .clicks(cx.listener({
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
    let worker = !agent.chief
        && (bench.project_on(&agent.place, &agent.project))
            .is_some_and(|project| project.chief.is_some());
    let away = agent.snoozed_until.is_some() || agent.settled_at.is_some() || !agent.reachable;
    let mark = match away {
        true => div().opacity(0.55).child(mark).into_any_element(),
        false => mark,
    };
    let status = (agent.reachable)
        .then(|| ui::turn_mark(&agent.id, agent.turn.as_ref()))
        .flatten();
    let fades = status.is_none();
    let pin = (bench.chat_of(&agent.id))
        .filter(|_| !elsewhere)
        .map(|chat| (chat.pinned, pin_chat(&cx.entity(), chat).1));
    let editing = bench
        .overlays
        .retitling
        .as_ref()
        .filter(|retitling| retitling.agent == agent.id && retitling.in_rail)
        .map(|retitling| crate::home::retitle_field(retitling, 13., cx).w_full());
    let said = (bench.chat_of(&agent.id)).and_then(|chat| bench.said_in(&chat.id));
    let host = host_tag(bench, agent, cx);
    let lead_apart = (hang.lead_apart.as_deref())
        .and_then(|lead| bench.agent_by_id(lead))
        .and_then(|lead| lead.name)
        .map(|lead| ui::meta(format!("@{lead} ›"), cx).pl(SPACE_2));
    let first = host.is_none() && lead_apart.is_none();
    let name = (agent.handle())
        .map(|name| ui::meta(format!("@{name}"), cx).when(first, |name| name.pl(SPACE_2)));
    let wake = (agent.snoozed_until).map(|until| crate::agents::settled::wake_label(until, cx));
    let turn_time = ui::turn_time(&agent.id, agent.turn.as_ref());
    let counts = counts(agent, cx);
    let shut_fold = fold.as_ref().filter(|fold| !fold.open);
    // The hover buttons cover the name and counts, which fade for them, so nothing slides.
    let covered = fades
        && (name.is_some() || counts.is_some())
        && shut_fold.is_none()
        && wake.is_none()
        && turn_time.is_none();
    let (name, counts) = match covered {
        true => {
            let tail = div()
                .flex()
                .items_center()
                .justify_end()
                .gap(SPACE_2)
                .min_w(px(if pin.is_some() { 40. } else { 20. }))
                .pr(SPACE_1)
                .children(name)
                .children(counts);
            (
                Some(ui::gives_way(AGENT_ROW, tail).into_any_element()),
                None,
            )
        }
        false => (
            name.map(|name| {
                let name = name.pr(SPACE_1);
                match fades {
                    true => ui::gives_way(AGENT_ROW, name).into_any_element(),
                    false => name.into_any_element(),
                }
            }),
            counts.map(|counts| match fades {
                true => ui::gives_way(AGENT_ROW, counts).into_any_element(),
                false => counts.into_any_element(),
            }),
        ),
    };
    let row = Row::new(
        SharedString::from(format!("agent-{}", agent.id)),
        agent.title.clone(),
    )
    .title_view(editing)
    .when_some(said, |row, said| {
        row.subtitle(crate::pickers::recent::said_line(said, cx))
    })
    .mark(22., mark)
    .when(!covered, |row| {
        row.hover_padding(AGENT_ROW, if pin.is_some() { 48. } else { 28. })
    })
    .chosen(chosen)
    .glided(glided && chosen)
    .compact(bench.rail_compact())
    .status(status)
    .when(away, Row::quiet)
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
                _ => {
                    if worker {
                        crate::keymap::counted("chat.openWorker", cx);
                    }
                    bench.open_found_agent(&id, cx)
                }
            }
        }
    }))
    .children(host)
    .children(lead_apart)
    .children(name)
    .children(shut_fold.map(|fold| {
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
    .children(wake)
    .children(turn_time)
    .children(counts);
    let lines = hang_lines(hang, cx);
    let card = crate::agents::card::view(bench, agent, project.as_deref(), cx);
    let close = agent.id.clone();
    let actions = ui::hover_actions(AGENT_ROW)
        .top_0()
        .h(px(crate::theme::dense(if bench.rail_compact() {
            27.
        } else {
            28.
        })))
        .children(pin.map(|(pinned, pin)| {
            let label = match pinned {
                true => t!("Unpin {title}", title = agent.title),
                false => t!("Pin {title}", title = agent.title),
            };
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
            t!("Close {title}", title = agent.title),
            cx.listener({
                let row = agent.clone();
                move |bench, _, _, cx| match elsewhere {
                    true => bench.close_elsewhere(&row, cx),
                    false => bench.close_agents(vec![close.clone()], cx),
                }
            }),
            cx,
        ));
    let chat = bench.chat_of(&agent.id);
    let dragged = crate::drag::DraggedChat {
        agent: (agent.name.clone())
            .filter(|_| !elsewhere)
            .map(|name| (agent.id.clone(), name)),
        recent: chat.filter(|_| !elsewhere).map(|chat| chat.id.clone()),
        pinned: chat.is_some_and(|chat| chat.pinned),
        label: agent.title.clone().into(),
    };
    let (menu_bench, menu_row) = (cx.weak_entity(), agent.clone());
    let wrap = div()
        .id(SharedString::from(format!("agent-menu-{}", agent.id)))
        .group(AGENT_ROW)
        .relative();
    let label = match &agent.name {
        Some(name) if agent.titled => format!("{}, @{name}", agent.title),
        _ => agent.title.clone(),
    };
    let keyed_id = format!("agent-{}", agent.id);
    let row = crate::drag::chat(wrap, dragged, cx.weak_entity())
        .pl(px(INDENT * hang.depth as f32))
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
        .context_menu(ui::menu(move |popup, window, cx| {
            match menu_bench.update(cx, |bench, cx| AgentMenu::of_row(bench, &menu_row, cx)) {
                Ok(Some(menu)) => menu.build(popup, window, cx),
                _ => popup,
            }
        }));
    (bench.rails.keyed(Rail::Agents)).row(keyed_id, label, row, cx)
}

/// The projects a list of every project shows, each led by its @Chief: those with a chief's chat
/// or agents, the one that most needs the person first. A project with an open call or an agent
/// that needs the person comes first, then one with work under way, then the rest, each rank in
/// the order the core lists projects.
fn ranked_projects(bench: &Workbench) -> Vec<ProjectInfo> {
    let rank = |project: &ProjectInfo| match project.standing {
        ProjectStanding::NeedsYou => 0,
        ProjectStanding::AtWork => 1,
        ProjectStanding::AtRest | ProjectStanding::Unknown => 2,
    };
    let mut projects: Vec<ProjectInfo> = (bench.projects().iter())
        .filter(|project| {
            project.chief.is_some() || project.chief_home.is_some() || project.agents > 0
        })
        .cloned()
        .collect();
    projects.sort_by_key(rank);
    projects
}

/// This host's project that each other host's copy of it folds into, keyed by that copy's host
/// and project there.
fn home_projects(bench: &Workbench) -> HashMap<(Place, String), String> {
    (bench.folded().into_iter())
        .filter_map(|folded| Some((folded.own?.id, folded.copies)))
        .flat_map(|(own, copies)| {
            (copies.into_iter())
                .filter(|copy| copy.place != Place::This)
                .map(move |copy| ((copy.place, copy.project), own.clone()))
        })
        .collect()
}

/// This host's project that the copy `project` on the host at `place` folds into, so the rail
/// heads it with the same @Chief whichever host is in front.
fn home_project<'a>(
    bench: &'a Workbench,
    homes: &HashMap<(Place, String), String>,
    place: &Place,
    project: &str,
) -> Option<&'a ProjectInfo> {
    let home = homes.get(&(place.clone(), project.to_owned()))?;
    (bench.this_view()?.workspace.projects.iter()).find(|project| project.id == *home)
}

/// The project whose @Chief leads the agent's row: this host's copy of its project.
fn home_of<'a>(homes: &'a HashMap<(Place, String), String>, agent: &'a AgentRow) -> &'a str {
    (homes.get(&(agent.place.clone(), agent.project.clone()))).map_or(&agent.project, |home| home)
}

/// The project's chief, here or on its home host, while its chat is open.
fn chief_of<'a>(project: &ProjectInfo, agents: &'a [AgentRow]) -> Option<&'a AgentRow> {
    let home = project
        .chief_home
        .as_ref()
        .map(|home| home.agent_id.as_str());
    (home.or(project.chief.as_deref())).and_then(|id| agents.iter().find(|agent| agent.id == id))
}

/// A project's @Chief row: Sikemux's mark, @Chief, "Chief of staff", the project's name where
/// projects mix, the host it lives on when that is another, and how its turn stands. Opening it
/// opens the chief's chat, made the first time on the host it lives on.
fn chief_row(
    bench: &Workbench,
    project: &ProjectInfo,
    chief: Option<&AgentRow>,
    named: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let chosen = chief.is_some_and(|chief| bench.agent().is_some_and(|front| front.id == chief.id));
    let place = named.then(|| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .pl(SPACE_2)
            .text_color(t.text_tertiary)
            .child(ui::icon(Icon::Folder, 11.))
            .child(ui::text(project.name.clone(), TEXT_2XS, FontWeight::MEDIUM))
    });
    let home = project.chief_home.as_ref().map(|home| {
        let said = match home.away_since {
            Some(since) => t!(
                "Runs on {host}, out of reach since {at}",
                host = home.host_name,
                at = ui::time::clock(since)
            ),
            None => t!("Runs on {host}", host = home.host_name),
        };
        host_mark(format!("chief-home-{}", project.id), said.into(), cx)
    });
    let take_over = (project.chief_home.as_ref())
        .filter(|home| home.away_since.is_some())
        .map(|_| {
            let project_id = project.id.clone();
            ui::Button::new(
                SharedString::from(format!("chief-take-over-{}", project.id)),
                t!("Take over here"),
                ui::Variant::Outline,
            )
            .sized(18., 6., TEXT_2XS)
            .on_click(cx.listener(move |bench, _, _, cx| {
                cx.stop_propagation();
                bench.take_over_chief(&project_id, cx);
            }))
        });
    let away = (project.chief_home.as_ref()).and_then(|home| {
        let since = home.away_since?;
        let said = t!(
            "{host}, out of reach since {at}",
            host = home.host_name,
            at = ui::time::clock(since)
        );
        Some(ui::text(said, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary))
    });
    let (id, project_id) = (chief.map(|chief| chief.id.clone()), project.id.clone());
    let home_project = project.chief_home.as_ref().and_then(|home| {
        let copy = (project.copies.iter()).find(|copy| copy.host == home.host)?;
        Some((home.host.clone(), copy.project.clone()))
    });
    Row::new(
        SharedString::from(format!("chief-{}", project.id)),
        rows::CHIEF.to_owned(),
    )
    .mark(22., ui::chief_mark(20.))
    .chosen(chosen)
    .compact(bench.rail_compact())
    .status(chief.and_then(|chief| ui::turn_mark(&chief.id, chief.turn.as_ref())))
    .detail(rows::chief_role())
    .children(place)
    .children(home)
    .children(take_over)
    .when_some(away, |row, away| row.subtitle(away))
    .on_click(
        cx.listener(move |bench, _, _, cx| match (&id, &home_project) {
            (Some(id), _) => bench.open_agent(id, cx),
            (None, Some((host, project))) => {
                let on = Target::new(&bench.asks, bench.place_of(host));
                bench.open_chief_on(on, project, cx);
            }
            (None, None) => bench.open_chief(&project_id, cx),
        }),
    )
    .into_any_element()
}

/// The host an agent runs on, on its row, while that is not this host.
fn host_tag(bench: &Workbench, agent: &AgentRow, cx: &App) -> Option<AnyElement> {
    let Place::Account(key) = &agent.place else {
        return None;
    };
    let host = bench.host_name(key);
    let said = match agent.reachable {
        true => t!("Runs on {host}", host = host),
        false => t!("Runs on {host}, out of reach", host = host),
    };
    Some(host_mark(format!("host-of-{}", agent.id), said.into(), cx))
}

fn host_mark(id: String, said: SharedString, cx: &App) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .flex_none()
        .pl(SPACE_2)
        .tooltip(ui::tooltip(said))
        .child(crate::rail::host_glyph(crate::theme::ROW_GLYPH, true, cx))
        .into_any_element()
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
                    .rounded(theme::radius_2())
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

/// The thing an agent waits for while another holds it, cut short so the task keeps its room, with
/// the whole name on hover.
fn thing_waited(agent_id: &str, label: String, color: Hsla) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("thing-waited-{agent_id}")))
        .flex()
        .items_center()
        .gap(px(3.))
        .min_w_0()
        .tooltip(ui::tooltip(label.clone()))
        .child(ui::icon(Icon::Hourglass, 11.).text_color(color))
        .child(
            ui::text(label, TEXT_2XS, FontWeight::MEDIUM)
                .max_w(px(56.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis(),
        )
}

pub fn counts(agent: &AgentRow, cx: &App) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    (agent.subagents > 0 || agent.tasks > 0 || agent.line.is_some()).then(|| {
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
        let labelled = |glyph: Icon, color, label: String| {
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .child(ui::icon(glyph, 11.).text_color(color))
                .child(
                    ui::text(label, TEXT_2XS, FontWeight::MEDIUM)
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
            .when_some(agent.line.as_ref(), |row, line| match line.by {
                Some(_) => row.child(thing_waited(&agent.id, line.what.clone(), t.warn)),
                None => row.child(labelled(
                    Icon::Hourglass,
                    t.warn,
                    crate::turn_line::in_line(line.place),
                )),
            })
    })
}

fn pages(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let border = Tokens::get(cx).border;
    let page = |id: &'static str,
                glyph: Icon,
                label: &'static str,
                shown: TabRailPage,
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
        .border_t(crate::theme::border())
        .border_color(border)
        .child(page(
            "page-agents",
            Icon::Agents,
            t!("Agents"),
            TabRailPage::Agents,
            cx,
        ))
        .child(page(
            "page-terminals",
            Icon::Terminal,
            t!("Terminals"),
            TabRailPage::Terminals,
            cx,
        ))
        .child(page(
            "page-files",
            Icon::File,
            t!("Files"),
            TabRailPage::Files,
            cx,
        ))
}

/// The Files page's head, with the copy switch while the project has more than one copy.
fn files_header(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let title = ui::text(t!("Files"), TEXT_XS, FontWeight::MEDIUM).text_color(t.text_primary);
    let chip = (bench.folded_in_front())
        .filter(|folded| folded.copies.len() > 1)
        .and_then(|folded| crate::copies::copy_chip("files-copy", bench, &folded, cx));
    let title = div()
        .flex()
        .flex_1()
        .items_center()
        .justify_between()
        .gap(SPACE_2)
        .child(title)
        .children(chip);
    header_frame(title, [], bench, cx)
}

pub(crate) fn page_header(
    title: impl Into<SharedString>,
    bench: &Workbench,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let title = ui::text(title, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_primary);
    header_frame(title, [], bench, cx)
}

/// The account host chosen in front, by its name; none for this host or an SSH host.
fn other_host(bench: &Workbench) -> Option<SharedString> {
    match bench.chosen_host() {
        crate::hosts::HostChoice::Account(key) => Some(bench.host_name(&key)),
        _ => None,
    }
}

/// A rail page with nothing to list: a faint mark, what is missing, and the key that fills it.
pub(crate) fn rail_empty(
    glyph: Icon,
    title: impl Into<SharedString>,
    press: Option<(CommandId, &'static str)>,
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
                .child(t!("Press"))
                .child(ui::kbd(keys, cx))
                .child(doing),
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

/// The rows that need the person but are scrolled out of the rail, where each was drawn last.
#[derive(Default)]
struct OutOfSight {
    above: Vec<Bounds<Pixels>>,
    below: Vec<Bounds<Pixels>>,
}

/// Reads where the rows were drawn last, so it comes before the rows are laid out again.
fn out_of_sight(bench: &Workbench, agents: &[AgentRow]) -> OutOfSight {
    let view = bench.rails.scroll.bounds();
    let mut unseen = OutOfSight::default();
    if view.size.height <= px(0.) {
        return unseen;
    }
    let glide = bench.rails.glide.borrow();
    for agent in agents
        .iter()
        .filter(|agent| agent.group == AgentGroup::Blocked)
    {
        let id = match agent.chief {
            true => format!("chief:{}", agent.project),
            false => agent.id.clone(),
        };
        let Some(row) = glide.drawn_at(&id) else {
            continue;
        };
        if row.bottom() <= view.top() + px(EDGE_FADE) {
            unseen.above.push(row);
        } else if row.top() >= view.bottom() - px(EDGE_FADE) {
            unseen.below.push(row);
        }
    }
    unseen
}

impl OutOfSight {
    /// A chip at the rail's top edge, or its bottom one, counting the rows past it that need
    /// the person, which scrolls the nearest of them into sight.
    fn chip(&self, above: bool, cx: &mut Context<Workbench>) -> Option<AnyElement> {
        let (rows, glyph) = match above {
            true => (&self.above, Icon::ArrowUp),
            false => (&self.below, Icon::ArrowDown),
        };
        let nearest = match above {
            true => rows
                .iter()
                .max_by(|a, b| f32::from(a.top()).total_cmp(&f32::from(b.top()))),
            false => rows
                .iter()
                .min_by(|a, b| f32::from(a.top()).total_cmp(&f32::from(b.top()))),
        }
        .copied()?;
        let t = Tokens::get(cx).clone();
        let pill = div()
            .id(match above {
                true => "needs-you-above",
                false => "needs-you-below",
            })
            .rounded(px(10.))
            .bg(t.surface)
            .shadow(theme::shadow_small())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .h(px(20.))
                    .px(SPACE_2)
                    .rounded(px(10.))
                    .bg(t.warn.opacity(0.2))
                    .text_color(t.warn)
                    .child(ui::icon(glyph, 11.).text_color(t.warn))
                    .child(ui::text(t!("Needs you"), TEXT_3XS, FontWeight::MEDIUM))
                    .when(rows.len() > 1, |pill| {
                        pill.child(
                            ui::text(rows.len().to_string(), TEXT_3XS, FontWeight::MEDIUM)
                                .font_features(theme::tabular()),
                        )
                    }),
            )
            .clicks(cx.listener(move |bench, _, _, cx| {
                let scroll = &bench.rails.scroll;
                let view = scroll.bounds();
                let by = match above {
                    true => view.top() + px(EDGE_FADE) - nearest.top(),
                    false => view.bottom() - px(EDGE_FADE) - nearest.bottom(),
                };
                let offset = scroll.offset();
                scroll.set_offset(gpui_kit::point(offset.x, offset.y + by));
                cx.notify();
            }));
        let at = div().absolute().left_0().right_0().flex().justify_center();
        Some(
            match above {
                true => at.top(px(4.)),
                false => at.bottom(px(4.)),
            }
            .child(pill)
            .into_any_element(),
        )
    }
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
        (Icon::Stack, SharedString::from(t!("All projects"))),
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

impl Workbench {
    /// The terminal an agent runs in, and the host it is on, while it runs in one rather than
    /// as a chat.
    pub fn agent_terminal(&self, agent: &str) -> Option<(Place, SessionId)> {
        let place = self.chat_target(agent).place().clone();
        let session = self.kept_agent(agent)?.pty_id?;
        Some((place, session))
    }

    /// Whether the agent runs in its CLI in a terminal, there now or starting again.
    pub fn in_terminal(&self, agent: &str) -> bool {
        self.kept_agent(agent)
            .is_some_and(|kept| kept.terminal || kept.pty_id.is_some())
    }

    /// Whether the agent is asleep with its terminal ended, to start again on its session.
    pub fn terminal_asleep(&self, agent: &str) -> bool {
        self.kept_agent(agent)
            .is_some_and(|kept| kept.terminal && kept.asleep && kept.pty_id.is_none())
    }

    /// The agent as the core of the host it runs on keeps it.
    pub(crate) fn kept_agent(
        &self,
        agent: &str,
    ) -> Option<&sikemux_view::workspace::WorkspaceAgent> {
        self.view_at(self.chat_target(agent).place())?
            .workspace
            .agents
            .iter()
            .find(|kept| kept.id == agent)
    }

    /// Moves an agent between its chat and its own CLI in a terminal, carrying what was
    /// typed and not sent.
    pub fn switch_view(&self, agent: &str, terminal: bool) {
        let to = if terminal {
            AgentViewKind::Terminal
        } else {
            AgentViewKind::Chat
        };
        self.chat_target(agent)
            .call_typed::<call::SwitchAgentView>(AgentViewParams {
                agent_id: agent.to_owned(),
                to,
            });
    }
}
