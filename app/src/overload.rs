//! The title bar's badge while this Mac is overloaded, and its popover: each simulator and
//! build agents run or wait for, with Stop on each.

use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::StopBuildParams;
use sikemux_view::device::HostLoad;

use crate::core::Target;
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_XS,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

const POPOVER_WIDTH: f32 = 340.;
const OVERLOADED: &str = if cfg!(target_os = "macos") {
    "Mac overloaded"
} else {
    "Computer overloaded"
};

fn this_computer() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    }
}
const MARK: f32 = 18.;

/// One heavy thing in the list, and what stopping it does.
struct Heavy {
    agent_id: Option<String>,
    what: String,
    detail: String,
    waiting: bool,
    stop: Stop,
}

#[derive(Clone)]
enum Stop {
    Simulator(String),
    Build(u64),
}

/// The badge, while this Mac's core says it is overloaded.
pub(crate) fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let load = bench
        .this_view()?
        .load
        .as_ref()
        .filter(|load| load.overloaded)?;
    let t = Tokens::get(cx).clone();
    let tip = summary(load, &heavy(bench, load, cx));
    let this = cx.entity().downgrade();
    Some(
        Popover::new("overload-chip")
            .anchor(Anchor::TopRight)
            .offset(SPACE_1H)
            .trigger_with(move |open, _, _| {
                div()
                    .id("overload-chip-button")
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .rounded(RADIUS_2)
                    .cursor_pointer()
                    .bg(Hsla {
                        a: if open { 0.2 } else { 0.12 },
                        ..t.warn
                    })
                    .when(!open, |chip| chip.tooltip(ui::tooltip(tip.clone())))
                    .child(div().size(px(6.)).rounded_full().bg(t.warn))
                    .child(ui::text(OVERLOADED, TEXT_XS, FontWeight::MEDIUM).text_color(t.warn))
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
    let heavy = heavy(bench, load, cx);
    let average = load
        .load_average
        .map(|average| format!("load {average} · {} cores", load.cores))
        .unwrap_or_default();
    div()
        .w(px(POPOVER_WIDTH))
        .p(SPACE_1H)
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
                .child(ui::text(OVERLOADED, TEXT_XS, FontWeight::MEDIUM))
                .child(div().flex_1())
                .child(
                    ui::text(average, TEXT_2XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary),
                ),
        )
        .child(
            div()
                .px(SPACE_2)
                .pt(SPACE_1)
                .pb(SPACE_2)
                .child(
                    ui::text(summary(load, &heavy), TEXT_2XS, FontWeight::NORMAL)
                        .text_color(t.text_secondary),
                ),
        )
        .child(div().h(px(1.)).mx(SPACE_1).mb(SPACE_1).bg(t.border))
        .when(heavy.is_empty(), |list| {
            list.child(
                div().px(SPACE_2).py(SPACE_2).child(
                    ui::text(
                        format!("No agent's simulator or build is running; other programs keep {} busy.", this_computer()),
                        TEXT_2XS,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                ),
            )
        })
        .children(
            heavy
                .into_iter()
                .enumerate()
                .map(|(index, heavy)| row(bench, index, heavy, told.clone(), cx)),
        )
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
    let agent = heavy
        .agent_id
        .as_deref()
        .and_then(|id| bench.agent_by_id(id));
    let mark = match &agent {
        Some(agent) => agent.mark(MARK, cx),
        None => div().size(px(MARK)).into_any_element(),
    };
    let name = agent
        .and_then(|agent| agent.name)
        .map(|name| format!("@{name}"));
    let asks = bench.asks.clone();
    let sims = bench.sims.clone();
    let stop = heavy.stop.clone();
    div()
        .id(("overload-row", index))
        .h(px(42.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
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
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap(),
                        )
                        .children(name.map(|name| {
                            ui::text(name, TEXT_XS, FontWeight::NORMAL)
                                .flex_none()
                                .text_color(t.text_tertiary)
                        })),
                )
                .child(
                    ui::text(heavy.detail, TEXT_2XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap(),
                ),
        )
        .child(
            div().flex_none().ml(SPACE_3).child(
                Button::new(
                    ("overload-stop", index),
                    if heavy.waiting { "Cancel" } else { "Stop" },
                    Variant::Outline,
                )
                .sized(22., 8., TEXT_2XS)
                .on_click(move |_, _: &mut Window, cx: &mut App| match &stop {
                    Stop::Simulator(agent) => sims.read(cx).engine().stop(agent),
                    Stop::Build(id) => {
                        let stopped = Target::here(&asks)
                            .request_typed::<call::StopBuild>(StopBuildParams { id: *id });
                        let told = told.clone();
                        cx.spawn(async move |cx| {
                            let note = match stopped.await {
                                Ok(()) => crate::toast::success("Build stopped"),
                                Err(why) => {
                                    crate::toast::error(format!("The build kept running: {why}"))
                                }
                            };
                            cx.update(|cx| crate::toast::show(&told, note, cx));
                        })
                        .detach();
                    }
                }),
            ),
        )
        .into_any_element()
}

/// The simulators agents hold or wait for, as this window's engine knows them, then the
/// builds the core lists.
fn heavy(bench: &Workbench, load: &HostLoad, cx: &App) -> Vec<Heavy> {
    let now = ui::time::now_ms();
    let took = |since: u64| ui::time::elapsed(now.saturating_sub(since) / 1000);
    let (held, waiting) = bench.sims.read(cx).engine().heavy();
    let held = held.into_iter().map(|held| Heavy {
        what: match held.booting {
            true => format!("booting {}", held.name),
            false => held.name,
        },
        detail: format!("simulator · {}", took(held.since)),
        waiting: false,
        stop: Stop::Simulator(held.agent_id.clone()),
        agent_id: Some(held.agent_id),
    });
    let waiting = waiting.into_iter().map(|waiting| Heavy {
        what: "a simulator".to_owned(),
        detail: format!("waiting · {}", in_line(waiting.ahead as u32)),
        waiting: true,
        stop: Stop::Simulator(waiting.agent_id.clone()),
        agent_id: Some(waiting.agent_id),
    });
    let builds = load.builds.iter().map(|build| Heavy {
        agent_id: build.agent_id.clone(),
        what: build.command.clone(),
        detail: match build.ahead {
            Some(ahead) => format!("waiting · {}", in_line(ahead)),
            None => took(build.since),
        },
        waiting: build.ahead.is_some(),
        stop: Stop::Build(build.id),
    });
    held.chain(waiting).chain(builds).collect()
}

fn in_line(ahead: u32) -> String {
    match ahead {
        0 => "next in line".to_owned(),
        ahead => format!("{} ahead", ahead),
    }
}

/// What loads the Mac in a line, such as `@Yahiko booting iPhone Air, 3 builds`.
fn summary(load: &HostLoad, heavy: &[Heavy]) -> SharedString {
    let simulators = heavy
        .iter()
        .filter(|heavy| matches!(heavy.stop, Stop::Simulator(_)) && !heavy.waiting)
        .count();
    let running = load
        .builds
        .iter()
        .filter(|build| build.ahead.is_none())
        .count();
    let waiting = heavy.iter().filter(|heavy| heavy.waiting).count();
    let counted = |count: usize, one: &str, many: &str| match count {
        0 => None,
        1 => Some(format!("1 {one}")),
        count => Some(format!("{count} {many}")),
    };
    let parts: Vec<String> = [
        counted(simulators, "simulator", "simulators"),
        counted(running, "build", "builds"),
        counted(waiting, "waiting", "waiting"),
    ]
    .into_iter()
    .flatten()
    .collect();
    match parts.is_empty() {
        true => "More runs than the cores take".into(),
        false => parts.join(", ").into(),
    }
}
