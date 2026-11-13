//! The chip beside a checkout's branch under the composer: its ten ports, and on a click its
//! name, which of its ports listen and the environment everything started in it gets.

use gpui::Anchor;
use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, WeakEntity,
};
use sikemux_core::server::ports::checkout::CheckoutAbout;

use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_3, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS,
    TEXT_3XS_H, TEXT_XS,
};
use crate::ui::{self, IconButton};
use crate::workbench::Workbench;

const WIDTH: f32 = 360.;

/// The ports chip for `agent`, while it works in a checkout of its own that has them.
pub fn chip(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let known = (bench.shown()?.workspace.agents.iter()).find(|known| known.id == agent)?;
    let first = known.worktree.as_ref()?.first_port?;
    let t = Tokens::get(cx).clone();
    let about = bench.feeds.in_front().checkout.clone();
    let (bench, agent) = (cx.entity().downgrade(), agent.to_owned());
    let face = move |open: bool| {
        div()
            .id("checkout-ports")
            .h(px(20.))
            .px(SPACE_1H)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .when(open, |chip| {
                chip.bg(t.surface_hover).border_color(t.border_strong)
            })
            .hover(|style| style.bg(t.surface_hover).border_color(t.border_strong))
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
            .content(move |_, _, cx| card(about.clone(), &bench, &agent, cx))
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
    bench: &WeakEntity<Workbench>,
    agent: &str,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let frame = div()
        .w(px(WIDTH))
        .px(SPACE_3)
        .pt(SPACE_2H)
        .pb(px(11.))
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .rounded(RADIUS_3)
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
            .rounded(RADIUS_2)
            .when(listening, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(t.surface_hover))
                    .on_click(move |_, _, cx| {
                        bench
                            .update(cx, |bench, cx| bench.open_on_desk(&agent, &url, cx))
                            .ok();
                    })
            })
            .child(
                div()
                    .size(px(7.))
                    .flex_none()
                    .rounded_full()
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
                row.child(ui::icon(Icon::Link, 12.).text_color(t.text_tertiary))
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
        .to_owned()
    });
    let note = match (free, kept) {
        (Some(free), Some(kept)) => Some(format!(
            "{free} {}",
            t!("The project folder keeps {ports}.", ports = kept)
        )),
        (Some(free), None) => Some(free),
        (None, Some(kept)) => {
            Some(t!("The project folder keeps {ports}.", ports = kept).to_owned())
        }
        (None, None) => None,
    };
    let env = about.env.iter().map(|(key, value)| {
        div()
            .flex()
            .flex_wrap()
            .child(mono(key.clone(), t.text_primary))
            .child(mono("=".to_owned(), t.text_tertiary))
            .child(mono(value.clone(), t.text_secondary))
    });
    frame
        .child(heading(t!("Checkout"), cx))
        .child(checkout)
        .child(path)
        .child(heading(t!("Ports"), cx))
        .children(rows)
        .when_some(note, |card, note| {
            card.child(ui::text(note, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary))
        })
        .child(heading(t!("Env"), cx))
        .child(div().flex().flex_col().gap(px(3.)).children(env))
        .child(
            ui::text(
                t!("Worked out from sikemux.json. Every agent and process in this checkout gets them."),
                TEXT_2XS,
                FontWeight::NORMAL,
            )
            .pt(SPACE_1)
            .text_color(t.text_tertiary),
        )
        .into_any_element()
}
