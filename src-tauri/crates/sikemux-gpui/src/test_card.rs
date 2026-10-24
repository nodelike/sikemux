//! The Run chip's cards: the tests the task's last run failed, or why it stopped, each with
//! its output and an agent one click away, as the Mac's run button shows them.

use std::path::{Path, PathBuf};

use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, px, Anchor, AnyElement, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, StatefulInteractiveElement, Styled,
};
use serde_json::{json, Value};

use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::toast;
use crate::ui::{self, IconButton, Menu, MenuItem, Variant};
use crate::workbench::Workbench;

/// A finished run of the task the chip shows, with the task it ran.
pub struct Finished<'a> {
    pub run: &'a Value,
    pub task: &'a Value,
}

impl Finished<'_> {
    fn execution(&self) -> String {
        self.run["executionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn tests(&self) -> Option<&Value> {
        Some(&self.run["tests"]).filter(|tests| tests.is_object())
    }

    fn failed(&self) -> u64 {
        self.tests()
            .map_or(0, |tests| tests["failed"].as_u64().unwrap_or(0))
    }

    /// How the run ended in a word, when it failed.
    fn stopped(&self) -> Option<String> {
        (self.run["status"] == "failed").then(|| match self.run["exitCode"].as_i64() {
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
        Some(tests) => Some((
            format!("{} passed", tests["passed"].as_u64().unwrap_or(0)),
            false,
        )),
        None => finished.stopped().map(|stopped| (stopped, true)),
    }
}

/// Whether the chip offers "Why?": failed tests whose card was closed, or a run that stopped
/// without tests whose card is not open.
pub fn offers_why(bench: &Workbench, finished: &Finished) -> bool {
    let execution = finished.execution();
    match finished.tests() {
        Some(_) => finished.failed() > 0 && bench.tests_closed.as_deref() == Some(&execution),
        None => finished.stopped().is_some() && bench.failure_shown.as_deref() != Some(&execution),
    }
}

/// Opens the card of the person's last run again.
pub fn why(bench: &mut Workbench) {
    let (_, last) = bench.person_runs();
    let Some(run) = last else {
        return;
    };
    match run["tests"].is_object() {
        true => bench.tests_closed = None,
        false => bench.failure_shown = run["executionId"].as_str().map(str::to_owned),
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
    let execution = finished.execution();
    let body = match finished.tests() {
        Some(tests) if finished.failed() > 0 => {
            if bench.tests_closed.as_deref() == Some(&execution) {
                return None;
            }
            tests_card(bench, finished, tests, output, cx)
        }
        Some(_) => return None,
        None => {
            if bench.failure_shown.as_deref() != Some(&execution) {
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
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
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
        .gap(px(8.))
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
    tests: &Value,
    output: Option<(u64, String)>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let count = |key: &str| tests[key].as_u64().unwrap_or(0);
    let mut rest = format!(" · {} passed", count("passed"));
    if count("skipped") > 0 {
        rest.push_str(&format!(" · {} skipped", count("skipped")));
    }
    let title = div()
        .flex()
        .text_size(crate::theme::ui_text(12.))
        .child(
            div()
                .text_color(t.danger)
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{} failed", count("failed"))),
        )
        .child(div().text_color(t.text_tertiary).child(rest))
        .into_any_element();
    let failures = tests["failures"].as_array().map_or(&[][..], Vec::as_slice);
    let list = div()
        .id("run-card-failures")
        .max_h(px(280.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(failures.iter().enumerate().map(|(ix, failure)| {
            let class = failure["className"].as_str().unwrap_or_default();
            let name = failure["name"].as_str().unwrap_or_default();
            let message = failure["message"].as_str().unwrap_or_default().to_owned();
            let file = failure["file"].as_str().map(PathBuf::from);
            let line = failure["line"].as_u64().map(|line| line as u32);
            div()
                .flex()
                .gap(px(8.))
                .child(ui::icon(Icon::Close, 11.).text_color(t.danger).mt(px(3.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            ui::text(
                                format!("{} › {name}", short_class(class)),
                                12.,
                                FontWeight::NORMAL,
                            )
                            .text_color(t.text_primary),
                        )
                        .when(!message.is_empty(), |item| {
                            item.child(
                                ui::mono(message, 11., cx)
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
                                .text_size(crate::theme::ui_text(11.))
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
    let label = finished.task["label"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let command = finished.task["command"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let stopped = finished.stopped().unwrap_or_default();
    let title = div()
        .flex()
        .gap(px(6.))
        .text_size(crate::theme::ui_text(12.))
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
                .gap(px(4.))
                .child(ui::mono(command, 11., cx).text_color(t.text_secondary))
                .child(
                    ui::text(
                        "The reason is in its output, usually near the first error.",
                        11.5,
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
        .gap(px(6.))
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
    Popover::new("run-card-fix-menu")
        .anchor(Anchor::TopRight)
        .offset(px(4.))
        .trigger(
            ui::Button::new("run-card-fix", "Fix with an agent", Variant::Primary)
                .icon(Icon::Agent),
        )
        .content(move |_, _, cx| {
            let popover = cx.entity();
            Menu::new().children(choices.iter().enumerate().map(|(ix, choice)| {
                let (this, popover, to, text) = (
                    this.clone(),
                    popover.clone(),
                    choice.to.clone(),
                    request.clone(),
                );
                MenuItem::new(("run-card-fix", ix), choice.label.clone())
                    .mark(ui::mark(&choice.provider, 14.))
                    .on_click(move |_, window, cx| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        let (to, text) = (to.clone(), text.clone());
                        this.update(cx, |bench, cx| {
                            bench.close_run_card();
                            crate::send_to::send(bench, to, async move { Some(text) }, window, cx);
                        });
                    })
            }))
        })
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
    bench.watched_project().unwrap_or_default().to_owned()
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
fn test_fix_request(bench: &Workbench, finished: &Finished, failures: &[Value]) -> String {
    let project = project_path(bench);
    let mut lines = vec![format!(
        "These tests fail in {} ({}):",
        file_name(Path::new(&project)),
        finished.task["label"].as_str().unwrap_or_default()
    )];
    lines.extend(failures.iter().map(|failure| {
        let place = failure["file"].as_str().map(|file| {
            let file = relative(&project, file);
            match failure["line"].as_u64() {
                Some(line) => format!(" ({file}:{line})"),
                None => format!(" ({file})"),
            }
        });
        format!(
            "- {} › {}: {}{}",
            failure["className"].as_str().unwrap_or_default(),
            failure["name"].as_str().unwrap_or_default(),
            failure["message"].as_str().unwrap_or_default(),
            place.unwrap_or_default()
        )
    }));
    lines.push(format!(
        "Find the cause and fix it. Then run the task {} with task_start and check testResults in workspace_inspect.",
        finished.task["id"].as_str().unwrap_or_default()
    ));
    lines.join("\n")
}

/// What an agent is asked when a task stopped with an error: it starts the task itself to read why.
fn failure_fix_request(bench: &Workbench, finished: &Finished) -> String {
    let task = finished.task;
    let field = |key: &str| task[key].as_str().unwrap_or_default().to_owned();
    [
        format!(
            "The task \"{}\" in {} stopped: {}.",
            field("label"),
            file_name(Path::new(&project_path(bench))),
            finished.stopped().unwrap_or_default()
        ),
        format!("It runs `{}` in {}.", field("command"), field("cwd")),
        format!(
            "Start it with task_start (taskId {}) and read its output with task_read to see the error, then find the cause and fix it.",
            field("id")
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
            if let Some(execution) = last.and_then(|run| run["executionId"].as_str()) {
                self.tests_closed = Some(execution.to_owned());
            }
        }
        self.failure_shown = None;
    }
}
