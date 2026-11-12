//! The Run chip in the top bar: the task going, its clock, and the menu of
//! tasks.

use sikemux_view::layout::PaneKind;
use std::f32::consts::FRAC_PI_2;
use std::path::Path;
use std::rc::Rc;

use gpui::Anchor;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::Side;
use gpui_kit::{
    div, px, radians, AnyElement, Context, Hsla, IntoElement, ParentElement, Render, Styled,
    Transformation, WeakEntity, Window,
};
use sikemux_core::server::ports::PortOwner;
use sikemux_core::server::tasks::{RunView, Source, TaskView};

use crate::icons::Icon;
use crate::tabs::on_bench;
use crate::tasks::ports::PortRow;
use crate::theme::{Tokens, SPACE_1, SPACE_1H, SPACE_2, TEXT_2XS, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// Runs the project's task, or stops the run going, and says how the last one went.
pub(crate) fn run_chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let tasks = &bench.feeds.in_front().tasks.as_ref()?.tasks;
    let project = bench.watched.project.as_deref()?;
    let (running, last) = bench.person_runs();
    let task_of = |run: &RunView| tasks.iter().find(|task| task.id == run.run.task_id);
    let picked = bench
        .tasks
        .picked
        .get(project)
        .and_then(|id| tasks.iter().find(|task| task.id == *id));
    let selected = running.and_then(task_of).or(picked).or(tasks.first())?;
    let (id, label) = (selected.id.clone(), selected.label.clone());
    let last = last.filter(|run| running.is_none() && run.run.task_id == id);
    let finished = last.map(|run| crate::tasks::test_card::Finished {
        run,
        task: selected,
    });
    let outcome = finished.as_ref().and_then(crate::tasks::test_card::status);
    let failed = outcome
        .as_ref()
        .filter(|(_, wrong)| *wrong)
        .map(|(text, _)| text.clone());
    let passed = outcome
        .as_ref()
        .filter(|(_, wrong)| !*wrong)
        .map(|(text, _)| text.clone());
    let port = running.and_then(|run| bench.port_of_run(&run.run.execution_id));
    let port_number = port.map(|port| port.listening.port);
    let needs_java = (selected.java.as_ref())
        .filter(|java| java.runs_on.is_none())
        .map(|java| t!("needs Java {version}", version = java.required));
    let said = |text: String, color: Hsla| {
        div()
            .text_color(color)
            .child(t!("· {text}", text = text))
            .into_any_element()
    };
    let status = match (port_number, running, &failed, &passed) {
        (Some(port), ..) => Some(said(format!(":{port}"), t.text_tertiary)),
        (None, Some(_), ..) => Some(bench.tasks.clock.clone().into_any_element()),
        (None, None, Some(failed), _) => Some(said(failed.clone(), t.danger)),
        (None, None, None, Some(passed)) => Some(said(passed.clone(), t.live)),
        _ => needs_java.map(|text| said(text, t.warn)),
    };
    let dot = match (running, &failed) {
        (Some(_), _) if port.is_some() => Some(div().size(px(7.)).rounded_full().bg(t.live)),
        (Some(_), _) => Some(div().child(ui::run_mark("run-chip-mark", "running", 8.5, cx))),
        (None, Some(_)) => Some(div().size(px(7.)).rounded_full().bg(t.danger)),
        (None, None) if passed.is_some() => Some(div().size(px(7.)).rounded_full().bg(t.live)),
        _ => None,
    };
    let output = running
        .or(last.filter(|_| failed.is_some()))
        .and_then(|run| run.run.pty_id)
        .map(|session| (session, label.clone()));
    let card = crate::tasks::run_check::card(bench, project, cx).or_else(|| {
        finished
            .as_ref()
            .and_then(|finished| crate::tasks::test_card::card(bench, finished, output.clone(), cx))
    });
    let why = finished
        .as_ref()
        .filter(|finished| crate::tasks::test_card::offers_why(bench, finished))
        .map(|_| {
            let tip = if passed.is_none() && last.is_some_and(|run| run.tests.is_some()) {
                t!("Show the failed tests")
            } else {
                t!("Show why it stopped")
            };
            Button::new("run-why")
                .ghost()
                .compact()
                .h(px(24.))
                .px(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.danger)
                .tooltip(tip)
                .child(t!("Why?"))
                .on_click(cx.listener(|bench, _, _, cx| {
                    crate::tasks::test_card::why(bench);
                    cx.notify();
                }))
        });
    let menu = Rc::new(RunMenu {
        bench: cx.entity().downgrade(),
        running: running.map(|run| RunningTask {
            id: id.clone(),
            execution: run.run.execution_id.clone(),
            label: label.clone(),
            started_at: run.run.started_at.map(|at| at as i64),
            port: port_number,
        }),
        output: output.clone(),
        groups: task_groups(tasks, project),
        selected: id.clone(),
        others: (bench.feeds.in_front().ports.iter())
            .filter(|other| {
                !matches!(other.listening.owner, PortOwner::Terminal { .. }) && Some(*other) != port
            })
            .map(|other| PortRow::new(other, bench))
            .collect(),
        finders: crate::send_to::choices(bench),
        project_name: bench
            .project()
            .map(|project| project.name.clone())
            .unwrap_or_default(),
    });
    let chip = Button::new("run-task")
        .ghost()
        .compact()
        .h(px(24.))
        .max_w(px(320.))
        .px(SPACE_2)
        .gap(SPACE_1H)
        .rounded(crate::theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_secondary)
        .child(match dot {
            Some(dot) => dot.into_any_element(),
            None => ui::icon(Icon::Run, 11.)
                .text_color(t.live)
                .into_any_element(),
        })
        .child(if running.is_some() {
            t!("Running")
        } else {
            t!("Run")
        })
        .child(
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_family(crate::theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .children(status),
        )
        .child(
            ui::icon(Icon::Chevron, 10.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_tertiary),
        )
        .dropdown_menu_with_anchor(
            Anchor::TopRight,
            ui::menu(move |popup, window, cx| menu.build(popup, window, cx)),
        );
    Some(
        div()
            .relative()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .child(chip)
            .children(why)
            .children(card)
            .into_any_element(),
    )
}

/// How long the person's task has run, counted on the Run chip each second. The app's clock
/// draws it again in place, leaving the bar around it as it was.
#[derive(Default)]
pub struct RunClock {
    /// When the task started, in milliseconds, while it counts.
    started_at: Option<Option<i64>>,
}

impl RunClock {
    /// Counts from `started_at` while `counting`, or stops.
    pub fn set(&mut self, counting: bool, started_at: Option<i64>, cx: &mut Context<Self>) {
        let next = counting.then_some(started_at);
        if next == self.started_at {
            return;
        }
        self.started_at = next;
        cx.notify();
    }
}

impl Render for RunClock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.started_at.is_some() {
            crate::ui::clock::second(window, cx);
        }
        let t = Tokens::get(cx);
        let since = crate::ui::time::seconds_since(self.started_at.flatten());
        div()
            .text_color(t.text_tertiary)
            .child(t!("· {since}s", since = since))
    }
}

/// What the Run chip's menu lists, as it was when the chip was drawn.
pub(crate) struct RunMenu {
    bench: WeakEntity<Workbench>,
    running: Option<RunningTask>,
    /// The terminal the task runs in, or ran in when it failed, and the task's name.
    output: Option<(u64, String)>,
    groups: Vec<TaskGroup>,
    selected: String,
    /// Other things listening in the project, that tasks or agents started.
    others: Vec<PortRow>,
    /// Who can be asked to find the project's tasks, and the project's name for the asking.
    finders: Vec<crate::send_to::Choice>,
    project_name: String,
}

impl RunMenu {
    fn build(
        &self,
        popup: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let mut popup = popup
            .check_side(Side::Right)
            .scrollable(true)
            .min_w(px(220.))
            .max_w(px(360.))
            .max_h(px(480.));
        if let Some(running) = &self.running {
            let meta = match running.port {
                Some(port) => t!("up · :{port}", port = port),
                None => t!(
                    "running · {seconds}s",
                    seconds = crate::ui::time::seconds_since(running.started_at)
                ),
            };
            popup = popup.label(format!("{} · {meta}", running.label));
            let execution = running.execution.clone();
            popup = popup.item(
                PopupMenuItem::new(t!("Stop"))
                    .icon(Icon::Stop)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.stop_run(&execution)
                    })),
            );
            if let Some(port) = running.port {
                let url = format!("http://localhost:{port}");
                popup = popup.item(
                    PopupMenuItem::new(t!("Open localhost:{port}", port = port))
                        .icon(Icon::Globe)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            let task = running.id.clone();
            popup = popup.item(
                PopupMenuItem::new(t!("Restart"))
                    .icon(Icon::Refresh)
                    .on_click(on_bench(&self.bench, move |bench, cx| {
                        bench.start_task(&task, cx)
                    })),
            );
        }
        if let Some((session, label)) = &self.output {
            let (session, label) = (session.to_string(), label.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Show output"))
                    .icon(Icon::Terminal)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.show(PaneKind::Terminal, &session, Some(&label))
                    })),
            );
        }
        for group in &self.groups {
            popup = popup.separator().label(match &group.meta {
                Some(meta) => format!("{} · {meta}", group.title),
                None => group.title.clone(),
            });
            for (task, label) in &group.tasks {
                let picked = task.clone();
                popup = popup.item(
                    PopupMenuItem::new(label.clone())
                        .icon(Icon::Run)
                        .checked(*task == self.selected)
                        .on_click(on_bench(&self.bench, move |bench, cx| {
                            bench.pick_task(&picked, cx);
                            bench.start_task(&picked, cx);
                        })),
                );
            }
        }
        if !self.others.is_empty() {
            popup = popup.separator().label(t!("Running now"));
        }
        for other in &self.others {
            popup = popup.item(other.running_item(&self.bench));
        }
        let bench = self.bench.clone();
        let popup = popup.separator().label(t!("Your own processes")).item(
            PopupMenuItem::new(t!("Add a process…"))
                .icon(Icon::Plus)
                .on_click(move |_, window, cx| {
                    let bench = bench.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = bench.update(cx, |bench, cx| bench.add_task(window, cx));
                    });
                }),
        );
        if self.finders.is_empty() {
            return popup;
        }
        let (bench, finders, name) = (
            self.bench.clone(),
            self.finders.clone(),
            self.project_name.clone(),
        );
        popup.submenu(
            t!("Find processes with an agent…"),
            window,
            cx,
            ui::menu(move |menu, _, _| {
                finders.iter().fold(menu, |menu, choice| {
                    let (bench, to, name) = (bench.clone(), choice.to.clone(), name.clone());
                    menu.item(PopupMenuItem::new(choice.label.clone()).on_click(
                        move |_, window, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                bench.find_tasks_with(to.clone(), &name, window, cx)
                            });
                        },
                    ))
                })
            }),
        )
    }
}

pub(crate) struct RunningTask {
    id: String,
    execution: String,
    label: String,
    started_at: Option<i64>,
    port: Option<u16>,
}

pub(crate) struct TaskGroup {
    title: String,
    meta: Option<String>,
    /// Each task's id, and its label without the folder the group's heading names.
    tasks: Vec<(String, String)>,
}

/// sikemux.json's tasks, then one group for each folder the build files' tasks came from.
pub(crate) fn task_groups(tasks: &[TaskView], project: &str) -> Vec<TaskGroup> {
    let mut groups = Vec::new();
    let configured: Vec<_> = (tasks.iter())
        .filter(|task| matches!(task.source, Source::Project | Source::Personal))
        .map(|task| (task.id.clone(), task.label.clone()))
        .collect();
    if !configured.is_empty() {
        groups.push(TaskGroup {
            title: "sikemux.json".into(),
            meta: None,
            tasks: configured,
        });
    }
    let mut folders: Vec<(&str, Vec<&TaskView>)> = Vec::new();
    for task in
        (tasks.iter()).filter(|task| matches!(task.source, Source::Detected | Source::Rerun))
    {
        let cwd = task.cwd.as_str();
        match folders.iter_mut().find(|(folder, _)| *folder == cwd) {
            Some((_, folder)) => folder.push(task),
            None => folders.push((cwd, vec![task])),
        }
    }
    for (cwd, folder_tasks) in folders {
        let folder = if cwd == "." {
            Path::new(project).file_name().map_or_else(
                || project.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        } else {
            cwd.to_owned()
        };
        let first = folder_tasks[0];
        let tool = (first.command.split(' ').next())
            .and_then(|program| match program {
                "./mvnw" => Some(t!("Maven wrapper")),
                "mvn" => Some("Maven"),
                "./gradlew" => Some(t!("Gradle wrapper")),
                "gradle" => Some("Gradle"),
                "go" => Some("Go"),
                _ => None,
            })
            .map(str::to_owned);
        let java = first.java.as_ref().map(|java| match java.runs_on {
            None => t!("needs Java {version}", version = java.required),
            Some(runs_on) => format!("Java {runs_on}"),
        });
        let meta: Vec<String> = tool.into_iter().chain(java).collect();
        let suffix = format!(" ({folder})");
        groups.push(TaskGroup {
            tasks: folder_tasks
                .iter()
                .map(|task| {
                    let short = match cwd {
                        "." => task.label.as_str(),
                        _ => task.label.strip_suffix(&suffix).unwrap_or(&task.label),
                    };
                    (task.id.clone(), short.to_owned())
                })
                .collect(),
            title: folder,
            meta: (!meta.is_empty()).then(|| meta.join(" · ")),
        });
    }
    groups
}
