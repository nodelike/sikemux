//! The chosen host's detail: what it is, what about it needs the person and the fix for it,
//! how busy it has been, what its agents run and use, what happened on it lately, and what
//! it is.

use std::collections::BTreeMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_view::host_stats::{
    AgentUse, AlertLevel, HostAlert, HostAlertKind, HostEventKind, HostStats, StatsSpan,
};
use sikemux_view::inbox::AgentShown;
use sikemux_wire::protocol::AgentStatus;

use super::board::tile;
use super::chart::{history, Series};
use super::model::{self, Listed, State};
use super::{card, hair, ink, wash};
use crate::hosts::{HostChoice, Place, Sending};
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_1, RADIUS_2, RADIUS_3, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    SPACE_4, SPACE_5, SPACE_6, TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_MD, TEXT_SM,
    TEXT_XL, TEXT_XS,
};
use crate::ui::{self, Button, IconButton, Segmented, Variant};
use crate::workbench::Workbench;

/// The share of a resource past which a host struggles, dashed across its chart.
const LIMIT: f32 = 0.9;
const CHART_HEIGHT: f32 = 44.;
const ACTIVITY_SHOWN: usize = 8;

pub fn detail(
    bench: &Workbench,
    host: &Listed,
    listed: &[Listed],
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stats = host.stats();
    div()
        .px(px(22.))
        .pt(SPACE_6)
        .pb(px(56.))
        .flex()
        .flex_col()
        .child(head(bench, host, &t, cx))
        .when_some(stats, |detail, stats| {
            detail.child(specs(host, stats, &t, cx))
        })
        .children(needs_you(bench, host, listed, &t, cx))
        .when_some(stats.filter(|_| host.online()), |detail, stats| {
            detail.child(resources(bench, host, stats, &t, cx))
        })
        .children(agents(bench, host, listed, &t, cx))
        .when_some(
            stats.filter(|stats| !stats.top.is_empty()),
            |detail, stats| detail.child(top(bench, host, stats, &t, cx)),
        )
        .when_some(
            stats.filter(|stats| !stats.events.is_empty()),
            |detail, stats| detail.child(activity(host, stats, &t, cx)),
        )
        .when_some(stats, |detail, stats| detail.child(about(stats, &t, cx)))
        .into_any_element()
}

fn head(bench: &Workbench, host: &Listed, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let tint = host.state.tint(t);
    let sub: Vec<String> = [
        host.model(),
        host.stats()
            .map(|stats| stats.about.system.clone())
            .unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .fold(Vec::new(), |mut parts, part| {
        if !parts.contains(&part) {
            parts.push(part);
        }
        parts
    });
    div()
        .flex()
        .items_center()
        .gap(SPACE_3H)
        .min_w_0()
        .child(tile(host, 48., 24., t))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    ui::text(host.name.clone(), TEXT_XL, FontWeight::SEMIBOLD)
                        .truncate()
                        .letter_spacing(-0.02)
                        .text_color(t.text_primary),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .min_w_0()
                        .child(pill(&host.state.words(), tint))
                        .child(
                            ui::text(sub.join(" · "), TEXT_XS, FontWeight::NORMAL)
                                .truncate()
                                .text_color(t.text_tertiary),
                        ),
                ),
        )
        .child(
            div()
                .flex_none()
                .self_start()
                .mt(SPACE_1)
                .flex()
                .gap(SPACE_1H)
                .children(host.online().then(|| terminal_button(host, cx)))
                .children(
                    host.online()
                        .then(|| new_chat_button(bench, host, cx))
                        .flatten(),
                )
                .children(menu_button(bench, host, cx)),
        )
}

/// A state as a soft pill in its own colour, after a dot.
fn pill(words: &str, tint: Hsla) -> Div {
    div()
        .flex_none()
        .h(px(22.))
        .pl(SPACE_2)
        .pr(px(9.))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(px(11.))
        .bg(tint.opacity(0.13))
        .text_color(tint)
        .child(div().size(px(6.)).rounded_full().bg(tint))
        .child(ui::text(words.to_owned(), TEXT_2XS_H, FontWeight::MEDIUM))
}

fn terminal_button(host: &Listed, cx: &mut Context<Workbench>) -> AnyElement {
    let place = host.place.clone();
    Button::new("hosts-terminal", "Terminal", Variant::Outline)
        .icon(Icon::Terminal)
        .on_click(cx.listener(move |bench, _, _, cx| match &place {
            Place::This => {
                bench.choose_host(HostChoice::This, cx);
                bench.new_loose_terminal(cx);
            }
            Place::Account(key) => bench.new_terminal_on(key, cx),
        }))
        .into_any_element()
}

/// A new chat on the host, in the project in front there; none where it has no project.
fn new_chat_button(
    bench: &Workbench,
    host: &Listed,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let has_project = host
        .view()
        .is_some_and(|view| !view.workspace.projects.is_empty());
    if !has_project {
        return None;
    }
    let place = host.place.clone();
    let in_front = bench.place() == place;
    Some(
        IconButton::new("hosts-new-chat", Icon::Plus, "New chat here")
            .on_click(cx.listener(move |bench, _, window, cx| match &place {
                Place::Account(key) => bench.new_chat_on(key, cx),
                Place::This if in_front => bench.new_chat(window, cx),
                Place::This => {
                    bench.choose_host(HostChoice::This, cx);
                    bench.pick_project(crate::pickers::picker::ThenPicked::StartChat, window, cx);
                }
            }))
            .into_any_element(),
    )
}

/// The host's own menu, as its row in the rail has it.
fn menu_button(
    bench: &Workbench,
    host: &Listed,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let trigger = gpui_kit::component::button::Button::new("hosts-more")
        .child(ui::icon(Icon::More, 13.).text_color(t.text_secondary));
    let trigger = {
        use gpui_kit::component::button::ButtonVariants;
        trigger.ghost().compact().rounded(RADIUS_2)
    };
    use gpui_kit::component::menu::DropdownMenu;
    match &host.place {
        Place::This => {
            let menu =
                crate::rail::this_host_menu(cx.weak_entity(), bench.feeds.this.approval, false);
            Some(
                trigger
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, ui::menu(menu))
                    .into_any_element(),
            )
        }
        Place::Account(_) => {
            let menu = crate::rail::account_host_menu(bench, host.row?, false, cx)?;
            Some(
                trigger
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, ui::menu(menu))
                    .into_any_element(),
            )
        }
    }
}

fn specs(host: &Listed, stats: &HostStats, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let about = &stats.about;
    let (logo, tint) = model::logo(about.mark, t);
    let cell = |at: usize, label: &str, glyph: AnyElement, value: String| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACE_3H)
            .pt(px(11.))
            .pb(SPACE_3)
            .flex()
            .flex_col()
            .gap(px(5.))
            .when(at > 0, |cell| cell.border_l_1().border_color(hair(t)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(glyph)
                    .child(ui::caps(label, cx)),
            )
            .child(
                ui::text(value, TEXT_MD, FontWeight::MEDIUM)
                    .truncate()
                    .text_color(t.text_primary),
            )
    };
    let glyph = |icon: Icon| {
        ui::icon(icon, 12.)
            .text_color(t.text_tertiary)
            .into_any_element()
    };
    let system_tint = if host.online() { tint } else { t.text_tertiary };
    div()
        .mt(SPACE_5)
        .flex()
        .rounded(RADIUS_3)
        .border_1()
        .border_color(hair(t))
        .bg(card(t))
        .child(cell(
            0,
            "Cores",
            glyph(Icon::Activity),
            about.cores.to_string(),
        ))
        .child(cell(
            1,
            "Memory",
            glyph(Icon::Stack),
            model::size(about.memory_total),
        ))
        .child(cell(
            2,
            "Disk",
            glyph(Icon::Database),
            model::size(about.disk_total),
        ))
        .child(
            cell(
                3,
                "System",
                ui::icon(logo, 12.)
                    .text_color(system_tint)
                    .into_any_element(),
                about.system.clone(),
            )
            .flex_grow(1.3),
        )
}

/// A section's head: its icon and spaced capitals, how many it holds, and what sits at its end.
fn section(
    icon: Icon,
    label: &str,
    count: Option<String>,
    aside: Option<AnyElement>,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    div()
        .mt(px(28.))
        .mb(SPACE_3)
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::icon(icon, 13.).text_color(t.text_tertiary))
        .child(ui::caps(label, cx))
        .children(count.map(|count| {
            div()
                .px(SPACE_1H)
                .rounded(px(8.))
                .bg(ink(t, 0.06))
                .child(ui::mono(count, TEXT_3XS, cx))
        }))
        .children(aside.map(|aside| div().ml_auto().child(aside)))
}

/// A fix the page offers for what needs the person, and what it does when pressed.
struct Fix {
    words: String,
    /// The fix the alert suggests first, drawn as the primary action.
    first: bool,
    run: Box<Run>,
}

type Run = dyn Fn(&mut Workbench, &mut Context<Workbench>);

fn needs_you(
    bench: &Workbench,
    host: &Listed,
    listed: &[Listed],
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Option<Div> {
    let mut shown: Vec<AnyElement> = Vec::new();
    if let State::Offline { since } = host.state {
        let agents = host.agents().count();
        let said = match agents {
            0 => "Sikemux keeps trying to reach it.".to_owned(),
            1 => "1 agent was on it when it went. Sikemux keeps trying to reach it.".to_owned(),
            count => {
                format!("{count} agents were on it when it went. Sikemux keeps trying to reach it.")
            }
        };
        let title = format!("{} is out of reach", host.name);
        shown.push(alert_card(
            t.danger,
            Icon::Power,
            title,
            since.map(crate::hosts::lasted),
            said,
            Vec::new(),
            t,
            cx,
        ));
    }
    let alerts = host.stats().into_iter().flat_map(|stats| &stats.alerts);
    for (at, alert) in alerts.enumerate() {
        let (said, fixes) = explain(bench, host, listed, alert);
        let tint = crate::hosts::alert_tint(alert.level, t);
        let icon = match alert.level {
            AlertLevel::Danger => Icon::Warning,
            AlertLevel::Warn => Icon::Gauge,
        };
        let fixes = fixes
            .into_iter()
            .enumerate()
            .map(|(index, fix)| {
                let run = std::rc::Rc::new(fix.run);
                Button::new(
                    SharedString::from(format!("host-fix-{at}-{index}")),
                    fix.words,
                    if fix.first {
                        Variant::Primary
                    } else {
                        Variant::Outline
                    },
                )
                .on_click(cx.listener(move |bench, _, _, cx| run(bench, cx)))
                .into_any_element()
            })
            .collect();
        shown.push(alert_card(
            tint,
            icon,
            title_of(alert, host.stats()),
            Some(crate::hosts::lasted(alert.since)),
            said,
            fixes,
            t,
            cx,
        ));
    }
    if shown.is_empty() {
        return None;
    }
    let count = shown.len().to_string();
    Some(
        div()
            .flex()
            .flex_col()
            .child(section(
                Icon::Warning,
                "Needs you",
                Some(count),
                None,
                t,
                cx,
            ))
            .child(div().flex().flex_col().gap(SPACE_2).children(shown)),
    )
}

fn title_of(alert: &HostAlert, stats: Option<&HostStats>) -> String {
    let lasted = crate::hosts::lasted(alert.since);
    match alert.kind {
        HostAlertKind::Memory => {
            let swap = stats.map_or(0, |stats| stats.now.swap_used);
            match swap > 0 {
                true => format!(
                    "Memory at {}%, {} GB in swap",
                    alert.percent,
                    model::gb(swap)
                ),
                false => format!("Memory at {}%", alert.percent),
            }
        }
        HostAlertKind::Cpu => format!("CPU at {}% for {lasted}", alert.percent),
        HostAlertKind::Disk => format!("Disk at {}%", alert.percent),
        HostAlertKind::Offline => format!("Out of reach for {lasted}"),
        HostAlertKind::Unknown => format!("Load at {}%", alert.percent),
    }
}

/// What an alert comes from, in a sentence, and the fixes the app can make for it now.
fn explain(
    bench: &Workbench,
    host: &Listed,
    listed: &[Listed],
    alert: &HostAlert,
) -> (String, Vec<Fix>) {
    let Some(stats) = host.stats() else {
        return (String::new(), Vec::new());
    };
    let busiest = |by: fn(&AgentUse) -> u64| {
        (stats.agents.iter())
            .filter(|agent| by(agent) > 0)
            .max_by_key(|agent| by(agent))
    };
    let top = match alert.kind {
        HostAlertKind::Cpu => busiest(|agent| agent.cpu as u64),
        HostAlertKind::Memory => busiest(|agent| agent.memory),
        _ => None,
    };
    let roomiest = roomiest_other(host, listed);
    let mut said = String::new();
    if let Some(agent) = top {
        let name = host.agent_name(&agent.agent_id);
        said.push_str(&match alert.kind {
            HostAlertKind::Cpu => format!("{name} uses the most, {}", model::percent(agent.cpu)),
            _ => format!("{name} holds the most, {} GB", model::gb(agent.memory)),
        });
        if let Some(command) = &agent.command {
            said.push_str(&format!(", running {command}"));
        }
        said.push('.');
    }
    if alert.kind == HostAlertKind::Disk {
        if let Some(part) = stats.disk_parts.first() {
            said.push_str(&format!(
                "{} take the most, {}.",
                part_words(part.kind),
                model::size(part.bytes)
            ));
        }
    }
    if let Some(other) = roomiest {
        said.push_str(&format!(" {} has room.", other.name));
    }
    let mut fixes = Vec::new();
    if let (Some(agent), Some(other), Place::Account(_) | Place::This) =
        (top, roomiest, &host.place)
    {
        if let Some(fix) = move_fix(bench, host, other, &agent.agent_id) {
            fixes.push(fix);
        }
        if let Some(fix) = close_fix(host, &agent.agent_id) {
            fixes.push(fix);
        }
    }
    (said.trim().to_owned(), fixes)
}

/// The host other than `host` with the most memory free, among those that can take a project.
fn roomiest_other<'a>(host: &Listed, listed: &'a [Listed]) -> Option<&'a Listed<'a>> {
    (listed.iter())
        .filter(|other| other.key != host.key && other.online())
        .filter(|other| other.stats().is_some())
        .filter(|other| other.state == State::Healthy)
        .min_by(|a, b| {
            let used = |host: &Listed| host.percent(HostAlertKind::Memory).unwrap_or(100.);
            used(a).total_cmp(&used(b))
        })
}

/// Moving the project the agent works in to `to`, with every chat of it, named for the agent
/// when it is the project's only one there.
fn move_fix(bench: &Workbench, host: &Listed, to: &Listed, agent: &str) -> Option<Fix> {
    let shown = host.agent(agent)?;
    let project = shown.project.clone()?;
    let alone = host
        .agents()
        .filter(|other| other.project.as_ref() == Some(&project))
        .count()
        == 1;
    let name = shown
        .project_name
        .clone()
        .unwrap_or_else(|| project.clone());
    let destination = match &to.place {
        Place::Account(key) => key.clone(),
        Place::This => bench.remote.as_ref()?.core_id.clone(),
    };
    let words = match alone {
        true => format!("Move {} to {}", host.agent_name(agent), to.name),
        false => format!("Move {name} to {}", to.name),
    };
    let (from, label) = (host.place.clone(), to.name.clone());
    Some(Fix {
        words,
        first: true,
        run: Box::new(move |bench, cx| {
            bench.move_session_asking(
                Sending {
                    from: from.clone(),
                    project: project.clone(),
                    name: name.clone(),
                    host: destination.clone(),
                    label: label.clone(),
                    after: None,
                    moving: true,
                    uncommitted: true,
                },
                cx,
            )
        }),
    })
}

/// Closing the agent, which stops what it runs, once the person agrees.
fn close_fix(host: &Listed, agent: &str) -> Option<Fix> {
    host.agent(agent)?;
    let (place, id, name) = (host.place.clone(), agent.to_owned(), host.agent_name(agent));
    Some(Fix {
        words: format!("Close {name}"),
        first: false,
        run: Box::new(move |bench, cx| bench.close_agent_on(&place, &id, &name, cx)),
    })
}

#[allow(clippy::too_many_arguments)]
fn alert_card(
    tint: Hsla,
    icon: Icon,
    title: String,
    lasted: Option<String>,
    said: String,
    fixes: Vec<AnyElement>,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    div()
        .flex()
        .gap(SPACE_3H)
        .pl(SPACE_4)
        .pr(px(18.))
        .py(SPACE_4)
        .rounded(RADIUS_3)
        .border_1()
        .border_color(crate::theme::mix(tint, t.border, 0.26))
        .bg(wash(tint, t, 0.06))
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .bg(tint.opacity(0.15))
                .child(ui::icon(icon, 15.).text_color(tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .h(px(20.))
                        .child(
                            ui::text(title, TEXT_MD, FontWeight::SEMIBOLD)
                                .truncate()
                                .text_color(t.text_primary),
                        )
                        .children(
                            lasted
                                .map(|lasted| ui::mono(lasted, TEXT_2XS, cx).ml_auto().flex_none()),
                        ),
                )
                .when(!said.is_empty(), |text| {
                    text.child(
                        div()
                            .mt(px(2.))
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_secondary)
                            .child(said),
                    )
                })
                .when(!fixes.is_empty(), |text| {
                    text.child(
                        div()
                            .mt(SPACE_3H)
                            .flex()
                            .flex_wrap()
                            .gap(SPACE_1H)
                            .children(fixes),
                    )
                }),
        )
        .into_any_element()
}

fn resources(
    bench: &Workbench,
    host: &Listed,
    stats: &HostStats,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let spans = [StatsSpan::Minutes15, StatsSpan::Hour, StatsSpan::Day];
    let picked = spans
        .iter()
        .position(|span| *span == bench.hosts_page.span)
        .unwrap_or(1);
    let switch = Segmented::words(
        "hosts-span",
        vec![
            ("15m".into(), None),
            ("1h".into(), None),
            ("24h".into(), None),
        ],
        picked,
    )
    .on_select(cx.listener(move |bench, at: &usize, _, cx| {
        bench.hosts_page.span = spans[*at];
        bench.watch_hosts_page();
        bench.parts.redraw_stage(cx);
    }));
    let now = stats.now;
    let about = &stats.about;
    let end = now
        .at
        .max(stats.history.last().map_or(0, |sample| sample.at));
    let start = end.saturating_sub(span_ms(bench.hosts_page.span));
    let tint = |kind: HostAlertKind| {
        host.level(kind)
            .map_or(t.text_secondary, |level| crate::hosts::alert_tint(level, t))
    };
    let series = |kind: HostAlertKind,
                  share: &dyn Fn(&sikemux_view::host_stats::HostSample) -> f32| {
        let marks = (stats.events.iter())
            .filter(|event| event.at >= start)
            .filter_map(|event| match (&event.what, kind) {
                (HostEventKind::Killed { agent_id, .. }, HostAlertKind::Memory) => Some((
                    event.at,
                    format!(
                        "{} killed",
                        agent_id
                            .as_deref()
                            .map_or_else(|| "a process".to_owned(), |id| host.agent_name(id))
                    ),
                )),
                (HostEventKind::CommandStarted { agent_id, command }, HostAlertKind::Cpu) => {
                    Some((
                        event.at,
                        format!("{} started {command}", host.agent_name(agent_id)),
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let newest = marks.iter().map(|(at, _)| *at).max();
        Series {
            points: (stats.history.iter().chain(std::iter::once(&now)))
                .filter(|sample| sample.at >= start)
                .map(|sample| (sample.at, share(sample)))
                .collect(),
            start,
            end,
            tint: tint(kind),
            marks: marks
                .into_iter()
                .map(|(at, words)| (at, (Some(at) == newest).then_some(words)))
                .collect(),
            limit: Some(LIMIT),
        }
    };
    let share_of = |used: u64, total: u64| match total {
        0 => 0.,
        total => used as f32 / total as f32,
    };
    let (memory_total, disk_total) = (about.memory_total, about.disk_total);
    let swap = match now.swap_used {
        0 => String::new(),
        swap => format!(" · {} GB swap", model::gb(swap)),
    };
    let (memory_used, memory_of) = model::used_of(now.memory_used, memory_total);
    let (disk_used, disk_of) = model::used_of(now.disk_used, disk_total);
    let rows = [
        (
            HostAlertKind::Cpu,
            "CPU",
            Icon::Activity,
            format!(
                "{} cores · {} in use",
                about.cores,
                model::cores_used(now.cpu, about.cores)
            ),
            series(HostAlertKind::Cpu, &|sample| sample.cpu as f32 / 1000.),
        ),
        (
            HostAlertKind::Memory,
            "Memory",
            Icon::Stack,
            format!("{memory_used} / {memory_of}{swap}"),
            series(HostAlertKind::Memory, &|sample| {
                share_of(sample.memory_used, memory_total)
            }),
        ),
        (
            HostAlertKind::Disk,
            "Disk",
            Icon::Database,
            format!("{disk_used} / {disk_of}"),
            series(HostAlertKind::Disk, &|sample| {
                share_of(sample.disk_used, disk_total)
            }),
        ),
    ];
    let boxes: Vec<Div> = rows
        .into_iter()
        .enumerate()
        .map(|(at, (kind, label, icon, sub, series))| {
            let percent = host
                .percent(kind)
                .map_or_else(|| "—".to_owned(), |percent| format!("{percent:.0}%"));
            let level = host
                .level(kind)
                .map(|level| crate::hosts::alert_tint(level, t));
            let chart = match series.points.len() {
                0 | 1 => div()
                    .h(px(CHART_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        ui::text("Gathering history", TEXT_2XS_H, FontWeight::NORMAL)
                            .text_color(t.text_disabled),
                    )
                    .into_any_element(),
                _ => history(series, CHART_HEIGHT, t),
            };
            div()
                .px(SPACE_4)
                .pt(SPACE_3H)
                .pb(SPACE_3)
                .when(at > 0, |row| row.border_t_1().border_color(hair(t)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .min_w_0()
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(SPACE_1H)
                                .child(ui::icon(icon, 12.).text_color(t.text_tertiary))
                                .child(
                                    ui::text(label, TEXT_XS, FontWeight::NORMAL)
                                        .text_color(t.text_tertiary),
                                ),
                        )
                        .child(ui::mono(sub, TEXT_2XS, cx).truncate().min_w_0())
                        .child(
                            ui::mono(percent, TEXT_XS, cx)
                                .ml_auto()
                                .flex_none()
                                .text_color(level.unwrap_or(t.text_primary)),
                        ),
                )
                .child(div().mt(SPACE_3).child(chart))
                .when(kind == HostAlertKind::Disk, |row| {
                    row.child(axis(start, end, t, cx))
                        .children(disk_parts(stats, t, cx))
                })
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .child(section(
            Icon::Gauge,
            "Resources",
            None,
            Some(switch.into_any_element()),
            t,
            cx,
        ))
        .child(
            div()
                .rounded(RADIUS_3)
                .border_1()
                .border_color(hair(t))
                .bg(card(t))
                .children(boxes),
        )
}

fn span_ms(span: StatsSpan) -> u64 {
    match span {
        StatsSpan::Now | StatsSpan::Minutes15 => 15 * 60_000,
        StatsSpan::Hour => 60 * 60_000,
        StatsSpan::Day => 24 * 60 * 60_000,
    }
}

/// Five times under the charts, from where they start to now.
fn axis(start: u64, end: u64, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let step = end.saturating_sub(start) / 4;
    div()
        .mt(SPACE_1)
        .flex()
        .justify_between()
        .children((0..5).map(|at| {
            let words = match at {
                4 => "now".to_owned(),
                at => crate::ui::time::clock(start + step * at),
            };
            ui::mono(words, TEXT_3XS, cx).text_color(t.text_disabled)
        }))
}

fn part_words(kind: sikemux_view::host_stats::DiskPartKind) -> &'static str {
    use sikemux_view::host_stats::DiskPartKind;
    match kind {
        DiskPartKind::BuildOutput => "Build output",
        DiskPartKind::Worktrees => "Worktrees",
        DiskPartKind::Caches => "Caches",
        DiskPartKind::Containers => "Containers",
        DiskPartKind::Other | DiskPartKind::Unknown => "Everything else",
    }
}

fn disk_parts(stats: &HostStats, t: &Tokens, cx: &mut Context<Workbench>) -> Option<Div> {
    if stats.disk_parts.is_empty() {
        return None;
    }
    Some(
        div()
            .mt(SPACE_2H)
            .flex()
            .flex_col()
            .children(stats.disk_parts.iter().enumerate().map(|(at, part)| {
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .when(at > 0, |row| row.border_t_1().border_color(hair(t)))
                    .child(
                        ui::text(part_words(part.kind), TEXT_XS, FontWeight::NORMAL)
                            .flex_1()
                            .truncate()
                            .text_color(t.text_tertiary),
                    )
                    .child(
                        ui::mono(model::size(part.bytes), TEXT_2XS_H, cx)
                            .text_color(t.text_primary),
                    )
            })),
    )
}

/// One agent's row as the detail lists it.
struct AgentLine<'a> {
    id: String,
    shown: Option<&'a AgentShown>,
    using: Option<&'a AgentUse>,
    killed: bool,
}

fn agents(
    bench: &Workbench,
    host: &Listed,
    listed: &[Listed],
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Option<Div> {
    let stats = host.stats();
    let using: Vec<&AgentUse> = stats.into_iter().flat_map(|stats| &stats.agents).collect();
    let killed = host.killed();
    let mut lines: Vec<AgentLine> = (using.iter())
        .map(|using| AgentLine {
            id: using.agent_id.clone(),
            shown: host.agent(&using.agent_id),
            using: Some(*using),
            killed: killed.contains(&using.agent_id),
        })
        .collect();
    for shown in host.agents() {
        if !lines.iter().any(|line| line.id == shown.agent_id) {
            lines.push(AgentLine {
                id: shown.agent_id.clone(),
                shown: Some(shown),
                using: None,
                killed: killed.contains(&shown.agent_id),
            });
        }
    }
    let rest = stats.and_then(|stats| stats.rest.as_ref());
    if lines.is_empty() && rest.is_none() {
        return None;
    }
    let people: BTreeMap<String, Vec<&AgentLine>> =
        lines.iter().fold(BTreeMap::new(), |mut people, line| {
            if let Some(person) = line.using.and_then(|using| using.person.clone()) {
                people.entry(person).or_default().push(line);
            }
            people
        });
    let move_to: Vec<&Listed> = (listed.iter())
        .filter(|other| other.key != host.key && other.online())
        .collect();
    let row = |line: &AgentLine, indent: bool, cx: &mut Context<Workbench>| {
        agent_row(bench, host, &move_to, line, indent, t, cx)
    };
    let mut rows: Vec<AnyElement> = Vec::new();
    if people.is_empty() {
        for line in &lines {
            rows.push(row(line, false, cx));
        }
    } else {
        let share = |lines: &Vec<&AgentLine>| -> u32 {
            (lines.iter())
                .filter_map(|line| line.using)
                .map(|using| using.cpu as u32)
                .sum()
        };
        let mut people: Vec<(&String, &Vec<&AgentLine>)> = people.iter().collect();
        people.sort_by_key(|(_, lines)| std::cmp::Reverse(share(lines)));
        for (person, lines) in people {
            let folded = bench.hosts_page.folded.contains(person);
            rows.push(person_row(person, lines, folded, t, cx));
            if !folded {
                for line in lines {
                    rows.push(row(line, true, cx));
                }
            }
        }
        for line in lines
            .iter()
            .filter(|line| line.using.and_then(|using| using.person.as_ref()).is_none())
        {
            rows.push(row(line, false, cx));
        }
    }
    if let Some(rest) = rest {
        rows.push(rest_row(rest, t, cx));
    }
    let (label, count) = match people.is_empty() {
        true => ("Agents", lines.len().to_string()),
        false => (
            "People & agents",
            format!("{} · {}", people.len(), lines.len()),
        ),
    };
    Some(
        div()
            .flex()
            .flex_col()
            .child(section(
                if people.is_empty() {
                    Icon::Agents
                } else {
                    Icon::User
                },
                label,
                Some(count),
                Some(
                    ui::text("CPU · memory", TEXT_2XS_H, FontWeight::NORMAL)
                        .text_color(t.text_tertiary)
                        .into_any_element(),
                ),
                t,
                cx,
            ))
            .child(list(t).children(rows)),
    )
}

fn list(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(RADIUS_3)
        .border_1()
        .border_color(hair(t))
        .bg(card(t))
        .overflow_hidden()
}

/// A share and a size stacked at a row's end, as each agent's use shows.
fn using(
    cpu: Option<u16>,
    memory: Option<u64>,
    quiet: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let cpu = cpu
        .filter(|cpu| *cpu > 0)
        .map_or_else(|| "—".to_owned(), model::percent);
    div()
        .flex_none()
        .min_w(px(52.))
        .flex()
        .flex_col()
        .items_end()
        .child(ui::mono(cpu, TEXT_XS, cx).text_color(if quiet {
            t.text_disabled
        } else {
            t.text_primary
        }))
        .children(
            memory.map(|memory| ui::mono(format!("{} GB", model::gb(memory)), TEXT_3XS_H, cx)),
        )
}

fn agent_row(
    bench: &Workbench,
    host: &Listed,
    move_to: &[&Listed],
    line: &AgentLine,
    indent: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let shown = line.shown;
    let status = shown.map_or(AgentStatus::Idle, |shown| shown.status);
    let idle = !matches!(status, AgentStatus::Working | AgentStatus::Blocked) && !line.killed;
    let name = host.agent_name(&line.id);
    let command = line.using.and_then(|using| using.command.clone());
    let since = line.using.and_then(|using| using.command_since);
    let mark = match shown {
        Some(shown) => ui::still_mark(&shown.cli, shown.face.as_ref(), status.group(), 14., cx),
        None => ui::icon(Icon::Agent, 14.)
            .text_color(t.text_tertiary)
            .into_any_element(),
    };
    let group = SharedString::from(format!("host-agent-{}", line.id));
    let second: AnyElement = match (line.killed, &command) {
        (true, _) => div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .min_w_0()
            .child(
                ui::text("Killed for memory", TEXT_2XS_H, FontWeight::NORMAL)
                    .flex_none()
                    .text_color(t.danger),
            )
            .children(command.clone().map(|command| {
                ui::mono(command, TEXT_2XS, cx)
                    .truncate()
                    .line_through()
                    .text_color(t.text_tertiary)
            }))
            .into_any_element(),
        (false, Some(command)) if !idle || line.using.is_some_and(|using| using.cpu > 0) => {
            ui::mono(command.clone(), TEXT_2XS, cx)
                .truncate()
                .text_color(t.text_secondary)
                .into_any_element()
        }
        _ => ui::text(
            match status {
                AgentStatus::Asleep => "Asleep",
                AgentStatus::Stopped => "Stopped",
                AgentStatus::Starting => "Starting",
                _ => "Idle",
            },
            TEXT_2XS_H,
            FontWeight::NORMAL,
        )
        .text_color(t.text_tertiary)
        .into_any_element(),
    };
    let menu = agent_menu(bench, host, move_to, &line.id, cx);
    div()
        .id(group.clone())
        .group(group.clone())
        .flex()
        .items_center()
        .gap(px(11.))
        .pl(if indent { px(26.) } else { SPACE_3 })
        .pr(SPACE_2H)
        .py(px(9.))
        .border_t_1()
        .border_color(hair(t))
        .hover(|row| row.bg(t.surface_hover))
        .child(
            div()
                .size(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .bg(ink(t, 0.05))
                .when(idle, |mark| mark.opacity(0.55))
                .child(mark),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(SPACE_2)
                        .min_w_0()
                        .child(
                            ui::text(name, TEXT_SM, FontWeight::MEDIUM)
                                .flex_none()
                                .text_color(if idle {
                                    t.text_tertiary
                                } else {
                                    t.text_primary
                                }),
                        )
                        .children(shown.and_then(|shown| shown.project_name.clone()).map(
                            |project| {
                                ui::text(project, TEXT_2XS_H, FontWeight::NORMAL)
                                    .truncate()
                                    .text_color(t.text_tertiary)
                            },
                        ))
                        .children(since.filter(|_| !idle).map(|since| {
                            ui::mono(format!("· {}", crate::ui::time::age(since)), TEXT_3XS_H, cx)
                                .flex_none()
                                .text_color(t.text_disabled)
                        })),
                )
                .child(second),
        )
        .child(using(
            line.using.map(|using| using.cpu),
            line.using.map(|using| using.memory),
            idle,
            t,
            cx,
        ))
        .child(
            div()
                .w(px(22.))
                .flex_none()
                .invisible()
                .group_hover(group, |more| more.visible())
                .children(menu),
        )
        .into_any_element()
}

/// What can be done to one agent from here: moving its project to another host, and closing it.
fn agent_menu(
    bench: &Workbench,
    host: &Listed,
    move_to: &[&Listed],
    agent: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if !host.online() {
        return None;
    }
    let mut fixes: Vec<Fix> = (move_to.iter())
        .filter_map(|to| move_fix(bench, host, to, agent))
        .collect();
    fixes.extend(close_fix(host, agent));
    if fixes.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let fixes: Vec<(String, std::rc::Rc<Run>)> = fixes
        .into_iter()
        .map(|fix| (fix.words, std::rc::Rc::from(fix.run)))
        .collect();
    let bench_entity = cx.weak_entity();
    let trigger = {
        use gpui_kit::component::button::ButtonVariants;
        gpui_kit::component::button::Button::new(SharedString::from(format!(
            "host-agent-more-{agent}"
        )))
        .ghost()
        .compact()
        .rounded(RADIUS_1)
        .child(ui::icon(Icon::More, 12.).text_color(t.text_secondary))
    };
    use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
    Some(
        trigger
            .dropdown_menu_with_anchor(
                gpui_kit::Anchor::TopRight,
                ui::menu(move |popup, _, _| {
                    fixes.iter().fold(popup, |popup, (words, run)| {
                        let (bench, run) = (bench_entity.clone(), run.clone());
                        popup.item(PopupMenuItem::new(words.clone()).on_click(move |_, _, cx| {
                            let _ = bench.update(cx, |bench, cx| run(bench, cx));
                        }))
                    })
                }),
            )
            .into_any_element(),
    )
}

fn person_row(
    person: &str,
    lines: &[&AgentLine],
    folded: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let running = lines
        .iter()
        .filter(|line| {
            line.shown.is_some_and(|shown| {
                matches!(shown.status, AgentStatus::Working | AgentStatus::Blocked)
            })
        })
        .count();
    let idle = lines.len() - running;
    let cpu: u16 = lines
        .iter()
        .filter_map(|line| line.using)
        .map(|using| using.cpu)
        .sum();
    let memory: u64 = lines
        .iter()
        .filter_map(|line| line.using)
        .map(|using| using.memory)
        .sum();
    let summary = match (running, idle) {
        (0, idle) => format!("{idle} idle"),
        (running, 0) => format!("{running} running"),
        (running, idle) => format!("{running} running · {idle} idle"),
    };
    let key = person.to_owned();
    div()
        .id(SharedString::from(format!("host-person-{person}")))
        .flex()
        .items_center()
        .gap(px(11.))
        .px(SPACE_3)
        .py(px(9.))
        .border_t_1()
        .border_color(hair(t))
        .bg(ink(t, 0.02))
        .cursor_pointer()
        .hover(|row| row.bg(t.surface_hover))
        .on_click(cx.listener(move |bench, _, _, cx| {
            let folded = &mut bench.hosts_page.folded;
            if !folded.remove(&key) {
                folded.insert(key.clone());
            }
            bench.parts.redraw_stage(cx);
        }))
        .child(ui::initials(person, person, 24., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    ui::text(person.to_owned(), TEXT_SM, FontWeight::MEDIUM)
                        .text_color(t.text_primary),
                )
                .child(
                    ui::text(
                        match folded {
                            true => format!("{summary} · hidden"),
                            false => summary,
                        },
                        TEXT_2XS_H,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                ),
        )
        .child(using(Some(cpu), Some(memory), false, t, cx))
        .child(div().w(px(22.)).flex_none())
        .into_any_element()
}

fn rest_row(
    rest: &sikemux_view::host_stats::RestUse,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let named = rest
        .names
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = (rest.count as usize).saturating_sub(rest.names.len().min(3));
    let named = match more {
        0 => named,
        more => format!("{named} +{more}"),
    };
    div()
        .flex()
        .items_center()
        .gap(px(11.))
        .pl(SPACE_3)
        .pr(SPACE_2H)
        .py(px(9.))
        .border_t_1()
        .border_color(hair(t))
        .child(div().size(px(24.)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    ui::text("Everything else", TEXT_SM, FontWeight::NORMAL)
                        .text_color(t.text_tertiary),
                )
                .child(ui::mono(named, TEXT_2XS, cx).truncate()),
        )
        .child(using(Some(rest.cpu), Some(rest.memory), false, t, cx))
        .child(div().w(px(22.)).flex_none())
        .into_any_element()
}

fn top(
    bench: &Workbench,
    host: &Listed,
    stats: &HostStats,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let open = bench.hosts_page.top_open;
    let head = div()
        .id("hosts-top-fold")
        .mt(px(28.))
        .mb(SPACE_3)
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .cursor_pointer()
        .on_click(cx.listener(|bench, _, _, cx| {
            bench.hosts_page.top_open = !bench.hosts_page.top_open;
            bench.parts.redraw_stage(cx);
        }))
        .child(
            ui::icon(Icon::Chevron, 12.)
                .text_color(t.text_tertiary)
                .when(open, |chevron| {
                    chevron.with_transformation(gpui_kit::Transformation::rotate(
                        gpui_kit::radians(std::f32::consts::FRAC_PI_2),
                    ))
                }),
        )
        .child(ui::caps("Top processes", cx))
        .child(
            div()
                .px(SPACE_1H)
                .rounded(px(8.))
                .bg(ink(t, 0.06))
                .child(ui::mono(stats.top.len().to_string(), TEXT_3XS, cx)),
        )
        .child(
            ui::text("by CPU", TEXT_2XS_H, FontWeight::NORMAL)
                .ml_auto()
                .text_color(t.text_tertiary),
        );
    div().flex().flex_col().child(head).when(open, |section| {
        section.child(
            list(t).children(stats.top.iter().enumerate().map(|(at, process)| {
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .px(SPACE_3)
                    .h(px(34.))
                    .when(at > 0, |row| row.border_t_1().border_color(hair(t)))
                    .child(
                        ui::mono(process.name.clone(), TEXT_XS, cx)
                            .truncate()
                            .text_color(t.text_secondary),
                    )
                    .child(
                        ui::mono(process.pid.to_string(), TEXT_3XS_H, cx)
                            .text_color(t.text_disabled),
                    )
                    .children(process.agent_id.as_deref().map(|agent| {
                        ui::text(host.agent_name(agent), TEXT_2XS_H, FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                    }))
                    .child(div().flex_1())
                    .child(using(Some(process.cpu), Some(process.memory), false, t, cx))
            })),
        )
    })
}

fn activity(host: &Listed, stats: &HostStats, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let rows: Vec<Div> = stats
        .events
        .iter()
        .take(ACTIVITY_SHOWN)
        .filter_map(|event| {
            let (icon, tint, words) = match &event.what {
                HostEventKind::AlertStarted { alert, percent } => (
                    Icon::Warning,
                    t.warn,
                    format!(
                        "{} passed {percent}%",
                        crate::hosts::alert_kind_word(*alert)
                    ),
                ),
                HostEventKind::AlertEnded { alert } => (
                    Icon::Check,
                    t.text_tertiary,
                    format!("{} back to normal", crate::hosts::alert_kind_word(*alert)),
                ),
                HostEventKind::Killed { agent_id, name } => (
                    Icon::Warning,
                    t.danger,
                    match agent_id {
                        Some(agent) => {
                            format!("Killed {}’s {name} to free memory", host.agent_name(agent))
                        }
                        None => format!("Killed {name} to free memory"),
                    },
                ),
                HostEventKind::CommandStarted { agent_id, command } => (
                    Icon::Run,
                    t.text_tertiary,
                    format!("{} started {command}", host.agent_name(agent_id)),
                ),
                HostEventKind::Unknown => return None,
            };
            Some(
                div()
                    .min_h(px(36.))
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(crate::theme::mix(tint, t.surface_raised, 0.13))
                            .child(ui::icon(icon, 12.).text_color(tint)),
                    )
                    .child(
                        ui::text(words, TEXT_SM, FontWeight::NORMAL)
                            .flex_1()
                            .truncate()
                            .text_color(t.text_secondary),
                    )
                    .child(ui::mono(crate::ui::time::clock(event.at), TEXT_2XS, cx).flex_none()),
            )
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .child(section(Icon::Activity, "Activity", None, None, t, cx))
        .child(
            div()
                .relative()
                .px(SPACE_4)
                .py(SPACE_1H)
                .rounded(RADIUS_3)
                .border_1()
                .border_color(hair(t))
                .bg(card(t))
                .child(
                    div()
                        .absolute()
                        .left(px(28.))
                        .top(SPACE_6)
                        .bottom(SPACE_6)
                        .w(px(1.))
                        .bg(hair(t)),
                )
                .children(rows),
        )
}

fn about(stats: &HostStats, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let about = &stats.about;
    let rows: Vec<(Icon, &str, String, bool)> = [
        about
            .model
            .clone()
            .map(|model| (Icon::Laptop, "Model", model, false)),
        Some((Icon::Settings, "System", about.system.clone(), false)),
        about
            .gpu
            .clone()
            .map(|gpu| (Icon::Cube, "Graphics", gpu, false)),
        Some((Icon::Logo, "Sikemux", about.core_version.clone(), true)),
    ]
    .into_iter()
    .flatten()
    .collect();
    div()
        .flex()
        .flex_col()
        .child(section(Icon::InfoCircle, "About", None, None, t, cx))
        .child(
            div()
                .px(SPACE_4)
                .py(SPACE_1)
                .rounded(RADIUS_3)
                .border_1()
                .border_color(hair(t))
                .bg(card(t))
                .children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(at, (icon, label, value, mono))| {
                            div()
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .when(at > 0, |row| row.border_t_1().border_color(hair(t)))
                                .child(ui::icon(icon, 13.).text_color(t.text_tertiary))
                                .child(
                                    ui::text(label, TEXT_SM, FontWeight::NORMAL)
                                        .text_color(t.text_tertiary),
                                )
                                .child(
                                    match mono {
                                        true => ui::mono(value, TEXT_XS, cx),
                                        false => ui::text(value, TEXT_SM, FontWeight::NORMAL),
                                    }
                                    .ml_auto()
                                    .truncate()
                                    .text_color(t.text_primary),
                                )
                        }),
                ),
        )
}

impl Workbench {
    /// Closes the agent `id` on the host at `place`, once the person agrees, which stops it.
    pub fn close_agent_on(&mut self, place: &Place, id: &str, name: &str, cx: &mut Context<Self>) {
        let (place, id) = (place.clone(), id.to_owned());
        self.confirm(
            format!("Close {name}?"),
            "It stops what it is running now.",
            "Close",
            true,
            move |bench, yes, _, _| {
                if yes {
                    crate::core::Target::new(&bench.asks, place.clone())
                        .call_typed::<sikemux_core::server::call::CloseAgents>(
                        sikemux_core::server::workspace::CloseAgentsParams {
                            agent_ids: vec![id.clone()],
                        },
                    );
                }
            },
            cx,
        );
    }
}
