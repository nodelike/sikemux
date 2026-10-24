//! What is still going on under a chat's rows, stacked on its composer: the
//! questions the agent waits on, its subagents at work, its background tasks,
//! and the messages waiting their turn.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use serde_json::json;
use sikemux_wire::chat::{
    AsyncTask, QueuedMessage, SubagentCard, TaskState, WorkflowAgent, WorkflowAgentState,
};

use super::tools::word;
use super::Chat;
use crate::core::ChatAsk;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::ui::shimmer::shimmer;
use crate::workbench::{AgentRow, Workbench};

/// The groups, one above another, ending on the composer's top border in its fill.
pub fn stack(groups: Vec<AnyElement>, t: &Tokens) -> Option<AnyElement> {
    if groups.is_empty() {
        return None;
    }
    let ruled = groups.into_iter().enumerate().map(|(ix, group)| {
        div()
            .when(ix > 0, |group| group.border_t_1().border_color(t.border))
            .child(group)
    });
    let docked = div()
        .w_full()
        .max_w(px(780.))
        .flex()
        .flex_col()
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(t.border)
        .rounded_t(theme::RADIUS_3)
        .bg(t.surface_composer)
        .children(ruled);
    Some(
        div()
            .w_full()
            .px(px(36.))
            .flex()
            .justify_center()
            .child(docked)
            .into_any_element(),
    )
}

/// The line that says what kind of work a group is and how much of it, which
/// folds the group away; folded, it peeks at what the group holds.
pub struct Label {
    pub key: String,
    pub word: String,
    pub count: usize,
    /// Whether the group folds at all, and whether it is folded.
    pub folds: bool,
    pub folded: bool,
    pub icon: AnyElement,
    pub peek: Vec<String>,
    pub action: Option<AnyElement>,
}

impl Label {
    pub fn draw(self, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let (agent, key) = (agent.to_owned(), self.key.clone());
        let chevron = self.folds.then(|| {
            let chevron = motion::turn(
                format!("live-chevron:{key}"),
                motion::fold_angle(!self.folded),
                motion::DETAIL_FOLD,
                ui::icon(Icon::Chevron, 10.).text_color(t.text_disabled),
            );
            div()
                .ml_auto()
                .w(px(18.))
                .flex_none()
                .flex()
                .justify_center()
                .child(chevron)
        });
        div()
            .id(SharedString::from(format!("group:{}", self.key)))
            .w_full()
            .pt(px(6.))
            .pb(px(2.))
            .pl(px(11.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(crate::theme::ui_text(10.))
            .text_color(t.text_disabled)
            .when(self.folds, |label| {
                label
                    .cursor_pointer()
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        if let Some(chat) = bench.chats.get_mut(&agent) {
                            chat.toggle_group(&key);
                        }
                        cx.notify();
                    }))
            })
            .when(self.folded, |label| label.child(self.icon))
            .child(div().child(self.word.to_uppercase()))
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_color(t.text_tertiary)
                    .child(self.count.to_string()),
            )
            .when(self.folded, |label| {
                label.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .ml(px(2.))
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(10.5))
                        .text_color(t.text_tertiary)
                        .child(self.peek.join(" · ")),
                )
            })
            .children(self.action)
            .children(chevron)
            .into_any_element()
    }
}

fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// The colours of work going on: an agent's in the accent its card wears, a task's in green.
struct Tints {
    agent: Hsla,
    task: Hsla,
}

impl Tints {
    fn new(t: &Tokens) -> Tints {
        Tints {
            agent: theme::mix(t.accent, t.text_disabled, 0.62),
            task: theme::mix(t.live, t.text_disabled, 0.58),
        }
    }
}

/// A row of running work: its mark, name and what it is doing, then what it cost and how it stands.
fn task_row(
    t: &Tokens,
    tint: Hsla,
    mark: AnyElement,
    name: String,
    detail: Option<AnyElement>,
    cost: Option<String>,
    end: Option<AnyElement>,
) -> Div {
    div()
        .min_w_0()
        .pt(px(4.))
        .pb(px(4.))
        .pl(px(11.))
        .pr(px(8.))
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(crate::theme::ui_text(11.))
        .text_color(tint)
        .child(div().flex_none().child(mark))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(10.5))
                .text_color(t.text_primary)
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(10.5))
                .text_color(t.text_disabled)
                .children(detail),
        )
        .children(cost.map(|cost| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(10.))
                .text_color(t.text_disabled)
                .child(cost)
        }))
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .flex()
                .justify_center()
                .children(end),
        )
}

fn tokens(tokens: Option<u64>) -> Option<String> {
    tokens
        .filter(|tokens| *tokens > 0)
        .map(|tokens| format!("{} tokens", crate::home::compact(tokens as f64)))
}

fn cost(parts: [Option<String>; 2]) -> Option<String> {
    let cost: Vec<String> = parts.into_iter().flatten().collect();
    (!cost.is_empty()).then(|| cost.join(" · "))
}

/// The chat's subagents still at work, where the reader looks for live work.
pub fn subagents(agent: &AgentRow, chat: &Chat, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let running: &[SubagentCard] = chat
        .state
        .as_ref()
        .map_or(&[], |head| head.running_subagents.as_slice());
    if running.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let tint = Tints::new(&t).agent;
    let key = "subagent".to_owned();
    let folded = chat.groups_folded.contains(&key);
    let label = Label {
        word: plural("subagent", running.len()),
        count: running.len(),
        folds: true,
        folded,
        icon: ui::mark(&agent.provider, 16.),
        peek: running.iter().map(|card| card.name.clone()).collect(),
        action: None,
        key,
    }
    .draw(&agent.id, cx);
    let rows = running.iter().map(|card| {
        let activity = match &card.last_tool {
            Some(tool) => format!("{} {}", word(&tool.row), tool.row.target)
                .trim()
                .to_owned(),
            None => card
                .task
                .split('\n')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned(),
        };
        let calls = (card.calls > 0).then(|| {
            format!(
                "{} {}",
                card.calls,
                if card.calls == 1 { "call" } else { "calls" }
            )
        });
        let working = ui::icon(Icon::Hourglass, 11.)
            .text_color(tint)
            .into_any_element();
        task_row(
            &t,
            tint,
            ui::mark(&agent.provider, 16.),
            card.name.clone(),
            Some(shimmer(
                SharedString::from(format!("subagent-{}", card.session_id)),
                activity,
            )),
            cost([tokens(card.tokens), calls]),
            Some(working),
        )
        .into_any_element()
    });
    Some(
        div()
            .pb(px(7.))
            .flex()
            .flex_col()
            .child(label)
            .when(!folded, |group| group.children(rows))
            .into_any_element(),
    )
}

fn task_icon(kind: &str) -> Icon {
    match kind {
        "shell" => Icon::Command,
        "workflow" => Icon::Workflow,
        _ => Icon::Timer,
    }
}

/// What a task row says beside its name: who started it, and the first thing that says more than the name.
fn task_detail(task: &AsyncTask) -> Option<String> {
    let said = [
        task.summary.as_deref(),
        Some(task.description.as_str()),
        task.last_tool_name.as_deref(),
        Some(task.task_type.as_str()),
    ]
    .into_iter()
    .flatten()
    .find(|text| !text.is_empty() && *text != task.name);
    let owner = task.owner.as_ref().map(|owner| format!("from {owner}"));
    let detail: Vec<String> = owner.into_iter().chain(said.map(str::to_owned)).collect();
    (!detail.is_empty()).then(|| detail.join(" · "))
}

/// The background tasks, a group for each kind the agent named them by.
pub fn tasks(agent: &AgentRow, chat: &Chat, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    let tasks: &[AsyncTask] = chat
        .state
        .as_ref()
        .map_or(&[], |head| head.tasks.as_slice());
    let mut kinds: Vec<(&str, Vec<&AsyncTask>)> = Vec::new();
    for task in tasks {
        let kind = if task.task_type.is_empty() {
            "task"
        } else {
            task.task_type.as_str()
        };
        match kinds.iter_mut().find(|(held, _)| *held == kind) {
            Some((_, group)) => group.push(task),
            None => kinds.push((kind, vec![task])),
        }
    }
    let t = Tokens::get(cx).clone();
    let tints = Tints::new(&t);
    kinds
        .into_iter()
        .map(|(kind, group)| {
            let key = format!("task:{kind}");
            let folded = chat.groups_folded.contains(&key);
            let label = Label {
                word: plural(kind, group.len()),
                count: group.len(),
                folds: true,
                folded,
                icon: ui::icon(task_icon(kind), 12.)
                    .text_color(tints.task)
                    .into_any_element(),
                peek: group.iter().map(|task| task.name.clone()).collect(),
                action: None,
                key,
            }
            .draw(&agent.id, cx);
            let mut rows: Vec<AnyElement> = Vec::new();
            for task in group {
                let tint = if task.state == TaskState::Paused {
                    t.text_disabled
                } else {
                    tints.task
                };
                let stopping = chat.stopping.contains(&task.async_task_id);
                let stop = task
                    .can_stop
                    .then(|| stop(&agent.id, task, stopping, &t, cx));
                let finished = task
                    .agents
                    .iter()
                    .filter(|agent| {
                        matches!(
                            agent.state,
                            WorkflowAgentState::Completed | WorkflowAgentState::Failed
                        )
                    })
                    .count();
                let agents = (!task.agents.is_empty()).then(|| {
                    format!(
                        "{finished}/{} {}",
                        task.agents.len(),
                        if task.agents.len() == 1 {
                            "agent"
                        } else {
                            "agents"
                        }
                    )
                });
                let mark = ui::icon(task_icon(kind), 12.)
                    .text_color(tint)
                    .into_any_element();
                rows.push(
                    task_row(
                        &t,
                        tint,
                        mark,
                        task.name.clone(),
                        task_detail(task).map(|detail| match task.state {
                            TaskState::Running => shimmer(
                                SharedString::from(format!("task-{}", task.async_task_id)),
                                detail,
                            ),
                            _ => detail.into_any_element(),
                        }),
                        agents,
                        stop,
                    )
                    .into_any_element(),
                );
                rows.extend(
                    task.agents
                        .iter()
                        .map(|worker| workflow_agent(agent, worker, &t, &tints)),
                );
            }
            div()
                .pb(px(7.))
                .flex()
                .flex_col()
                .child(label)
                .when(!folded, |group| group.children(rows))
                .into_any_element()
        })
        .collect()
}

fn stop(
    agent: &str,
    task: &AsyncTask,
    stopping: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let (agent, id, danger) = (agent.to_owned(), task.async_task_id.clone(), t.danger);
    BaseButton::new(SharedString::from(format!("stop:{id}")))
        .accessibility_label(format!("Stop {}", task.name))
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_1)
        .text_color(t.text_disabled)
        .when(!stopping, |stop| {
            stop.hover(move |stop| stop.bg(Hsla { a: 0.16, ..danger }).text_color(danger))
        })
        .disabled(stopping)
        .on_click(cx.listener(move |bench, _, _, cx| {
            if let Some(chat) = bench.chats.get_mut(&agent) {
                chat.stopping.insert(id.clone());
            }
            bench.parts.redraw(&bench.parts.stage, cx);
            bench.ask_chat(ChatAsk::StopTask {
                agent: agent.clone(),
                task: id.clone(),
            })
        }))
        .child(ui::icon(Icon::Close, 10.).text_color(t.text_disabled))
        .into_any_element()
}

/// One of the agents a workflow started, set in under the workflow.
fn workflow_agent(
    agent: &AgentRow,
    worker: &WorkflowAgent,
    t: &Tokens,
    tints: &Tints,
) -> AnyElement {
    let detail = match worker.state {
        WorkflowAgentState::Failed => worker.error.clone().or(worker.phase.clone()),
        WorkflowAgentState::Running if worker.activity.is_some() => worker.activity.clone(),
        _ => worker.phase.clone(),
    };
    let calls = worker
        .tool_calls
        .filter(|calls| *calls > 0)
        .map(|calls| format!("{calls} tools"));
    let end = match worker.state {
        WorkflowAgentState::Running => Some(ui::icon(Icon::Hourglass, 11.).text_color(tints.agent)),
        WorkflowAgentState::Completed => Some(ui::icon(Icon::Check, 10.).text_color(tints.task)),
        WorkflowAgentState::Failed => Some(ui::icon(Icon::Close, 10.).text_color(t.danger)),
        WorkflowAgentState::Queued => None,
    };
    let quiet = matches!(
        worker.state,
        WorkflowAgentState::Queued | WorkflowAgentState::Completed
    );
    let mark = div()
        .when(quiet, |mark| mark.opacity(0.6))
        .child(ui::mark(&agent.provider, 16.))
        .into_any_element();
    task_row(
        t,
        tints.agent,
        mark,
        worker.label.clone(),
        detail.map(IntoElement::into_any_element),
        cost([tokens(worker.tokens), calls]),
        end.map(IntoElement::into_any_element),
    )
    .pl(px(26.))
    .into_any_element()
}

/// What a queued message says in its one line: its words, else what it carries.
fn queued_label(message: &QueuedMessage) -> String {
    if !message.text.is_empty() {
        return message.text.clone();
    }
    let paths = message
        .paths
        .iter()
        .map(|path| path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned());
    paths
        .chain(message.context.iter().map(|item| item.title.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Messages the person sent while a turn runs, waiting for it to end; while it
/// takes words mid-turn, each can go into it now, or all at once.
pub fn queued(agent: &str, chat: &Chat, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let head = chat.state.as_ref()?;
    let waiting = &head.queued;
    if waiting.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let live = Tints::new(&t).task;
    let steerable = head.running && head.capabilities["steering"] == true;
    let steer = |ids: Vec<String>, label: &str, keys: bool, cx: &mut Context<Workbench>| {
        let agent = agent.to_owned();
        let id = SharedString::from(format!("steer:{}", ids.join(",")));
        BaseButton::new(id)
            .h(px(20.))
            .px(px(7.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_1()
            .border_color(t.line)
            .rounded(theme::RADIUS_1)
            .bg(Hsla { a: 0.08, ..live })
            .hover(move |button| button.bg(Hsla { a: 0.18, ..live }))
            .text_size(crate::theme::ui_text(10.5))
            .text_color(t.text_primary)
            .on_click(cx.listener(move |bench, _, _, _| {
                bench.chat_target(&agent).call(
                    "steerQueued",
                    json!({ "agentId": agent, "ids": ids, "message": null }),
                );
            }))
            .child(label.to_owned())
            .when(keys, |button| {
                button.child(
                    div()
                        .text_size(crate::theme::ui_text(9.5))
                        .text_color(t.text_disabled)
                        .child("⌘↵"),
                )
            })
            .into_any_element()
    };
    let all = waiting.iter().map(|message| message.id.clone()).collect();
    let action = (steerable && waiting.len() > 1).then(|| {
        div()
            .ml_auto()
            .child(steer(all, "Steer all", true, cx))
            .into_any_element()
    });
    let label = Label {
        key: "queued".into(),
        word: "queued".into(),
        count: waiting.len(),
        folds: false,
        folded: false,
        icon: div().into_any_element(),
        peek: Vec::new(),
        action,
    };
    let label = label.draw(agent, cx);
    let rows: Vec<AnyElement> = waiting
        .iter()
        .map(|message| {
            let steer_one =
                steerable.then(|| steer(vec![message.id.clone()], "Steer", waiting.len() == 1, cx));
            let (agent, id, danger) = (agent.to_owned(), message.id.clone(), t.danger);
            let drop = BaseButton::new(SharedString::from(format!("drop:{id}")))
                .accessibility_label(format!("Drop {} from the queue", queued_label(message)))
                .size(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_1)
                .text_color(t.text_disabled)
                .hover(move |drop| drop.bg(Hsla { a: 0.16, ..danger }).text_color(danger))
                .on_click(cx.listener(move |bench, _, _, _| {
                    bench
                        .chat_target(&agent)
                        .call("dropQueued", json!({ "agentId": agent, "ids": [id] }));
                }))
                .child(ui::icon(Icon::Close, 10.).text_color(t.text_disabled));
            div()
                .min_w_0()
                .py(px(4.))
                .pl(px(11.))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(7.))
                .text_color(t.text_tertiary)
                .child(
                    ui::icon(Icon::Clock, 12.)
                        .flex_none()
                        .text_color(t.text_tertiary),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(12.))
                        .text_color(t.text_secondary)
                        .child(queued_label(message)),
                )
                .children(steer_one)
                .child(drop)
                .into_any_element()
        })
        .collect();
    Some(
        div()
            .pb(px(7.))
            .flex()
            .flex_col()
            .child(label)
            .children(rows)
            .into_any_element(),
    )
}
