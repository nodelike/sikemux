//! The chip beside a checkout's branch under the composer: its ten ports, and on a click its
//! name, which of its ports listen and the environment everything started in it gets.

use crate::ui::Clicks;
use gpui::Anchor;
use gpui_kit::base::Popover;
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window,
};
use sikemux_calls::ports::CheckoutAbout;

use crate::icons::Icon;
use crate::theme::{
    self, radius_2, radius_3, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS,
    TEXT_3XS_H, TEXT_XS,
};
use crate::ui::{self, IconButton};
use crate::workbench::Workbench;

const WIDTH: f32 = 360.;
/// How many environment values show before "Show all".
const FIRST_VALUES: usize = 5;
const ENV_ROW: f32 = 22.;
/// How tall the list of every value is, so the popover keeps its height however many there are.
const ENV_ROWS: f32 = 9.;
/// The open popover's height besides its list, with the gap over the chip.
const AROUND_LIST: f32 = 362.;
/// The title bar and the tabs under it, which the popover stays below.
const TOP_ROOM: f32 = 80.;

/// Whether the popover shows every environment value, and the filter over them.
pub struct EnvList {
    pub open: bool,
    pub filter: Entity<InputState>,
    /// Where the chip was last drawn, in window points from the top.
    pub chip_top: Rc<Cell<f32>>,
}

impl EnvList {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        Self {
            open: false,
            filter: cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter"))),
            chip_top: Rc::default(),
        }
    }
}

/// The ports chip for `agent`, while it works in a checkout of its own that has them.
pub fn chip(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let known = (bench.shown()?.workspace.agents.iter()).find(|known| known.id == agent)?;
    let first = known.worktree.as_ref()?.first_port?;
    let t = Tokens::get(cx).clone();
    let about = bench.feeds.in_front().checkout.clone();
    let shown = (bench.checkout_env.open, bench.checkout_env.filter.clone());
    let chip_top = bench.checkout_env.chip_top.clone();
    let list_rows = {
        let chip_top = chip_top.clone();
        move || {
            ((chip_top.get() - TOP_ROOM - AROUND_LIST) / ENV_ROW)
                .floor()
                .clamp(2., ENV_ROWS)
        }
    };
    let (bench, agent) = (cx.entity().downgrade(), agent.to_owned());
    let drawn_at = chip_top.clone();
    let face = move |open: bool| {
        div()
            .id("checkout-ports")
            .relative()
            .h(px(20.))
            .px(SPACE_1H)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .rounded(radius_2())
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .when(open, |chip| {
                chip.bg(t.surface_hover).border_color(t.border_strong)
            })
            .hover(|style| style.bg(t.surface_hover).border_color(t.border_strong))
            .child({
                let chip_top = drawn_at.clone();
                canvas(
                    move |bounds, _, _| chip_top.set(f32::from(bounds.origin.y)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
            .child(ui::icon(Icon::Plug, 11.).text_color(t.text_tertiary))
            .child(
                ui::text(range(first), TEXT_3XS_H, FontWeight::NORMAL)
                    .font_family(theme::MONO)
                    .font_features(theme::tabular())
                    .text_color(t.text_secondary),
            )
    };
    Some(
        Popover::new("checkout-ports-card")
            .anchor(Anchor::BottomLeft)
            .offset(SPACE_1H)
            .trigger_with(move |open, _, _| face(open).into_any_element())
            .content(move |_, _, cx| {
                let room = chip_top.get() - TOP_ROOM;
                let shown = (shown.0, shown.1.clone(), list_rows(), room);
                card(about.clone(), &shown, &bench, &agent, cx)
            })
            .into_any_element(),
    )
}

/// `21040–49` for the ten ports from 21040.
fn range(first: u16) -> String {
    format!("{first}–{:02}", (first + 9) % 100)
}

/// The popover: the checkout's name and folder, its ports and what listens on them, and the
/// environment it gives.
fn card(
    about: Option<CheckoutAbout>,
    shown: &(bool, Entity<InputState>, f32, f32),
    bench: &WeakEntity<Workbench>,
    agent: &str,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let frame = div()
        .id("checkout-card")
        .max_h(px(shown.3.max(ENV_ROWS * ENV_ROW)))
        .overflow_y_scroll()
        .w(px(WIDTH))
        .px(SPACE_3)
        .pt(SPACE_2H)
        .pb(px(11.))
        .rounded(radius_3())
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .shadow(t.shadow_float())
        .text_color(t.text_primary);
    let Some(about) = about else {
        return frame
            .child(
                ui::text(t!("Reading this checkout…"), TEXT_XS, FontWeight::NORMAL)
                    .text_color(t.text_tertiary),
            )
            .into_any_element();
    };
    let heading = |words: &str, cx: &App| {
        crate::ui::list_picker::heading(words, cx)
            .px_0()
            .pt(SPACE_1)
    };
    let mono = |value: String, ink| {
        ui::text(value, TEXT_XS, FontWeight::NORMAL)
            .font_family(theme::MONO)
            .font_features(theme::tabular())
            .text_color(ink)
    };
    let name = about.name.clone();
    let checkout = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(SPACE_2)
        .child(
            mono(about.name.clone(), t.text_primary)
                .min_w_0()
                .truncate(),
        )
        .child(
            IconButton::new("checkout-copy-name", Icon::Copy, t!("Copy the name")).on_click(
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(name.clone())),
            ),
        );
    let home = std::env::var("HOME").ok();
    let path = ui::text(
        crate::ui::path::tilde(&about.path.to_string_lossy(), home.as_deref()),
        TEXT_2XS,
        FontWeight::NORMAL,
    )
    .text_color(t.text_tertiary)
    .truncate();
    let rows = about.ports.iter().enumerate().map(|(at, port)| {
        let listening = port.process.is_some();
        let url = format!("http://localhost:{}", port.port);
        let (bench, agent) = (bench.clone(), agent.to_owned());
        div()
            .id(("checkout-port", at))
            .h(px(26.))
            .px(SPACE_1H)
            .mx(px(-6.))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .rounded(radius_2())
            .when(listening, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(t.surface_hover))
                    .clicks(move |_, _, cx| {
                        bench
                            .update(cx, |bench, cx| bench.open_on_desk(&agent, &url, cx))
                            .ok();
                    })
            })
            .child(
                div()
                    .size(px(7.))
                    .flex_none()
                    .rounded(crate::theme::radius_round())
                    .bg(if listening { t.live } else { t.text_disabled }),
            )
            .child(
                ui::text(port.name.clone(), TEXT_XS, FontWeight::NORMAL)
                    .w(px(52.))
                    .truncate(),
            )
            .child(mono(port.port.to_string(), t.text_primary).w(px(48.)))
            .child(
                ui::text(
                    port.process
                        .clone()
                        .unwrap_or_else(|| t!("not listening").to_owned()),
                    TEXT_XS,
                    FontWeight::NORMAL,
                )
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_tertiary),
            )
            .when(listening, |row| {
                row.child(ui::icon(Icon::ExternalLink, 12.).text_color(t.text_tertiary))
            })
    });
    let kept = (!about.main_ports.is_empty()).then(|| {
        about
            .main_ports
            .iter()
            .map(|(name, port)| format!("{name} {port}"))
            .collect::<Vec<_>>()
            .join(", ")
    });
    let free = about.unnamed().map(|(from, to)| {
        t!(
            "{from}–{to} are free for this checkout.",
            from = from,
            to = to
        )
    });
    let note = match (free, kept) {
        (Some(free), Some(kept)) => Some(format!(
            "{free} {}",
            t!("The project folder keeps {ports}.", ports = kept)
        )),
        (Some(free), None) => Some(free),
        (None, Some(kept)) => Some(t!("The project folder keeps {ports}.", ports = kept)),
        (None, None) => None,
    };
    let env = env_group(&about.env, shown, bench, cx);
    frame
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(heading(t!("Checkout"), cx))
                .child(checkout)
                .child(path)
                .child(heading(t!("Ports"), cx))
                .children(rows)
                .when_some(note, |card, note| {
                    card.child(
                        ui::text(note, TEXT_2XS, FontWeight::NORMAL)
                            .w_full()
                            .whitespace_normal()
                            .text_color(t.text_tertiary),
                    )
                })
                .child(heading(t!("Env"), cx))
                .child(env)
                .child(
                    ui::text(
                        t!("Worked out from sikemux.json. Every agent and process in this checkout gets them."),
                        TEXT_2XS,
                        FontWeight::NORMAL,
                    )
                    .pt(SPACE_1)
                    .w_full()
                    .whitespace_normal()
                    .text_color(t.text_tertiary),
                )
        )
        .into_any_element()
}

/// The ENV group: a few values, then all of them in a list of fixed height under a filter.
/// Each value is one line; the full value and a copy button show on hover.
fn env_group(
    env: &[(String, String)],
    (open, filter, list_rows, _): &(bool, Entity<InputState>, f32, f32),
    bench: &WeakEntity<Workbench>,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (open, filter) = (*open, filter.clone());
    let row = |at: usize, key: &str, value: &str| {
        let line = format!("{key}={value}");
        let copied = line.clone();
        let group = SharedString::from(format!("checkout-env-{at}"));
        div()
            .id(("checkout-env", at))
            .group(group.clone())
            .h(px(ENV_ROW))
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_1)
            .tooltip(ui::tooltip(line))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .text_size(theme::ui_text(TEXT_XS))
                    .font_family(theme::MONO)
                    .font_features(theme::tabular())
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(170.))
                            .truncate()
                            .text_color(t.text_primary)
                            .child(key.to_owned()),
                    )
                    .child(div().flex_none().text_color(t.text_tertiary).child("="))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_secondary)
                            .child(value.to_owned()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .opacity(0.)
                    .group_hover(group, |copy| copy.opacity(1.))
                    .child(
                        IconButton::new(("checkout-env-copy", at), Icon::Copy, t!("Copy"))
                            .small()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                            }),
                    ),
            )
    };
    if !open {
        let more = (env.len() > FIRST_VALUES).then(|| {
            let bench = bench.clone();
            ui::sign_in::link(
                "checkout-env-all",
                t!("Show all {count}", count = env.len()),
                &*cx,
            )
            .clicks(move |_, window, cx| {
                let opened = bench.update(cx, |bench, cx| {
                    bench.checkout_env.open = true;
                    cx.notify();
                    bench.checkout_env.filter.clone()
                });
                if let Ok(filter) = opened {
                    filter.update(cx, |filter, cx| filter.focus(window, cx));
                }
            })
        });
        return div()
            .flex()
            .flex_col()
            .children(
                env.iter()
                    .take(FIRST_VALUES)
                    .enumerate()
                    .map(|(at, (key, value))| row(at, key, value)),
            )
            .children(more.map(|more| div().pt(SPACE_1).child(more)))
            .into_any_element();
    }
    let fewer = {
        let bench = bench.clone();
        ui::sign_in::link("checkout-env-fewer", t!("Show fewer"), &*cx)
            .flex_none()
            .clicks(move |_, _, cx| {
                bench
                    .update(cx, |bench, cx| {
                        bench.checkout_env.open = false;
                        cx.notify();
                    })
                    .ok();
            })
    };
    let query = filter.read(cx).value().trim().to_lowercase();
    let found: Vec<_> = (env.iter().enumerate())
        .filter(|(_, (key, value))| {
            query.is_empty()
                || key.to_lowercase().contains(&query)
                || value.to_lowercase().contains(&query)
        })
        .collect();
    let empty = found.is_empty().then(|| {
        ui::text(t!("Nothing matches"), TEXT_XS, FontWeight::NORMAL).text_color(t.text_tertiary)
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(ui::Field::new(&filter).small().mono().icon(Icon::Search)),
                )
                .child(fewer),
        )
        .child(
            div()
                .id("checkout-env-list")
                .h(px(list_rows * ENV_ROW))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(
                    found
                        .into_iter()
                        .map(|(at, (key, value))| row(at, key, value)),
                )
                .children(empty),
        )
        .into_any_element()
}
