//! The code host's CI: its runs as a list, and one run's jobs, laid out as the host
//! draws them, beside the job picked.

use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, point, px, AnyElement, App, Bounds, Context, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, PathBuilder, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Window,
};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{GraphBox, Job, OpenRun, Outcome, RunList, Workflow};

use super::{
    branch_chip, elapsed, event_label, list_head, notice, outcome_label, outcome_word, Area,
    HostArea, Read, Said,
};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    TEXT_2XS, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS,
};
use crate::ui::time::ago_written as ago;
use crate::ui::{self, Button, Choice, IconButton, Segmented, Variant};

pub const LIST: &str = "host-runs";
pub const RUN: &str = "host-run";

/// Which runs the list shows by how they went.
const FILTERS: [(&str, &str); 6] = [
    ("all", "All"),
    ("in_progress", "Running"),
    ("queued", "Queued"),
    ("success", "Passed"),
    ("failure", "Failed"),
    ("cancelled", "Cancelled"),
];

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Summary,
    Logs,
}

pub struct Runs {
    pub filter: usize,
    pub workflow: Option<String>,
    pub page: u32,
    pub list: Read<RunList>,
    pub workflows: Option<Vec<Workflow>>,
    /// The run opened from the list, or from a pull request's checks.
    pub open: Option<Opened>,
    /// Picking a workflow to list, while the menu is open.
    pub picking: bool,
    /// A workflow being started by hand, while its sheet is open.
    pub dispatching: Option<super::run_actions::Dispatch>,
    /// The run whose menu is open, from its page or its row.
    pub more: Option<super::run_actions::MoreRun>,
}

pub struct Opened {
    pub id: String,
    /// The attempt shown, else the latest.
    pub attempt: Option<u64>,
    pub job: Option<String>,
    pub tab: Tab,
    pub view: Read<OpenRun>,
    /// The pull request it was opened from, which Back returns to.
    pub from: Option<u64>,
    /// Lands on the job that failed once the jobs are known.
    pick_failed: bool,
    pub busy: bool,
    /// The log of the job picked, once its log shows.
    pub log: Option<super::logs::Log>,
    pub extras: super::run_actions::Extras,
}

impl Runs {
    pub fn new() -> Self {
        Self {
            filter: 0,
            workflow: None,
            page: 1,
            list: None,
            workflows: None,
            open: None,
            picking: false,
            dispatching: None,
            more: None,
        }
    }
}

impl HostArea {
    pub(super) fn watch_runs(&self) {
        match &self.runs.open {
            Some(opened) => {
                let mut params = self.located();
                params["runId"] = json!(opened.id);
                params["attempt"] = json!(opened.attempt);
                self.ask_watch(RUN, "codeHostRun", params);
            }
            None => {
                let mut params =
                    json!({ "project": self.project, "page": self.runs.page, "perPage": 30 });
                let status = FILTERS[self.runs.filter].0;
                if status != "all" {
                    params["status"] = json!(status);
                }
                if let Some(workflow) = &self.runs.workflow {
                    params["workflowId"] = json!(workflow);
                }
                self.ask_watch(LIST, "codeHostRuns", params);
            }
        }
    }

    pub(super) fn read_workflows(&mut self, cx: &mut Context<Self>) {
        if self.runs.workflows.is_some() {
            return;
        }
        self.runs.workflows = Some(Vec::new());
        let workflows = self.plugin("workflows", json!({}));
        cx.spawn(async move |area, cx| {
            let workflows: Vec<Workflow> = workflows
                .await
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            area.update(cx, |area, cx| {
                area.runs.workflows = Some(workflows);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the run's view, answering whether it opened its failed job's log.
    pub(super) fn take_run(&mut self, value: &Value) -> bool {
        let Some(opened) = &mut self.runs.open else {
            return false;
        };
        let mut logs = false;
        let view: Result<OpenRun, SharedString> = super::answer(value);
        if let (true, Ok(view)) = (opened.pick_failed, &view) {
            if !view.jobs.is_empty() {
                opened.pick_failed = false;
                if opened.job.is_none() {
                    if let Some(failed) = &view.failed_job {
                        opened.job = Some(failed.clone());
                        opened.tab = Tab::Logs;
                        opened.log = Some(super::logs::Log::new(failed.clone()));
                        logs = true;
                    }
                }
            }
        }
        opened.view = Some(view);
        logs
    }

    /// Opens run `id`, from the pull request `from` when one of its checks opened it.
    pub fn open_run(&mut self, id: String, from: Option<u64>, cx: &mut Context<Self>) {
        self.runs.open = Some(Opened {
            id,
            attempt: None,
            job: None,
            tab: Tab::Summary,
            view: None,
            from,
            pick_failed: from.is_some(),
            busy: false,
            log: None,
            extras: Default::default(),
        });
        self.set_area(Area::Actions, cx);
        self.watch();
        cx.notify();
    }

    /// Leaves the run for wherever it was opened from.
    fn leave_run(&mut self, cx: &mut Context<Self>) {
        let from = self.runs.open.take().and_then(|opened| opened.from);
        if let Some(number) = from {
            self.set_area(Area::Pulls, cx);
            self.open_pull(number, cx);
        }
        self.watch();
        cx.notify();
    }

    pub(super) fn run_write(
        &mut self,
        method: &'static str,
        more: Value,
        done: String,
        failed: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(opened) = &mut self.runs.open {
            opened.busy = true;
        }
        let answer = self.plugin(method, more);
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                if let Some(opened) = &mut area.runs.open {
                    opened.busy = false;
                }
                match answer {
                    Ok(_) => area.say(Said::Done(done.into()), cx),
                    Err(error) => area.say(Said::Failed(format!("{failed}: {error}").into()), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn rerun(&mut self, failed_only: bool, cx: &mut Context<Self>) {
        let Some(id) = self.runs.open.as_ref().map(|opened| opened.id.clone()) else {
            return;
        };
        let (done, failed) = if failed_only {
            (
                "Re-running the failed jobs",
                "Could not re-run the failed jobs",
            )
        } else {
            ("Re-running every job", "Could not re-run every job")
        };
        let params = json!({ "runId": id, "failedOnly": failed_only });
        self.run_write("rerun", params, done.into(), failed.into(), cx);
    }

    fn cancel_run(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.runs.open.as_ref().map(|opened| opened.id.clone()) else {
            return;
        };
        self.run_write(
            "cancel",
            json!({ "runId": id }),
            "Cancelled the run".into(),
            "Could not cancel the run".into(),
            cx,
        );
    }
}

/// The workflows the list can be narrowed to, as a menu.
pub fn workflow_choices(area: &HostArea, cx: &mut Context<HostArea>) -> Vec<Choice> {
    let view = cx.entity().downgrade();
    let pick = |workflow: Option<String>| {
        let view = view.clone();
        move |_: &mut Window, cx: &mut App| {
            let workflow = workflow.clone();
            view.update(cx, |area, cx| {
                area.runs.picking = false;
                area.runs.workflow = workflow;
                area.runs.page = 1;
                area.runs.list = None;
                area.watch();
                cx.notify();
            })
            .ok();
        }
    };
    let mut choices = vec![Choice::new(None, "Every workflow", pick(None))];
    for workflow in area.runs.workflows.iter().flatten() {
        let mut choice = Choice::new(None, workflow.name.clone(), pick(Some(workflow.id.clone())));
        if !workflow.active {
            choice = choice.hint("off");
        }
        choices.push(choice);
    }
    choices
}

pub fn list(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let runs = &area.runs;
    let workflow_name = |id: &str| {
        runs.workflows
            .iter()
            .flatten()
            .find(|workflow| workflow.id == id)
            .map(|workflow| workflow.name.clone())
    };
    let chosen = runs.workflow.as_deref().and_then(workflow_name);
    let dispatchable = runs
        .workflow
        .as_deref()
        .and_then(|id| {
            runs.workflows
                .iter()
                .flatten()
                .find(|workflow| workflow.id == id)
        })
        .filter(|workflow| workflow.active && area.host.ci.dispatch)
        .cloned();
    let total = runs
        .list
        .as_ref()
        .and_then(|list| list.as_ref().ok())
        .map(|list| list.total);
    let head = list_head(Area::Actions, &area.host, cx)
        .child(
            Segmented::words(
                "run-filter",
                FILTERS
                    .iter()
                    .map(|(_, label)| (SharedString::from(*label), None))
                    .collect(),
                runs.filter,
            )
            .on_select(cx.listener(|area, index: &usize, _, cx| {
                area.runs.filter = *index;
                area.runs.page = 1;
                area.runs.list = None;
                area.watch();
                cx.notify();
            })),
        )
        .child(div().flex_1())
        .children(total.map(|total| {
            ui::meta(
                format!("{total} run{}", if total == 1 { "" } else { "s" }),
                cx,
            )
        }))
        .child(
            Button::new(
                "run-workflow",
                chosen.unwrap_or_else(|| "Every workflow".into()),
                Variant::Outline,
            )
            .icon(Icon::Workflow)
            .on_click(cx.listener(|area, _, _, cx| {
                area.runs.picking = true;
                area.read_workflows(cx);
                cx.notify();
            })),
        )
        .when_some(dispatchable, |head, workflow| {
            head.child(
                IconButton::new("run-dispatch", Icon::Run, "Run workflow")
                    .small()
                    .on_click(cx.listener(move |area, _, window, cx| {
                        area.start_dispatch(workflow.clone(), window, cx)
                    })),
            )
        })
        .child(super::shell::account(area, cx));
    let body = match &runs.list {
        None => notice("Loading runs", None, cx),
        Some(Err(error)) => notice("Could not read runs", Some(error.clone()), cx),
        Some(Ok(list)) if list.runs.is_empty() => {
            notice("No runs", Some("Nothing matches this filter.".into()), cx)
        }
        Some(Ok(list)) => {
            let names: HashMap<&str, &str> = runs
                .workflows
                .iter()
                .flatten()
                .map(|workflow| (workflow.id.as_str(), workflow.name.as_str()))
                .collect();
            let pager = (runs.page > 1 || list.next_page.is_some()).then(|| {
                let older = list.next_page.is_some();
                div()
                    .px(SPACE_3)
                    .py(SPACE_2H)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        Button::new("runs-newer", "Newer", Variant::Outline)
                            .disabled(runs.page <= 1)
                            .on_click(cx.listener(|area, _, _, cx| {
                                area.runs.page = area.runs.page.saturating_sub(1).max(1);
                                area.watch();
                                cx.notify();
                            })),
                    )
                    .child(ui::meta(format!("Page {}", runs.page), cx))
                    .child(
                        Button::new("runs-older", "Older", Variant::Outline)
                            .disabled(!older)
                            .on_click(cx.listener(|area, _, _, cx| {
                                area.runs.page += 1;
                                area.watch();
                                cx.notify();
                            })),
                    )
            });
            super::carded(div().flex().flex_col(), cx)
                .children(list.runs.iter().enumerate().map(|(index, listed)| {
                    let run = &listed.run;
                    let workflow = names
                        .get(run.workflow_id.as_str())
                        .copied()
                        .unwrap_or(&run.name);
                    let actor = run.actor.clone().unwrap_or_else(|| "someone".into());
                    let who = if run.short_sha.is_empty() {
                        format!("{workflow} #{} · {actor}", run.run_number)
                    } else {
                        format!(
                            "{workflow} #{} · {} by {actor}",
                            run.run_number, run.short_sha
                        )
                    };
                    let started = run.started_at.as_deref().unwrap_or(&run.created_at);
                    let took = elapsed(
                        started,
                        (!listed.unfinished).then_some(run.updated_at.as_str()),
                    );
                    let place = if run.pull_requests.is_empty() {
                        run.branch
                            .as_ref()
                            .map(|branch| branch_chip(branch, t.text_secondary, cx))
                    } else {
                        let numbers: Vec<String> = run
                            .pull_requests
                            .iter()
                            .map(|number| format!("#{number}"))
                            .collect();
                        Some(branch_chip(&numbers.join(" "), t.text_secondary, cx))
                    };
                    let id = run.id.clone();
                    let more = super::run_actions::MoreRun::of(run, !listed.unfinished);
                    div()
                        .id(("run", index))
                        .px(SPACE_3)
                        .py(SPACE_2H)
                        .flex()
                        .items_center()
                        .gap(SPACE_3)
                        .when(index > 0, |row| row.border_t_1().border_color(t.border))
                        .cursor_pointer()
                        .hover(move |row| row.bg(t.surface_hover))
                        .on_click(
                            cx.listener(move |area, _, _, cx| area.open_run(id.clone(), None, cx)),
                        )
                        .child(area.face(Some(&actor), run.avatar_url.as_deref(), 28., cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(SPACE_2)
                                        .child(ui::run_mark(
                                            format!("run-row-{}", run.id),
                                            outcome_word(run.outcome),
                                            16.,
                                            cx,
                                        ))
                                        .child(
                                            ui::text(
                                                if run.title.is_empty() {
                                                    run.name.clone()
                                                } else {
                                                    run.title.clone()
                                                },
                                                13.,
                                                FontWeight::SEMIBOLD,
                                            )
                                            .text_color(t.text_primary)
                                            .min_w_0()
                                            .truncate(),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(SPACE_1H)
                                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                                        .text_color(t.text_tertiary)
                                        .child(div().flex_none().child(who))
                                        .children(place),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap(px(3.))
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(ago(started))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(SPACE_1)
                                        .font_features(theme::tabular())
                                        .child(
                                            ui::icon(Icon::Clock, 11.).text_color(t.text_tertiary),
                                        )
                                        .child(took),
                                ),
                        )
                        .child(super::run_actions::more_of(
                            ("run-row-more", index),
                            more,
                            cx,
                        ))
                }))
                .children(pager)
                .into_any_element()
        }
    };
    super::list_page("host-runs", head, body, cx)
}

pub fn view(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    super::logs::prepare(area, window, cx);
    let area = &*area;
    let t = Tokens::get(cx).clone();
    let opened = area.runs.open.as_ref().expect("a run is open");
    let back = if opened.from.is_some() {
        "the pull request"
    } else {
        "the runs"
    };
    let found = match &opened.view {
        None => return notice("Loading run", None, cx),
        Some(Err(error)) => {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .children((!area.on_a_desk()).then(|| back_button(back, cx)))
                .child(notice("Could not read the run", Some(error.clone()), cx))
                .into_any_element()
        }
        Some(Ok(found)) => found,
    };
    let left = side(area, found, back, cx);
    let right = main(area, found, window, cx);
    div()
        .size_full()
        .flex()
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(t.border)
                .child(left),
        )
        .child(div().flex_1().min_w_0().h_full().child(right))
        .into_any_element()
}

fn back_button(back: &str, cx: &mut Context<HostArea>) -> impl IntoElement {
    IconButton::new("run-back", Icon::Chevron, format!("Back to {back}"))
        .turned(std::f32::consts::PI)
        .small()
        .on_click(cx.listener(|area, _, _, cx| area.leave_run(cx)))
}

fn side(area: &HostArea, found: &OpenRun, back: &str, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = area.runs.open.as_ref().expect("a run is open");
    let ci = &area.host.ci;
    let host = area.host.label.as_str();
    let run = &found.run;
    let live = found.unfinished;
    let took = found
        .timing
        .as_ref()
        .and_then(|timing| timing.run_duration_ms)
        .map(|ms| crate::ui::time::span(ms / 1000))
        .unwrap_or_else(|| {
            elapsed(
                run.started_at.as_deref().unwrap_or(&run.created_at),
                (!live).then_some(run.updated_at.as_str()),
            )
        });
    let failed = !live && run.outcome != Outcome::Success;
    let url = run.url.clone();
    let head = div()
        .px(SPACE_2H)
        .pt(SPACE_2H)
        .pb(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children((!area.on_a_desk()).then(|| back_button(back, cx)))
        .child(ui::run_mark(
            format!("run-head-{}", run.id),
            outcome_word(run.outcome),
            15.,
            cx,
        ))
        .child(ui::mono(format!("#{}", run.run_number), 12., cx).text_color(t.text_primary))
        .children(
            run.branch
                .as_ref()
                .map(|branch| branch_chip(branch, t.text_secondary, cx)),
        )
        .child(div().flex_1())
        .child(
            div()
                .id("run-took")
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .font_features(theme::tabular())
                .text_color(t.text_tertiary)
                .tooltip(ui::tooltip(outcome_label(run.outcome)))
                .child(took),
        )
        .child(super::run_actions::more_button(found, cx));
    let busy = opened.busy;
    let actions = div()
        .px(SPACE_2H)
        .pb(SPACE_2H)
        .flex()
        .flex_wrap()
        .gap(SPACE_1H)
        .when(live, |row| {
            row.child(
                Button::new("run-cancel", "Cancel run", Variant::Outline)
                    .icon(Icon::Close)
                    .disabled(busy)
                    .on_click(cx.listener(move |area, _, _, cx| {
                        let number = area
                            .runs
                            .open
                            .as_ref()
                            .and_then(|opened| opened.view.as_ref()?.as_ref().ok())
                            .map_or(0, |view| view.run.run_number);
                        area.ask(
                            super::Asking::new(
                                "Cancel this run?",
                                format!("Run #{number} will stop where it is."),
                                "Cancel run",
                                true,
                                |area, cx| area.cancel_run(cx),
                            ),
                            cx,
                        );
                    })),
            )
        })
        .when(!live && failed && ci.rerun_failed, |row| {
            row.child(
                Button::new("run-rerun-failed", "Re-run failed", Variant::Primary)
                    .icon(Icon::Refresh)
                    .disabled(busy)
                    .on_click(cx.listener(|area, _, _, cx| area.rerun(true, cx))),
            )
        })
        .when(!live, |row| {
            row.child(
                Button::new("run-rerun", "Re-run all", Variant::Outline)
                    .icon(Icon::Refresh)
                    .disabled(busy)
                    .on_click(cx.listener(|area, _, _, cx| area.rerun(false, cx))),
            )
        })
        .child(
            Button::new("run-on-host", host.to_owned(), Variant::Outline)
                .icon(Icon::Globe)
                .on_click(move |_, _, cx| cx.open_url(&url)),
        );
    let attempts = (ci.attempts && found.attempts > 1).then(|| {
        let latest = found.attempts;
        let shown = opened.attempt.unwrap_or(latest);
        div().px(SPACE_2).pb(SPACE_2).child(
            Segmented::words(
                "run-attempts",
                (1..=latest)
                    .map(|attempt| (SharedString::from(format!("Attempt {attempt}")), None))
                    .collect(),
                (shown - 1) as usize,
            )
            .fill()
            .on_select(cx.listener(move |area, index: &usize, _, cx| {
                let attempt = *index as u64 + 1;
                if let Some(opened) = &mut area.runs.open {
                    opened.attempt = (attempt != latest).then_some(attempt);
                    opened.view = None;
                }
                area.watch();
                cx.notify();
            })),
        )
    });
    let artifacts = super::run_actions::artifact_count(area);
    let artifacts_shown = artifacts > 0 && opened.extras.artifacts_page;
    let pages = (artifacts > 0).then(|| {
        div().px(SPACE_2H).pb(SPACE_2).child(
            Segmented::words(
                "run-pages",
                vec![
                    (SharedString::from("Jobs"), None),
                    (SharedString::from("Artifacts"), Some(artifacts as u64)),
                ],
                usize::from(artifacts_shown),
            )
            .on_select(cx.listener(|area, index: &usize, _, cx| {
                if let Some(opened) = &mut area.runs.open {
                    opened.extras.artifacts_page = *index == 1;
                }
                cx.notify();
            })),
        )
    });
    let jobs = div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(SPACE_3)
                .py(SPACE_1)
                .flex()
                .gap(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_secondary)
                .child("Jobs")
                .child(
                    div()
                        .text_color(t.text_tertiary)
                        .child(found.jobs.len().to_string()),
                ),
        )
        .when(found.jobs.is_empty(), |jobs| {
            jobs.child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_1H)
                    .child(ui::meta("No jobs yet", cx)),
            )
        })
        .children(found.jobs.iter().enumerate().map(|(index, listed)| {
            let job = &listed.job;
            let chosen = opened.job.as_deref() == Some(job.id.as_str());
            let took = if job.outcome == Outcome::Skipped {
                "skipped".to_owned()
            } else {
                job.started_at
                    .as_deref()
                    .map(|started| elapsed(started, job.completed_at.as_deref()))
                    .unwrap_or_default()
            };
            let id = job.id.clone();
            div()
                .id(("run-job", index))
                .h(px(28.))
                .px(SPACE_3)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .cursor_pointer()
                .when(chosen, |row| row.bg(t.surface_chosen))
                .hover(move |row| row.bg(t.surface_hover))
                .on_click(cx.listener(move |area, _, _, cx| area.show_job(id.clone(), cx)))
                .child(ui::run_mark(
                    format!("run-job-{}", job.id),
                    outcome_word(job.outcome),
                    14.,
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary)
                        .child(job.name.clone()),
                )
                .children(
                    listed
                        .stopped_at
                        .as_ref()
                        .map(|step| ui::meta(format!("at {step}"), cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .font_features(theme::tabular())
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(took),
                )
        }));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(head)
        .child(actions)
        .children(attempts)
        .children(super::run_actions::approvals(area, found, cx))
        .children(pages)
        .child(
            div()
                .id("run-jobs")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(if artifacts_shown {
                    super::run_actions::artifacts(area, cx)
                } else {
                    jobs
                }),
        )
        .into_any_element()
}

impl HostArea {
    /// Opens a job of the open run at its log.
    pub(super) fn show_job(&mut self, id: String, cx: &mut Context<Self>) {
        let job = self
            .runs
            .open
            .as_ref()
            .and_then(|opened| opened.view.as_ref()?.as_ref().ok())
            .and_then(|view| view.jobs.iter().find(|listed| listed.job.id == id))
            .map(|listed| listed.job.clone());
        if let Some(job) = job {
            self.read_annotations(&job, cx);
        }
        if let Some(opened) = &mut self.runs.open {
            if opened.log.as_ref().is_none_or(|log| log.job != id) {
                opened.log = Some(super::logs::Log::new(id.clone()));
            }
            opened.job = Some(id);
            opened.tab = Tab::Logs;
        }
        self.watch();
        cx.notify();
    }
}

fn main(
    area: &HostArea,
    found: &OpenRun,
    window: &mut Window,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = area.host.label.as_str();
    let opened = area.runs.open.as_ref().expect("a run is open");
    let run = &found.run;
    let actor = run.actor.clone().unwrap_or_else(|| "someone".into());
    let picked: Option<&Job> = opened
        .job
        .as_ref()
        .and_then(|id| found.jobs.iter().find(|listed| &listed.job.id == id))
        .map(|listed| &listed.job);
    let tabs = [
        (Tab::Summary, "Summary".to_owned()),
        (
            Tab::Logs,
            picked.map_or_else(|| "Logs".to_owned(), |job| format!("Logs · {}", job.name)),
        ),
    ];
    let attempt = if run.attempt > 1 {
        format!(" · attempt {}", run.attempt)
    } else {
        String::new()
    };
    let url = run.url.clone();
    let (sha, short) = (run.sha.clone(), run.short_sha.clone());
    let headline = div()
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_2H)
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .gap(SPACE_3)
                .child(area.face(Some(&actor), run.avatar_url.as_deref(), 36., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_1)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_2)
                                .child(ui::run_mark(
                                    format!("run-summary-{}", run.id),
                                    outcome_word(run.outcome),
                                    16.,
                                    cx,
                                ))
                                .child(
                                    ui::text(
                                        if run.title.is_empty() {
                                            run.name.clone()
                                        } else {
                                            run.title.clone()
                                        },
                                        15.,
                                        FontWeight::SEMIBOLD,
                                    )
                                    .text_color(t.text_primary)
                                    .min_w_0()
                                    .truncate(),
                                )
                                .child(
                                    IconButton::new(
                                        "run-open-host",
                                        Icon::Globe,
                                        format!("Open on {host}"),
                                    )
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_1H)
                                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                                .text_color(t.text_tertiary)
                                .child(format!(
                                    "{} #{} · {actor} · {} {}{attempt}",
                                    run.name,
                                    run.run_number,
                                    event_label(&run.event).to_lowercase(),
                                    ago(&run.created_at)
                                ))
                                .child(
                                    div()
                                        .id("run-sha")
                                        .cursor_pointer()
                                        .tooltip(ui::tooltip("Copy the commit"))
                                        .on_click(cx.listener(move |area, _, _, cx| {
                                            cx.write_to_clipboard(
                                                gpui_kit::ClipboardItem::new_string(sha.clone()),
                                            );
                                            area.say(
                                                Said::Done(format!("Copied {short}").into()),
                                                cx,
                                            );
                                        }))
                                        .child(branch_chip(&run.short_sha, t.text_secondary, cx)),
                                )
                                .children(
                                    run.branch
                                        .as_ref()
                                        .map(|branch| branch_chip(branch, t.text_secondary, cx)),
                                )
                                .children(run.pull_requests.iter().map(|number| {
                                    branch_chip(&format!("#{number}"), t.text_secondary, cx)
                                })),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_1H)
                .children(tabs.into_iter().map(|(tab, label)| {
                    let on = opened.tab == tab;
                    div()
                        .id(SharedString::from(format!("run-tab-{}", tab == Tab::Logs)))
                        .h(px(24.))
                        .px(SPACE_2H)
                        .flex()
                        .items_center()
                        .rounded_full()
                        .border_1()
                        .border_color(if on { t.border_strong } else { t.border })
                        .when(on, |chip| chip.bg(t.surface_active))
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(if on { t.text_primary } else { t.text_secondary })
                        .cursor_pointer()
                        .on_click(cx.listener(move |area, _, _, cx| {
                            if let Some(opened) = &mut area.runs.open {
                                opened.tab = tab;
                            }
                            area.watch();
                            cx.notify();
                        }))
                        .child(label)
                })),
        );
    let body = match opened.tab {
        Tab::Summary => summary(area, found, window, cx),
        Tab::Logs => match picked {
            Some(job) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(super::run_actions::job_bar(area, job, cx))
                .children(super::run_actions::annotations(area, job, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(super::logs::view(area, job, cx)),
                )
                .into_any_element(),
            None => notice("Pick a job on the left to read its log.", None, cx),
        },
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(headline)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// The run at a glance: what it was billed, and its jobs as the host draws them.
fn summary(
    area: &HostArea,
    found: &OpenRun,
    window: &mut Window,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = area.runs.open.as_ref().expect("a run is open");
    let billed = found
        .timing
        .as_ref()
        .filter(|_| area.host.ci.billing)
        .map(|timing| {
            timing
                .billable
                .iter()
                .map(|each| each.total_ms.div_ceil(60_000))
                .sum::<u64>()
        })
        .filter(|minutes| *minutes > 0);
    let run = &found.run;
    let name = run
        .path
        .as_deref()
        .and_then(|path| path.rsplit('/').next())
        .unwrap_or(&run.name)
        .to_owned();
    let total = found.jobs.len();
    div()
        .id("run-summary")
        .size_full()
        .overflow_scroll()
        .p(SPACE_4)
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .children(billed.map(|minutes| ui::meta(format!("Billed {minutes} min"), cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(
                    div()
                        .font_family(theme::MONO)
                        .text_color(t.text_primary)
                        .child(name),
                )
                .when(total > 0, |head| {
                    head.child(ui::meta(
                        format!(
                            "{} of {total} job{} done",
                            found.done,
                            if total == 1 { "" } else { "s" }
                        ),
                        cx,
                    ))
                    .when(found.failed > 0, |head| {
                        head.child(
                            div()
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.danger)
                                .child(format!("· {} failed", found.failed)),
                        )
                    })
                })
                .child(div().flex_1())
                .children(super::run_actions::file_toggle(area, found, cx)),
        )
        .child(if found.graph.columns.is_empty() {
            ui::meta("No jobs yet", cx).into_any_element()
        } else {
            graph(found, opened.job.as_deref(), cx)
        })
        .child(super::run_actions::below_graph(area, window, cx))
        .into_any_element()
}

const COLUMN: f32 = 220.;
const GAP: f32 = 56.;
const BOX_GAP: f32 = 14.;
const TITLE: f32 = 24.;
const JOB: f32 = 28.;
const PAD: f32 = 4.;

/// Where a box sits in the graph, and the height of its first row's middle.
struct Placed {
    x: f32,
    y: f32,
    height: f32,
    joint: f32,
}

fn box_height(graph_box: &GraphBox) -> f32 {
    let title = if graph_box.title.is_some() { TITLE } else { 0. };
    title + JOB * graph_box.jobs.len() as f32 + PAD * 2.
}

/// The run's jobs in columns, each after every column it waits for, joined by lines
/// from a box to the boxes that wait for it.
fn graph(found: &OpenRun, chosen: Option<&str>, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let jobs: HashMap<&str, &Job> = found
        .jobs
        .iter()
        .map(|listed| (listed.job.id.as_str(), &listed.job))
        .collect();
    let mut placed: HashMap<String, Placed> = HashMap::new();
    let mut height: f32 = 0.;
    for (column, boxes) in found.graph.columns.iter().enumerate() {
        let x = column as f32 * (COLUMN + GAP);
        let mut y = 0.;
        for graph_box in boxes {
            let tall = box_height(graph_box);
            let title = if graph_box.title.is_some() { TITLE } else { 0. };
            placed.insert(
                graph_box.id.clone(),
                Placed {
                    x,
                    y,
                    height: tall,
                    joint: y + PAD + title + JOB / 2.,
                },
            );
            y += tall + BOX_GAP;
        }
        height = height.max(y);
    }
    let width = found.graph.columns.len() as f32 * (COLUMN + GAP) - GAP;
    let edges: Vec<(f32, f32, f32, f32)> = found
        .graph
        .edges
        .iter()
        .filter_map(|edge| {
            let (from, to) = (placed.get(&edge.from)?, placed.get(&edge.to)?);
            Some((from.x + COLUMN, from.joint, to.x, to.joint))
        })
        .collect();
    let line = t.border_strong;
    let boxes = found.graph.columns.iter().flatten().map(|graph_box| {
        let at = &placed[&graph_box.id];
        div()
            .absolute()
            .left(px(at.x))
            .top(px(at.y))
            .w(px(COLUMN))
            .h(px(at.height))
            .p(px(PAD))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface_raised)
            .children(graph_box.title.as_ref().map(|title| {
                div()
                    .h(px(TITLE))
                    .px(SPACE_1H)
                    .flex()
                    .items_center()
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_tertiary)
                    .truncate()
                    .child(title.clone())
            }))
            .children(graph_box.jobs.iter().enumerate().map(|(index, entry)| {
                let job = entry.job.as_deref().and_then(|id| jobs.get(id).copied());
                let outcome = match job {
                    Some(job) => outcome_word(job.outcome),
                    None if entry.skipped => "skipped",
                    None => "idle",
                };
                let took = job
                    .filter(|job| job.outcome != Outcome::Skipped)
                    .and_then(|job| {
                        Some(elapsed(
                            job.started_at.as_deref()?,
                            job.completed_at.as_deref(),
                        ))
                    });
                let on = job.is_some_and(|job| Some(job.id.as_str()) == chosen);
                let row = div()
                    .id(SharedString::from(format!(
                        "graph-{}-{index}",
                        graph_box.id
                    )))
                    .h(px(JOB))
                    .px(SPACE_1H)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(RADIUS_2)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(if job.is_some() {
                        t.text_primary
                    } else {
                        t.text_tertiary
                    })
                    .when(on, |row| row.bg(t.surface_chosen))
                    .child(ui::run_mark(
                        format!("graph-mark-{}-{index}", graph_box.id),
                        outcome,
                        14.,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(entry.label.clone()),
                    )
                    .children(took.map(|took| {
                        div()
                            .flex_none()
                            .text_size(crate::theme::ui_text(TEXT_3XS_H))
                            .font_features(theme::tabular())
                            .text_color(t.text_tertiary)
                            .child(took)
                    }));
                match job {
                    Some(job) => {
                        let id = job.id.clone();
                        row.cursor_pointer()
                            .hover(move |row| row.bg(t.surface_hover))
                            .on_click(
                                cx.listener(move |area, _, _, cx| area.show_job(id.clone(), cx)),
                            )
                            .into_any_element()
                    }
                    None => row.into_any_element(),
                }
            }))
    });
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_edges(&edges, line, bounds, window),
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full(),
        )
        .children(boxes)
        .into_any_element()
}

/// Lines out of a box sideways, along the middle of the gap, and into the box that
/// waits for it sideways, with a dot where each meets a box.
fn paint_edges(
    edges: &[(f32, f32, f32, f32)],
    ink: Hsla,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let origin = bounds.origin;
    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
    for &(x1, y1, x2, y2) in edges {
        let turn = x2 - GAP / 2.;
        let mut path = PathBuilder::stroke(px(1.2));
        path.move_to(at(x1, y1));
        if (y2 - y1).abs() < 0.5 {
            path.line_to(at(x2, y2));
        } else {
            let radius = 6f32.min((y2 - y1).abs() / 2.);
            let down = if y2 > y1 { 1. } else { -1. };
            path.line_to(at(turn - radius, y1));
            path.curve_to(at(turn, y1 + down * radius), at(turn, y1));
            path.line_to(at(turn, y2 - down * radius));
            path.curve_to(at(turn + radius, y2), at(turn, y2));
            path.line_to(at(x2, y2));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, ink);
        }
        for (x, y) in [(x1, y1), (x2, y2)] {
            let mut dot = PathBuilder::fill();
            dot.move_to(at(x + 3., y));
            dot.arc_to(point(px(3.), px(3.)), px(0.), false, true, at(x - 3., y));
            dot.arc_to(point(px(3.), px(3.)), px(0.), false, true, at(x + 3., y));
            if let Ok(dot) = dot.build() {
                window.paint_path(dot, ink);
            }
        }
    }
}
