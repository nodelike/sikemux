//! What agents wait on the person to approve, as the core's inbox holds it, answered from wherever
//! it shows: an action that leaves this machine, and a gate only the person passes. The core keeps
//! the list and clears it, so an answer from anywhere clears every place that shows it.

use gpui_kit::{
    div, AnyElement, App, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_core::server::board::PassParams as BoardPassParams;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::ship::HoldParams;
use sikemux_view::inbox::{InboxItem, InboxKind};

use crate::core::Target;
use crate::theme::{SPACE_1, TEXT_XS};
use crate::toast;
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

impl Workbench {
    /// What the inbox holds for `agent` to approve, in the core's order.
    pub fn approvals_of(&self, agent: &str) -> Vec<InboxItem> {
        let view = self.view_at(self.chat_target(agent).place());
        view.map_or(Vec::new(), |view| {
            (view.inbox.items.iter())
                .filter(|item| {
                    item.need.is_some()
                        && item.agent_id.as_deref() == Some(agent)
                        && matches!(item.kind, InboxKind::Ship | InboxKind::Gate)
                })
                .cloned()
                .collect()
        })
    }
}

/// Asks the core to answer, saying so when it could not.
fn answer<C: Call + 'static>(
    target: &Target,
    params: C::Params,
    bench: WeakEntity<Workbench>,
    cx: &mut App,
) where
    C::Params: Serialize,
    C::Answer: DeserializeOwned,
{
    let asked = target.request_typed::<C>(params);
    cx.spawn(async move |cx| {
        if let Err(error) = asked.await {
            let note = toast::error(error).autohide(false);
            let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
        }
    })
    .detach();
}

/// The exact command or call a held action runs, which allowing it lets through once: clipped
/// on one line, whole on hover.
pub fn held_action(id: &str, what: &str, cx: &App) -> AnyElement {
    let t = crate::theme::Tokens::get(cx);
    ui::mono(what.to_owned(), TEXT_XS, cx)
        .id(SharedString::from(format!("held-action-{id}")))
        .flex_shrink_1()
        .min_w_0()
        .truncate()
        .text_color(t.text_secondary)
        .tooltip(ui::tooltip(what.to_owned()))
        .into_any_element()
}

/// Allows a held action once, or refuses it, without opening anything.
pub fn ship_buttons(
    hold: &str,
    busy: bool,
    target: Target,
    bench: WeakEntity<Workbench>,
) -> AnyElement {
    let button = |label: &'static str, id: String, allow: bool| {
        let (hold, target, bench) = (hold.to_owned(), target.clone(), bench.clone());
        Button::new(SharedString::from(id), label, Variant::Outline)
            .disabled(busy)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let params = HoldParams { hold: hold.clone() };
                match allow {
                    true => answer::<call::AllowShip>(&target, params, bench.clone(), cx),
                    false => answer::<call::RefuseShip>(&target, params, bench.clone(), cx),
                }
            })
    };
    div()
        .flex()
        .flex_none()
        .gap(SPACE_1)
        .child(button("Allow once", format!("ship-allow-{hold}"), true))
        .child(button("Refuse", format!("ship-refuse-{hold}"), false))
        .into_any_element()
}

/// Passes the gate a Gate item waits at, as the person, in the gate's own word.
pub fn pass_button(
    item: &InboxItem,
    busy: bool,
    target: Target,
    bench: WeakEntity<Workbench>,
) -> Option<AnyElement> {
    let (gate, card) = (item.gate?, item.card.clone()?);
    let environment = item.environment.clone();
    let words = environment.as_ref().map_or_else(
        || crate::goals::gate_name(gate).to_owned(),
        |environment| format!("Pass {environment}"),
    );
    let id = SharedString::from(format!("pass-{}", item.id));
    Some(
        Button::new(id, words, Variant::Primary)
            .disabled(busy)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let params = BoardPassParams {
                    project: card.project.clone(),
                    card: card.id,
                    gate,
                    environment: environment.clone(),
                };
                answer::<call::BoardPass>(&target, params, bench.clone(), cx);
            })
            .into_any_element(),
    )
}
