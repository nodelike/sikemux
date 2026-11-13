//! Starting one of the project's tasks, and the card its checks raise when one fails, as the
//! Mac's Run chip shows it: what is missing, then Run anyway, or Install and run or Fix and run
//! when the core can set right what is missing.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Styled,
    Window,
};
use sikemux_agents::harness::runs::RunStatus;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_1H, SPACE_2, TEXT_2XS_H, TEXT_XS};
use crate::toast;
use crate::ui::{self, Variant};
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::tasks::TaskStartParams;
use sikemux_calls::tasks::TaskStarted;
use sikemux_corekit::tasks::check::Check;
use sikemux_corekit::tasks::check::CheckItem;

/// A task held back by its checks, waiting on the person.
pub struct RunCheck {
    project: String,
    task: String,
    label: String,
    checks: Vec<Check>,
    fixing: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum How {
    Checked,
    Anyway,
    Fixed,
}

impl Workbench {
    /// Starts `task` in the project in front once its checks pass.
    pub fn start_task(&mut self, task: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.watched.project.clone() {
            self.start_task_in(project, task, cx);
        }
    }

    /// Starts `task` in the project at `project` once its checks pass.
    pub fn start_task_in(&mut self, project: String, task: &str, cx: &mut Context<Self>) {
        self.run_task(project, task.to_owned(), How::Checked, cx);
    }

    fn run_task(&mut self, project: String, task: String, how: How, cx: &mut Context<Self>) {
        if self
            .overlays
            .run_check
            .as_ref()
            .is_some_and(|check| check.fixing)
        {
            return;
        }
        let label = self.task_label(&task);
        let asked = self
            .target()
            .request_typed::<call::TaskStart>(TaskStartParams {
                project: project.to_owned(),
                task_id: task.to_owned(),
                force: how == How::Anyway,
                fix: how == How::Fixed,
            });
        if let Some(check) = self
            .overlays
            .run_check
            .as_mut()
            .filter(|_| how == How::Fixed)
        {
            check.fixing = true;
        }
        cx.notify();
        cx.spawn(async move |bench, cx| {
            let answer = asked.await;
            let _ = bench.update(cx, |bench, cx| {
                match answer {
                    Ok(TaskStarted::Held { checks }) => {
                        bench.overlays.run_check = Some(RunCheck {
                            project,
                            task,
                            label,
                            checks,
                            fixing: false,
                        })
                    }
                    Ok(TaskStarted::Running { execution_id }) => {
                        bench.overlays.run_check = None;
                        bench.tasks.to_show.insert(execution_id);
                        bench.show_started_runs();
                        bench.notify(
                            toast::success(t!("Started process: {label}", label = label)),
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(check) = bench.overlays.run_check.as_mut() {
                            check.fixing = false;
                        }
                        bench.notify(
                            toast::error(t!(
                                "Start process {label}: {error}",
                                label = label,
                                error = error
                            )),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the terminal of each run the person started here once the run has one, and
    /// forgets a run that ended without one.
    pub(crate) fn show_started_runs(&mut self) {
        if self.tasks.to_show.is_empty() {
            return;
        }
        let runs: Vec<(String, Option<u64>, bool, String, String)> =
            (self.feeds.in_front().runs.iter())
                .map(|listed| &listed.run)
                .filter(|run| self.tasks.to_show.contains(&run.execution_id))
                .map(|run| {
                    (
                        run.execution_id.clone(),
                        run.pty_id,
                        matches!(run.status, RunStatus::AwaitingTrust | RunStatus::Starting),
                        run.label.clone().unwrap_or_else(|| run.task_id.clone()),
                        run.project.clone(),
                    )
                })
                .collect();
        for (execution, session, waiting, label, project) in runs {
            let project = (self.projects().iter())
                .find(|open| open.path.as_os_str() == project.as_str())
                .map(|open| open.id.clone());
            match (session, project) {
                (Some(session), Some(project)) => self.open_in_front(
                    OpenTab::new(
                        At::project(&project),
                        PaneKind::Terminal,
                        session.to_string(),
                    )
                    .titled(label),
                ),
                (None, _) if waiting => continue,
                _ => {}
            }
            self.tasks.to_show.remove(&execution);
        }
    }

    /// Puts in `to`'s composer the ask to find how the project `name` is run, tested and built,
    /// and save those as tasks, starting its chat first when it is a new one.
    pub fn find_tasks_with(
        &mut self,
        to: crate::send_to::Recipient,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = sikemux_calls::tasks::find_processes_prompt(name);
        crate::send_to::send(self, to, async move { Some(text) }, window, cx);
    }

    fn task_label(&self, task: &str) -> String {
        (self.feeds.in_front().tasks.as_ref())
            .and_then(|read| read.tasks.iter().find(|listed| listed.id == task))
            .map_or(task, |listed| listed.label.as_str())
            .to_owned()
    }

    fn close_run_check(&mut self, cx: &mut Context<Self>) {
        self.overlays.run_check = None;
        cx.notify();
    }
}

/// The card under the Run chip of the project at `project`, while one of its tasks waits on
/// its checks.
pub fn card(bench: &Workbench, project: &str, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let check = bench
        .overlays
        .run_check
        .as_ref()
        .filter(|check| check.project == project)?;
    let t = Tokens::get(cx).clone();
    let failed = || check.checks.iter().filter(|check| !check.item.ok());
    let installs = failed().any(|check| !matches!(check.item, CheckItem::Port { .. }));
    let fixable = installs
        || failed().any(|check| {
            matches!(
                check.item,
                CheckItem::Port {
                    free_port: Some(_),
                    ..
                }
            )
        });
    let title = ui::text(
        t!("{name} can't start yet", name = check.label),
        TEXT_XS,
        FontWeight::SEMIBOLD,
    )
    .text_color(t.text_primary)
    .into_any_element();
    let items = check.checks.iter().map(|item| {
        let ok = item.item.ok();
        div()
            .flex()
            .items_start()
            .gap(SPACE_2)
            .pt(SPACE_2)
            .border_t_1()
            .border_color(t.border)
            .child(
                ui::icon(if ok { Icon::Check } else { Icon::Close }, 11.)
                    .mt(SPACE_0)
                    .text_color(if ok { t.live } else { t.danger }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .min_w_0()
                    .child(
                        ui::text(item.title.clone(), TEXT_XS, FontWeight::NORMAL)
                            .whitespace_normal()
                            .text_color(t.text_primary),
                    )
                    .child(
                        ui::text(item.detail.clone(), TEXT_2XS_H, FontWeight::NORMAL)
                            .whitespace_normal()
                            .text_color(t.text_tertiary),
                    ),
            )
    });
    let run = |how: How| {
        let (project, task) = (check.project.clone(), check.task.clone());
        cx.listener(move |bench: &mut Workbench, _, _, cx| {
            bench.run_task(project.clone(), task.clone(), how, cx)
        })
    };
    let actions = div()
        .flex()
        .justify_end()
        .gap(SPACE_1H)
        .child(
            ui::Button::new("run-check-anyway", t!("Run anyway"), Variant::Outline)
                .disabled(check.fixing)
                .on_click(run(How::Anyway)),
        )
        .when(fixable, |actions| {
            let label = match (check.fixing, installs) {
                (true, _) => t!("Fixing…"),
                (false, true) => t!("Install and run"),
                (false, false) => t!("Fix and run"),
            };
            actions.child(
                ui::Button::new("run-check-fix", label, Variant::Primary)
                    .disabled(check.fixing)
                    .on_click(run(How::Fixed)),
            )
        });
    let body = crate::tasks::test_card::frame(cx)
        .on_mouse_down_out(cx.listener(|bench, _, _, cx| bench.close_run_check(cx)))
        .child(crate::tasks::test_card::head_closing(
            title,
            |bench| bench.overlays.run_check = None,
            cx,
        ))
        .children(items)
        .child(actions)
        .into_any_element();
    Some(crate::tasks::test_card::under_chip(body))
}
