//! The desk's Tasks view: the project's one list of tasks as the core sorts it, a section each
//! for the calls waiting on the person, finished work, work moving, work nobody holds, dropped
//! work and finished work already opened.

use std::collections::HashSet;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
};
use sikemux_calls::board::{BoardParams, BoardUpdateParams};
use sikemux_calls::call;
use sikemux_view::task_list::{ProjectTasks, TaskFact, TaskOf, TaskRow, TaskSection};
use sikemux_view::workspace::PullChecks;

use crate::core::Target;
use crate::icons::Icon;
use crate::reads::Read;
use crate::theme::{self, Tokens, SPACE_2};
use crate::toast;
use crate::ui::{self, Clicks};
use crate::workbench::Workbench;

const COLUMN: f32 = 720.;
const SIDE: f32 = 40.;
/// A section shows this many rows until it is unfolded, once it holds more than one over.
const FOLDED: usize = 5;

/// What one desk's Tasks view keeps between draws.
#[derive(Default)]
pub(crate) struct View {
    scroll: ScrollHandle,
    /// Sections drawn whole; Opened starts shut and the rest show their first rows.
    unfolded: HashSet<TaskSection>,
}

pub(crate) fn watch(target: &Target, folder: &str) {
    target.watch_folder::<call::ProjectTasks>(
        Read::TaskList,
        BoardParams {
            project: folder.to_owned(),
        },
        folder.to_owned(),
    );
}

/// Reads the list again, as after the person's calls moved.
pub(crate) fn reread(bench: &Workbench) {
    if let Some(project) = bench.project() {
        watch(&bench.target(), &project.path.to_string_lossy());
    }
}

/// The list of the project of agent `agent`'s desk, once the core has said.
pub(crate) fn of_desk<'a>(bench: &'a Workbench, agent: &str) -> Option<&'a ProjectTasks> {
    let folder = crate::calls::project_of(bench, agent)?;
    (bench.feeds.at(&bench.place())?.task_list.as_ref()).filter(|list| list.project == folder)
}

fn count(list: &ProjectTasks, section: TaskSection) -> usize {
    (list.rows.iter())
        .filter(|row| row.section == section)
        .count()
}

/// The sections that hold any task, in the order they are drawn.
fn sections(list: &ProjectTasks) -> Vec<(TaskSection, Vec<&TaskRow>)> {
    let mut sections: Vec<(TaskSection, Vec<&TaskRow>)> = Vec::new();
    for row in &list.rows {
        match sections.last_mut() {
            Some((section, rows)) if *section == row.section => rows.push(row),
            _ => sections.push((row.section, vec![row])),
        }
    }
    sections
}

fn words(section: TaskSection) -> &'static str {
    match section {
        TaskSection::ForYou => t!("For you"),
        TaskSection::Finished => t!("Finished"),
        TaskSection::Moving => t!("Moving"),
        TaskSection::Queued => t!("Queued"),
        TaskSection::Dropped => t!("Dropped"),
        TaskSection::Opened => t!("Opened"),
    }
}

/// Turns agent `agent`'s desk to its tasks with `section` at the top.
pub(crate) fn show_section(
    bench: &mut Workbench,
    agent: &str,
    section: TaskSection,
    cx: &mut Context<Workbench>,
) {
    let at = (of_desk(bench, agent))
        .and_then(|list| sections(list).iter().position(|(it, _)| *it == section));
    bench.desks.update(cx, |desks, _| {
        let view = desks.task_views.entry(agent.to_owned()).or_default();
        if let Some(at) = at {
            view.scroll.scroll_to_item(at);
        }
    });
    bench.show_tasks(agent, cx);
}

/// The counts that head the view, each of which brings its section to the top.
pub(crate) fn counts(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Div {
    let t = Tokens::get(cx).clone();
    let handle = cx.weak_entity();
    let list = of_desk(bench, agent);
    let jumps = [
        (TaskSection::ForYou, t!("for you")),
        (TaskSection::Finished, t!("finished")),
        (TaskSection::Dropped, t!("dropped")),
    ]
    .into_iter()
    .filter_map(|(section, words)| {
        let count = count(list?, section);
        (count > 0).then_some((section, words, count))
    })
    .map(|(section, words, count)| {
        let (handle, agent) = (handle.clone(), agent.to_owned());
        div()
            .id(SharedString::from(format!("tasks-jump-{section:?}")))
            .flex()
            .gap(px(5.))
            .cursor_pointer()
            .hover(|jump| jump.text_color(t.text_secondary))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_secondary)
                    .font_features(theme::tabular())
                    .child(count.to_string()),
            )
            .child(words)
            .clicks(move |_, _, cx| {
                let _ = handle.update(cx, |bench, cx| show_section(bench, &agent, section, cx));
            })
    });
    div()
        .flex()
        .items_center()
        .gap(theme::SPACE_3)
        .whitespace_nowrap()
        .text_size(theme::ui_text(theme::TEXT_XS_H))
        .text_color(t.text_tertiary)
        .children(jumps)
}

/// The Calls page's foot: how much finished and dropped, each opening the tasks at its section.
pub(crate) fn calls_foot(
    bench: &WeakEntity<Workbench>,
    agent: &str,
    cx: &App,
) -> Option<AnyElement> {
    let strong = bench.upgrade()?;
    let list = of_desk(strong.read(cx), agent)?;
    let t = Tokens::get(cx).clone();
    let parts: Vec<AnyElement> = [TaskSection::Finished, TaskSection::Dropped]
        .into_iter()
        .map(|section| (section, count(list, section)))
        .filter(|(_, count)| *count > 0)
        .map(|(section, count)| {
            let (bench, agent) = (bench.clone(), agent.to_owned());
            div()
                .id(SharedString::from(format!("calls-foot-{section:?}")))
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .py(px(3.))
                .mx(px(-6.))
                .my(px(-3.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|part| part.bg(t.surface_hover))
                .child(ui::caps(words(section), cx))
                .child(
                    ui::caps(&count.to_string(), cx)
                        .text_color(t.text_primary)
                        .font_features(theme::tabular()),
                )
                .clicks(move |_, _, cx| {
                    let _ = bench.update(cx, |bench, cx| show_section(bench, &agent, section, cx));
                })
                .into_any_element()
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut foot = div().flex().justify_center().items_center().gap(SPACE_2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            foot = foot.child(ui::caps("·", cx).text_color(t.text_disabled));
        }
        foot = foot.child(part);
    }
    Some(foot.into_any_element())
}

/// The list itself, a column of its sections.
pub(crate) fn body(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(list) = of_desk(bench, agent) else {
        return div().into_any_element();
    };
    if list.rows.is_empty() {
        return div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACE_2)
            .child(ui::icon(Icon::ListTodo, 18.).text_color(t.text_disabled))
            .child(ui::meta(t!("No tasks yet"), cx))
            .into_any_element();
    }
    let (scroll, unfolded) = bench.desks.update(cx, |desks, _| {
        let view = desks.task_views.entry(agent.to_owned()).or_default();
        (view.scroll.clone(), view.unfolded.clone())
    });
    let handle = cx.weak_entity();
    let sections = sections(list);
    let last = sections.len().saturating_sub(1);
    div()
        .id(SharedString::from(format!("tasks-{agent}")))
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .pt(px(24.))
        .pb(px(32.))
        .px(px(SIDE))
        .children(
            sections
                .into_iter()
                .enumerate()
                .map(|(at, (section, rows))| {
                    let open = unfolded.contains(&section);
                    div()
                        .w_full()
                        .max_w(px(COLUMN))
                        .mx_auto()
                        .when(at > 0, |it| it.pt(px(22.)))
                        .when(at == last, |it| it.pb(px(8.)))
                        .child(match section {
                            TaskSection::Opened if !open => {
                                opened_shut(agent, rows.len(), &handle, cx)
                            }
                            _ => cap(section, rows.len(), &t, cx),
                        })
                        .when(section != TaskSection::Opened || open, |it| {
                            it.child(rows_of(bench, agent, section, &rows, open, &handle, cx))
                        })
                }),
        )
        .into_any_element()
}

fn cap(section: TaskSection, count: usize, t: &Tokens, cx: &App) -> Div {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(words(section), cx))
        .child(
            ui::caps(&count.to_string(), cx)
                .text_color(t.text_disabled)
                .font_features(theme::tabular()),
        )
        .child(div().flex_1().h(px(1.)).bg(t.line))
}

/// Opened while shut: one line that opens it.
fn opened_shut(agent: &str, count: usize, handle: &WeakEntity<Workbench>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div().child(
        div()
            .id("tasks-opened")
            .h(px(22.))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .cursor_pointer()
            .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_disabled))
            .child(ui::caps(words(TaskSection::Opened), cx))
            .child(
                ui::caps(&count.to_string(), cx)
                    .text_color(t.text_disabled)
                    .font_features(theme::tabular()),
            )
            .child(
                div()
                    .text_size(theme::ui_text(theme::TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(t!("finished work you have opened")),
            )
            .clicks(unfold(handle, agent, TaskSection::Opened)),
    )
}

fn unfold(
    handle: &WeakEntity<Workbench>,
    agent: &str,
    section: TaskSection,
) -> impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut App) + 'static {
    let (handle, agent) = (handle.clone(), agent.to_owned());
    move |_, _, cx| {
        let _ = handle.update(cx, |bench, cx| {
            bench.desks.update(cx, |desks, _| {
                let view = desks.task_views.entry(agent.clone()).or_default();
                view.unfolded.insert(section);
            });
            bench.parts.redraw_stage(cx);
        });
    }
}

fn rows_of(
    bench: &Workbench,
    agent: &str,
    section: TaskSection,
    rows: &[&TaskRow],
    open: bool,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    let folds = !open && rows.len() > FOLDED + 1;
    let shown = if folds { FOLDED } else { rows.len() };
    div()
        .mt(px(6.))
        .mx(px(-6.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(
            (rows.iter())
                .take(shown)
                .map(|row| row_of(bench, agent, row, handle, cx)),
        )
        .when(folds, |it| {
            it.child(
                div()
                    .id(SharedString::from(format!("tasks-more-{section:?}")))
                    .h(px(26.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|more| more.bg(t.surface_hover))
                    .text_size(theme::ui_text(theme::TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(ui::icon(Icon::Chevron, 12.).text_color(t.text_disabled))
                    .child(t!("{count} more", count = rows.len() - FOLDED))
                    .clicks(unfold(handle, agent, section)),
            )
        })
}

fn mark(row: &TaskRow, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let icon = match (&row.fact, row.section) {
        (Some(TaskFact::Call { call, .. }), _) => return crate::calls::kind_mark(*call, cx),
        (_, TaskSection::Moving) => Icon::CircleMoving,
        (_, TaskSection::Queued) => Icon::CircleDashed,
        (_, TaskSection::Dropped) => Icon::CircleSlash,
        (Some(TaskFact::Pull { .. }), _) => Icon::PullRequest,
        (Some(TaskFact::Landed { .. } | TaskFact::Commits { .. }), _) => Icon::Commit,
        _ => Icon::Window,
    };
    ui::icon(icon, 12.)
        .text_color(t.text_tertiary)
        .into_any_element()
}

fn checks_words(checks: Option<PullChecks>, pull: bool) -> Option<&'static str> {
    Some(match (checks?, pull) {
        (PullChecks::Pending, true) => t!("checks running"),
        (PullChecks::Passing, true) => t!("checks passed"),
        (PullChecks::Failing, true) => t!("checks failed"),
        (PullChecks::Pending, false) => t!("CI running"),
        (PullChecks::Passing, false) => t!("CI green"),
        (PullChecks::Failing, false) => t!("CI red"),
    })
}

/// The muted line beside a row's title, saying what the core checked of it.
fn fact_words(row: &TaskRow) -> String {
    if let Some(dropped) = &row.dropped {
        return match &dropped.instead {
            Some(instead) => t!("superseded by {title}", title = instead),
            None => dropped.why.clone(),
        };
    }
    let with = |head: String, checks: Option<&str>| match checks {
        Some(checks) => format!("{head} · {checks}"),
        None => head,
    };
    match &row.fact {
        None => String::new(),
        Some(TaskFact::Landed { sha, checks }) => with(
            t!("landed {sha}", sha = sha.get(..7).unwrap_or(sha)),
            checks_words(*checks, false),
        ),
        Some(TaskFact::Pull { number, checks }) => with(
            t!("PR #{number}", number = number),
            checks_words(*checks, true),
        ),
        Some(TaskFact::Commits { count, checks }) => with(
            match count {
                1 => t!("1 commit").to_owned(),
                count => t!("{count} commits", count = count),
            },
            checks_words(*checks, false),
        ),
        Some(TaskFact::NoCommits) => t!("no commits yet").to_owned(),
        Some(TaskFact::NobodySince { at }) => {
            t!("nobody since {time}", time = ui::time::clock(*at))
        }
        Some(TaskFact::Call {
            choices,
            page,
            pictures,
            ..
        }) => {
            let mut parts = Vec::new();
            if *page {
                parts.push(t!("page").to_owned());
            }
            match (pictures, choices) {
                (0, 0) => {}
                (0, 1) => parts.push(t!("1 choice").to_owned()),
                (0, choices) => parts.push(t!("{count} choices", count = choices)),
                (1, _) => parts.push(t!("1 picture").to_owned()),
                (pictures, _) => parts.push(t!("{count} pictures", count = pictures)),
            }
            parts.join(" · ")
        }
    }
}

fn row_of(
    bench: &Workbench,
    agent: &str,
    row: &TaskRow,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = match &row.of {
        TaskOf::Task { id } => format!("task-{id}"),
        TaskOf::Call { id } => format!("task-call-{id}"),
    };
    let holder = row.holder.as_ref().map(|holder| {
        let mark = match bench.agent_by_id(&holder.agent_id) {
            Some(agent) => agent.mark(12., cx),
            None => ui::icon(Icon::Agent, 12.).into_any_element(),
        };
        crate::chat::agent_pill_of(
            &holder.agent_id,
            &holder.name,
            mark,
            theme::TEXT_2XS_H,
            bench.target(),
            handle,
            cx,
        )
    });
    let undo = match (&row.of, row.section) {
        (TaskOf::Task { id }, TaskSection::Dropped) => {
            let (handle, id) = (handle.clone(), *id);
            Some(
                ui::Button::new(
                    SharedString::from(format!("{key}-undo")),
                    t!("Undo"),
                    ui::Variant::Outline,
                )
                .sized(20., 7., theme::TEXT_2XS_H)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = handle.update(cx, |bench, cx| undrop(bench, id, cx));
                }),
            )
        }
        _ => None,
    };
    let opens = {
        let (handle, agent, row) = (handle.clone(), agent.to_owned(), row.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let _ = handle.update(cx, |bench, cx| open(bench, &agent, &row, cx));
        }
    };
    div()
        .id(SharedString::from(key))
        .h(px(30.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(px(5.))
        .whitespace_nowrap()
        .text_size(theme::ui_text(theme::TEXT_SM))
        .cursor_pointer()
        .hover(|it| it.bg(t.surface_hover))
        .child(mark(row, cx))
        .child(
            div()
                .min_w_0()
                .max_w(relative(0.58))
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(row.title.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_tertiary)
                .child(fact_words(row)),
        )
        .children(holder)
        .children(undo)
        .child(
            div()
                .flex_none()
                .min_w(px(26.))
                .flex()
                .justify_end()
                .text_size(theme::ui_text(theme::TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .font_features(theme::tabular())
                .child(ui::time::age(row.since)),
        )
        .clicks(opens)
        .into_any_element()
}

fn update(project: String, id: u64) -> BoardUpdateParams {
    BoardUpdateParams {
        project,
        id,
        title: None,
        details: None,
        acceptance: None,
        promise: None,
        asserts: None,
        status: None,
        result: None,
        depends_on: None,
        parent: None,
        undrop: false,
        opened: false,
    }
}

fn folder(bench: &Workbench) -> Option<String> {
    (bench.project()).map(|project| project.path.to_string_lossy().into_owned())
}

fn undrop(bench: &mut Workbench, id: u64, cx: &mut Context<Workbench>) {
    let Some(project) = folder(bench) else {
        return;
    };
    let asked = (bench.target()).request_typed::<call::BoardUpdate>(BoardUpdateParams {
        undrop: true,
        ..update(project, id)
    });
    bench.report(
        asked,
        t!("It was not put back"),
        |bench, _, cx| bench.notify(toast::success(t!("Put back")), cx),
        cx,
    );
}

/// Opens what a row stands for: a call on the Calls page, a task the person asked on its sheet,
/// else the chat of the agent holding it. Finished work counts as opened from then on.
fn open(bench: &mut Workbench, agent: &str, row: &TaskRow, cx: &mut Context<Workbench>) {
    let id = match &row.of {
        TaskOf::Call { id } => {
            bench.calls.update(cx, |cards, cx| cards.open(id, cx));
            return bench.show_calls(agent, cx);
        }
        TaskOf::Task { id } => *id,
    };
    if row.section == TaskSection::Finished {
        if let Some(project) = folder(bench) {
            (bench.target()).call_typed::<call::BoardUpdate>(BoardUpdateParams {
                opened: true,
                ..update(project, id)
            });
        }
    }
    if row.asked {
        if let Some(project) = bench.project().map(|project| project.id.clone()) {
            return bench.open_sheet(&project, id, cx);
        }
    }
    if let Some(holder) = &row.holder {
        bench.open_agent(&holder.agent_id, cx);
    }
}
