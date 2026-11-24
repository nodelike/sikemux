//! An agent's ask to sign in to connectors again: a card with a row for each, signed in from
//! there, which folds to one line once every one is back.

use super::rows::{notice_box, Drawn};
use super::*;
use crate::connector_sign_in::{self, Stand};
use sikemux_view::chat::{SignInCard, SignInConnector, SignInState};

use crate::theme::{SPACE_2, SPACE_3, TEXT_XS_H};

const TILE: f32 = 28.;

pub(super) fn card(card: &SignInCard, looks: &Drawn, cx: &App) -> AnyElement {
    if card.done() {
        return done(card, looks, cx);
    }
    let t = Tokens::get(cx).clone();
    let bench = looks.bench.upgrade();
    let head = div()
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .min_w_0()
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .child(ui::icon(Icon::Lock, 12.).text_color(looks.quiet))
        .child(
            div()
                .flex_none()
                .text_color(looks.ink)
                .child(t!("@{name} needs you signed in", name = card.name.clone())),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.quiet)
                .child(card.why.clone()),
        );
    let rows = card.connectors.iter().enumerate().map(|(at, connector)| {
        let stand = bench
            .as_ref()
            .and_then(|bench| connector_sign_in::stand(bench.read(cx), &connector.id));
        row(card, connector, stand, at, looks, cx)
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .overflow_hidden()
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_raised)
        .child(head)
        .children(rows)
        .into_any_element()
}

fn row(
    card: &SignInCard,
    connector: &SignInConnector,
    stand: Option<Stand>,
    at: usize,
    looks: &Drawn,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let failed = match (&stand, &connector.state) {
        (Some(Stand::Refused(reason)), _) => Some(reason.clone()),
        (_, SignInState::Failed { reason }) => Some(reason.clone().into()),
        _ => None,
    };
    let line: SharedString = match (&connector.state, &stand, &failed) {
        (SignInState::SignedIn, ..) => t!("Signed in").into(),
        (_, _, Some(reason)) => reason.clone(),
        (_, Some(Stand::Busy), _) => t!("Signing in…").into(),
        _ => t!("Not signed in").into(),
    };
    let line_ink = match failed {
        Some(_) => t.danger,
        None => looks.quiet,
    };
    let action = match (&connector.state, &stand) {
        (SignInState::SignedIn, _) => ui::icon(Icon::Check, 14.)
            .text_color(t.live)
            .into_any_element(),
        (_, Some(Stand::Busy)) => crate::ui::sign_in::spinner("sign-in-card", cx),
        _ => {
            let label = match failed {
                Some(_) => t!("Try again"),
                None => t!("Sign in"),
            };
            let (bench, plugin) = (looks.bench.clone(), connector.id.clone());
            ui::Button::new(
                SharedString::from(format!("sign-in-card:{}:{}", card.id, connector.id)),
                label,
                ui::Variant::Primary,
            )
            .sized(24., 10., 11.5)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let _ = bench.update(cx, |bench, cx| {
                    connector_sign_in::open_in_browser(bench, &plugin, window, cx)
                });
            })
            .into_any_element()
        }
    };
    div()
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_3)
        .min_w_0()
        .border_t(crate::theme::border())
        .border_color(if at == 0 { t.border } else { t.line_soft })
        .child(
            div()
                .flex_none()
                .size(px(TILE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(crate::theme::radius_2())
                .bg(t.surface_hover)
                .border(crate::theme::border())
                .border_color(t.line_soft)
                .child(ui::picture(
                    connector_sign_in::mark(&connector.id),
                    16.,
                    t.text_secondary,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .child(
                    div()
                        .truncate()
                        .text_color(looks.ink)
                        .child(connector.name.clone()),
                )
                .child(div().truncate().text_color(line_ink).child(line)),
        )
        .child(div().flex_none().child(action))
        .into_any_element()
}

/// Every connector is back: one line naming them, and that the agent was told.
fn done(card: &SignInCard, looks: &Drawn, cx: &App) -> AnyElement {
    let names: Vec<&str> = (card.connectors.iter())
        .map(|connector| connector.name.as_str())
        .collect();
    notice_box(looks)
        .child(ui::icon(Icon::Check, 12.).text_color(Tokens::get(cx).live))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(looks.ink)
                .child(t!("Signed in to {names}", names = names.join(", "))),
        )
        .child(
            div()
                .flex_none()
                .child(t!("· @{name} carried on", name = card.name.clone())),
        )
        .into_any_element()
}
