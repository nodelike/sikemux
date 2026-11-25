//! The desk's Tasks view: the project's one list of tasks as the core sorts it, a section each
//! for the calls waiting on the person, finished work, work moving, work nobody holds, dropped
//! work, what teammates' agents asked about its tasks, and finished work already opened.
//! Teammates' tasks on a board shared with them sit in the same sections, read-only, except For
//! you, which stays the person's own.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
};
use sikemux_calls::board::{BoardParams, BoardUpdateParams, TeammateAskingParams};
use sikemux_calls::call;
use sikemux_view::task_list::{
    AskingOff, ProjectTasks, SharedBy, TaskFact, TaskHolder, TaskOf, TaskRow, TaskSection,
    TeammateAsk, TeammateAsks,
};
use sikemux_view::teammates::Teammate;
use sikemux_view::workspace::PullChecks;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{Ask, Target};
use crate::icons::Icon;
use crate::picture::{Picture, Remote};
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
    /// Only the person's own tasks, leaving out teammates'.
    yours: bool,
    teammates_unfolded: bool,
    /// The teammates' asks drawn open, by when they were asked and who asked.
    open_asks: HashSet<(u64, String)>,
    /// Which of the Share button's labels fits beside the counts, longest first.
    pub(crate) share_fit: Rc<Cell<usize>>,
}

/// One task in the list: the person's own, or one on the board a teammate shares.
#[derive(Clone, Copy)]
struct Line<'a> {
    row: &'a TaskRow,
    by: Option<&'a Teammate>,
}

pub(crate) fn watch(target: &Target, folder: &str) {
    let of_folder = || BoardParams {
        project: folder.to_owned(),
    };
    target.watch_folder::<call::ProjectTasks>(Read::TaskList, of_folder(), folder.to_owned());
    target.watch_folder::<call::ProjectSharing>(Read::Sharing, of_folder(), folder.to_owned());
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

fn yours_only(bench: &Workbench, agent: &str, cx: &App) -> bool {
    (bench.desks.read(cx).task_views.get(agent)).is_some_and(|view| view.yours)
}

/// The person's tasks and, unless `yours`, teammates' in the same order the core sorts the
/// person's: by section, then newest first.
fn lines(list: &ProjectTasks, yours: bool) -> Vec<Line<'_>> {
    let mine = (list.rows.iter()).map(|row| Line { row, by: None });
    let theirs = (list.teammates.iter())
        .filter(|_| !yours)
        .filter(|task| task.row.section != TaskSection::ForYou)
        .map(|task| Line {
            row: &task.row,
            by: Some(&task.person),
        });
    let mut lines: Vec<Line> = mine.chain(theirs).collect();
    if !list.teammates.is_empty() {
        lines.sort_by(|one, other| {
            (one.row.section.cmp(&other.row.section)).then(other.row.since.cmp(&one.row.since))
        });
    }
    lines
}

fn count(lines: &[Line], section: TaskSection) -> usize {
    (lines.iter())
        .filter(|line| line.row.section == section)
        .count()
}

/// The sections that hold any task, in the order they are drawn.
fn sections<'a>(lines: &[Line<'a>]) -> Vec<(TaskSection, Vec<Line<'a>>)> {
    let mut sections: Vec<(TaskSection, Vec<Line>)> = Vec::new();
    for line in lines {
        match sections.last_mut() {
            Some((section, rows)) if *section == line.row.section => rows.push(*line),
            _ => sections.push((line.row.section, vec![*line])),
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

/// How many sections are drawn before the teammates' asks.
fn before_teammates(sections: &[(TaskSection, Vec<Line>)]) -> usize {
    (sections.iter())
        .filter(|(section, _)| *section <= TaskSection::Dropped)
        .count()
}

/// Where among the view's blocks `section` is drawn; the teammates' asks when none.
fn place(list: &ProjectTasks, yours: bool, section: Option<TaskSection>) -> Option<usize> {
    let sections = sections(&lines(list, yours));
    let before = before_teammates(&sections);
    let Some(section) = section else {
        return list.asked.is_some().then_some(before);
    };
    let at = sections.iter().position(|(it, _)| *it == section)?;
    Some(at + usize::from(at >= before && list.asked.is_some()))
}

/// Turns agent `agent`'s desk to its tasks with `section` at the top, or the teammates' asks
/// when none.
pub(crate) fn show_section(
    bench: &mut Workbench,
    agent: &str,
    section: impl Into<Option<TaskSection>>,
    cx: &mut Context<Workbench>,
) {
    let section = section.into();
    let yours = yours_only(bench, agent, cx);
    let at = of_desk(bench, agent).and_then(|list| place(list, yours, section));
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
    let lines = list.map(|list| lines(list, yours_only(bench, agent, cx)));
    let asked = (list.and_then(|list| list.asked.as_ref())).map_or(0, |asks| asks.asks.len());
    let jumps = [
        (Some(TaskSection::ForYou), t!("for you")),
        (Some(TaskSection::Finished), t!("finished")),
        (Some(TaskSection::Dropped), t!("dropped")),
        (None, t!("asked")),
    ]
    .into_iter()
    .filter_map(|(section, words)| {
        let count = match section {
            Some(section) => count(lines.as_deref()?, section),
            None => asked,
        };
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
    let whose = list
        .filter(|list| !list.teammates.is_empty())
        .map(|_| whose(bench, agent, cx));
    div()
        .flex()
        .items_center()
        .gap(theme::SPACE_3)
        .whitespace_nowrap()
        .text_size(theme::ui_text(theme::TEXT_XS_H))
        .text_color(t.text_tertiary)
        .children(whose)
        .children(jumps)
}

/// Everyone's tasks or only the person's own.
fn whose(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let agent = agent.to_owned();
    let yours = yours_only(bench, &agent, cx);
    let choose = cx.listener(move |bench, chosen: &usize, _, cx| {
        bench.desks.update(cx, |desks, _| {
            let view = desks.task_views.entry(agent.clone()).or_default();
            view.yours = *chosen == 1;
            view.scroll.scroll_to_item(0);
        });
        bench.parts.redraw_stage(cx);
    });
    ui::Segmented::words(
        "tasks-whose",
        vec![(t!("Everyone").into(), None), (t!("Yours").into(), None)],
        usize::from(yours),
    )
    .slide()
    .on_select(choose)
    .into_any_element()
}

/// The list itself, a column of its sections.
pub(crate) fn body(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(list) = of_desk(bench, agent) else {
        return div().into_any_element();
    };
    let lines = lines(list, yours_only(bench, agent, cx));
    if lines.is_empty() && list.asked.is_none() {
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
    let (scroll, unfolded, teammates_unfolded, open_asks) = bench.desks.update(cx, |desks, _| {
        let view = desks.task_views.entry(agent.to_owned()).or_default();
        (
            view.scroll.clone(),
            view.unfolded.clone(),
            view.teammates_unfolded,
            view.open_asks.clone(),
        )
    });
    let handle = cx.weak_entity();
    let sections = sections(&lines);
    let before = before_teammates(&sections);
    let mut blocks: Vec<Div> = (sections.into_iter())
        .map(|(section, rows)| {
            let open = unfolded.contains(&section);
            div()
                .child(match section {
                    TaskSection::Opened if !open => opened_shut(agent, rows.len(), &handle, cx),
                    _ => cap(words(section), rows.len(), &t, cx),
                })
                .when(section != TaskSection::Opened || open, |it| {
                    it.child(rows_of(bench, agent, section, &rows, open, &handle, cx))
                })
        })
        .collect();
    if let Some(asks) = &list.asked {
        let block = teammates(
            bench,
            agent,
            asks,
            teammates_unfolded,
            &open_asks,
            &handle,
            cx,
        );
        blocks.insert(before, block);
    }
    let theirs_shown = lines.iter().any(|line| line.by.is_some());
    if let (true, Some(first)) = (theirs_shown, blocks.first_mut()) {
        let notes = (list.shared_by.iter()).map(|org| shared_note(org, &t));
        *first = div()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pb(px(18.))
                    .children(notes),
            )
            .child(std::mem::replace(first, div()));
    }
    let last = blocks.len().saturating_sub(1);
    div()
        .id(SharedString::from(format!("tasks-{agent}")))
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .pt(px(24.))
        .pb(px(32.))
        .px(px(SIDE))
        .children(blocks.into_iter().enumerate().map(|(at, block)| {
            block
                .w_full()
                .max_w(px(COLUMN))
                .mx_auto()
                .when(at > 0, |it| it.pt(px(22.)))
                .when(at == last, |it| it.pb(px(8.)))
        }))
        .into_any_element()
}

fn cap(words: &str, count: usize, t: &Tokens, cx: &App) -> Div {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(words, cx))
        .child(
            ui::caps(&count.to_string(), cx)
                .text_color(t.text_disabled)
                .font_features(theme::tabular()),
        )
        .child(div().flex_1().h(px(1.)).bg(t.line))
}

/// Who shares the project with the person through org `org`, and what the person can do with
/// their tasks.
fn shared_note(org: &SharedBy, t: &Tokens) -> Div {
    let who = match org.people.as_slice() {
        [one] => t!(
            "{one} shares this project with {org}.",
            one = one.name,
            org = org.name
        ),
        [one, two] => t!(
            "{one} and {two} share this project with {org}.",
            one = one.name,
            two = two.name,
            org = org.name
        ),
        people => t!(
            "{count} teammates share this project with {org}.",
            count = people.len(),
            org = org.name
        ),
    };
    let what = match org.can_ask {
        true => t!("Others' tasks are read only; your agents can ask about them."),
        false => t!("Others' tasks are read only."),
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(theme::ui_text(theme::TEXT_XS_H))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::Users, 14.).text_color(t.text_tertiary))
        .child(div().min_w_0().child(format!("{who} {what}")))
}

/// The section of what teammates' agents asked about the project's tasks.
fn teammates(
    bench: &Workbench,
    agent: &str,
    asks: &TeammateAsks,
    unfolded: bool,
    open_asks: &HashSet<(u64, String)>,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    let stop_everyone = asks.off.is_none().then(|| {
        let handle = handle.clone();
        ui::Button::new("teammates-stop", t!("Stop everyone"), ui::Variant::Outline)
            .sized(20., 7., theme::TEXT_2XS_H)
            .on_click(move |_, _, cx| {
                let _ = handle.update(cx, |bench, cx| set_asking(bench, None, true, cx));
            })
    });
    let off = asks.off.map(|off| {
        let (title, why) = match off {
            AskingOff::Stopped => (
                t!("Teammates can't ask about this project"),
                t!("their agents get the board's facts only"),
            ),
            AskingOff::Unpaid => (t!("Asking is off"), t!("the org's Team plan is unpaid")),
        };
        let title = div()
            .min_w_0()
            .truncate()
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text_primary)
            .child(title)
            .into_any_element();
        line(slashed(&t), title, why, &t)
            .when(off == AskingOff::Stopped, |it| {
                it.child(turn_on("teammates-on", None, handle))
            })
            .into_any_element()
    });
    let stopped = (asks.stopped.iter()).map(|person| {
        line(
            slashed(&t),
            teammate_pill(person, None, bench, cx),
            t!("can't ask about this project"),
            &t,
        )
        .child(turn_on(
            &format!("teammates-on-{}", person.member),
            Some(&person.member),
            handle,
        ))
        .into_any_element()
    });
    let folds = !unfolded && asks.asks.len() > FOLDED + 1;
    let shown = if folds { FOLDED } else { asks.asks.len() };
    let rows = (asks.asks.iter()).take(shown).map(|asked| {
        let stopped = asks
            .stopped
            .iter()
            .any(|person| person.member == asked.by.member);
        let open = open_asks.contains(&(asked.at, asked.by.member.clone()));
        ask_row(
            bench,
            agent,
            asked,
            open,
            asks.off.is_none() && !stopped,
            handle,
            cx,
        )
    });
    div()
        .child(cap(t!("Asked by teammates"), asks.asks.len(), &t, cx).children(stop_everyone))
        .child(
            div()
                .mt(px(6.))
                .mx(px(-6.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(off)
                .children(stopped)
                .children(rows)
                .when(folds, |it| {
                    let handle = handle.clone();
                    let agent = agent.to_owned();
                    it.child(
                        more("tasks-more-teammates", asks.asks.len() - FOLDED, &t).clicks(
                            move |_, _, cx| {
                                let _ = handle.update(cx, |bench, cx| {
                                    bench.desks.update(cx, |desks, _| {
                                        let view =
                                            desks.task_views.entry(agent.clone()).or_default();
                                        view.teammates_unfolded = true;
                                    });
                                    bench.parts.redraw_stage(cx);
                                });
                            },
                        ),
                    )
                }),
        )
}

/// A row of the teammates' section that is a state rather than an ask: a mark, a title and a
/// muted line, with its controls after.
fn line(mark: AnyElement, title: AnyElement, why: &str, t: &Tokens) -> Div {
    div()
        .h(px(30.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(9.))
        .whitespace_nowrap()
        .text_size(theme::ui_text(theme::TEXT_SM))
        .child(mark)
        .child(title)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_tertiary)
                .child(why.to_owned()),
        )
}

fn slashed(t: &Tokens) -> AnyElement {
    (ui::icon(Icon::CircleSlash, 12.).text_color(t.text_tertiary)).into_any_element()
}

fn turn_on(key: &str, member: Option<&str>, handle: &WeakEntity<Workbench>) -> ui::Button {
    let (handle, member) = (handle.clone(), member.map(str::to_owned));
    ui::Button::new(
        SharedString::from(key.to_owned()),
        t!("Turn on"),
        ui::Variant::Outline,
    )
    .sized(20., 7., theme::TEXT_2XS_H)
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        let member = member.clone();
        let _ = handle.update(cx, |bench, cx| set_asking(bench, member, false, cx));
    })
}

fn ask_row(
    bench: &Workbench,
    agent: &str,
    asked: &TeammateAsk,
    open: bool,
    stoppable: bool,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = (asked.at, asked.by.member.clone());
    let id = format!("teammate-ask-{}-{}", asked.at, asked.by.member);
    let toggles = {
        let (handle, agent, key) = (handle.clone(), agent.to_owned(), key.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let _ = handle.update(cx, |bench, cx| {
                bench.desks.update(cx, |desks, _| {
                    let view = desks.task_views.entry(agent.clone()).or_default();
                    if !view.open_asks.remove(&key) {
                        view.open_asks.insert(key.clone());
                    }
                });
                bench.parts.redraw_stage(cx);
            });
        }
    };
    let question = div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_primary)
        .child(match open {
            true => asked.question.clone(),
            false => asked.headline.clone(),
        });
    let head = div()
        .id(SharedString::from(id.clone()))
        .min_h(px(30.))
        .px(px(6.))
        .flex()
        .gap(px(9.))
        .rounded(px(5.))
        .text_size(theme::ui_text(theme::TEXT_SM))
        .cursor_pointer()
        .map(|it| match open {
            true => it.py(px(6.)).items_start(),
            false => it
                .items_center()
                .whitespace_nowrap()
                .hover(|it| it.bg(t.surface_hover)),
        })
        .child(
            div()
                .flex_none()
                .h(theme::ui_text(theme::TEXT_SM) * 1.5)
                .flex()
                .items_center()
                .child(ui::icon(Icon::Message, 12.).text_color(t.text_tertiary)),
        )
        .map(|it| match open {
            true => it.child(question.flex_1().min_w_0()),
            false => it
                .child(question.flex_initial().min_w_0().truncate())
                .child(
                    div()
                        .flex_none()
                        .max_w(relative(0.36))
                        .truncate()
                        .text_color(t.text_tertiary)
                        .child(asked.task_title.clone()),
                )
                .child(div().flex_1()),
        })
        .child(teammate_pill(
            &asked.by,
            Some((&asked.agent, asked.agent_cli.as_deref().unwrap_or_default())),
            bench,
            cx,
        ))
        .child(
            div()
                .flex_none()
                .min_w(px(26.))
                .flex()
                .justify_end()
                .text_size(theme::ui_text(theme::TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .font_features(theme::tabular())
                .child(ui::time::age(asked.at)),
        )
        .clicks(toggles);
    if !open {
        return head.into_any_element();
    }
    let source = match &asked.from_work_of {
        Some(name) => t!(
            "on {title} · answered from @{name}'s work",
            title = asked.task_title,
            name = name
        ),
        None => t!(
            "on {title} · nobody held it, so only the board's facts went back",
            title = asked.task_title
        ),
    };
    let stop = stoppable.then(|| {
        let (handle, member) = (handle.clone(), asked.by.member.clone());
        ui::Button::new(
            SharedString::from(format!("{id}-stop")),
            t!("Stop {name}", name = asked.by.name),
            ui::Variant::Ghost,
        )
        .sized(20., 7., theme::TEXT_2XS_H)
        .on_click(move |_, _, cx| {
            let member = member.clone();
            let _ = handle.update(cx, |bench, cx| set_asking(bench, Some(member), true, cx));
        })
    });
    div()
        .flex()
        .flex_col()
        .rounded(px(5.))
        .bg(t.surface_hover)
        .child(head)
        .child(
            div()
                .pl(px(27.))
                .pr(px(6.))
                .pb(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme::ui_text(theme::TEXT_SM))
                .child(div().text_color(t.text_primary).child(asked.answer.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme::ui_text(theme::TEXT_XS))
                                .text_color(t.text_tertiary)
                                .child(source),
                        )
                        .children(stop),
                ),
        )
        .into_any_element()
}

fn set_asking(
    bench: &mut Workbench,
    member: Option<String>,
    stop: bool,
    cx: &mut Context<Workbench>,
) {
    let Some(project) = folder(bench) else {
        return;
    };
    let asked = (bench.target()).request_typed::<call::TeammateAsking>(TeammateAskingParams {
        project,
        member,
        stop,
    });
    bench.report(
        asked,
        t!("Asking was not changed"),
        move |bench, _, cx| {
            let done = if stop { t!("Stopped") } else { t!("Turned on") };
            bench.notify(toast::success(done), cx);
            reread(bench);
        },
        cx,
    );
}

fn more(key: &str, count: usize, t: &Tokens) -> gpui_kit::Stateful<Div> {
    div()
        .id(SharedString::from(key.to_owned()))
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
        .child(t!("{count} more", count = count))
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
    rows: &[Line],
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
        .children((rows.iter()).take(shown).map(|line| match line.by {
            None => row_of(bench, agent, line.row, handle, cx),
            Some(person) => teammates_row(bench, person, line.row, cx),
        }))
        .when(folds, |it| {
            it.child(
                more(&format!("tasks-more-{section:?}"), rows.len() - FOLDED, &t)
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
                parts.push(t!("artifact").to_owned());
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

/// The mark of a holder this window no longer has open: the face it wore, else its CLI's logo.
fn closed_mark(holder: &TaskHolder, cx: &App) -> AnyElement {
    if holder.face.is_none() && holder.cli.is_none() {
        return ui::icon(Icon::Agent, 12.).into_any_element();
    }
    let cli = holder.cli.as_deref().unwrap_or_default();
    let idle = sikemux_wire::protocol::AgentGroup::Idle;
    ui::still_mark(cli, holder.face.as_ref(), idle, theme::TEXT_2XS_H + 1., cx)
}

fn row_of(
    bench: &Workbench,
    agent: &str,
    row: &TaskRow,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = row_key(row);
    let holder =
        row.holder
            .as_ref()
            .map(|holder| match bench.agent_by_id(&holder.agent_id) {
                Some(agent) => crate::chat::agent_pill(&agent, theme::TEXT_2XS_H, handle, cx)
                    .into_any_element(),
                None => crate::chat::unknown_agent_pill(
                    &holder.agent_id,
                    &holder.name,
                    closed_mark(holder, cx),
                    theme::TEXT_2XS_H,
                    bench.target(),
                    handle,
                    cx,
                ),
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
    row_line(row, &t, cx)
        .id(SharedString::from(key))
        .cursor_pointer()
        .hover(|it| it.bg(t.surface_hover))
        .children(holder)
        .children(undo)
        .child(age(row, &t))
        .clicks(opens)
        .into_any_element()
}

/// A row's mark, title and what the core checked of it, for what follows to end.
fn row_line(row: &TaskRow, t: &Tokens, cx: &App) -> Div {
    div()
        .h(px(30.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(px(5.))
        .whitespace_nowrap()
        .text_size(theme::ui_text(theme::TEXT_SM))
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
}

fn age(row: &TaskRow, t: &Tokens) -> Div {
    div()
        .flex_none()
        .min_w(px(26.))
        .flex()
        .justify_end()
        .text_size(theme::ui_text(theme::TEXT_2XS_H))
        .text_color(t.text_tertiary)
        .font_features(theme::tabular())
        .child(ui::time::age(row.since))
}

fn row_key(row: &TaskRow) -> String {
    match &row.of {
        TaskOf::Task { id } => format!("task-{id}"),
        TaskOf::Call { id } => format!("task-call-{id}"),
    }
}

/// A teammate's task: the same line with their name and agent in place of the person's, and
/// nothing to do with it.
fn teammates_row(bench: &Workbench, person: &Teammate, row: &TaskRow, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let agent = (row.holder.as_ref()).map(|holder| {
        (
            holder.name.as_str(),
            holder.cli.as_deref().unwrap_or_default(),
        )
    });
    row_line(row, &t, cx)
        .id(SharedString::from(format!(
            "{}-{}",
            person.member,
            row_key(row)
        )))
        .child(teammate_pill(person, agent, bench, cx))
        .child(age(row, &t))
        .into_any_element()
}

/// A person, by their picture and name, and the agent of theirs on a task, by its mark and name.
pub(crate) fn teammate_pill(
    person: &Teammate,
    agent: Option<(&str, &str)>,
    bench: &Workbench,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let size = 12.;
    div()
        .flex_none()
        .pl(theme::SPACE_1)
        .pr(theme::SPACE_1H)
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::radius_2())
        .bg(t.surface_raised)
        .text_color(t.text_primary)
        .text_size(theme::ui_text(theme::TEXT_2XS_H))
        .whitespace_nowrap()
        .child(face(person, size, &bench.asks, cx))
        .child(person.name.clone())
        .when_some(agent, |pill, (name, cli)| {
            pill.child(div().text_color(t.text_disabled).child("·"))
                .child(ui::mark(cli, size))
                .child(format!("@{name}"))
        })
        .into_any_element()
}

/// A person's picture, or their initials when they have none.
pub(crate) fn face(
    person: &Teammate,
    size: f32,
    asks: &UnboundedSender<Ask>,
    cx: &App,
) -> AnyElement {
    match &person.picture {
        Some(url) => gpui_kit::img(
            Picture::Remote(Remote::new(url.clone(), asks.clone())).source_at(px(size)),
        )
        .size(px(size))
        .flex_none()
        .rounded_full()
        .into_any_element(),
        None => ui::initials(&person.name, &person.member, size, cx).into_any_element(),
    }
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
            let asker = (bench.view_at(&bench.place()))
                .and_then(|view| view.calls.iter().find(|call| &call.id == id))
                .filter(|call| !crate::calls::listed_in(call, agent))
                .map(|call| call.asker.agent_id.clone());
            bench.calls.update(cx, |cards, cx| cards.open(id, cx));
            return bench.show_calls(asker.as_deref().unwrap_or(agent), cx);
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
        if let Some(project) = bench.project() {
            let (project, folder) = (
                project.id.clone(),
                project.path.to_string_lossy().into_owned(),
            );
            return bench.open_sheet(project, folder, id, cx);
        }
    }
    if let Some(holder) = &row.holder {
        bench.open_agent(&holder.agent_id, cx);
    }
}
