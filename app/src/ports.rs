//! The ports the project's terminals, tasks and agents listen on, as the Run menu lists them.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{
    div, px, App, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, WeakEntity,
};
use sikemux_core::server::{PortOwner, ProjectPort};
use sikemux_wire::protocol::PaneKind;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H};
use crate::ui;
use crate::workbench::Workbench;

/// One listening port, with who listens on it as the person reads it.
pub struct PortRow {
    pub port: u16,
    process: String,
    owner: Owner,
    /// A task's, agent's or terminal's name.
    label: String,
    /// The tab that shows who listens, by its kind and target.
    reveal: Option<(&'static str, String)>,
    /// The task run listening, which stopping it ends.
    pub execution: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Terminal,
    Task,
    Agent,
}

impl PortRow {
    pub fn new(port: &ProjectPort, bench: &Workbench) -> Self {
        let agent_named =
            |id: &str| (bench.view.as_ref()?.workspace.agents.iter()).find(|agent| agent.id == id);
        let (kind, label, reveal) = match &port.listening.owner {
            PortOwner::Agent {
                agent_id, label, ..
            } => (
                Owner::Agent,
                label.clone(),
                agent_named(agent_id).map(|agent| ("agent", agent.id.clone())),
            ),
            PortOwner::Task {
                agent_id,
                label,
                session_id,
                ..
            } => match agent_id.as_deref().and_then(agent_named) {
                Some(agent) => (
                    Owner::Agent,
                    agent.title.clone(),
                    Some(("agent", agent.id.clone())),
                ),
                None => (
                    Owner::Task,
                    label.clone(),
                    Some(("terminal", session_id.to_string())),
                ),
            },
            PortOwner::Terminal { session_id, .. } => {
                let session = session_id.to_string();
                let tab = bench.strip().and_then(|strip| {
                    bench.tabs(&strip).iter().find(|tab| {
                        crate::tabs::first_view(&tab.pane) == Some((PaneKind::Terminal, &session))
                    })
                });
                (
                    Owner::Terminal,
                    tab.map_or_else(
                        || "terminal".to_owned(),
                        |tab| crate::tabs::label(bench, tab).to_string(),
                    ),
                    tab.map(|_| ("terminal", session.clone())),
                )
            }
        };
        let execution = match &port.listening.owner {
            PortOwner::Task { execution_id, .. } => Some(execution_id.clone()),
            _ => None,
        };
        Self {
            port: port.listening.port,
            process: port.listening.process.clone(),
            owner: kind,
            label,
            reveal,
            execution,
        }
    }

    /// The row the Run menu's "Running now" shows: it reveals who listens, and stops a task.
    pub fn running_item(&self, bench: &WeakEntity<Workbench>) -> PopupMenuItem {
        let port = self.port;
        let process = if self.process.is_empty() {
            "process".to_owned()
        } else {
            self.process.clone()
        };
        let owner = if self.owner == Owner::Agent {
            self.label.clone()
        } else {
            "You".to_owned()
        };
        let stop = self
            .execution
            .clone()
            .map(|execution| (bench.clone(), execution));
        PopupMenuItem::element(move |_, cx| {
            let t = Tokens::get(cx).clone();
            let stop = stop.clone().map(|(bench, execution)| {
                div()
                    .id(("stop-port", port as usize))
                    .p(px(3.))
                    .rounded(crate::theme::RADIUS_2)
                    .hover(move |style| style.bg(t.surface_hover))
                    .tooltip(ui::tooltip(format!("Stop what listens on {port}")))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = bench.update(cx, |bench, _| bench.stop_run(&execution));
                    })
                    .child(ui::icon(Icon::Stop, 9.).text_color(t.danger))
            });
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(div().size(px(7.)).rounded_full().bg(t.live))
                .child(div().flex_1().min_w_0().truncate().child(process.clone()))
                .child(
                    div()
                        .font_family(crate::theme::MONO)
                        .text_color(t.text_tertiary)
                        .child(format!(":{port}")),
                )
                .child(div().text_color(t.text_tertiary).child(owner.clone()))
                .children(stop)
        })
        .on_click(self.reveal_click(bench))
    }

    fn reveal_click(
        &self,
        bench: &WeakEntity<Workbench>,
    ) -> impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut App) + Clone + 'static {
        let (bench, reveal) = (bench.clone(), self.reveal.clone());
        move |_, _, cx| {
            if let Some((kind, target)) = &reveal {
                let _ = bench.update(cx, |bench, _| bench.show(kind, target, None));
            }
        }
    }
}
