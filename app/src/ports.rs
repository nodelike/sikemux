//! The ports the project's terminals, tasks and agents listen on, as the Run menu lists them.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{
    div, px, App, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, WeakEntity,
};
use serde_json::Value;
use sikemux_wire::protocol::PaneKind;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H};
use crate::ui;
use crate::workbench::Workbench;

/// One listening port, with who listens on it as the person reads it.
pub struct PortRow {
    pub port: u64,
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
    pub fn new(port: &Value, bench: &Workbench) -> Option<Self> {
        let owner = &port["owner"];
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let session = owner["sessionId"].as_u64().map(|id| id.to_string());
        let agent = owner["agentId"].as_str().and_then(|id| {
            bench
                .view
                .as_ref()?
                .workspace
                .agents
                .iter()
                .find(|agent| agent.id == id)
        });
        let (kind, label, reveal) = match (owner["kind"].as_str()?, agent) {
            ("agent", agent) => (
                Owner::Agent,
                text(&owner["label"]),
                agent.map(|agent| ("agent", agent.id.clone())),
            ),
            ("task", Some(agent)) => (
                Owner::Agent,
                agent.title.clone(),
                Some(("agent", agent.id.clone())),
            ),
            ("task", None) => (
                Owner::Task,
                text(&owner["label"]),
                session.map(|session| ("terminal", session)),
            ),
            _ => {
                let tab = session.as_ref().and_then(|session| {
                    let strip = bench.strip()?;
                    bench.tabs(&strip).iter().find(|tab| {
                        crate::tabs::first_view(&tab.pane) == Some((PaneKind::Terminal, session))
                    })
                });
                (
                    Owner::Terminal,
                    tab.map_or_else(
                        || "terminal".to_owned(),
                        |tab| crate::tabs::label(bench, tab).to_string(),
                    ),
                    tab.and(session).map(|session| ("terminal", session)),
                )
            }
        };
        Some(Self {
            port: port["port"].as_u64()?,
            process: text(&port["process"]),
            owner: kind,
            label,
            reveal,
            execution: owner["executionId"].as_str().map(str::to_owned),
        })
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
