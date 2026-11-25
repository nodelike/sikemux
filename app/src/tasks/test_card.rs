//! The card of the tests the person's last run failed, with its output and an agent one click
//! away, hung under the top bar's right end until the person closes it.

use crate::ui::Clicks;
use sikemux_view::layout::PaneKind;
use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, px, Anchor, AnyElement, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, StatefulInteractiveElement, Styled,
};
use sikemux_calls::tasks::RunView;
use sikemux_calls::tasks::TaskView;
use sikemux_corekit::tasks::results::TestFailure;
use sikemux_corekit::tasks::results::TestResults;

use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_XS};
use crate::ui::{self, IconButton, Variant};
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::tasks::RerunParams;

/// A finished run, with the task it ran.
struct Finished<'a> {
    run: &'a RunView,
    task: &'a TaskView,
}

impl Finished<'_> {
    fn execution(&self) -> String {
        self.run.run.execution_id.clone()
    }
}

/// The run whose failed tests the person closed, by its execution.
#[derive(Default)]
pub struct RunCards {
    tests_closed: Option<String>,
}

/// The failed tests of the person's last run, until they are closed.
pub fn card(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let (running, last) = bench.person_runs();
    let run = last.filter(|_| running.is_none())?;
    let tests = run.tests.as_ref().filter(|tests| tests.failed > 0)?;
    if bench.tasks.cards.tests_closed.as_deref() == Some(&run.run.execution_id) {
        return None;
    }
    let task = (bench.feeds.in_front().tasks.as_ref())
        .and_then(|read| read.tasks.iter().find(|task| task.id == run.run.task_id))?;
    let output = run.run.pty_id.map(|session| (session, task.label.clone()));
    let finished = Finished { run, task };
    Some(under_bar(tests_card(bench, &finished, tests, output, cx)))
}

/// `body` hanging under the top bar's right end, over what is beneath it.
pub(crate) fn under_bar(body: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top(px(34.))
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
            IconButton::new("run-card-close", Icon::Close, t!("Close"))
                .small()
                .on_click(cx.listener(move |bench, _, _, cx| {
                    close(bench);
                    bench.parts.redraw(&bench.parts.bar, cx);
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
    let mut rest = format!(" · {}", t!("{count} passed", count = tests.passed));
    if tests.skipped > 0 {
        rest.push_str(&format!(
            " · {}",
            t!("{count} skipped", count = tests.skipped)
        ));
    }
    let title = div()
        .flex()
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(
            div()
                .text_color(t.danger)
                .font_weight(FontWeight::MEDIUM)
                .child(t!("{count} failed", count = tests.failed)),
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
                                Some(line) => {
                                    format!("{}:{line}", crate::ui::path::file_name(&file))
                                }
                                None => crate::ui::path::file_name(&file),
                            };
                            div()
                                .id(("run-card-file", ix))
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.text_secondary)
                                .underline()
                                .child(shown)
                                .clicks(cx.listener(move |bench, _, _, cx| {
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
                    ui::Button::new("run-card-rerun", t!("Rerun failed"), Variant::Outline)
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

/// The card's buttons, Show output first when the run has a terminal.
fn actions(output: Option<(u64, String)>, cx: &mut Context<Workbench>) -> gpui_kit::Div {
    div()
        .flex()
        .justify_end()
        .gap(SPACE_1H)
        .children(output.map(|(session, label)| {
            ui::Button::new("run-card-output", t!("Show output"), Variant::Outline)
                .icon(Icon::Terminal)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.close_run_card();
                    bench.show(PaneKind::Terminal, &session.to_string(), Some(&label));
                    bench.parts.redraw(&bench.parts.bar, cx);
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
        ui::Button::new("run-card-fix", t!("Fix with an agent"), Variant::Primary)
            .icon(Icon::Agent),
        move |mut popup, _, cx| {
            for choice in &choices {
                let (this, to, text, provider) = (
                    this.clone(),
                    choice.to.clone(),
                    request.clone(),
                    choice.provider.clone(),
                );
                let tint = crate::theme::brand(&provider, cx);
                popup = popup.item(
                    ui::MenuRow::new(choice.label.clone())
                        .mark(move || ui::mark_tinted(&provider, 14., tint))
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

/// Adds a task that runs only the failed tests and starts it.
fn rerun_failed(bench: &Workbench, execution: &str, cx: &mut Context<Workbench>) {
    let rerun = bench
        .target()
        .request_typed::<call::TaskRerunFailed>(RerunParams {
            execution_id: execution.to_owned(),
        });
    bench.report(
        rerun,
        t!("Could not rerun the failed tests"),
        |_, _, _| {},
        cx,
    );
}

/// `FooTest` for a Java class, `price.test.ts` for a file a JavaScript runner names.
fn short_class(class: &str) -> String {
    let script = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|end| class.ends_with(end));
    if class.contains('/') || script {
        return crate::ui::path::file_name(Path::new(class));
    }
    class.rsplit('.').next().unwrap_or(class).to_owned()
}

fn project_path(bench: &Workbench) -> String {
    bench
        .watched
        .project
        .as_deref()
        .unwrap_or_default()
        .to_owned()
}

/// What an agent is asked when the person hands it failing tests.
fn test_fix_request(bench: &Workbench, finished: &Finished, failures: &[TestFailure]) -> String {
    let project = project_path(bench);
    sikemux_corekit::tasks::results::fix_prompt(
        &project,
        &crate::ui::path::file_name(Path::new(&project)),
        &finished.task.label,
        &finished.task.id,
        failures,
    )
}

impl Workbench {
    /// Closes the failed tests' card.
    pub fn close_run_card(&mut self) {
        let (running, last) = self.person_runs();
        if let Some(run) = last.filter(|_| running.is_none()) {
            self.tasks.cards.tests_closed = Some(run.run.execution_id.clone());
        }
    }
}
