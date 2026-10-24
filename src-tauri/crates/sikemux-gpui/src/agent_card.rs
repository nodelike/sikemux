//! What an agent row stands for, in a card beside it once the pointer rests on the row:
//! its project, its worktree's branch and folder, and that branch's pull requests.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, point, px, Anchor, AnchoredPositionMode, AnyElement, Context,
    FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Task,
};
use serde_json::json;
use sikemux_wire::protocol::WorkspaceAgent;

use crate::icons::Icon;
use crate::theme::Tokens;
use crate::ui;
use crate::workbench::{AgentRow, Workbench};

const OPEN_AFTER: Duration = Duration::from_millis(450);
const CLOSE_AFTER: Duration = Duration::from_millis(140);

#[derive(Default)]
pub struct Card {
    /// The agent whose card shows.
    agent: Option<String>,
    timer: Option<Task<()>>,
    pulls: Vec<CardPull>,
}

struct CardPull {
    number: u64,
    title: SharedString,
    url: String,
}

impl Workbench {
    /// The pointer came onto an agent's row, or left it: its card opens after a rest, at
    /// once when another card already shows, and closes a moment after the pointer leaves.
    pub fn rest_on_agent(&mut self, agent: &str, inside: bool, cx: &mut Context<Self>) {
        if !inside {
            return self.leave_card(cx);
        }
        if self.agent_card.agent.as_deref() == Some(agent) {
            self.agent_card.timer = None;
            return;
        }
        let wait = if self.agent_card.agent.is_some() {
            Duration::ZERO
        } else {
            OPEN_AFTER
        };
        let agent = agent.to_owned();
        self.agent_card.timer = Some(cx.spawn(async move |bench, cx| {
            cx.background_executor().timer(wait).await;
            bench
                .update(cx, |bench, cx| bench.open_card(agent, cx))
                .ok();
        }));
    }

    /// The pointer is on the card itself, which keeps it open.
    fn stay_on_card(&mut self) {
        self.agent_card.timer = None;
    }

    fn leave_card(&mut self, cx: &mut Context<Self>) {
        self.agent_card.timer = Some(cx.spawn(async move |bench, cx| {
            cx.background_executor().timer(CLOSE_AFTER).await;
            bench.update(cx, |bench, cx| bench.close_card(cx)).ok();
        }));
    }

    pub fn close_card(&mut self, cx: &mut Context<Self>) {
        self.agent_card = Card::default();
        cx.notify();
    }

    fn open_card(&mut self, agent: String, cx: &mut Context<Self>) {
        self.agent_card = Card {
            agent: Some(agent.clone()),
            ..Card::default()
        };
        cx.notify();
        let Some((folder, branch)) = known(self, &agent).and_then(|known| {
            let worktree = known.worktree.as_ref()?;
            let folder = worktree["path"].as_str()?.to_owned();
            Some((folder, worktree["branch"].as_str()?.to_owned()))
        }) else {
            return;
        };
        let asked = crate::core::request(
            &self.asks,
            "codeHostPulls",
            json!({ "project": folder, "state": "all" }),
        );
        cx.spawn(async move |bench, cx| {
            let Ok(list) = asked.await else {
                return;
            };
            let pulls = list["pulls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|listed| listed["ownBranch"] == true)
                .map(|listed| &listed["pull"])
                .filter(|pull| pull["head"] == branch.as_str())
                .filter_map(|pull| {
                    Some(CardPull {
                        number: pull["number"].as_u64()?,
                        title: pull["title"].as_str()?.to_owned().into(),
                        url: pull["url"].as_str()?.to_owned(),
                    })
                })
                .collect();
            bench
                .update(cx, |bench, cx| {
                    if bench.agent_card.agent.as_deref() == Some(agent.as_str()) {
                        bench.agent_card.pulls = pulls;
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }
}

fn known<'a>(bench: &'a Workbench, id: &str) -> Option<&'a WorkspaceAgent> {
    bench
        .view
        .as_ref()?
        .workspace
        .agents
        .iter()
        .find(|agent| agent.id == id)
}

/// The card for `agent` when it is the one showing, placed beside its row, toward the
/// middle of the window.
pub fn view(
    bench: &Workbench,
    agent: &AgentRow,
    project: Option<&str>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if bench.agent_card.agent.as_deref() != Some(agent.id.as_str()) {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let worktree = known(bench, &agent.id).and_then(|known| known.worktree.clone());
    let branch = worktree
        .as_ref()
        .and_then(|worktree| worktree["branch"].as_str())
        .map(str::to_owned);
    let folder = worktree
        .as_ref()
        .and_then(|worktree| worktree["path"].as_str())
        .map(|path| crate::chat::tilde(std::path::Path::new(path)));
    let who: Vec<String> = agent
        .name
        .iter()
        .map(|name| format!("@{name}"))
        .chain(Some(
            bench
                .launchers()
                .iter()
                .find(|launcher| launcher.provider == agent.provider)
                .map_or_else(
                    || agent.provider.clone(),
                    |launcher| launcher.label.to_string(),
                ),
        ))
        .collect();
    let fact = |icon: Option<Icon>, body: AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(icon.map(|icon| ui::icon(icon, 13.).text_color(t.text_tertiary))),
            )
            .child(div().min_w_0().truncate().child(body))
    };
    let mut facts: Vec<AnyElement> = Vec::new();
    if let Some(project) = project {
        facts.push(
            fact(Some(Icon::Folder), project.to_owned().into_any_element()).into_any_element(),
        );
    }
    if let Some(branch) = branch {
        let reviewed = agent.id.clone();
        facts.push(
            fact(
                Some(Icon::Git),
                ui::mono(branch, 11., cx)
                    .id("card-branch")
                    .text_color(t.text_primary)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .tooltip(ui::tooltip("Review its work"))
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        bench.close_card(cx);
                        crate::agent_review::open(bench, &reviewed, cx);
                    }))
                    .into_any_element(),
            )
            .into_any_element(),
        );
    }
    if let Some(folder) = folder {
        facts.push(
            fact(
                None,
                ui::mono(folder, 11., cx)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            )
            .into_any_element(),
        );
    }
    if !bench.agent_card.pulls.is_empty() {
        let pulls = div()
            .flex()
            .gap(px(8.))
            .children(bench.agent_card.pulls.iter().map(|pull| {
                let (agent, url) = (agent.id.clone(), pull.url.clone());
                ui::mono(format!("#{}", pull.number), 11., cx)
                    .id(SharedString::from(format!("card-pull-{}", pull.number)))
                    .text_color(t.text_primary)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .tooltip(ui::tooltip(pull.title.clone()))
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        bench.open_on_desk(&agent, &url);
                        bench.close_card(cx);
                    }))
            }));
        facts.push(fact(Some(Icon::PullRequest), pulls.into_any_element()).into_any_element());
    }
    let id = agent.id.clone();
    let card = div()
        .id("agent-card")
        .occlude()
        .w(px(300.))
        .pt(px(10.))
        .px(px(12.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(crate::theme::RADIUS_3)
        .shadow(t.shadow_float())
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_secondary)
        .on_hover(cx.listener(move |bench, inside: &bool, _, cx| {
            if *inside {
                bench.stay_on_card();
            } else {
                bench.rest_on_agent(&id, false, cx);
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(ui::agent_mark(agent, 22., cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::theme::ui_text(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .child(agent.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(t.text_tertiary)
                                .child(who.join(" · ")),
                        ),
                ),
        )
        .when(!facts.is_empty(), |card| {
            card.child(
                div()
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(t.border)
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(facts),
            )
        });
    Some(
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .position(point(px(-8.), px(-6.)))
                .anchor(Anchor::TopRight)
                .snap_to_window()
                .child(card),
        )
        .with_priority(1)
        .into_any_element(),
    )
}
