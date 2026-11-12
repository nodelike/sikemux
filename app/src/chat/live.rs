//! What is still going on under a chat's rows, stacked on its composer: the
//! questions the agent waits on, its subagents at work, its background tasks,
//! the messages waiting their turn, and those share viewers wrote.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, AnyElement, Context, Div, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled,
};
use sikemux_core::server::call;
use sikemux_core::server::chat::calls::QueuedParams;
use sikemux_core::server::share::GuestMessageParams;
use sikemux_core::server::triggers::TriggerParams;
use sikemux_view::chat::{
    AsyncTask, ChatTrigger, QueuedMessage, SubagentCard, TaskState, TriggerWhen, WorkflowAgent,
    WorkflowAgentState,
};

use super::tools::word;
use super::Chat;
use crate::agents::rows::AgentRow;
use crate::core::ChatAsk;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui;
use crate::ui::shimmer::shimmer;
use crate::workbench::Workbench;

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
            .pt(SPACE_1H)
            .pb(SPACE_0)
            .pl(px(11.))
            .pr(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(t.text_disabled)
            .when(self.folds, |label| {
                label
                    .cursor_pointer()
                    .on_click(cx.listener(move |bench, _, _, cx| {
                        if let Some(chat) = bench.chats.open.get_mut(&agent) {
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
                        .ml(SPACE_0)
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(self.peek.join(" · ")),
                )
            })
            .children(self.action)
            .children(chevron)
            .into_any_element()
    }
}

/// A group's rows under its label, folding open and shut as the label is pressed.
fn folding(key: &str, folded: bool, rows: Vec<AnyElement>) -> AnyElement {
    let key = key.to_owned();
    motion::with_window(move |window, cx| {
        let shown = motion::follow(
            SharedString::from(format!("live-fold:{key}")),
            if folded { 0. } else { 1. },
            if folded {
                motion::FOLD_AWAY
            } else {
                motion::UNFOLD
            },
            window,
            cx,
        );
        if shown <= 0. {
            return div().into_any_element();
        }
        let rows = div().flex().flex_col().children(rows);
        let id = SharedString::from(format!("live-rows:{key}"));
        motion::folded(id, shown, rows, window, cx)
    })
    .into_any_element()
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

fn tokens(tokens: Option<u64>) -> Option<String> {
    tokens
        .filter(|tokens| *tokens > 0)
        .map(|tokens| format!("{} tokens", crate::ui::numbers::compact(tokens as f64)))
}

fn cost(parts: [Option<String>; 2]) -> Option<String> {
    let cost: Vec<String> = parts.into_iter().flatten().collect();
    (!cost.is_empty()).then(|| cost.join(" · "))
}

/// The kinds of running work, each with the mark the dock's header and rows show it by.
fn task_icon(kind: &str) -> Icon {
    match kind {
        "shell" => Icon::Terminal,
        "workflow" => Icon::Workflow,
        _ => Icon::Timer,
    }
}

fn task_kind(task: &AsyncTask) -> &str {
    if task.task_type.is_empty() {
        "task"
    } else {
        task.task_type.as_str()
    }
}

/// What a task row says beside its name: who started it, and the first thing that says more than the name.
fn task_detail(task: &AsyncTask) -> Option<String> {
    let said = [task.summary.as_deref(), Some(task.description.as_str())]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty() && *text != task.name);
    let owner = task.owner.as_ref().map(|owner| format!("from {owner}"));
    let detail: Vec<String> = owner.into_iter().chain(said.map(str::to_owned)).collect();
    (!detail.is_empty()).then(|| one_line(&detail.join(" · ")))
}

/// A task named by a whole script, such as a heredoc command, shows its first line that says something.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// How long a task has run, as a clock reads it.
fn elapsed(task: &AsyncTask) -> Option<String> {
    let seconds = task.usage.as_ref()?.duration_ms / 1000;
    (seconds > 0).then(|| match seconds {
        s if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        s => format!("{}:{:02}", s / 60, s % 60),
    })
}

const RUNNING: &str = "dock";
const QUEUED: &str = "queued";
const GUESTS: &str = "guests";
const TRIGGERS: &str = "triggers";
const HANDOFF: &str = "handoff";

/// A group of the dock: what its header says, on its own or within the line the folded dock
/// draws, the button it offers, and its rows.
struct Group {
    key: &'static str,
    words: Vec<AnyElement>,
    action: Option<AnyElement>,
    rows: Vec<AnyElement>,
}

/// The sheet tucked under the composer's top edge, holding all that sits on the composer, a
/// hairline between each part: what the agent asks of the person, the messages waiting their
/// turn, what is running for it, and its triggers. Each group folds on its own; once all are
/// folded they share one line.
pub fn dock(
    agent: &AgentRow,
    chat: &Chat,
    asks: Vec<AnyElement>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let handoff = handoff(agent, chat, cx);
    let guests = guests(&agent.id, chat, cx);
    let queued = queued(&agent.id, chat, cx);
    let running = running(agent, chat, cx);
    let triggers = triggers(&agent.id, chat, cx);
    let groups: Vec<Group> = [handoff, guests, queued, running, triggers]
        .into_iter()
        .flatten()
        .collect();
    if asks.is_empty() && groups.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let folded = |group: &Group| chat.groups_folded.contains(group.key);
    let all_folded = !groups.is_empty() && groups.iter().all(folded);
    let mut parts: Vec<AnyElement> = asks;
    if all_folded {
        let keys: Vec<&'static str> = groups.iter().map(|group| group.key).collect();
        let mut words: Vec<AnyElement> = Vec::new();
        let mut action = None;
        let mut groups = groups;
        groups.sort_by_key(|group| group.key != RUNNING);
        for (at, group) in groups.into_iter().enumerate() {
            if at > 0 {
                words.push(
                    div()
                        .text_color(t.text_disabled)
                        .child("·")
                        .into_any_element(),
                );
            }
            words.extend(group.words);
            action = match group.key {
                QUEUED => group.action.or(action),
                _ => action.or(group.action),
            };
        }
        parts.push(header(
            &agent.id,
            "dock-folded",
            Fold::Open(keys),
            words,
            action,
            true,
            &t,
            cx,
        ));
    } else {
        let mut groups = groups.into_iter().peekable();
        if let Some(group) = groups.next_if(|group| group.key == QUEUED) {
            parts.push(draw_group(&agent.id, group, chat, &t, cx));
        }
        parts.extend(groups.map(|group| draw_group(&agent.id, group, chat, &t, cx)));
    }
    let ruled = parts.into_iter().enumerate().map(|(at, part)| {
        div()
            .when(at > 0, |part| {
                part.child(div().h(px(1.)).mx(SPACE_2).my(SPACE_0).bg(t.line))
            })
            .child(part)
    });
    let sheet = div()
        .w_full()
        .pt(SPACE_0)
        .px(SPACE_1H)
        .pb(DOCK_TUCK + SPACE_1)
        .flex()
        .flex_col()
        .rounded_t(theme::RADIUS_3)
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(t.line)
        .bg(linear_gradient(
            180.,
            linear_color_stop(t.surface_composer, 0.),
            linear_color_stop(t.surface_sunken, 1.),
        ))
        .children(ruled);
    Some(
        div()
            .w_full()
            .px(SPACE_4)
            .mb(-DOCK_TUCK)
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(super::column::composer(cx))
                    .px(SPACE_4)
                    .child(sheet),
            )
            .into_any_element(),
    )
}

fn draw_group(
    agent: &str,
    group: Group,
    chat: &Chat,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let folded = chat.groups_folded.contains(group.key);
    let id = format!("dock-{}", group.key);
    let head = header(
        agent,
        &id,
        Fold::One(group.key),
        group.words,
        group.action,
        folded,
        t,
        cx,
    );
    div()
        .flex()
        .flex_col()
        .child(head)
        .child(folding(group.key, folded, group.rows))
        .into_any_element()
}

/// What pressing a header does: fold or unfold its own group, or open every group at once.
enum Fold {
    One(&'static str),
    Open(Vec<&'static str>),
}

/// A group's header, or the folded dock's one line: its words, its button, and the chevron.
#[allow(clippy::too_many_arguments)]
fn header(
    agent: &str,
    id: &str,
    fold: Fold,
    words: Vec<AnyElement>,
    action: Option<AnyElement>,
    folded: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let agent = agent.to_owned();
    let chevron = motion::turn(
        SharedString::from(format!("{id}-chevron")),
        motion::fold_angle(true) * if folded { -1. } else { 1. },
        motion::DETAIL_FOLD,
        ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary),
    );
    div()
        .id(SharedString::from(id.to_owned()))
        .h(px(30.))
        .pl(SPACE_2H)
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_tertiary)
        .on_click(cx.listener(move |bench, _, _, cx| {
            if let Some(chat) = bench.chats.open.get_mut(&agent) {
                match &fold {
                    Fold::One(key) => chat.toggle_group(key),
                    Fold::Open(keys) => keys.iter().for_each(|key| {
                        chat.groups_folded.remove(*key);
                    }),
                }
            }
            cx.notify();
        }))
        .children(words)
        .child(div().flex_1())
        .children(action)
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .flex()
                .justify_center()
                .child(chevron),
        )
        .into_any_element()
}

/// A group's count beside its icon, as its header says it.
fn counted(icon: AnyElement, words: String, t: &Tokens) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(icon)
        .child(ui::text(words, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_secondary))
        .into_any_element()
}

/// The subagents and background tasks running for the chat, which fold down to a count of
/// each kind.
fn running(agent: &AgentRow, chat: &Chat, cx: &mut Context<Workbench>) -> Option<Group> {
    let head = chat.state.as_ref()?;
    let subagents: &[SubagentCard] = &head.running_subagents;
    let tasks: &[AsyncTask] = &head.tasks;
    let count = subagents.len() + tasks.len();
    if count == 0 {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let tints = Tints::new(&t);
    let mut kinds: Vec<(&str, Vec<&AsyncTask>)> = Vec::new();
    for task in tasks {
        let kind = task_kind(task);
        match kinds.iter_mut().find(|(held, _)| *held == kind) {
            Some((_, group)) => group.push(task),
            None => kinds.push((kind, vec![task])),
        }
    }
    let live = div().size(px(6.)).flex_none().rounded_full().bg(t.live);
    let mut words = vec![counted(
        live.into_any_element(),
        format!("{count} running"),
        &t,
    )];
    if chat.groups_folded.contains(RUNNING) {
        let subagent = (!subagents.is_empty())
            .then(|| kind_count(ui::mark(&agent.provider, 13.), subagents.len(), &t));
        let tasks = kinds.iter().map(|(kind, group)| {
            let icon = ui::icon(task_icon(kind), 12.)
                .text_color(tints.task)
                .into_any_element();
            kind_count(icon, group.len(), &t)
        });
        words.push(
            div()
                .ml(SPACE_1)
                .flex()
                .items_center()
                .gap(SPACE_3)
                .children(subagent.into_iter().chain(tasks))
                .into_any_element(),
        );
    }
    let stoppable: Vec<&AsyncTask> = (tasks.iter())
        .filter(|task| task.can_stop && !chat.stopping.contains(&task.async_task_id))
        .collect();
    let action = (stoppable.len() > 1).then(|| stop_all(&agent.id, &stoppable, &t, cx));
    let mut rows: Vec<AnyElement> = Vec::new();
    rows.extend(
        subagents
            .iter()
            .map(|card| subagent_row(agent, card, chat, &t, &tints, cx)),
    );
    for (kind, group) in &kinds {
        for task in group {
            rows.push(task_row_of(agent, kind, task, chat, &t, &tints, cx));
            rows.extend(
                task.agents
                    .iter()
                    .map(|worker| workflow_agent(agent, worker, &t, &tints)),
            );
        }
    }
    Some(Group {
        key: RUNNING,
        words,
        action,
        rows,
    })
}

/// How far the dock runs on under the composer's top edge.
const DOCK_TUCK: gpui_kit::Pixels = SPACE_3H;

/// A kind of running work in the folded header: its mark and how many.
fn kind_count(mark: AnyElement, count: usize, t: &Tokens) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(mark)
        .child(
            div()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary)
                .child(count.to_string()),
        )
        .into_any_element()
}

/// Stops every task that can be stopped, as each row's own stop does.
fn stop_all(
    agent: &str,
    tasks: &[&AsyncTask],
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let agent = agent.to_owned();
    let ids: Vec<String> = tasks
        .iter()
        .map(|task| task.async_task_id.clone())
        .collect();
    let (ink, hover) = (t.text_tertiary, t.text_primary);
    BaseButton::new("dock-stop-all")
        .accessibility_label("Stop all background tasks")
        .h(px(22.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .rounded(theme::RADIUS_1)
        .text_size(theme::ui_text(TEXT_2XS_H))
        .text_color(ink)
        .hover(move |button| button.text_color(hover))
        .on_click(cx.listener(move |bench, _, _, cx| {
            if let Some(chat) = bench.chats.open.get_mut(&agent) {
                chat.stopping.extend(ids.iter().cloned());
            }
            bench.parts.redraw_stage(cx);
            for id in &ids {
                bench.ask_chat(ChatAsk::StopTask {
                    agent: agent.clone(),
                    task: id.clone(),
                });
            }
        }))
        .child("Stop all")
        .into_any_element()
}

/// A row of running work: its mark, its name, what it is doing, then what it has cost and its end.
fn work_row(
    t: &Tokens,
    mark: AnyElement,
    name: String,
    detail: Option<(AnyElement, bool)>,
    meta: Option<String>,
    end: Option<AnyElement>,
) -> Div {
    div()
        .min_w_0()
        .h(px(30.))
        .pl(SPACE_2H)
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .rounded(theme::RADIUS_2)
        .hover(|row| row.bg(t.surface_hover))
        .child(div().flex_none().flex().child(mark))
        .child(
            ui::text(name, TEXT_XS_H, FontWeight::MEDIUM)
                .min_w_0()
                .truncate()
                .text_color(t.text_primary)
                .map(|name| match detail.is_some() {
                    true => name.flex_shrink_0().max_w(gpui_kit::relative(0.5)),
                    false => name.flex_1(),
                }),
        )
        .children(detail.is_some().then(|| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .children(detail.map(|(detail, command)| {
                    div()
                        .truncate()
                        .when(command, |detail| detail.font_family(theme::MONO))
                        .child(detail)
                }))
        }))
        .children(meta.map(|meta| {
            div()
                .flex_none()
                .font_features(theme::tabular())
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(meta)
        }))
        .child(
            div()
                .min_w(px(18.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .gap(SPACE_1H)
                .children(end),
        )
}

fn subagent_row(
    agent: &AgentRow,
    card: &SubagentCard,
    chat: &Chat,
    t: &Tokens,
    tints: &Tints,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let activity = match &card.last_tool {
        Some(tool) => format!("{} {}", word(&tool.row), tool.row.target)
            .trim()
            .to_owned(),
        None => one_line(&card.task),
    };
    let calls = (card.calls > 0).then(|| {
        format!(
            "{} {}",
            card.calls,
            if card.calls == 1 { "call" } else { "calls" }
        )
    });
    let end = if card.can_stop {
        let stopping = chat.stopping.contains(&card.session_id);
        stop(&agent.id, &card.session_id, &card.name, stopping, t, cx)
    } else {
        ui::icon(Icon::Hourglass, 11.)
            .text_color(tints.agent)
            .into_any_element()
    };
    let activity = shimmer(
        SharedString::from(format!("subagent-{}", card.session_id)),
        activity,
    );
    work_row(
        t,
        ui::mark(&agent.provider, 13.),
        card.name.clone(),
        Some((activity, false)),
        cost([tokens(card.tokens), calls]),
        Some(end),
    )
    .into_any_element()
}

fn task_row_of(
    agent: &AgentRow,
    kind: &str,
    task: &AsyncTask,
    chat: &Chat,
    t: &Tokens,
    tints: &Tints,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let tint = if task.state == TaskState::Paused {
        t.text_disabled
    } else {
        tints.task
    };
    let stopping = chat.stopping.contains(&task.async_task_id);
    let stop = task
        .can_stop
        .then(|| stop(&agent.id, &task.async_task_id, &task.name, stopping, t, cx));
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
    let used = task
        .usage
        .as_ref()
        .and_then(|usage| tokens(Some(usage.total_tokens)));
    let mark = ui::icon(task_icon(kind), 12.)
        .text_color(tint)
        .into_any_element();
    let detail = task_detail(task).map(|detail| {
        let detail = match task.state {
            TaskState::Running => shimmer(
                SharedString::from(format!("task-{}", task.async_task_id)),
                detail,
            ),
            _ => detail.into_any_element(),
        };
        (detail, matches!(kind, "shell" | "monitor"))
    });
    work_row(
        t,
        mark,
        one_line(&task.name),
        detail,
        cost([agents.or(used), elapsed(task)]),
        stop,
    )
    .into_any_element()
}

fn stop(
    agent: &str,
    id: &str,
    name: &str,
    stopping: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let (agent, id, danger) = (agent.to_owned(), id.to_owned(), t.danger);
    BaseButton::new(SharedString::from(format!("stop:{id}")))
        .accessibility_label(format!("Stop {name}"))
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
            if let Some(chat) = bench.chats.open.get_mut(&agent) {
                chat.stopping.insert(id.clone());
            }
            bench.parts.redraw_stage(cx);
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
        .child(ui::mark(&agent.provider, 13.))
        .into_any_element();
    work_row(
        t,
        mark,
        worker.label.clone(),
        detail.map(|detail| (detail.into_any_element(), false)),
        cost([tokens(worker.tokens), calls]),
        end.map(IntoElement::into_any_element),
    )
    .pl(px(33.))
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

fn agent_label(provider: &str) -> &str {
    sikemux_core::agent_catalog::agent(provider).map_or(provider, |agent| agent.label)
}

/// What the chat's next message hands to the agent that took it over, or that starts it as a
/// fork, with a button to send it without.
fn handoff(agent: &AgentRow, chat: &Chat, cx: &mut Context<Workbench>) -> Option<Group> {
    let handoff = chat.state.as_ref()?.handoff.as_ref()?;
    let t = Tokens::get(cx).clone();
    let mut said = Vec::new();
    if handoff.request {
        said.push("first request".to_owned());
    }
    said.push(match handoff.exchanges {
        1 => "the latest exchange".to_owned(),
        count => format!("{count} latest exchanges"),
    });
    match handoff.commands {
        0 => {}
        1 => said.push("1 command".to_owned()),
        count => said.push(format!("{count} commands")),
    }
    said.push(format!("~{}k tokens", handoff.tokens.div_ceil(1000)));
    let lead = match handoff.forked_from {
        Some(_) => format!("Forked from a {} chat", agent_label(&handoff.from)),
        None => format!("Hands over to {}", agent_label(&agent.provider)),
    };
    let marks = div()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .child(ui::mark(&handoff.from, 12.))
        .when(handoff.forked_from.is_none(), |marks| {
            marks
                .child(div().text_color(t.text_disabled).child("→"))
                .child(ui::mark(&agent.provider, 12.))
        })
        .into_any_element();
    let words = vec![
        counted(marks, lead, &t),
        div()
            .text_color(t.text_tertiary)
            .child(said.join(" · "))
            .into_any_element(),
    ];
    let id = agent.id.clone();
    let leave_out = BaseButton::new(SharedString::from(format!("handoff-leave:{id}")))
        .h(px(20.))
        .px(px(7.))
        .flex()
        .items_center()
        .border_1()
        .border_color(t.line)
        .rounded(theme::RADIUS_1)
        .hover(|button| button.bg(t.surface_hover))
        .text_size(crate::theme::ui_text(TEXT_3XS_H))
        .text_color(t.text_secondary)
        .on_click(cx.listener(move |bench, _, _, cx| {
            bench.drop_handoff(&id);
            cx.stop_propagation();
        }))
        .child("Leave out")
        .into_any_element();
    Some(Group {
        key: HANDOFF,
        words,
        action: Some(leave_out),
        rows: Vec::new(),
    })
}

/// Messages the person sent while a turn runs, waiting for it to end; while it
/// takes words mid-turn, each can go into it now, or all at once.
fn queued(agent: &str, chat: &Chat, cx: &mut Context<Workbench>) -> Option<Group> {
    let head = chat.state.as_ref()?;
    let waiting = &head.queued;
    if waiting.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let live = Tints::new(&t).task;
    let steerable = head.running && head.capabilities.steering;
    let steer = |ids: Vec<String>, label: &str, keys: bool, cx: &mut Context<Workbench>| {
        let agent = agent.to_owned();
        let id = SharedString::from(format!("steer:{}", ids.join(",")));
        tinted_button(id, label, live, &t)
            .on_click(cx.listener(move |bench, _, _, _| {
                bench
                    .chat_target(&agent)
                    .call_typed::<call::SteerQueued>(QueuedParams {
                        agent_id: agent.clone(),
                        ids: ids.clone(),
                        message: None,
                    });
            }))
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
    let action = (steerable && waiting.len() > 1).then(|| steer(all, "Steer all", true, cx));
    let arrow = ui::icon(Icon::ArrowUp, 11.)
        .text_color(t.text_tertiary)
        .into_any_element();
    let words = vec![counted(arrow, format!("{} queued", waiting.len()), &t)];
    let rows: Vec<AnyElement> = waiting
        .iter()
        .map(|message| {
            let steer_one =
                steerable.then(|| steer(vec![message.id.clone()], "Steer", waiting.len() == 1, cx));
            let (agent, id) = (agent.to_owned(), message.id.clone());
            let label = format!("Drop {} from the queue", queued_label(message));
            let drop = remove_button(SharedString::from(format!("drop:{id}")), label, &t)
                .on_click(cx.listener(move |bench, _, _, _| {
                    bench
                        .chat_target(&agent)
                        .call_typed::<call::DropQueued>(QueuedParams {
                            agent_id: agent.clone(),
                            ids: vec![id.clone()],
                            message: None,
                        });
                }))
                .into_any_element();
            let icon = match message.text.is_empty() && !message.paths.is_empty() {
                true => Icon::Image,
                false => Icon::Message,
            };
            let mark = ui::icon(icon, 12.)
                .text_color(t.text_tertiary)
                .into_any_element();
            let end = div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .children(steer_one)
                .child(drop)
                .into_any_element();
            work_row(
                &t,
                mark,
                one_line(&queued_label(message)),
                None,
                None,
                Some(end),
            )
            .into_any_element()
        })
        .collect();
    Some(Group {
        key: QUEUED,
        words,
        action,
        rows,
    })
}

/// A small button of the dock, washed in `tint`.
fn tinted_button(id: SharedString, label: &str, tint: Hsla, t: &Tokens) -> BaseButton {
    BaseButton::new(id)
        .h(px(20.))
        .px(px(7.))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .border_1()
        .border_color(t.line)
        .rounded(theme::RADIUS_1)
        .bg(Hsla { a: 0.08, ..tint })
        .hover(move |button| button.bg(Hsla { a: 0.18, ..tint }))
        .text_size(crate::theme::ui_text(TEXT_3XS_H))
        .text_color(t.text_primary)
        .child(label.to_owned())
}

/// The cross that takes a row out of the dock.
fn remove_button(id: SharedString, label: String, t: &Tokens) -> BaseButton {
    let danger = t.danger;
    BaseButton::new(id)
        .accessibility_label(label)
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_1)
        .text_color(t.text_disabled)
        .hover(move |remove| remove.bg(Hsla { a: 0.16, ..danger }).text_color(danger))
        .child(ui::icon(Icon::Close, 10.).text_color(t.text_disabled))
}

/// What viewers of the chat's share link wrote, waiting for the person to send each to the
/// agent or dismiss it.
fn guests(agent: &str, chat: &Chat, cx: &mut Context<Workbench>) -> Option<Group> {
    let waiting = &chat.state.as_ref()?.guests;
    if waiting.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let accent = t.accent;
    let icon = ui::icon(Icon::Link, 11.)
        .text_color(t.text_tertiary)
        .into_any_element();
    let words = vec![counted(icon, format!("{} from guests", waiting.len()), &t)];
    let rows = waiting
        .iter()
        .map(|message| {
            let params = GuestMessageParams {
                agent_id: agent.to_owned(),
                id: message.id.clone(),
            };
            let (agent, sending) = (agent.to_owned(), params.clone());
            let send = tinted_button(
                SharedString::from(format!("guest-send:{}", message.id)),
                "Send",
                accent,
                &t,
            )
            .on_click(cx.listener(move |bench, _, _, _| {
                bench
                    .chat_target(&agent)
                    .call_typed::<call::SendGuestMessage>(sending.clone());
            }));
            let (agent, dismissing) = (params.agent_id.clone(), params);
            let dismiss = remove_button(
                SharedString::from(format!("guest-dismiss:{}", message.id)),
                "Dismiss the guest's message".to_owned(),
                &t,
            )
            .on_click(cx.listener(move |bench, _, _, _| {
                bench
                    .chat_target(&agent)
                    .call_typed::<call::DismissGuestMessage>(dismissing.clone());
            }));
            let mark = ui::icon(Icon::Message, 12.)
                .text_color(t.text_tertiary)
                .into_any_element();
            let end = div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(send)
                .child(dismiss)
                .into_any_element();
            let who = message.name.clone().unwrap_or_else(|| "Guest".to_owned());
            let text = div().child(one_line(&message.text)).into_any_element();
            work_row(&t, mark, who, Some((text, false)), None, Some(end)).into_any_element()
        })
        .collect();
    Some(Group {
        key: GUESTS,
        words,
        action: None,
        rows,
    })
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// How often a trigger fires, in a few words; none for one that fires once.
fn repeats(when: &TriggerWhen) -> Option<String> {
    match when {
        TriggerWhen::Once { .. } | TriggerWhen::Webhook { .. } | TriggerWhen::Ci { .. } => None,
        TriggerWhen::Every { seconds } => Some(match seconds {
            s if s % 86_400 == 0 => format!(
                "every {} {}",
                s / 86_400,
                plural("day", (s / 86_400) as usize)
            ),
            s if s % 3_600 == 0 => format!("every {} h", s / 3_600),
            s => format!("every {} min", s.div_ceil(60)),
        }),
        TriggerWhen::Daily { weekdays, .. } => Some(match weekdays.as_slice() {
            [] => "daily".to_owned(),
            [1, 2, 3, 4, 5] => "weekdays".to_owned(),
            days => days
                .iter()
                .filter_map(|day| WEEKDAYS.get(usize::from(*day)))
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
        }),
    }
}

/// When the trigger fires: on each request for a webhook, else how often and when next.
pub(crate) fn fires(trigger: &ChatTrigger) -> String {
    match &trigger.when {
        TriggerWhen::Webhook { .. } => return "on each request".to_owned(),
        TriggerWhen::Ci { on } => return format!("when CI on {on} ends"),
        _ => {}
    }
    let next = trigger
        .next_at
        .and_then(|at| i64::try_from(at).ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(crate::agents::limits::day_and_time)
        .unwrap_or_default();
    match repeats(&trigger.when) {
        Some(repeats) => format!("{repeats} · next {next}"),
        None => next,
    }
}

/// The triggers the agent set on its chat, each with when it next sends the chat its
/// prompt, which the person can remove.
fn triggers(agent: &str, chat: &Chat, cx: &mut Context<Workbench>) -> Option<Group> {
    let set = &chat.state.as_ref()?.triggers;
    if set.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let clock = |size: f32| {
        ui::icon(Icon::Timer, size)
            .text_color(t.warn)
            .into_any_element()
    };
    let words = vec![counted(
        clock(11.),
        format!("{} {}", set.len(), plural("trigger", set.len())),
        &t,
    )];
    let rows: Vec<AnyElement> = set
        .iter()
        .map(|trigger| {
            let (agent, id) = (agent.to_owned(), trigger.id);
            let (copy, remove) = trigger_buttons(
                trigger,
                &t,
                cx.listener(move |bench, _, _, _| {
                    bench
                        .chat_target(&agent)
                        .call_typed::<call::RemoveTrigger>(TriggerParams { trigger: id });
                }),
            );
            let end = div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .children(copy)
                .child(remove)
                .into_any_element();
            work_row(
                &t,
                clock(12.),
                trigger.title.clone(),
                None,
                Some(fires(trigger)),
                Some(end),
            )
            .into_any_element()
        })
        .collect();
    Some(Group {
        key: TRIGGERS,
        words,
        action: None,
        rows,
    })
}

/// A trigger's buttons: one that copies a webhook's address, and one that removes it.
fn trigger_buttons(
    trigger: &ChatTrigger,
    t: &Tokens,
    remove: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> (Option<BaseButton>, BaseButton) {
    let danger = t.danger;
    let remove = BaseButton::new(SharedString::from(format!("trigger-remove:{}", trigger.id)))
        .accessibility_label(format!("Remove trigger {}", trigger.title))
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_1)
        .text_color(t.text_disabled)
        .hover(move |remove| remove.bg(Hsla { a: 0.16, ..danger }).text_color(danger))
        .on_click(remove)
        .child(ui::icon(Icon::Close, 10.).text_color(t.text_disabled));
    let copy = match &trigger.when {
        TriggerWhen::Webhook { url } => {
            let url = url.clone();
            Some(
                BaseButton::new(SharedString::from(format!("trigger-copy:{}", trigger.id)))
                    .accessibility_label(format!("Copy the address of trigger {}", trigger.title))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_1)
                    .hover(|copy| copy.bg(t.surface_hover))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.clone()))
                    })
                    .child(ui::icon(Icon::Copy, 11.).text_color(t.text_disabled)),
            )
        }
        _ => None,
    };
    (copy, remove)
}

/// One trigger: what it is for, when it next fires, and a button that removes it.
pub fn trigger_row(
    trigger: &ChatTrigger,
    t: &Tokens,
    remove: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let (copy, remove) = trigger_buttons(trigger, t, remove);
    div()
        .min_w_0()
        .py(SPACE_1)
        .pl(px(11.))
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(px(7.))
        .child(ui::icon(Icon::Timer, 12.).flex_none().text_color(t.cmd))
        .child(
            div()
                .flex_auto()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(trigger.title.clone()),
        )
        .child({
            let mut when = div()
                .min_w_0()
                .truncate()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_3XS))
                .text_color(t.text_disabled)
                .child(fires(trigger));
            when.style().flex_shrink = Some(4.);
            when
        })
        .children(copy)
        .child(remove)
        .into_any_element()
}
