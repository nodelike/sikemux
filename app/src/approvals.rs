//! What agents wait on the person to approve, as the core's inbox holds it, answered from wherever
//! it shows: an action that leaves this machine, and a gate only the person passes. The core keeps
//! the list and clears it, so an answer from anywhere clears every place that shows it.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, App, IntoElement, ParentElement, SharedString, Styled, WeakEntity,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_core::server::board::BoardPassParams;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::ship::HoldParams;
use sikemux_view::inbox::{InboxItem, InboxKind, ShipHold};

use crate::core::Target;
use crate::theme::SPACE_1;
use crate::toast;
use crate::ui::{Button, Variant};
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

/// Asks the core to answer, saying so when it could not, and `done` when it could and there is
/// more to say than the item leaving.
fn answer<C: Call + 'static>(
    target: &Target,
    params: C::Params,
    done: Option<String>,
    bench: WeakEntity<Workbench>,
    cx: &mut App,
) where
    C::Params: Serialize,
    C::Answer: DeserializeOwned,
{
    let asked = target.request_typed::<C>(params);
    cx.spawn(async move |cx| {
        let note = match asked.await {
            Err(error) => Some(toast::error(error).autohide(false)),
            Ok(_) => done.map(toast::success),
        };
        if let Some(note) = note {
            let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
        }
    })
    .detach();
}

/// Refuses a held action or allows it once, without opening anything; where the core offers it,
/// always allows what the action lands in, in its project or every project of its folder, as the
/// main button. The buttons wrap rather than run past a narrow column.
pub fn ship_buttons(
    hold: &ShipHold,
    busy: bool,
    target: Target,
    bench: WeakEntity<Workbench>,
) -> AnyElement {
    let id = hold.id.as_str();
    let hold_folder = hold.let_go_in.clone();
    let once_leads = hold.let_go.is_none();
    let button = |label: &'static str, key: String, allow: bool| {
        let (hold, target, bench) = (id.to_owned(), target.clone(), bench.clone());
        let variant = if allow && once_leads {
            Variant::Primary
        } else {
            Variant::Outline
        };
        Button::new(SharedString::from(key), label, variant)
            .disabled(busy)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let params = HoldParams { hold: hold.clone() };
                match allow {
                    true => answer::<call::AllowShip>(&target, params, None, bench.clone(), cx),
                    false => answer::<call::RefuseShip>(&target, params, None, bench.clone(), cx),
                }
            })
    };
    div()
        .flex()
        .flex_wrap()
        .min_w_0()
        .gap(SPACE_1)
        .child(button("Refuse", format!("ship-refuse-{id}"), false))
        .child(button("Allow once", format!("ship-allow-{id}"), true))
        .when_some(hold.let_go.clone(), |row, covers| {
            let (hold, target, bench) = (id.to_owned(), target.clone(), bench.clone());
            let (label, tooltip) = match &hold_folder {
                Some(folder) => (
                    format!("Always allow {covers} in {folder}"),
                    format!("Deploys there stop asking you in every project in {folder}. Production still waits. Remove it under Environments."),
                ),
                None => (
                    format!("Always allow {covers}"),
                    "Deploys there stop asking you in this project. Production still waits. Change it under Environments.".to_owned(),
                ),
            };
            row.child(
                Button::new(
                    SharedString::from(format!("ship-let-go-{hold}")),
                    label,
                    Variant::Primary,
                )
                .tooltip(tooltip)
                .disabled(busy)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let params = HoldParams { hold: hold.clone() };
                    let done = format!("{covers} goes unasked now. Change it under Environments.");
                    answer::<call::LetShipGo>(&target, params, Some(done), bench.clone(), cx);
                }),
            )
        })
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
        || crate::ui::slice_state::gate_name(gate).to_owned(),
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
                answer::<call::BoardPass>(&target, params, None, bench.clone(), cx);
            })
            .into_any_element(),
    )
}
