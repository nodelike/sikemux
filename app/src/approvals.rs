//! What agents wait on the person to approve, as the core's inbox holds it, answered from wherever
//! it shows: an action that leaves this machine, and a gate only the person passes. The core keeps
//! the list and clears it, so an answer from anywhere clears every place that shows it.

use gpui_kit::{AnyElement, App, IntoElement, SharedString, WeakEntity};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_core::server::board::BoardPassParams;
use sikemux_core::server::call::{self, Call};
use sikemux_view::inbox::{InboxItem, InboxKind};

use crate::core::Target;
use crate::toast;
use crate::ui::{Button, Variant};
use crate::workbench::Workbench;

impl Workbench {
    /// The gates `agent` waits at for the person to pass, in the core's order. Its held actions
    /// are calls, which its chat draws as their cards.
    pub fn approvals_of(&self, agent: &str) -> Vec<InboxItem> {
        let view = self.view_at(self.chat_target(agent).place());
        view.map_or(Vec::new(), |view| {
            (view.inbox.items.iter())
                .filter(|item| {
                    item.need.is_some()
                        && item.agent_id.as_deref() == Some(agent)
                        && item.kind == InboxKind::Gate
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
