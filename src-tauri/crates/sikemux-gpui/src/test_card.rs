//! The Run chip's cards: the tests the task's last run failed, or why it stopped, each with
//! its output and an agent one click away, as the Mac's run button shows them.

use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, px, Anchor, AnyElement, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, StatefulInteractiveElement, Styled,
};
use serde_json::json;
use sikemux_core::harness::runs::RunStatus;
use sikemux_core::server::{RunView, TaskView};
use sikemux_core::tasks::results::{TestFailure, TestResults};

use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::theme::{
    Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_2XS_H, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, IconButton, Variant};
use crate::workbench::Workbench;

/// A finished run of the task the chip shows, with the task it ran.
pub struct Finished<'a> {
    pub run: &'a RunView,
    pub task: &'a TaskView,
}

impl Finished<'_> {
    fn execution(&self) -> String {
        self.run.run.execution_id.clone()
    }

    fn tests(&self) -> Option<&TestResults> {
        self.run.tests.as_ref()
    }

    fn failed(&self) -> u32 {
        self.tests().map_or(0, |tests| tests.failed)
    }

    /// How the run ended in a word, when it failed.
    fn stopped(&self) -> Option<String> {
        (self.run.run.status == RunStatus::Failed).then(|| match self.run.run.exit_code {
            Some(code) => format!("exited {code}"),
            None => "failed".into(),
        })
    }
}

/// What the chip says of the run: how many tests failed or passed, else how it stopped,
/// and whether that went wrong.
pub fn status(finished: &Finished) -> Option<(String, bool)> {
    match finished.tests() {
        Some(_) if finished.failed() > 0 => Some((format!("{} failed", finished.failed()), true)),
        Some(tests) => Some((format!("{} passed", tests.passed), false)),
        None => finished.stopped().map(|stopped| (stopped, true)),
    }
}

/// Which card the Run chip shows, by the run's execution: the failed tests until the person
/// closes them, and why a run stopped once they ask.
#[derive(Default)]
pub struct RunCards {
    tests_closed: Option<String>,
    why_open: Option<String>,
}

impl RunCards {
    fn open(&self, finished: &Finished) -> bool {
        let execution = finished.execution();
        match finished.tests() {
            Some(_) => self.tests_closed.as_deref() != Some(&execution),
            None => self.why_open.as_deref() == Some(&execution),
        }
    }
}

/// Whether the chip offers "Why?": failed tests whose card was closed, or a run that stopped
/// without tests whose card is not open.
pub fn offers_why(bench: &Workbench, finished: &Finished) -> bool {
    let shown = bench.run_cards.open(finished);
    match finished.tests() {
        Some(_) => finished.failed() > 0 && !shown,
        None => finished.stopped().is_some() && !shown,
    }
}

/// Opens the card of the person's last run again.
pub fn why(bench: &mut Workbench) {
    let (_, last) = bench.person_runs();
    let Some(run) = last else {
        return;
    };
    match run.tests.is_some() {
        true => bench.run_cards.tests_closed = None,
        false => bench.run_cards.why_open = Some(run.run.execution_id.clone()),
    }
}

/// The card the run shows under the chip, if any: its failed tests until they are closed,
/// or why it stopped once "Why?" asked.
pub fn card(
    bench: &Workbench,
    finished: &Finished,
    output: Option<(u64, String)>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let shown = bench.run_cards.open(finished);
    let body = match finished.tests() {
        Some(tests) if finished.failed() > 0 => {
            if !shown {
                return None;
            }
            tests_card(bench, finished, tests, output, cx)
        }
        Some(_) => return None,
        None => {
            if !shown {
                return None;
            }
            failure_card(bench, finished, output, cx)
        }
    };
    Some(under_chip(body))
}

/// `body` hanging under the Run chip, over what is beneath it.
pub(crate) fn under_chip(body: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top(px(30.))
        .right_0()
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window()
                    .child(body),
            )
            .with_priority(1),
        )
        .into_any_element()
}

pub(crate) fn frame(cx: &mut Context<Workbench>) -> gpui_kit::Stateful<gpui_kit::Div> {
    ui::float(cx)
        .id("run-card")
        .w(px(380.))
        .p(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .occlude()
}

fn head(title: AnyElement, cx: &mut Context<Workbench>) -> impl IntoElement {
    head_closing(title, Workbench::close_run_card, cx)
}

/// A card's title with the × that closes it by `close`.
pub(crate) fn head_closing(
    title: AnyElement,
    close: fn(&mut Workbench),
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(div().flex_1().min_w_0().child(title))
        .child(
            IconButton::new("run-card-close", Icon::Close, "Close")
                .small()
                .on_click(cx.listener(move |bench, _, _, cx| {
                    close(bench);
                    cx.notify();
                })),
        )
}

fn tests_card(
    bench: &Workbench,
    finished: &Finished,
    tests: &TestResults,
    output: Option<(u64, String)>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let mut rest = format!(" · {} passed", tests.passed);
    if tests.skipped > 0 {
        rest.push_str(&format!(" · {} skipped", tests.skipped));
    }
    let title = div()
        .flex()
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(
            div()
                .text_color(t.danger)
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{} failed", tests.failed)),
        )
        .child(div().text_color(t.text_tertiary).child(rest))
        .into_any_element();
    let failures = tests.failures.as_slice();
    let list = div()
        .id("run-card-failures")
        .max_h(px(280.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .children(failures.iter().enumerate().map(|(ix, failure)| {
            let (class, name) = (failure.class_name.as_str(), failure.name.as_str());
            let message = failure.message.clone();
            let file = failure.file.as_deref().map(PathBuf::from);
            let line = failure.line;
            div()
                .flex()
                .gap(SPACE_2)
                .child(ui::icon(Icon::Close, 11.).text_color(t.danger).mt(px(3.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
                        .child(
                            ui::text(
                                format!("{} › {name}", short_class(class)),
                                TEXT_XS,
                                FontWeight::NORMAL,
                            )
                            .text_color(t.text_primary),
                        )
                        .when(!message.is_empty(), |item| {
                            item.child(
                                ui::mono(message, TEXT_2XS, cx)
                                    .text_color(t.text_tertiary)
                                    .line_clamp(3),
                            )
                        })
                        .children(file.map(|file| {
                            let shown = match line {
                                Some(line) => format!("{}:{line}", file_name(&file)),
                                None => file_name(&file),
                            };
                            div()
                                .id(("run-card-file", ix))
                                .cursor_pointer()
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.text_secondary)
                                .underline()
                                .child(shown)
                                .on_click(cx.listener(move |bench, _, _, cx| {
                                    bench.close_run_card();
                                    let at = OpenAt {
                                        path: file.clone(),
                                        line: line.map_or(0, |line| line.saturating_sub(1)),
                                        column: None,
                                    };
                                    crate::editor::go(bench, at, cx);
                                }))
                        })),
                )
        }));
    let execution = finished.execution();
    let request = test_fix_request(bench, finished, failures);
    frame(cx)
        .child(head(title, cx))
        .child(list)
        .child(
            actions(output, cx)
                .child(
                    ui::Button::new("run-card-rerun", "Rerun failed", Variant::Outline)
                        .icon(Icon::Refresh)
                        .on_click(cx.listener(move |bench, _, _, cx| {
                            bench.close_run_card();
                            rerun_failed(bench, &execution, cx);
                        })),
                )
                .child(fix_button(bench, request, cx)),
        )
        .into_any_element()
}

fn failure_card(
    bench: &Workbench,
    finished: &Finished,
    output: Option<(u64, String)>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let label = finished.task.label.clone();
    let command = finished.task.command.clone();
    let stopped = finished.stopped().unwrap_or_default();
    let title = div()
        .flex()
        .gap(SPACE_1H)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(div().text_color(t.text_primary).child(label))
        .child(div().text_color(t.danger).child(stopped))
        .into_any_element();
    let request = failure_fix_request(bench, finished);
    frame(cx)
        .child(head(title, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(ui::mono(command, TEXT_2XS, cx).text_color(t.text_secondary))
                .child(
                    ui::text(
                        "The reason is in its output, usually near the first error.",
                        TEXT_2XS_H,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                ),
        )
        .child(actions(output, cx).child(fix_button(bench, request, cx)))
        .into_any_element()
}

/// The card's buttons, Show output first when the run has a terminal.
fn actions(output: Option<(u64, String)>, cx: &mut Context<Workbench>) -> gpui_kit::Div {
    div()
        .flex()
        .justify_end()
        .gap(SPACE_1H)
        .children(output.map(|(session, label)| {
            ui::Button::new("run-card-output", "Show output", Variant::Outline)
                .icon(Icon::Terminal)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.close_run_card();
                    bench.show("terminal", &session.to_string(), Some(&label));
                    cx.notify();
                }))
        }))
}

/// "Fix with an agent": the project's agents and a new chat with each, to hand the request to.
fn fix_button(bench: &Workbench, request: String, cx: &mut Context<Workbench>) -> impl IntoElement {
    let this = cx.entity();
    let choices = crate::send_to::choices(bench);
    ui::menu_button(
        "run-card-fix-menu",
        Anchor::TopRight,
        ui::Button::new("run-card-fix", "Fix with an agent", Variant::Primary).icon(Icon::Agent),
        move |mut popup, _, cx| {
            for choice in &choices {
                let (this, to, text, provider) = (
                    this.clone(),
                    choice.to.clone(),
                    request.clone(),
                    choice.provider.clone(),
                );
                popup = popup.item(
                    ui::MenuRow::new(choice.label.clone())
                        .mark(move || ui::mark(&provider, 14.))
                        .item(cx)
                        .on_click(move |_, window, cx| {
                            let (to, text) = (to.clone(), text.clone());
                            this.update(cx, |bench, cx| {
                                bench.close_run_card();
                                crate::send_to::send(
                                    bench,
                                    to,
                                    async move { Some(text) },
                                    window,
                                    cx,
                                );
                            });
                        }),
                );
            }
            popup
        },
    )
}

/// Adds a task that runs only the failed tests and starts it, making it the one the chip runs.
fn rerun_failed(bench: &Workbench, execution: &str, cx: &mut Context<Workbench>) {
    let rerun = bench
        .target()
        .request("taskRerunFailed", json!({ "executionId": execution }));
    cx.spawn(async move |bench, cx| {
        let answer = rerun.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(answer) => {
                if let Some(task) = answer["taskId"].as_str() {
                    bench.pick_task(task, cx);
                }
            }
            Err(error) => {
                let note = toast::error(format!("Could not rerun the failed tests: {error}"));
                bench.notify(note.autohide(false), cx)
            }
        });
    })
    .detach();
}

/// `FooTest` for a Java class, `price.test.ts` for a file a JavaScript runner names.
fn short_class(class: &str) -> String {
    let script = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|end| class.ends_with(end));
    if class.contains('/') || script {
        return file_name(Path::new(class));
    }
    class.rsplit('.').next().unwrap_or(class).to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
}

fn project_path(bench: &Workbench) -> String {
    bench
        .watched_project
        .as_deref()
        .unwrap_or_default()
        .to_owned()
}

fn relative(project: &str, path: &str) -> String {
    match path
        .strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) => rest.to_owned(),
        None if path == project => ".".into(),
        None => path.to_owned(),
    }
}

/// What an agent is asked when the person hands it failing tests: only the failures, and how
/// to check the fix.
fn test_fix_request(bench: &Workbench, finished: &Finished, failures: &[TestFailure]) -> String {
    let project = project_path(bench);
    let mut lines = vec![format!(
        "These tests fail in {} ({}):",
        file_name(Path::new(&project)),
        finished.task.label
    )];
    lines.extend(failures.iter().map(|failure| {
        let place = failure.file.as_deref().map(|file| {
            let file = relative(&project, file);
            match failure.line {
                Some(line) => format!(" ({file}:{line})"),
                None => format!(" ({file})"),
            }
        });
        format!(
            "- {} › {}: {}{}",
            failure.class_name,
            failure.name,
            failure.message,
            place.unwrap_or_default()
        )
    }));
    lines.push(format!(
        "Find the cause and fix it. Then run the task {} with task_start and check testResults in workspace_inspect.",
        finished.task.id
    ));
    lines.join("\n")
}

/// What an agent is asked when a task stopped with an error: it starts the task itself to read why.
fn failure_fix_request(bench: &Workbench, finished: &Finished) -> String {
    let task = finished.task;
    [
        format!(
            "The task \"{}\" in {} stopped: {}.",
            task.label,
            file_name(Path::new(&project_path(bench))),
            finished.stopped().unwrap_or_default()
        ),
        format!("It runs `{}` in {}.", task.command, task.cwd),
        format!(
            "Start it with task_start (taskId {}) and read its output with task_read to see the error, then find the cause and fix it.",
            task.id
        ),
        "Start it again afterwards and check that it keeps running.".to_owned(),
    ]
    .join("\n")
}

impl Workbench {
    /// Closes whichever card the Run chip shows.
    pub fn close_run_card(&mut self) {
        let (running, last) = self.person_runs();
        if running.is_none() {
            if let Some(run) = last {
                self.run_cards.tests_closed = Some(run.run.execution_id.clone());
            }
        }
        self.run_cards.why_open = None;
    }
}
