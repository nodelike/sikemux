//! What takes the tab rail's place while Home shows: the row chosen in Home's list, with what it
//! needs of the person, its slices, and each agent with what is out on its desk.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, relative, AnyElement, Context, Div, Entity,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use sikemux_core::server::board::PassParams as BoardPassParams;
use sikemux_core::server::call;
use sikemux_view::goals::{AgentDesk, GoalAgent, GoalRow, GoalSlice};
use sikemux_view::inbox::{InboxItem, InboxKind, InboxNeed};
use sikemux_view::slice::SliceState;

use super::room::{
    agent_face, agent_state, clock_words, need_title, need_word, next_ink, next_words, Chosen,
};
use super::{state_dot, At, GoalsPane};
use crate::desk::DeskShown;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, TEXT_2XS, TEXT_2XS_H, TEXT_LG};
use crate::theme::{TEXT_SM, TEXT_XS, TEXT_XS_H};
use crate::ui::{self, Button, Variant};

/// Part of Home drawn on its own and cached, so the window redrawing for an agent's stream does
/// not draw it again unless the pane changed.
pub struct PaneView {
    pane: WeakEntity<GoalsPane>,
    draw: fn(&mut GoalsPane, &mut Window, &mut Context<GoalsPane>) -> AnyElement,
    _following: Subscription,
}

impl PaneView {
    pub(super) fn new(
        pane: &Entity<GoalsPane>,
        draw: fn(&mut GoalsPane, &mut Window, &mut Context<GoalsPane>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            pane: pane.downgrade(),
            draw,
            _following: cx.observe(pane, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for PaneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        self.pane
            .update(cx, |pane, cx| draw(pane, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// What takes the tab rail's place: the row chosen in Home's list.
pub(super) fn draw(
    pane: &mut GoalsPane,
    _: &mut Window,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    match pane.room.chosen.clone() {
        Some(Chosen::Goal(project, card)) => match pane.room.goal(&project, card).cloned() {
            Some(row) => goal(pane, &row, cx),
            None => nothing(cx),
        },
        Some(Chosen::Need(id)) => match pane.inbox_item(&id, cx) {
            Some(item) => need(pane, &item, cx),
            None => nothing(cx),
        },
        Some(Chosen::Agent(id)) => match pane.room.agent(&id).cloned() {
            Some(agent) => loose_agent(pane, &agent, cx),
            None => nothing(cx),
        },
        None => nothing(cx),
    }
}

fn nothing(cx: &mut Context<GoalsPane>) -> AnyElement {
    div()
        .p(px(18.))
        .child(ui::meta("Nothing chosen.", cx))
        .into_any_element()
}

/// The head of the panel: a title, a line under it with a clock, and what can be done about it.
fn head(
    title: String,
    state: SliceState,
    line: Option<String>,
    clock: Option<String>,
    under: Option<AnyElement>,
    buttons: Vec<AnyElement>,
    cx: &Context<GoalsPane>,
) -> Div {
    let t = Tokens::get(cx);
    let ink = next_ink(state, cx);
    let tint = match state {
        SliceState::NeedsYou => Some(t.warn),
        SliceState::Failed => Some(t.danger),
        _ => None,
    };
    div()
        .flex_none()
        .pt(px(18.))
        .px(px(18.))
        .pb(px(14.))
        .when_some(tint, |head, tint| {
            head.bg(linear_gradient(
                170.,
                linear_color_stop(tint.opacity(0.12), 0.),
                linear_color_stop(tint.opacity(0.), 0.85),
            ))
        })
        .child(
            div()
                .text_size(theme::ui_text(TEXT_LG))
                .font_weight(FontWeight::MEDIUM)
                .line_height(relative(1.3))
                .text_color(t.text_primary)
                .child(title),
        )
        .when(line.is_some() || clock.is_some(), |head| {
            head.child(
                div()
                    .mt(px(9.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(state_dot(state, 9., cx))
                    .children(line.map(|line| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::ui_text(TEXT_XS_H))
                            .line_height(relative(1.3))
                            .text_color(ink)
                            .child(line)
                    }))
                    .children(clock.map(|clock| {
                        ui::text(clock, TEXT_XS, FontWeight::NORMAL)
                            .flex_none()
                            .text_color(t.text_tertiary)
                    })),
            )
        })
        .children(under)
        .child(
            div()
                .mt(px(13.))
                .flex()
                .flex_wrap()
                .gap(SPACE_1H)
                .children(buttons),
        )
}

fn goal(pane: &mut GoalsPane, row: &GoalRow, cx: &mut Context<GoalsPane>) -> AnyElement {
    let items: Vec<InboxItem> = (row.needs.iter())
        .filter_map(|id| pane.inbox_item(id, cx))
        .collect();
    let (first, rest) = match items.split_first() {
        Some((first, rest)) => (Some(first), rest),
        None => (None, &[][..]),
    };
    let mut buttons = first.map_or(Vec::new(), |item| actions(pane, item, cx));
    let (project, card) = (row.project.clone(), row.card);
    let failed = (row.slices.iter()).find(|slice| slice.state == SliceState::Failed);
    if let (None, Some(failed)) = (first, failed) {
        let (folder, slice) = (project.clone(), failed.card);
        buttons.push(
            Button::new("goal-see-why", "See why", Variant::Outline)
                .on_click(cx.listener(move |pane, _, _, cx| pane.open_slice(&folder, slice, cx)))
                .into_any_element(),
        );
    }
    let focused = (row.agents.clone(), row.title.clone(), row.project.clone());
    buttons.push(
        Button::new("goal-focus", "Focus", Variant::Outline)
            .disabled(row.agents.is_empty())
            .on_click(cx.listener(move |pane, _, _, cx| {
                let (agents, title, project) = focused.clone();
                let _ = pane.bench.update(cx, |bench, cx| {
                    bench.focus_goal(title, agents, &project, card, cx)
                });
            }))
            .into_any_element(),
    );
    buttons.push(
        Button::new("goal-open", "Open", Variant::Outline)
            .on_click(cx.listener(move |pane, _, _, cx| pane.open_row(&project, card, cx)))
            .into_any_element(),
    );
    let open: usize = (row.agents.iter())
        .filter_map(|id| pane.room.agent(id))
        .map(|agent| on_desk(&agent.desk))
        .sum();
    let agents: Vec<GoalAgent> = (row.agents.iter())
        .filter_map(|id| pane.room.agent(id).cloned())
        .collect();
    let t = Tokens::get(cx).clone();
    let list_ink = t.text_tertiary;
    div()
        .id("goal-panel")
        .size_full()
        .flex()
        .flex_col()
        .child(head(
            row.title.clone(),
            row.state,
            row.next.as_ref().map(next_words),
            row.clock.map(clock_words),
            first.and_then(|item| held_lines(item, cx)),
            buttons,
            cx,
        ))
        .when(!row.slices.is_empty(), |panel| {
            panel.child(slice_path(&row.project, &row.slices, cx))
        })
        .child(
            div()
                .id("goal-panel-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .when(!rest.is_empty(), |list| {
                    list.child(rule_label(format!("{} more need you", rest.len()), cx))
                        .children(rest.iter().map(|item| more_need(pane, item, cx)))
                })
                .child(rule_label(
                    format!(
                        "{} agent{} · {open} open",
                        agents.len(),
                        if agents.len() == 1 { "" } else { "s" }
                    ),
                    cx,
                ))
                .child(
                    div()
                        .p(SPACE_2)
                        .when(agents.is_empty(), |list| {
                            list.child(
                                ui::text(
                                    "No agents on this goal now.",
                                    TEXT_XS,
                                    FontWeight::NORMAL,
                                )
                                .py(px(14.))
                                .px(px(10.))
                                .text_color(list_ink),
                            )
                        })
                        .children(agents.iter().map(|agent| agent_block(pane, agent, cx))),
                ),
        )
        .into_any_element()
}

/// Something that waits on the person about no goal.
fn need(pane: &mut GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> AnyElement {
    let state = match item.need {
        Some(InboxNeed::Fix) => SliceState::Failed,
        _ => SliceState::NeedsYou,
    };
    let word = item.need.map(need_word);
    let who = item.name.as_ref().map(|name| format!("@{name}"));
    let line = [word.map(str::to_owned), who]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let mut buttons = actions(pane, item, cx);
    if let Some(agent) = item.agent_id.clone() {
        buttons.push(
            Button::new("need-open-chat", "Open", Variant::Outline)
                .on_click(cx.listener(move |pane, _, _, cx| pane.open_agent(&agent, cx)))
                .into_any_element(),
        );
    }
    let offer = (item.kind == InboxKind::SeededRules)
        .then(|| seeded_offer(pane, item, cx))
        .flatten();
    let agent = (item.agent_id.as_deref()).and_then(|id| pane.room.agent(id).cloned());
    div()
        .id("need-panel")
        .size_full()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(head(
            need_title(item),
            state,
            Some(line),
            Some(ui::time::age(item.at)),
            held_lines(item, cx),
            buttons,
            cx,
        ))
        .children(offer.map(|offer| div().px(px(18.)).child(offer)))
        .children(agent.map(|agent| {
            div()
                .p(SPACE_2)
                .child(agent_block(pane, &agent, cx))
                .into_any_element()
        }))
        .into_any_element()
}

/// The rules offered from past sessions, when their project is the one in front, whose memory the
/// pane has read; else the way to bring it there.
fn seeded_offer(
    pane: &mut GoalsPane,
    item: &InboxItem,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    if pane.seeds_here(item) {
        return super::rules::offer(pane, false, cx);
    }
    let folder = item.project.clone()?;
    Some(
        div()
            .flex()
            .pb(px(14.))
            .child(
                Button::new("seeded-bring", "Go to its project", Variant::Outline)
                    .on_click(cx.listener(move |pane, _, _, cx| pane.bring(folder.clone(), cx))),
            )
            .into_any_element(),
    )
}

/// An agent working no goal, with what is on its desk.
fn loose_agent(pane: &mut GoalsPane, agent: &GoalAgent, cx: &mut Context<GoalsPane>) -> AnyElement {
    let opened = agent.id.clone();
    let buttons = vec![Button::new("agent-open", "Open", Variant::Outline)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open_agent(&opened, cx)))
        .into_any_element()];
    let project = (pane.bench.upgrade()).and_then(|bench| {
        let bench = bench.read(cx);
        (bench.projects().iter())
            .find(|project| Some(project.id.as_str()) == agent.project.as_deref())
            .map(|project| project.name.clone())
    });
    let line = [agent.name.as_ref().map(|name| format!("@{name}")), project]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .id("agent-panel")
        .size_full()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(head(
            agent.task.clone(),
            agent_state(agent),
            Some(line),
            None,
            None,
            buttons,
            cx,
        ))
        .child(rule_label(format!("{} open", on_desk(&agent.desk)), cx))
        .child(div().p(SPACE_2).child(agent_block(pane, agent, cx)))
        .into_any_element()
}

fn on_desk(desk: &AgentDesk) -> usize {
    desk.pages.len()
        + desk.files.len()
        + desk.terminals.len()
        + usize::from(desk.simulator.is_some())
}

/// A label in spaced capitals over the rest of the line's hairline.
fn rule_label(words: String, cx: &Context<GoalsPane>) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .pt(px(14.))
        .px(px(18.))
        .pb(px(2.))
        .child(ui::caps(&words, cx).text_size(theme::ui_text(10.)))
        .child(div().flex_1().h(px(1.)).bg(t.border))
}

/// A goal's slices as a line of marks, those behind it joined in green. Each opens its slice.
fn slice_path(project: &str, slices: &[GoalSlice], cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let last = slices.len().saturating_sub(1);
    div()
        .flex()
        .flex_none()
        .py(px(14.))
        .px(px(18.))
        .border_t_1()
        .border_b_1()
        .border_color(t.border)
        .children(slices.iter().enumerate().map(|(at, slice)| {
            let ink = match slice.state {
                SliceState::NeedsYou => t.warn,
                SliceState::Failed => t.danger,
                SliceState::Moving => t.text_secondary,
                _ => t.text_tertiary,
            };
            let joint = match slice.state {
                SliceState::Done => t.live.opacity(0.45),
                _ => t.line,
            };
            let (folder, card) = (project.to_owned(), slice.card);
            div()
                .id(SharedString::from(format!("path-{}", slice.card)))
                .relative()
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .when(at != last, |step| {
                    step.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .left(px(12.))
                            .right(SPACE_1)
                            .h(px(2.))
                            .rounded(px(1.))
                            .bg(joint),
                    )
                })
                .child(div().mb(SPACE_1H).child(state_dot(slice.state, 8., cx)))
                .child(
                    ui::text(slice.title.clone(), TEXT_2XS, FontWeight::NORMAL)
                        .pr(SPACE_1H)
                        .truncate()
                        .text_color(ink),
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.open_slice(&folder, card, cx);
                }))
        }))
        .into_any_element()
}

/// One agent: what it does and its name, then what is out on its desk, each opening there.
fn agent_block(pane: &GoalsPane, agent: &GoalAgent, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = agent.id.clone();
    let chips = desk_chips(agent, cx);
    let line = div()
        .id(SharedString::from(format!("panel-agent-{}", agent.id)))
        .py(px(9.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |line| line.bg(t.surface_hover))
        .on_click(cx.listener(move |pane, _, _, cx| pane.open_agent(&opened, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .child(agent_face(agent, 18., cx))
                .child(
                    ui::text(agent.task.clone(), TEXT_SM, FontWeight::NORMAL)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary),
                )
                .children(agent.name.as_ref().map(|name| {
                    ui::text(format!("@{name}"), TEXT_XS, FontWeight::NORMAL)
                        .flex_none()
                        .text_color(t.text_tertiary)
                })),
        )
        .when(!chips.is_empty(), |line| {
            line.child(
                div()
                    .mt(px(7.))
                    .ml(px(27.))
                    .flex()
                    .flex_wrap()
                    .gap(SPACE_1)
                    .children(chips),
            )
        });
    super::with_agent_menu(line, Some(agent.id.clone()), &pane.bench)
}

fn desk_chips(agent: &GoalAgent, cx: &mut Context<GoalsPane>) -> Vec<AnyElement> {
    let desk = &agent.desk;
    let file_name = |path: &String| {
        (std::path::Path::new(path).file_name())
            .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned())
    };
    let terminals = (desk.terminals.iter()).map(|terminal| {
        (
            Icon::Terminal,
            terminal.title.clone(),
            DeskShown::Terminal {
                session: terminal.session,
                title: terminal.title.clone(),
            },
        )
    });
    let pages = (desk.pages.iter()).map(|page| {
        let title = if page.title.is_empty() {
            page.url.clone()
        } else {
            page.title.clone()
        };
        (Icon::Globe, title, DeskShown::Page(page.id.clone()))
    });
    let simulator =
        (desk.simulator.iter()).map(|device| (Icon::Phone, device.clone(), DeskShown::Simulator));
    let files = (desk.files.iter())
        .map(|path| (Icon::Editor, file_name(path), DeskShown::File(path.clone())));
    let shown: Vec<(Icon, String, DeskShown)> = terminals
        .chain(pages)
        .chain(simulator)
        .chain(files)
        .collect();
    shown
        .into_iter()
        .enumerate()
        .map(|(at, (glyph, label, shown))| {
            let agent = agent.id.clone();
            desk_chip(
                SharedString::from(format!("desk-chip-{agent}-{at}")),
                glyph,
                label,
                cx,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                cx.stop_propagation();
                let (agent, shown) = (agent.clone(), shown.clone());
                let _ = pane
                    .bench
                    .update(cx, |bench, cx| bench.show_on_agent_desk(&agent, shown, cx));
            }))
            .into_any_element()
        })
        .collect()
}

fn desk_chip(
    id: SharedString,
    glyph: Icon,
    label: String,
    cx: &Context<GoalsPane>,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .flex()
        .flex_none()
        .max_w(px(180.))
        .items_center()
        .gap(SPACE_1)
        .py(px(2.))
        .px(px(7.))
        .rounded(theme::RADIUS_2)
        .bg(t.surface_hover)
        .cursor_pointer()
        .text_color(t.text_secondary)
        .hover(move |chip| chip.bg(t.surface_active).text_color(t.text_primary))
        .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
        .child(
            ui::text(label, TEXT_2XS_H, FontWeight::NORMAL)
                .min_w_0()
                .truncate(),
        )
}

/// The command a held action runs and why it waits, under what it is.
fn held_lines(item: &InboxItem, cx: &mut Context<GoalsPane>) -> Option<AnyElement> {
    let hold = item.hold.as_ref()?;
    Some(
        div()
            .mt(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(crate::approvals::held_action(&hold.id, &hold.what, cx))
            .children(
                hold.why
                    .clone()
                    .map(|why| ui::meta(why, cx).whitespace_normal()),
            )
            .into_any_element(),
    )
}

/// What can be done about a need right here. Only passing a gate or unblocking takes the accent:
/// a question's answers are a choice among equals.
fn actions(pane: &GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> Vec<AnyElement> {
    if let (InboxKind::Gate, Some(gate), Some(card)) = (item.kind, item.gate, &item.card) {
        let (project, id, environment) = (card.project.clone(), card.id, item.environment.clone());
        return vec![Button::new(
            SharedString::from(format!("approve-{}", item.id)),
            "Approve",
            Variant::Primary,
        )
        .on_click(cx.listener(move |pane, _, _, cx| {
            let params = BoardPassParams {
                project: project.clone(),
                card: id,
                gate,
                environment: environment.clone(),
            };
            pane.ask::<call::BoardPass>(params, |_| {}, cx);
        }))
        .into_any_element()];
    }
    if let Some(hold) = &item.hold {
        let here = crate::core::Target::here(&pane.asks);
        return vec![crate::approvals::ship_buttons(
            &hold.id,
            false,
            here,
            pane.bench.clone(),
        )];
    }
    if item.stalled.is_some() {
        return super::home::stall_buttons(item, cx).into_iter().collect();
    }
    if let (Some(question), false) = (&item.question_id, item.answers.is_empty()) {
        return (item.answers.iter().enumerate())
            .map(|(choice, answer)| {
                let question = question.clone();
                Button::new(
                    SharedString::from(format!("answer-{question}-{choice}")),
                    answer.clone(),
                    Variant::Outline,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.answer(question.clone(), choice, cx);
                }))
                .into_any_element()
            })
            .collect();
    }
    if item.kind == InboxKind::SeededRules {
        return Vec::new();
    }
    let (label, variant) = match item.need {
        Some(InboxNeed::Fix) => ("See why", Variant::Outline),
        Some(InboxNeed::Unblock) => ("Unblock", Variant::Primary),
        _ if item.kind == InboxKind::Question => ("Answer", Variant::Primary),
        _ => ("Review", Variant::Primary),
    };
    let opened = item.clone();
    vec![Button::new(
        SharedString::from(format!("act-{}", item.id)),
        label,
        variant,
    )
    .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
    .into_any_element()]
}

/// Another need of the same goal, under the first.
fn more_need(pane: &GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tint = match item.need {
        Some(InboxNeed::Fix) => t.danger,
        _ => t.warn,
    };
    let word = item.need.map_or("", need_word);
    div()
        .py(SPACE_2)
        .px(px(18.))
        .child(
            div()
                .text_size(theme::ui_text(TEXT_XS_H))
                .line_height(relative(1.3))
                .text_color(tint)
                .child(format!("{word} · {}", need_title(item))),
        )
        .children(held_lines(item, cx))
        .child(
            div()
                .mt(SPACE_1H)
                .flex()
                .flex_wrap()
                .gap(SPACE_1)
                .children(actions(pane, item, cx)),
        )
        .into_any_element()
}

impl GoalsPane {
    /// Opens slice `card` of the project at `folder`, bringing that project to the front.
    fn open_slice(&mut self, folder: &str, card: u64, cx: &mut Context<Self>) {
        if self.here.as_deref() != Some(folder) {
            self.bring(folder.to_owned(), cx);
        }
        self.go(At::Slice(card), cx);
    }

    /// Whether the rules a Seeded item offers are the ones the pane has read.
    fn seeds_here(&self, item: &InboxItem) -> bool {
        item.project.is_none() || item.project == self.here
    }
}
