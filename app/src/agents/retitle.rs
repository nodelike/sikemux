//! An agent's title edited in place, in its header or its tab rail row.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext, Context, Entity, Subscription, Window};
use sikemux_calls::call;
use sikemux_calls::workspace::AgentPatch;
use sikemux_calls::workspace::UpdateAgentParams;

use crate::core::Target;
use crate::workbench::Workbench;

/// An agent's title while it is edited in place, in a field holding it.
pub struct Retitling {
    pub agent: String,
    /// Edited in its tab rail row rather than its header.
    pub in_rail: bool,
    pub input: Entity<InputState>,
    title: String,
    /// The core that keeps its title.
    target: Target,
    _events: Subscription,
}

impl Workbench {
    /// Swaps an agent's title in its header, or its tab rail row when `in_rail`, for a field
    /// holding it, which Enter or clicking away saves and Escape leaves unchanged.
    pub fn retitle(
        &mut self,
        agent: &str,
        in_rail: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let here = self.this_view().and_then(|view| {
            view.workspace
                .agents
                .iter()
                .find(|kept| kept.id == agent)
                .map(|kept| (kept.title.clone(), self.target()))
        });
        let there = || {
            let row = self.worker_row(agent)?;
            let target = Target::new(&self.asks, self.asked_at(&row));
            Some((
                row.handle().map_or_else(String::new, |_| row.title.clone()),
                target,
            ))
        };
        let Some((title, target)) = here.or_else(there) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title.clone()));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        let events = cx.subscribe(&input, |bench, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                bench.finish_retitle(true, cx);
            }
        });
        self.overlays.retitling = Some(Retitling {
            agent: agent.to_owned(),
            in_rail,
            input,
            title,
            target,
            _events: events,
        });
        self.parts.redraw_stage(cx);
        self.parts.redraw(&self.parts.rail, cx);
    }

    pub fn finish_retitle(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(retitling) = self.overlays.retitling.take() else {
            return;
        };
        let title: String = retitling
            .input
            .read(cx)
            .value()
            .trim()
            .chars()
            .take(72)
            .collect();
        if save && !title.is_empty() && title != retitling.title {
            (retitling.target).call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                retitling.agent,
                AgentPatch {
                    title: Some(title),
                    renamed: Some(true),
                    ..AgentPatch::default()
                },
            ));
        }
        self.parts.redraw_stage(cx);
        self.parts.redraw(&self.parts.rail, cx);
    }
}
