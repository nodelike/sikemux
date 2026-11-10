//! The page under its detail: how many hosts are up and agents running, a search, filters,
//! every host as a card or a table row, then the devices.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_view::host_stats::{AlertLevel, HostAlertKind};

use super::model::{self, Counts, Listed, State};
use super::{hair, ink};
use crate::icons::Icon;
use crate::keymap;
use crate::theme::{
    self, Tokens, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_6, TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_MD, TEXT_XL, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, Field, Segmented, Toggle, Variant};
use crate::workbench::actions;
use crate::workbench::Workbench;

const CARD_MIN: f32 = 262.;
const CARD_MAX: f32 = 380.;
const GAP: f32 = 12.;

/// The page's search, buttons and filters, which stay in place, and the hosts and devices,
/// which scroll under them.
pub struct Board {
    pub controls: AnyElement,
    pub body: AnyElement,
}

pub fn board(
    bench: &Workbench,
    listed: &[Listed],
    chosen: Option<&str>,
    cx: &mut Context<Workbench>,
) -> Board {
    let t = Tokens::get(cx).clone();
    let page = &bench.hosts_page;
    let query = page.search.read(cx).value().trim().to_lowercase();
    let needing = listed.iter().filter(|host| host.state.needs_you()).count();
    let shown: Vec<&Listed> = (listed.iter())
        .filter(|host| !page.needs_you || host.state.needs_you())
        .filter(|host| query.is_empty() || matches(host, &query))
        .collect();
    let hosts = match (shown.is_empty(), page.table) {
        (true, _) => empty(&t, listed.is_empty()).into_any_element(),
        (false, true) => table(&shown, chosen, &t, cx),
        (false, false) => cards(&shown, chosen, &t, cx),
    };
    let controls = div()
        .flex_none()
        .px(SPACE_6)
        .pt(SPACE_6)
        .flex()
        .flex_col()
        .child(head(bench, listed, page, &t, cx))
        .child(
            div()
                .mt(SPACE_6)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::caps("Hosts", cx))
                .child(
                    ui::mono(listed.len().to_string(), TEXT_2XS_H, cx).text_color(t.text_tertiary),
                ),
        )
        .child(filters(listed.len(), needing, page, &t, cx))
        .into_any_element();
    let body = div()
        .px(SPACE_6)
        .pb(px(48.))
        .flex()
        .flex_col()
        .child(hosts)
        .child(super::devices::devices(bench, &query, &t, cx))
        .into_any_element();
    Board { controls, body }
}

fn matches(host: &Listed, query: &str) -> bool {
    let found = |words: &str| words.to_lowercase().contains(query);
    found(&host.name)
        || found(&host.model())
        || host.agents().any(|agent| {
            agent
                .name
                .as_deref()
                .is_some_and(|name| found(name) || found(&format!("@{name}")))
        })
        || (host.stats().into_iter())
            .flat_map(|stats| &stats.agents)
            .any(|agent| agent.person.as_deref().is_some_and(found))
}

fn head(
    bench: &Workbench,
    listed: &[Listed],
    page: &super::Page,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let online = listed.iter().filter(|host| host.online()).count();
    let counts: Vec<Counts> = listed.iter().map(Listed::counts).collect();
    let running: usize = counts.iter().map(|counts| counts.running).sum();
    let agents: usize = counts.iter().map(Counts::total).sum();
    let figure = |of: usize, all: usize, words: &str| {
        div()
            .flex()
            .items_baseline()
            .gap(px(4.))
            .child(ui::mono(of.to_string(), TEXT_XS, cx).text_color(t.text_primary))
            .child(ui::text(
                format!("of {all} {words}"),
                TEXT_XS_H,
                FontWeight::NORMAL,
            ))
    };
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap(SPACE_4)
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    ui::text("Devices", TEXT_XL, FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .letter_spacing(-0.02),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .text_color(t.text_tertiary)
                        .child(figure(online, listed.len(), "online"))
                        .child(div().size(px(3.)).rounded_full().bg(t.text_disabled))
                        .child(figure(running, agents, "agents running")),
                ),
        )
        .child(
            div()
                .ml_auto()
                .flex()
                .flex_wrap()
                .justify_end()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .w(px(240.))
                        .child(Field::new(&page.search).icon(Icon::Search).small()),
                )
                .child(
                    Button::new("hosts-add", "Add host", Variant::Outline)
                        .icon(Icon::Plus)
                        .keys(keymap::hint(actions::OpenHost::ID, cx))
                        .on_click(|_, window, cx| {
                            keymap::dispatch(actions::OpenHost::ID, window, cx)
                        }),
                )
                .child(super::devices::pair_button(bench)),
        )
}

fn filters(
    all: usize,
    needing: usize,
    page: &super::Page,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let needs_you = page.needs_you;
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .mt(SPACE_2H)
        .mb(SPACE_3H)
        .child(
            Toggle::new("hosts-all", "All", !needs_you)
                .count(all, t.text_tertiary)
                .on_click(cx.listener(|bench, _, _, cx| {
                    bench.hosts_page.needs_you = false;
                    bench.parts.redraw_stage(cx);
                })),
        )
        .child(
            Toggle::new("hosts-needs-you", "Needs you", needs_you)
                .count(
                    needing,
                    if needing > 0 {
                        t.danger
                    } else {
                        t.text_tertiary
                    },
                )
                .on_click(cx.listener(|bench, _, _, cx| {
                    bench.hosts_page.needs_you = true;
                    bench.parts.redraw_stage(cx);
                })),
        )
        .child(
            div().ml_auto().child(
                Segmented::new(
                    "hosts-layout",
                    vec![
                        (Icon::Dashboard, "Cards".into()),
                        (Icon::Table, "Table".into()),
                    ],
                    usize::from(page.table),
                )
                .on_select(cx.listener(|bench, at: &usize, _, cx| {
                    bench.hosts_page.table = *at == 1;
                    bench.parts.redraw_stage(cx);
                })),
            ),
        )
}

fn empty(t: &Tokens, none_at_all: bool) -> Div {
    div()
        .h(px(120.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_dashed()
        .border_color(t.border)
        .rounded(RADIUS_4)
        .text_color(t.text_tertiary)
        .child(ui::text(
            match none_at_all {
                true => "No hosts yet",
                false => "No host matches",
            },
            TEXT_XS_H,
            FontWeight::NORMAL,
        ))
}

fn cards(
    shown: &[&Listed],
    chosen: Option<&str>,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(GAP))
        .children(shown.iter().map(|host| {
            div()
                .flex_1()
                .min_w(px(CARD_MIN))
                .max_w(px(CARD_MAX))
                .flex()
                .child(host_card(host, chosen == Some(host.key.as_str()), t, cx))
        }))
        .into_any_element()
}

/// The ground of a host's card or table, `share` of the ink mixed in for hover and choice.
pub(super) fn lifted(t: &Tokens, share: f32) -> Hsla {
    theme::mix(t.text_primary, t.surface_sunken, share).opacity(0.85)
}

fn state_mark(host: &Listed, tint: Hsla) -> AnyElement {
    match host.state.icon() {
        Some(glyph) => ui::icon(glyph, 13.).text_color(tint).into_any_element(),
        None => div()
            .size(px(6.))
            .mx(px(3.5))
            .rounded_full()
            .bg(tint)
            .into_any_element(),
    }
}

type Clicked = dyn Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App);

/// What a click on a host's card or row does: shows its detail.
fn chooses(key: &str, cx: &mut Context<Workbench>) -> Box<Clicked> {
    let key = key.to_owned();
    Box::new(cx.listener(move |bench, _, _, cx| bench.choose_on_page(key.clone(), cx)))
}

fn host_card(host: &Listed, chosen: bool, t: &Tokens, cx: &mut Context<Workbench>) -> AnyElement {
    let ground = lifted(t, 0.);
    div()
        .id(SharedString::from(format!("host-card-{}", host.key)))
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(RADIUS_4)
        .border_1()
        .border_color(hair(t))
        .overflow_hidden()
        .cursor_pointer()
        .bg(ground)
        .when(chosen, |card| {
            card.bg(lifted(t, 0.05))
                .border_color(t.accent.opacity(0.35))
        })
        .when(!chosen, |card| card.hover(|card| card.bg(lifted(t, 0.03))))
        .on_click(chooses(&host.key, cx))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(11.))
                .px(SPACE_4)
                .py(SPACE_3H)
                .border_b_1()
                .border_color(hair(t))
                .child(tile(host, 36., 18., t))
                .child(name_and_model(host, TEXT_MD, t))
                .child(state_corner(host, t, cx)),
        )
        .child(
            div()
                .px(SPACE_4)
                .pt(SPACE_3H)
                .pb(SPACE_4)
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .children(METERS.iter().map(|kind| meter(host, *kind, false, t, cx))),
        )
        .child(
            div()
                .mt_auto()
                .min_h(px(43.))
                .px(SPACE_4)
                .py(SPACE_3)
                .border_t_1()
                .border_color(hair(t))
                .flex()
                .items_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .text_color(t.text_tertiary)
                        .child(ui::icon(Icon::Agents, 12.).text_color(t.text_tertiary))
                        .child(ui::text("Agents", TEXT_XS, FontWeight::NORMAL)),
                )
                .child(div().ml_auto().child(counts(host.counts(), t))),
        )
        .into_any_element()
}

pub const METERS: [HostAlertKind; 3] = [
    HostAlertKind::Cpu,
    HostAlertKind::Memory,
    HostAlertKind::Disk,
];

/// A host's logo on its tile, greyed while it is out of reach.
pub fn tile(host: &Listed, size: f32, glyph: f32, t: &Tokens) -> Div {
    let (logo, tint) = model::logo(host.mark(), t);
    let online = host.online();
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 0.28))
        .bg(ink(t, 0.05))
        .border_1()
        .border_color(hair(t))
        .when(!online, |tile| tile.opacity(0.55))
        .child(ui::icon(logo, glyph).text_color(if online { tint } else { t.text_tertiary }))
}

fn name_and_model(host: &Listed, size: f32, t: &Tokens) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            ui::text(host.name.clone(), size, FontWeight::SEMIBOLD)
                .truncate()
                .text_color(if host.online() {
                    t.text_primary
                } else {
                    t.text_tertiary
                }),
        )
        .child(
            ui::text(host.model(), TEXT_2XS_H, FontWeight::NORMAL)
                .truncate()
                .text_color(t.text_tertiary),
        )
}

/// How the host stands, at a card's top right: its icon and words, and how long below.
fn state_corner(host: &Listed, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let tint = host.state.tint(t);
    let quiet = host.state == State::Healthy;
    div()
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(state_mark(host, tint))
                .child(
                    ui::text(
                        host.state.words(),
                        TEXT_XS,
                        if quiet {
                            FontWeight::NORMAL
                        } else {
                            FontWeight::MEDIUM
                        },
                    )
                    .text_color(if quiet { t.text_tertiary } else { tint }),
                ),
        )
        .children(
            host.state
                .lasted()
                .map(|lasted| ui::mono(lasted, TEXT_3XS_H, cx)),
        )
}

/// What a meter says of `kind`: how much is used of how much, what share, and its colour.
struct Reading {
    used: Option<String>,
    of: String,
    share: Option<f32>,
    tint: Option<Hsla>,
}

fn reading(host: &Listed, kind: HostAlertKind, short: bool, t: &Tokens) -> Reading {
    let Some(stats) = host.stats() else {
        return Reading {
            used: None,
            of: String::new(),
            share: None,
            tint: None,
        };
    };
    let (about, now) = (&stats.about, &stats.now);
    let online = host.online();
    let (used, of) = match kind {
        HostAlertKind::Cpu => (
            model::cores_used(now.cpu, about.cores),
            match short {
                true => about.cores.to_string(),
                false => format!("{} cores", about.cores),
            },
        ),
        HostAlertKind::Memory => model::used_of(now.memory_used, about.memory_total),
        _ => model::used_of(now.disk_used, about.disk_total),
    };
    Reading {
        used: online.then_some(used),
        of,
        share: online
            .then(|| host.percent(kind))
            .flatten()
            .map(|percent| percent / 100.),
        tint: host.level(kind).map(|level| match level {
            AlertLevel::Warn => t.warn,
            AlertLevel::Danger => t.danger,
        }),
    }
}

pub(super) fn meter(
    host: &Listed,
    kind: HostAlertKind,
    short: bool,
    t: &Tokens,
    cx: &gpui_kit::App,
) -> Div {
    let reading = reading(host, kind, short, t);
    let (key, glyph) = match kind {
        HostAlertKind::Cpu => ("CPU", Icon::Activity),
        HostAlertKind::Memory => ("Mem", Icon::Stack),
        _ => ("Disk", Icon::Database),
    };
    let lit = reading.tint.unwrap_or(ink(t, 0.62));
    let percent = reading
        .share
        .map_or_else(|| "—".to_owned(), |share| format!("{:.0}%", share * 100.));
    let amount = div()
        .flex()
        .items_baseline()
        .min_w_0()
        .overflow_hidden()
        .children(reading.used.map(|used| {
            ui::mono(used, if short { TEXT_3XS_H } else { TEXT_2XS }, cx)
                .text_color(t.text_secondary)
        }))
        .child(
            ui::mono(
                match (short, reading.share.is_some()) {
                    (true, true) => format!("/{}", reading.of),
                    (false, true) => format!(" / {}", reading.of),
                    (_, false) => reading.of.clone(),
                },
                if short { TEXT_3XS_H } else { TEXT_2XS },
                cx,
            )
            .when(reading.share.is_none(), |of| of.text_color(t.text_disabled)),
        );
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .when(!short, |top| {
                    top.child(
                        div()
                            .w(px(46.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .text_color(t.text_tertiary)
                            .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
                            .child(ui::text(key, TEXT_XS, FontWeight::NORMAL)),
                    )
                })
                .child(amount)
                .child(
                    ui::mono(percent, if short { TEXT_2XS_H } else { TEXT_XS }, cx)
                        .ml_auto()
                        .flex_none()
                        .text_color(match (reading.share.is_some(), reading.tint) {
                            (false, _) => t.text_disabled,
                            (true, Some(tint)) => tint,
                            (true, None) => t.text_primary,
                        }),
                ),
        )
        .child(
            div()
                .mt(px(if short { 5. } else { 6. }))
                .child(super::chart::ticks(
                    reading.share.unwrap_or(0.),
                    lit,
                    ink(t, 0.1),
                    if short { 7. } else { 8. },
                )),
        )
}

/// How many of a host's agents run, idle, were killed or stopped, each after its dot.
pub fn counts(counts: Counts, t: &Tokens) -> Div {
    if counts.is_empty() {
        return div()
            .child(ui::text("No agents", TEXT_XS, FontWeight::NORMAL).text_color(t.text_tertiary));
    }
    let count = |count: usize, words: &'static str, dot: Div| {
        (count > 0).then(|| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .child(dot.mr(px(2.)))
                .child(
                    ui::text(count.to_string(), TEXT_XS, FontWeight::SEMIBOLD)
                        .text_color(t.text_primary),
                )
                .child(ui::text(words, TEXT_XS, FontWeight::NORMAL).text_color(t.text_tertiary))
        })
    };
    let dot = |tint: Hsla| div().size(px(6.)).rounded_full().bg(tint);
    let ring = || {
        div()
            .size(px(6.))
            .rounded_full()
            .border_1()
            .border_color(t.text_tertiary)
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(SPACE_3H)
        .gap_y(SPACE_1)
        .children(count(counts.running, "running", dot(t.live)))
        .children(count(counts.idle, "idle", dot(t.text_tertiary)))
        .children(count(counts.killed, "killed", dot(t.danger)))
        .children(count(counts.stopped, "stopped", ring()))
        .children(count(counts.away, "out of reach", ring()))
}

/// The columns of the table, as shares of its width: host, status, CPU, memory, disk, agents.
const COLUMNS: [f32; 6] = [0.19, 0.17, 0.14, 0.17, 0.16, 0.17];

fn table(
    shown: &[&Listed],
    chosen: Option<&str>,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let column = |at: usize| div().w(relative(COLUMNS[at])).min_w_0().pr(SPACE_6);
    let heads = [
        ("Host", None),
        ("Status", None),
        ("CPU", Some(Icon::Activity)),
        ("Memory", Some(Icon::Stack)),
        ("Disk", Some(Icon::Database)),
        ("Agents", Some(Icon::Agents)),
    ];
    div()
        .rounded(RADIUS_4)
        .border_1()
        .border_color(hair(t))
        .bg(lifted(t, 0.))
        .overflow_hidden()
        .child(
            div()
                .h(px(36.))
                .px(SPACE_4)
                .flex()
                .items_center()
                .border_b_1()
                .border_color(hair(t))
                .bg(ink(t, 0.015))
                .children(heads.into_iter().enumerate().map(|(at, (words, glyph))| {
                    column(at).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .children(
                                glyph.map(|glyph| ui::icon(glyph, 11.).text_color(t.text_tertiary)),
                            )
                            .child(
                                ui::text(words.to_uppercase(), TEXT_3XS, FontWeight::MEDIUM)
                                    .letter_spacing(0.12)
                                    .truncate()
                                    .text_color(t.text_tertiary),
                            ),
                    )
                })),
        )
        .children(shown.iter().enumerate().map(|(at, host)| {
            let picked = chosen == Some(host.key.as_str());
            let tint = host.state.tint(t);
            let quiet = host.state == State::Healthy;
            div()
                .id(SharedString::from(format!("host-row-{}", host.key)))
                .min_h(px(58.))
                .px(SPACE_4)
                .flex()
                .items_center()
                .cursor_pointer()
                .when(at > 0, |row| row.border_t_1().border_color(hair(t)))
                .when(picked, |row| row.bg(ink(t, 0.05)))
                .when(!picked, |row| row.hover(|row| row.bg(ink(t, 0.03))))
                .on_click(chooses(&host.key, cx))
                .child(
                    column(0).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2H)
                            .child(tile(host, 30., 15., t))
                            .child(name_and_model(host, 13., t)),
                    ),
                )
                .child(
                    column(1).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(state_mark(host, tint))
                            .child(
                                ui::text(
                                    host.state.words(),
                                    TEXT_XS,
                                    if quiet {
                                        FontWeight::NORMAL
                                    } else {
                                        FontWeight::MEDIUM
                                    },
                                )
                                .truncate()
                                .text_color(if quiet {
                                    t.text_tertiary
                                } else {
                                    tint
                                }),
                            ),
                    ),
                )
                .children(
                    METERS
                        .iter()
                        .enumerate()
                        .map(|(at, kind)| column(at + 2).child(meter(host, *kind, true, t, cx))),
                )
                .child(column(5).child(counts(host.counts(), t).flex_nowrap().overflow_hidden()))
        }))
        .into_any_element()
}
