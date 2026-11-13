//! The title bar's badge while agents' heavy commands wait their turn on this host, or while it
//! is overloaded, and its popover: what runs, then what waits in line, with Stop, Cancel and
//! Go first.

use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AsyncApp, Context, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::{GoFirstParams, StopBuildParams};
use sikemux_view::device::HostLoad;

use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_XS,
};
use crate::turn_line;
use crate::ui::{self, Button, Group, Variant};
use crate::workbench::Workbench;

const POPOVER_WIDTH: f32 = 360.;
fn overloaded() -> &'static str {
    match cfg!(target_os = "macos") {
        true => t!("Mac overloaded"),
        false => t!("Computer overloaded"),
    }
}
const MARK: f32 = 18.;

/// One heavy thing in the list, and what can be done about it.
struct Heavy {
    /// The agents it runs for, the one that started it first.
    agents: Vec<String>,
    what: String,
    detail: Vec<String>,
    before_commit: bool,
    /// Its place in its line while it waits, 1 for next.
    place: Option<u32>,
    stop: Stop,
}

#[derive(Clone)]
enum Stop {
    Simulator(String),
    Build(u64),
}

/// The popover's groups, in the order they show.
struct Groups {
    running: Vec<Heavy>,
    in_line: Vec<Heavy>,
}

impl Groups {
    fn labelled(self) -> [(&'static str, Vec<Heavy>); 2] {
        [(t!("Running"), self.running), (t!("In line"), self.in_line)]
    }
}

/// The badge, while something waits its turn on this host or its core says it is overloaded.
pub(crate) fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let load = bench.this_view()?.load.as_ref()?;
    let groups = groups(bench, load, cx);
    let waiting = groups.in_line.len();
    if waiting == 0 && !load.overloaded {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let tip = summary(&groups);
    let this = cx.entity().downgrade();
    Some(
        Popover::new("overload-chip")
            .anchor(Anchor::TopRight)
            .offset(SPACE_1H)
            .trigger_with(move |open, _, _| {
                let chip = div()
                    .id("overload-chip-button")
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .rounded(RADIUS_2)
                    .cursor_pointer()
                    .when(!open, |chip| chip.tooltip(ui::tooltip(tip.clone())));
                match waiting {
                    0 => chip
                        .bg(Hsla {
                            a: if open { 0.2 } else { 0.12 },
                            ..t.warn
                        })
                        .child(div().size(px(6.)).rounded_full().bg(t.warn))
                        .child(
                            ui::text(overloaded(), TEXT_XS, FontWeight::MEDIUM).text_color(t.warn),
                        ),
                    waiting => chip
                        .bg(match open {
                            true => t.surface_chosen,
                            false => t.surface_hover,
                        })
                        .child(ui::icon(Icon::Hourglass, 12.).text_color(t.warn))
                        .child(
                            ui::text(
                                t!("{count} in line", count = waiting),
                                TEXT_XS,
                                FontWeight::MEDIUM,
                            )
                            .font_features(theme::tabular())
                            .text_color(t.text_primary),
                        ),
                }
                .into_any_element()
            })
            .content(move |_, _, cx| {
                let Some(bench) = this.upgrade() else {
                    return div().into_any_element();
                };
                let bench = bench.read(cx);
                let Some(load) = bench.this_view().and_then(|view| view.load.as_ref()) else {
                    return div().into_any_element();
                };
                popover(bench, load, &this, cx)
            })
            .into_any_element(),
    )
}

fn popover(
    bench: &Workbench,
    load: &HostLoad,
    told: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let groups = groups(bench, load, cx);
    let said = summary(&groups);
    let empty = groups.running.is_empty() && groups.in_line.is_empty();
    let average = match load.load_average {
        Some(average) => t!(
            "load {average} · {cores} cores",
            average = average,
            cores = load.cores
        ),
        None => t!("{cores} cores", cores = load.cores),
    };
    let mut index = 0;
    let lists = groups
        .labelled()
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .enumerate()
        .map(|(at, (label, rows))| {
            let rows = rows.into_iter().map(|heavy| {
                index += 1;
                row(bench, index, heavy, told.clone(), cx)
            });
            let group = Group::new(label, "").children(rows);
            match at {
                0 => group.first(),
                _ => group,
            }
        })
        .collect::<Vec<_>>();
    div()
        .w(px(POPOVER_WIDTH))
        .p(SPACE_1H)
        .pb(SPACE_2)
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(RADIUS_4)
        .shadow(t.shadow_float())
        .child(
            div()
                .px(SPACE_2)
                .pt(SPACE_1H)
                .flex()
                .items_baseline()
                .child(ui::text(t!("This host"), TEXT_XS, FontWeight::MEDIUM))
                .child(div().flex_1())
                .child(
                    ui::text(average, TEXT_2XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(match load.overloaded {
                            true => t.warn,
                            false => t.text_tertiary,
                        }),
                ),
        )
        .child(
            div().px(SPACE_2).pt(SPACE_1).pb(SPACE_2).child(
                ui::text(said, TEXT_2XS, FontWeight::NORMAL)
                    .font_features(theme::tabular())
                    .text_color(t.text_secondary),
            ),
        )
        .child(div().h(px(1.)).mx(SPACE_1).mb(SPACE_2).bg(t.border))
        .when(empty, |list| {
            list.child(
                div().px(SPACE_2).py(SPACE_2).child(
                    ui::text(
                        t!("No agent's heavy command or simulator is running; other programs keep this host busy."),
                        TEXT_2XS,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                ),
            )
        })
        .children(lists)
        .into_any_element()
}

fn row(
    bench: &Workbench,
    index: usize,
    heavy: Heavy,
    told: WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let group = SharedString::from(format!("overload-row-{index}"));
    let mut agents = heavy.agents.iter().filter_map(|id| bench.agent_by_id(id));
    let first = agents.next();
    let mark = match &first {
        Some(agent) => agent.mark(MARK, cx),
        None => div().size(px(MARK)).into_any_element(),
    };
    let names: Vec<String> = first
        .into_iter()
        .chain(agents)
        .filter_map(|agent| agent.name.map(|name| format!("@{name}")))
        .collect();
    let asks = bench.asks.clone();
    let sims = bench.sims.clone();
    let stop = heavy.stop.clone();
    let what = heavy.what.clone();
    let waiting = heavy.place.is_some();
    let go_first = match (&heavy.stop, heavy.place) {
        (Stop::Build(id), Some(place)) if place > 1 => Some(
            go_first_button(
                ("overload-go-first", index),
                Target::here(&asks),
                *id,
                heavy.what.clone(),
                told.clone(),
            )
            .sized(22., 8., TEXT_2XS),
        ),
        _ => None,
    };
    let last = heavy.detail.len().saturating_sub(1);
    let detail = heavy
        .detail
        .into_iter()
        .enumerate()
        .flat_map(|(at, part)| {
            let shown = ui::text(part, TEXT_2XS, FontWeight::NORMAL)
                .font_features(theme::tabular())
                .whitespace_nowrap()
                .map(|shown| match at < last {
                    true => shown.flex_none(),
                    false => shown.min_w_0().overflow_hidden().text_ellipsis(),
                });
            let dot = (at > 0)
                .then(|| ui::text("·", TEXT_2XS, FontWeight::NORMAL).text_color(t.text_disabled));
            dot.into_iter().chain([shown])
        })
        .collect::<Vec<_>>();
    div()
        .id(("overload-row", index))
        .group(group.clone())
        .relative()
        .min_h(px(42.))
        .py(SPACE_1)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .hover(|row| row.bg(t.surface_hover))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .gap(SPACE_1)
                        .min_w_0()
                        .child(
                            ui::text(heavy.what, TEXT_XS, FontWeight::NORMAL)
                                .flex_none()
                                .whitespace_nowrap(),
                        )
                        .when(!names.is_empty(), |line| {
                            line.child(
                                ui::text(names.join(", "), TEXT_XS, FontWeight::NORMAL)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(t.text_tertiary),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .min_w_0()
                        .text_color(t.text_tertiary)
                        .children(detail)
                        .when(heavy.before_commit, |line| {
                            line.child(
                                ui::text(t!("before commit"), TEXT_2XS, FontWeight::MEDIUM)
                                    .flex_none()
                                    .px(px(5.))
                                    .rounded(RADIUS_2)
                                    .bg(Hsla {
                                        a: 0.14,
                                        ..t.accent
                                    })
                                    .text_color(t.accent),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex_none()
                .ml(SPACE_3)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .when_some(go_first, |buttons, go_first| {
                    buttons.child(
                        div()
                            .opacity(0.)
                            .group_hover(group, |shown| shown.opacity(1.))
                            .child(go_first),
                    )
                })
                .child(
                    Button::new(
                        ("overload-stop", index),
                        match waiting {
                            true => t!("Cancel"),
                            false => t!("Stop"),
                        },
                        Variant::Outline,
                    )
                    .sized(22., 8., TEXT_2XS)
                    .on_click(move |_, _: &mut Window, cx: &mut App| match &stop {
                        Stop::Simulator(agent) => sims.read(cx).engine().stop(agent),
                        Stop::Build(id) => {
                            let stopped = Target::here(&asks)
                                .request_typed::<call::StopBuild>(StopBuildParams { id: *id });
                            let what = what.clone();
                            tell(told.clone(), cx, async move {
                                match (stopped.await, waiting) {
                                    (Ok(()), true) => crate::toast::success(t!(
                                        "{what} left the line",
                                        what = what
                                    )),
                                    (Ok(()), false) => {
                                        crate::toast::success(t!("{what} stopped", what = what))
                                    }
                                    (Err(why), _) => crate::toast::error(t!(
                                        "{what} kept going: {why}",
                                        what = what,
                                        why = why
                                    )),
                                }
                            });
                        }
                    }),
                ),
        )
        .into_any_element()
}

/// "Go first": sends the heavy command `id` waiting in line to its front, and says so.
pub(crate) fn go_first_button(
    id: impl Into<gpui_kit::ElementId>,
    target: Target,
    build: u64,
    what: String,
    told: WeakEntity<Workbench>,
) -> Button {
    Button::new(id, t!("Go first"), Variant::Outline)
        .icon(Icon::ArrowUp)
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let moved = target.request_typed::<call::GoFirst>(GoFirstParams { id: build });
            let what = what.clone();
            tell(told.clone(), cx, async move {
                match moved.await {
                    Ok(()) => crate::toast::success(t!("{what} goes next", what = what)),
                    Err(why) => crate::toast::error(t!(
                        "{what} kept its place: {why}",
                        what = what,
                        why = why
                    )),
                }
            });
        })
}

fn tell(
    told: WeakEntity<Workbench>,
    cx: &mut App,
    note: impl std::future::Future<Output = gpui_kit::component::notification::Notification> + 'static,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let note = note.await;
        cx.update(|cx| crate::toast::show(&told, note, cx));
    })
    .detach();
}

/// What runs, held simulators first, then what waits, in line order, as this window's
/// simulator engine and the core's line know them.
fn groups(bench: &Workbench, load: &HostLoad, cx: &App) -> Groups {
    let now = ui::time::now_ms();
    let took = |since: u64| ui::time::elapsed(now.saturating_sub(since) / 1000);
    let (held, waiting) = bench.sims.read(cx).engine().heavy();
    let mut running: Vec<Heavy> = held
        .into_iter()
        .map(|held| Heavy {
            what: match held.booting {
                true => t!("booting {name}", name = held.name),
                false => held.name,
            },
            detail: vec![t!("simulator").to_owned(), took(held.since)],
            before_commit: false,
            place: None,
            stop: Stop::Simulator(held.agent_id.clone()),
            agents: vec![held.agent_id],
        })
        .collect();
    let mut in_line = Vec::new();
    for build in &load.builds {
        let agents: Vec<String> = (build.agent_id.iter())
            .chain(&build.joined)
            .cloned()
            .collect();
        let heavy = |detail, place| Heavy {
            agents: agents.clone(),
            what: build.command.clone(),
            detail,
            before_commit: build.before_commit,
            place,
            stop: Stop::Build(build.id),
        };
        match build.ahead {
            Some(ahead) => {
                let wait = (build.agent_id.as_deref())
                    .and_then(|id| bench.agent_by_id(id))
                    .and_then(|agent| agent.line)
                    .and_then(|line| line.expected_ms);
                let place = ahead + 1;
                let detail = [turn_line::place(place)]
                    .into_iter()
                    .chain(wait.map(turn_line::about))
                    .collect();
                in_line.push(heavy(detail, Some(place)));
            }
            None => {
                let mut detail = vec![took(build.since)];
                if !build.joined.is_empty() {
                    detail.push(match build.joined.len() {
                        1 => t!("one run for both").to_owned(),
                        _ => t!("one run for all {count}", count = build.joined.len() + 1),
                    });
                }
                running.push(heavy(detail, None));
            }
        }
    }
    in_line.sort_by_key(|heavy| heavy.place);
    in_line.extend(waiting.into_iter().map(|waiting| Heavy {
        what: t!("a simulator").to_owned(),
        detail: vec![turn_line::in_line(waiting.ahead as u32 + 1)],
        before_commit: false,
        place: Some(waiting.ahead as u32 + 1),
        stop: Stop::Simulator(waiting.agent_id.clone()),
        agents: vec![waiting.agent_id],
    }));
    Groups { running, in_line }
}

/// What runs and waits in a line, such as `2 running, 3 in line`.
fn summary(groups: &Groups) -> SharedString {
    let parts: Vec<String> = [
        (!groups.running.is_empty()).then(|| t!("{count} running", count = groups.running.len())),
        (!groups.in_line.is_empty()).then(|| t!("{count} in line", count = groups.in_line.len())),
    ]
    .into_iter()
    .flatten()
    .collect();
    match parts.is_empty() {
        true => t!("More runs than the cores take").into(),
        false => parts.join(", ").into(),
    }
}
