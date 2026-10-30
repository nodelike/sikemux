//! What an agent row stands for, in a card beside it once the pointer rests on the row:
//! its project, its worktree's branch and folder, and that branch's pull requests.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, canvas, deferred, div, point, px, Anchor, AnchoredPositionMode, AnyElement, Context,
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    SharedString, StatefulInteractiveElement, Styled, Task,
};
use serde_json::json;
use sikemux_plugin_api::codehost::PullList;
use sikemux_wire::protocol::WorkspaceAgent;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::workbench::{AgentRow, Workbench};

const OPEN_AFTER: Duration = Duration::from_millis(450);
const CLOSE_AFTER: Duration = Duration::from_millis(140);

#[derive(Default)]
pub struct Card {
    /// The agent whose card shows.
    agent: Option<String>,
    /// Where its row sat when the card first drew; the card closes once the row moves.
    row_at: Option<Point<Pixels>>,
    /// The list scrolled, so rows sliding under a still pointer open no card until it moves.
    held: bool,
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
        if self.agent_card.held {
            return;
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
        let held = self.agent_card.held;
        self.agent_card = Card {
            held,
            ..Card::default()
        };
        cx.notify();
    }

    /// The agents list scrolled: any card closes, and none opens until the pointer moves.
    pub fn hold_card(&mut self, cx: &mut Context<Self>) {
        self.agent_card.held = true;
        if self.agent_card.agent.is_some() || self.agent_card.timer.is_some() {
            self.close_card(cx);
        }
    }

    /// The pointer moved on an agent's row after a scroll held cards shut: its card opens
    /// once the pointer rests there.
    pub fn move_on_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.agent_card.held {
            self.agent_card.held = false;
            self.rest_on_agent(agent, true, cx);
        }
    }

    fn open_card(&mut self, agent: String, cx: &mut Context<Self>) {
        self.agent_card = Card {
            agent: Some(agent.clone()),
            ..Card::default()
        };
        cx.notify();
        let Some((folder, branch)) = known(self, &agent).and_then(|known| {
            let worktree = known.worktree.as_ref()?;
            let folder = worktree.path.to_str()?.to_owned();
            Some((folder, worktree.branch.clone()))
        }) else {
            return;
        };
        let asked = crate::core::request(
            &self.asks,
            "codeHostPulls",
            json!({ "project": folder, "state": "all" }),
        );
        cx.spawn(async move |bench, cx| {
            let Some(list) = asked
                .await
                .ok()
                .and_then(|answer| crate::codehost::answer::<PullList>(&answer).ok())
            else {
                return;
            };
            let pulls = (list.pulls.into_iter())
                .filter(|listed| listed.own_branch && listed.pull.head.as_ref() == Some(&branch))
                .map(|listed| CardPull {
                    number: listed.pull.number,
                    title: listed.pull.title.into(),
                    url: listed.pull.url,
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

/// Laid over the row whose card shows: closes the card once the row moves on screen, as it
/// does when its list scrolls, its rail slides or the window resizes, so the card never
/// rides along with it.
pub fn follow_row(bench: Entity<Workbench>) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            let origin = bounds.origin;
            let at = bench.read(cx).agent_card.row_at;
            let moved = at.is_some_and(|at| {
                (at.x - origin.x).abs() > px(0.5) || (at.y - origin.y).abs() > px(0.5)
            });
            if at.is_none() || moved {
                cx.defer(move |cx| {
                    bench.update(cx, |bench, cx| match moved {
                        true => bench.close_card(cx),
                        false => bench.agent_card.row_at = Some(origin),
                    })
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
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
    let branch = worktree.as_ref().map(|worktree| worktree.branch.clone());
    let folder = worktree
        .as_ref()
        .and_then(|worktree| worktree.path.to_str())
        .map(|path| {
            crate::ui::path::tilde(
                path,
                bench
                    .home_at(bench.chat_target(&agent.id).place())
                    .as_deref(),
            )
        });
    let who: Vec<String> = agent
        .name
        .iter()
        .map(|name| format!("@{name}"))
        .chain(Some(bench.agent_name(&agent.provider).to_owned()))
        .collect();
    let fact = |icon: Option<Icon>, body: AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
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
                ui::mono(branch, TEXT_2XS, cx)
                    .id("card-branch")
                    .text_color(t.text_primary)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .tooltip(ui::tooltip("Review its work"))
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        bench.close_card(cx);
                        crate::agents::review::open(bench, &reviewed, cx);
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
                ui::mono(folder, TEXT_2XS, cx)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            )
            .into_any_element(),
        );
    }
    if !bench.agent_card.pulls.is_empty() {
        let pulls = div()
            .flex()
            .gap(SPACE_2)
            .children(bench.agent_card.pulls.iter().map(|pull| {
                let (agent, url) = (agent.id.clone(), pull.url.clone());
                ui::mono(format!("#{}", pull.number), TEXT_2XS, cx)
                    .id(SharedString::from(format!("card-pull-{}", pull.number)))
                    .text_color(t.text_primary)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .tooltip(ui::tooltip(pull.title.clone()))
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        bench.open_on_desk(&agent, &url, cx);
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
        .pt(SPACE_2H)
        .px(SPACE_3)
        .pb(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(crate::theme::RADIUS_3)
        .shadow(t.shadow_float())
        .text_size(crate::theme::ui_text(TEXT_XS))
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
                .gap(SPACE_2H)
                .min_w_0()
                .child(agent.mark(22., cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_SM))
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
                    .pt(SPACE_2H)
                    .border_t_1()
                    .border_color(t.border)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
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
