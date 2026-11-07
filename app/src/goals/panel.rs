//! What takes the tab rail's place while Home shows: the row chosen in Home's list, with its
//! slices and each agent with what is out on its desk. What a call asks stays on its card.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, relative, AnyElement, Context, Div, Entity,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use sikemux_view::goals::{AgentDesk, GoalAgent, GoalRow, GoalSlice};
use sikemux_view::inbox::{InboxItem, InboxKind, InboxNeed};
use sikemux_view::slice::SliceState;

use super::room::{agent_face, clock_words, goal_says, next_ink, Chosen};
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
    title: AnyElement,
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
        .child(title)
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
    let mut buttons = Vec::new();
    let line = goal_says(row);
    let (project, card) = (row.project.clone(), row.card);
    let failed = (row.slices.iter()).find(|slice| slice.state == SliceState::Failed);
    if let Some(failed) = failed {
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
    let only_itself = match row.slices.as_slice() {
        [] => true,
        [slice] => slice.title == row.title,
        _ => false,
    };
    let t = Tokens::get(cx).clone();
    let list_ink = t.text_tertiary;
    div()
        .id("goal-panel")
        .size_full()
        .flex()
        .flex_col()
        .child(head(
            title(row.title.clone(), cx),
            row.state,
            line,
            row.clock.map(clock_words),
            None,
            buttons,
            cx,
        ))
        .when(!only_itself, |panel| {
            panel.child(slice_path(&row.project, &row.slices, cx))
        })
        .child(
            div()
                .id("goal-panel-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
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
                        .children(
                            (agents.iter()).map(|agent| agent_block(pane, agent, &row.title, cx)),
                        ),
                ),
        )
        .into_any_element()
}

/// What a call is about besides what its card asks: where it is from, and the agent asking with
/// what is out on its desk.
fn need(pane: &mut GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> AnyElement {
    let state = match item.need {
        Some(InboxNeed::Fix) => SliceState::Failed,
        _ => SliceState::NeedsYou,
    };
    let agent = (item.agent_id.as_deref()).and_then(|id| pane.room.agent(id).cloned());
    let from = pane.call_from(item, cx);
    let shown = from
        .or_else(|| agent.as_ref().map(|agent| agent.task.clone()))
        .unwrap_or_else(|| item.heading.clone());
    let line = item.name.as_ref().map(|name| format!("@{name}"));
    let mut buttons = Vec::new();
    if let Some(agent) = item.agent_id.clone() {
        buttons.push(
            Button::new("need-open-chat", "Open chat", Variant::Outline)
                .on_click(cx.listener(move |pane, _, _, cx| pane.open_agent(&agent, cx)))
                .into_any_element(),
        );
    }
    let offer = (item.kind == InboxKind::SeededRules)
        .then(|| seeded_offer(pane, item, cx))
        .flatten();
    let title_said = shown.clone();
    div()
        .id("need-panel")
        .size_full()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(head(title(shown, cx), state, line, None, None, buttons, cx))
        .children(offer.map(|offer| div().px(px(18.)).child(offer)))
        .children(agent.map(|agent| {
            div()
                .p(SPACE_2)
                .child(agent_block(pane, &agent, &title_said, cx))
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
    let line = [
        agent.left.clone(),
        agent.name.as_ref().map(|name| format!("@{name}")),
        project,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let offered = (agent.offer.clone()).map(|offer| {
        div()
            .mt(SPACE_2)
            .child(ui::meta(format!("Offered: {offer}"), cx).whitespace_normal())
            .into_any_element()
    });
    div()
        .id("agent-panel")
        .size_full()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(head(
            title(agent.task.clone(), cx),
            agent.state,
            Some(line),
            None,
            offered,
            buttons,
            cx,
        ))
        .child(rule_label(format!("{} open", on_desk(&agent.desk)), cx))
        .child(
            div()
                .p(SPACE_2)
                .child(agent_block(pane, agent, &agent.task, cx)),
        )
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

/// One agent: what it does and its name, then what is out on its desk, each opening there. A
/// task that is the panel's `title` already is left to the title.
fn agent_block(
    pane: &GoalsPane,
    agent: &GoalAgent,
    title: &str,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = agent.id.clone();
    let chips = desk_chips(agent, cx);
    let name = agent.name.as_ref().map(|name| format!("@{name}"));
    let (task, name) = match name {
        Some(name) if agent.task == title => (name, None),
        name => (agent.task.clone(), name),
    };
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
                    ui::text(task, TEXT_SM, FontWeight::NORMAL)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary),
                )
                .children(name.map(|name| {
                    ui::text(name, TEXT_XS, FontWeight::NORMAL)
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

/// A panel's title, wrapping as long as it is.
fn title(words: String, cx: &Context<GoalsPane>) -> AnyElement {
    div()
        .text_size(theme::ui_text(TEXT_LG))
        .font_weight(FontWeight::MEDIUM)
        .line_height(relative(1.3))
        .text_color(Tokens::get(cx).text_primary)
        .child(words)
        .into_any_element()
}

impl GoalsPane {
    /// Opens slice `card` of the project at `folder`, bringing that project to the front.
    pub(super) fn open_slice(&mut self, folder: &str, card: u64, cx: &mut Context<Self>) {
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
