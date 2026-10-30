//! What can be done to a run beyond reading it: its menu, approving what it waits on,
//! a job's own actions and what CI flagged in it, the summaries jobs wrote, the
//! workflow file, artifacts, and starting a workflow by hand.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::{Markdown, Pen};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{
    Annotation, Artifact, Job, JobSummary, OpenRun, Run, Workflow, WorkflowFile,
};

use super::{outcome_word, Asking, HostArea, Said};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    TEXT_2XS, TEXT_2XS_H, TEXT_XS,
};
use crate::ui::{self, Button, Choice, IconButton, Variant};

/// What pressing one of the dispatch sheet's buttons does.
type Pressed = Rc<dyn Fn(&mut Window, &mut App)>;

/// Summaries load for this many jobs until the person asks for the rest.
const SUMMARIES_FIRST: usize = 10;
/// Annotations show for a job before the rest fold behind a count.
const ANNOTATIONS_FIRST: usize = 3;

/// A run whose menu is open, from its own page or from its row in the list.
#[derive(Clone)]
pub struct MoreRun {
    id: String,
    url: String,
    number: u64,
    finished: bool,
}

impl MoreRun {
    pub fn of(run: &Run, finished: bool) -> Self {
        Self {
            id: run.id.clone(),
            url: run.url.clone(),
            number: run.run_number,
            finished,
        }
    }
}

#[derive(Default)]
pub struct Extras {
    annotations: HashMap<String, Vec<Annotation>>,
    asked_annotations: Vec<String>,
    all_annotations: bool,
    summaries: Option<Vec<(String, Rc<Markdown>)>>,
    all_summaries: bool,
    file_shown: bool,
    file: Option<Result<WorkflowFile, SharedString>>,
    artifacts: Option<Vec<Artifact>>,
    /// The side column shows the artifacts rather than the jobs.
    pub artifacts_page: bool,
    saving: Option<String>,
}

/// A workflow being started by hand: the branch or tag, and its inputs as
/// `name=value` lines, which its host checks.
pub struct Dispatch {
    workflow: Workflow,
    branch: Entity<InputState>,
    inputs: Entity<TextareaState>,
    busy: bool,
}

impl HostArea {
    fn extras(&mut self) -> Option<&mut Extras> {
        Some(&mut self.runs.open.as_mut()?.extras)
    }

    /// Reads what the run's view leaves out, as soon as it is known to be there.
    pub(super) fn read_extras(&mut self, cx: &mut Context<Self>) {
        let ci = self.host.ci.clone();
        let Some(opened) = &mut self.runs.open else {
            return;
        };
        let Some(Ok(view)) = &opened.view else {
            return;
        };
        let run_id = view.run.id.clone();
        let finished = view.run.status == "completed";
        let summaries: Vec<(String, String)> = view
            .summaries
            .iter()
            .filter_map(|id| view.jobs.iter().find(|listed| &listed.job.id == id))
            .filter_map(|listed| Some((listed.job.name.clone(), listed.job.check_run_id.clone()?)))
            .collect();
        let extras = &mut opened.extras;
        if ci.artifacts && finished && extras.artifacts.is_none() {
            extras.artifacts = Some(Vec::new());
            let found = self.plugin("artifacts", json!({ "runId": run_id }));
            cx.spawn(async move |area, cx| {
                let artifacts: Vec<Artifact> = found
                    .await
                    .ok()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                area.update(cx, |area, cx| {
                    if let Some(extras) = area.extras() {
                        extras.artifacts = Some(artifacts);
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        if ci.summaries
            && !summaries.is_empty()
            && self
                .runs
                .open
                .as_ref()
                .is_some_and(|opened| opened.extras.summaries.is_none())
        {
            self.read_summaries(summaries, cx);
        }
    }

    fn read_summaries(&mut self, jobs: Vec<(String, String)>, cx: &mut Context<Self>) {
        let everything = self
            .runs
            .open
            .as_ref()
            .is_some_and(|opened| opened.extras.all_summaries);
        let asked: Vec<_> = jobs
            .into_iter()
            .take(if everything {
                usize::MAX
            } else {
                SUMMARIES_FIRST
            })
            .map(|(name, check)| {
                (
                    name,
                    self.plugin("jobSummary", json!({ "checkRunId": check })),
                )
            })
            .collect();
        if let Some(extras) = self.extras() {
            extras.summaries = Some(Vec::new());
        }
        cx.spawn(async move |area, cx| {
            let mut found = Vec::new();
            for (name, summary) in asked {
                let summary: Option<JobSummary> = summary
                    .await
                    .ok()
                    .and_then(|value| serde_json::from_value(value).ok());
                if let Some(summary) = summary.filter(|summary| !summary.body.trim().is_empty()) {
                    found.push((name, Rc::new(crate::editor::preview::render(&summary.body))));
                }
            }
            area.update(cx, |area, cx| {
                if let Some(extras) = area.extras() {
                    extras.summaries = Some(found);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads what CI flagged in `job`, once.
    pub(super) fn read_annotations(&mut self, job: &Job, cx: &mut Context<Self>) {
        if !self.host.ci.annotations {
            return;
        }
        let Some(check) = job.check_run_id.clone() else {
            return;
        };
        let Some(extras) = self.extras() else {
            return;
        };
        if extras.asked_annotations.contains(&check) {
            return;
        }
        extras.asked_annotations.push(check.clone());
        let found = self.plugin("annotations", json!({ "checkRunId": check }));
        cx.spawn(async move |area, cx| {
            let annotations: Vec<Annotation> = found
                .await
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            area.update(cx, |area, cx| {
                if let Some(extras) = area.extras() {
                    extras.annotations.insert(check, annotations);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_file(&mut self, workflow: String, cx: &mut Context<Self>) {
        let Some(extras) = self.extras() else {
            return;
        };
        extras.file_shown = !extras.file_shown;
        if extras.file_shown && extras.file.is_none() {
            let found = self.plugin("workflowFile", json!({ "workflowId": workflow }));
            cx.spawn(async move |area, cx| {
                let file = found.await.and_then(|value| {
                    serde_json::from_value::<WorkflowFile>(value)
                        .map_err(|error| error.to_string().into())
                });
                area.update(cx, |area, cx| {
                    if let Some(extras) = area.extras() {
                        extras.file = Some(file);
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn rerun_job(&mut self, job: &Job, debug: bool, cx: &mut Context<Self>) {
        let name = job.name.clone();
        let params = json!({ "jobId": job.id, "debug": debug });
        self.run_write(
            "rerunJob",
            params,
            format!("Re-running {name}"),
            format!("Could not re-run {name}"),
            cx,
        );
    }

    fn answer_approval(&mut self, approve: bool, cx: &mut Context<Self>) {
        let Some(view) = self
            .runs
            .open
            .as_ref()
            .and_then(|opened| opened.view.as_ref()?.as_ref().ok())
        else {
            return;
        };
        let mine: Vec<_> = view
            .approvals
            .iter()
            .filter(|each| each.can_approve)
            .collect();
        let names = mine
            .iter()
            .map(|each| each.environment.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let ids: Vec<u64> = mine.iter().map(|each| each.environment_id).collect();
        let state = if approve { "approved" } else { "rejected" };
        let params = json!({ "runId": view.run.id, "environmentIds": ids, "state": state });
        let done = if approve {
            format!("Approved {names}")
        } else {
            format!("Rejected {names}")
        };
        self.run_write(
            "reviewDeployment",
            params,
            done,
            "Could not answer the deployment".into(),
            cx,
        );
    }

    fn save_artifact(&mut self, artifact: &Artifact, cx: &mut Context<Self>) {
        let Some(extras) = self.extras() else {
            return;
        };
        extras.saving = Some(artifact.id.clone());
        let name = artifact.name.clone();
        let saved = self.download(
            "downloadArtifact",
            json!({ "artifactId": artifact.id, "fileName": name }),
        );
        cx.spawn(async move |area, cx| {
            let saved = saved.await;
            area.update(cx, |area, cx| {
                if let Some(extras) = area.extras() {
                    extras.saving = None;
                }
                let said = match saved {
                    Ok(path) => Said::Done(format!("Saved {name} to {path}").into()),
                    Err(error) => {
                        Said::Failed(format!("Could not download {name}: {error}").into())
                    }
                };
                area.say(said, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Opens the sheet that starts `workflow` by hand.
    pub(super) fn start_dispatch(
        &mut self,
        workflow: Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let branch = self
            .pulls
            .branch
            .as_ref()
            .map(|(branch, _)| branch.clone())
            .unwrap_or_default();
        let field = cx.new(|cx| {
            let mut field = InputState::new(window, cx).placeholder("main");
            field.set_value(branch, window, cx);
            field
        });
        let inputs = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Inputs, one name=value a line")
        });
        self.runs.dispatching = Some(Dispatch {
            workflow,
            branch: field,
            inputs,
            busy: false,
        });
        cx.notify();
    }

    fn dispatch(&mut self, cx: &mut Context<Self>) {
        let Some(dispatch) = &mut self.runs.dispatching else {
            return;
        };
        let git_ref = dispatch.branch.read(cx).value().trim().to_owned();
        if git_ref.is_empty() || dispatch.busy {
            return;
        }
        let mut inputs = serde_json::Map::new();
        for line in dispatch.inputs.read(cx).value().lines() {
            if let Some((name, value)) = line.split_once('=') {
                if !name.trim().is_empty() {
                    inputs.insert(name.trim().to_owned(), Value::from(value.trim()));
                }
            }
        }
        dispatch.busy = true;
        let name = dispatch.workflow.name.clone();
        let params =
            json!({ "workflowId": dispatch.workflow.id, "gitRef": git_ref, "inputs": inputs });
        let started = self.plugin("dispatch", params);
        cx.spawn(async move |area, cx| {
            let started = started.await;
            area.update(cx, |area, cx| {
                match started {
                    Ok(_) => {
                        area.runs.dispatching = None;
                        area.say(
                            Said::Done(format!("Started {name} on {git_ref}").into()),
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(dispatch) = &mut area.runs.dispatching {
                            dispatch.busy = false;
                        }
                        area.say(
                            Said::Failed(format!("Could not start {name}: {error}").into()),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

pub fn more_button(view: &OpenRun, cx: &mut Context<HostArea>) -> impl IntoElement {
    more_of("run-more", MoreRun::of(&view.run, !view.unfinished), cx)
}

/// The button that opens `run`'s menu.
pub fn more_of(
    id: impl Into<gpui_kit::ElementId>,
    run: MoreRun,
    cx: &mut Context<HostArea>,
) -> impl IntoElement {
    IconButton::new(id, Icon::More, "More run actions")
        .small()
        .on_click(cx.listener(move |area, _, _, cx| {
            cx.stop_propagation();
            area.runs.more = Some(run.clone());
            cx.notify();
        }))
}

/// A run's menu: debugging, its link, and deleting its logs or itself, the last three only
/// for an account that may write CI.
pub fn more_choices(area: &HostArea, run: &MoreRun, cx: &mut Context<HostArea>) -> Vec<Choice> {
    let ci = &area.host.ci;
    let finished = run.finished;
    let writable = area.can_write_ci();
    let host = area.host.label.as_str();
    let this = cx.entity().downgrade();
    let act = move |then: super::Then| {
        let this = this.clone();
        move |_: &mut Window, cx: &mut App| {
            let then = then.clone();
            this.update(cx, |area, cx| {
                area.runs.more = None;
                then(area, cx);
                cx.notify();
            })
            .ok();
        }
    };
    let (id, url, number) = (run.id.clone(), run.url.clone(), run.number);
    let mut choices = Vec::new();
    if writable && ci.debug_logs && finished {
        let id = id.clone();
        choices.push(Choice::new(
            None,
            "Re-run all with debug logs",
            act(Rc::new(move |area, cx| {
                let params = json!({ "runId": id, "failedOnly": false, "debug": true });
                area.run_write(
                    "rerun",
                    params,
                    "Re-running everything with debug logs".into(),
                    "Could not re-run it".into(),
                    cx,
                );
            })),
        ));
    }
    let copied = url.clone();
    choices.push(Choice::new(
        None,
        "Copy link",
        act(Rc::new(move |area, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copied.clone()));
            area.say(Said::Done("Copied the link".into()), cx);
        })),
    ));
    choices.push(Choice::new(
        None,
        format!("Open on {host}"),
        act(Rc::new(move |_, cx| cx.open_url(&url))),
    ));
    if writable && ci.delete_logs && finished {
        let id = id.clone();
        choices.push(
            Choice::new(
                None,
                "Delete all logs",
                act(Rc::new(move |area, cx| {
                    let id = id.clone();
                    area.ask(
                        Asking::new(
                            "Delete this run's logs?",
                            format!("Every job log from run #{number} goes for good. The run itself stays."),
                            "Delete logs",
                            true,
                            move |area, cx| {
                                area.run_write(
                                    "deleteRunLogs",
                                    json!({ "runId": id }),
                                    "Deleted the logs".into(),
                                    "Could not delete the logs".into(),
                                    cx,
                                )
                            },
                        ),
                        cx,
                    );
                })),
            )
            .destructive(),
        );
    }
    if writable && ci.delete_runs && finished {
        choices.push(
            Choice::new(
                None,
                "Delete run",
                act(Rc::new(move |area, cx| {
                    let id = id.clone();
                    area.ask(
                        Asking::new(
                            "Delete this run?",
                            format!("Run #{number}, its logs and its artifacts go for good."),
                            "Delete run",
                            true,
                            move |area, cx| {
                                area.run_write(
                                    "deleteRun",
                                    json!({ "runId": id }),
                                    format!("Deleted run #{number}"),
                                    "Could not delete the run".into(),
                                    cx,
                                );
                                if area.runs.open.as_ref().is_some_and(|open| open.id == id) {
                                    area.runs.open = None;
                                }
                                area.watch();
                            },
                        ),
                        cx,
                    );
                })),
            )
            .destructive(),
        );
    }
    choices
}

/// What the run waits on someone to approve, with the answer when it is the person's
/// to give.
pub fn approvals(area: &HostArea, view: &OpenRun, cx: &mut Context<HostArea>) -> Option<Div> {
    let t = Tokens::get(cx).clone();
    if !area.host.ci.approvals || view.approvals.is_empty() {
        return None;
    }
    let pending = &view.approvals;
    let names = pending
        .iter()
        .map(|each| each.environment.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let wait = pending
        .iter()
        .map(|each| each.wait_minutes)
        .max()
        .unwrap_or(0);
    let mine = pending.iter().any(|each| each.can_approve);
    let busy = area.runs.open.as_ref().is_some_and(|opened| opened.busy);
    let who = pending
        .iter()
        .flat_map(|each| each.reviewers.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let to_reject = pending
        .iter()
        .filter(|each| each.can_approve)
        .map(|each| each.environment.clone())
        .collect::<Vec<_>>()
        .join(", ");
    Some(
        div()
            .mx(SPACE_2H)
            .mb(SPACE_2)
            .px(SPACE_2H)
            .py(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.warn.opacity(0.34))
            .bg(t.warn.opacity(0.08))
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .child(
                ui::text("Waiting for approval", 12., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary),
            )
            .child(ui::meta(
                if wait > 0 {
                    format!("{names} · {wait}m wait")
                } else {
                    names
                },
                cx,
            ))
            .when(!mine, |callout| {
                callout.child(ui::meta(
                    if who.is_empty() {
                        "Someone else has to sign this off.".to_owned()
                    } else {
                        who
                    },
                    cx,
                ))
            })
            .when(mine, |callout| {
                callout.child(
                    div()
                        .flex()
                        .gap(SPACE_1H)
                        .child(
                            Button::new("approval-reject", "Reject", Variant::Outline)
                                .disabled(busy)
                                .on_click(cx.listener(move |area, _, _, cx| {
                                    area.ask(
                                        Asking::new(
                                            "Reject this deployment?",
                                            format!("{to_reject} will not run."),
                                            "Reject",
                                            true,
                                            |area, cx| area.answer_approval(false, cx),
                                        ),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new("approval-approve", "Approve", Variant::Primary)
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|area, _, _, cx| area.answer_approval(true, cx)),
                                ),
                        ),
                )
            }),
    )
}

/// The picked job's own head: how it went, where it ran, and running it again.
pub fn job_bar(area: &HostArea, job: &Job, cx: &mut Context<HostArea>) -> Div {
    let t = Tokens::get(cx).clone();
    let ci = &area.host.ci;
    let host = area.host.label.as_str();
    let completed = job.status == "completed";
    let busy = area.runs.open.as_ref().is_some_and(|opened| opened.busy);
    let url = job.url.clone();
    div()
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(ui::run_mark(
            format!("job-head-{}", job.id),
            outcome_word(job.outcome),
            14.,
            cx,
        ))
        .child(ui::text(job.name.clone(), 12.5, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .children(
            job.runner
                .as_ref()
                .map(|runner| ui::meta(format!("on {runner}"), cx)),
        )
        .child(div().flex_1())
        .when(ci.rerun_job && completed, |bar| {
            let rerun = job.clone();
            bar.child(
                Button::new("job-rerun", "Re-run this job", Variant::Outline)
                    .icon(Icon::Refresh)
                    .disabled(busy)
                    .on_click(cx.listener(move |area, _, _, cx| area.rerun_job(&rerun, false, cx))),
            )
        })
        .when(ci.rerun_job && ci.debug_logs && completed, |bar| {
            let rerun = job.clone();
            bar.child(
                Button::new("job-debug", "With debug logs", Variant::Outline)
                    .disabled(busy)
                    .on_click(cx.listener(move |area, _, _, cx| area.rerun_job(&rerun, true, cx))),
            )
        })
        .children(url.map(|url| {
            Button::new("job-on-host", host.to_owned(), Variant::Outline)
                .icon(Icon::Globe)
                .on_click(move |_, _, cx| cx.open_url(&url))
        }))
}

/// What CI flagged in the job, a row each, the first few until asked for all.
pub fn annotations(area: &HostArea, job: &Job, cx: &mut Context<HostArea>) -> Option<Div> {
    let t = Tokens::get(cx).clone();
    let extras = &area.runs.open.as_ref()?.extras;
    let found = extras.annotations.get(job.check_run_id.as_ref()?)?;
    if found.is_empty() {
        return None;
    }
    let shown = if extras.all_annotations {
        found.len()
    } else {
        found.len().min(ANNOTATIONS_FIRST)
    };
    let rest = found.len() - shown;
    let project = area.project.clone();
    Some(
        div()
            .px(SPACE_3)
            .py(SPACE_1H)
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .border_b_1()
            .border_color(t.border)
            .children(
                found
                    .iter()
                    .take(shown)
                    .enumerate()
                    .map(|(index, annotation)| {
                        let (glyph, ink) = match annotation.level.as_str() {
                            "failure" => (Icon::Close, t.danger),
                            "warning" => (Icon::Warning, t.warn),
                            _ => (Icon::Eye, t.text_tertiary),
                        };
                        let place =
                            annotation
                                .path
                                .as_ref()
                                .map(|path| match annotation.start_line {
                                    Some(line) => format!("{path}:{line}"),
                                    None => path.clone(),
                                });
                        let open = annotation.path.clone().map(|path| crate::editor::OpenAt {
                            path: std::path::Path::new(&project).join(path),
                            line: annotation.start_line.unwrap_or(1).saturating_sub(1) as u32,
                            column: None,
                        });
                        div()
                            .flex()
                            .items_start()
                            .gap(SPACE_2)
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .child(
                                div()
                                    .pt(SPACE_0)
                                    .child(ui::icon(glyph, 13.).text_color(ink)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(t.text_secondary)
                                    .children(annotation.title.as_ref().map(|title| {
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text_primary)
                                            .child(title.clone())
                                    }))
                                    .child(annotation.message.clone()),
                            )
                            .children(place.map(|place| {
                                div()
                                    .id(("annotation-place", index))
                                    .flex_none()
                                    .font_family(theme::MONO)
                                    .text_size(crate::theme::ui_text(TEXT_2XS))
                                    .text_color(t.text_tertiary)
                                    .when_some(open, |place, open| {
                                        place
                                            .cursor_pointer()
                                            .hover(move |style| style.text_color(t.text_primary))
                                            .tooltip(ui::tooltip("Open it in the editor"))
                                            .on_click(move |_, window, cx| {
                                                window.dispatch_action(Box::new(open.clone()), cx)
                                            })
                                    })
                                    .child(place)
                            }))
                    }),
            )
            .when(
                rest > 0 || extras.all_annotations && found.len() > ANNOTATIONS_FIRST,
                |list| {
                    list.child(
                        div()
                            .id("annotations-more")
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.accent)
                            .cursor_pointer()
                            .on_click(cx.listener(|area, _, _, cx| {
                                if let Some(extras) = area.extras() {
                                    extras.all_annotations = !extras.all_annotations;
                                }
                                cx.notify();
                            }))
                            .child(if rest > 0 {
                                format!("{rest} more")
                            } else {
                                "Show fewer".into()
                            }),
                    )
                },
            ),
    )
}

/// The link that shows the file a run came from, beside its graph's head.
pub fn file_toggle(
    area: &HostArea,
    view: &OpenRun,
    cx: &mut Context<HostArea>,
) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let path = view.run.path.as_ref()?;
    if !area.host.ci.workflow_file || path.starts_with("dynamic/") {
        return None;
    }
    let shown = area
        .runs
        .open
        .as_ref()
        .is_some_and(|opened| opened.extras.file_shown);
    let workflow = view.run.workflow_id.clone();
    Some(
        div()
            .id("run-file")
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(t.accent)
            .cursor_pointer()
            .on_click(cx.listener(move |area, _, _, cx| area.toggle_file(workflow.clone(), cx)))
            .child(if shown {
                "Hide workflow file"
            } else {
                "Workflow file"
            })
            .into_any_element(),
    )
}

/// The workflow file, while it is shown, then each summary a job wrote.
pub fn below_graph(area: &HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> Div {
    let t = Tokens::get(cx).clone();
    let Some(extras) = area.runs.open.as_ref().map(|opened| &opened.extras) else {
        return div();
    };
    let file = extras.file_shown.then(|| match &extras.file {
        None => ui::meta("Reading the workflow file…", cx).into_any_element(),
        Some(Err(error)) => {
            ui::meta(format!("Could not read the workflow file: {error}"), cx).into_any_element()
        }
        Some(Ok(file)) => div()
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .px(SPACE_2H)
                    .py(SPACE_1H)
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .font_family(theme::MONO)
                    .child(file.path.clone()),
            )
            .child(
                div()
                    .px(SPACE_2H)
                    .py(SPACE_2)
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_primary)
                    .children(
                        file.text
                            .lines()
                            .map(|line| div().whitespace_nowrap().child(line.to_owned())),
                    ),
            )
            .into_any_element(),
    });
    let pen = Pen::new(Rc::new(crate::editor::preview::reading_linked(
        13.,
        &area.bench,
        cx,
    )));
    let summaries = extras
        .summaries
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, (name, body))| {
            div()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .px(SPACE_3)
                        .py(px(7.))
                        .border_b_1()
                        .border_color(t.border)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .child(format!("{name} summary")),
                )
                .child(div().px(SPACE_3H).py(SPACE_2H).child(pen.markdown(
                    ("run-summary", index),
                    body,
                    window,
                    cx,
                )))
        })
        .collect::<Vec<_>>();
    let more = area
        .runs
        .open
        .as_ref()
        .and_then(|opened| opened.view.as_ref()?.as_ref().ok())
        .map(|view| view.summaries.len())
        .filter(|total| !extras.all_summaries && *total > SUMMARIES_FIRST)
        .map(|total| {
            let rest = total - SUMMARIES_FIRST;
            div()
                .id("summaries-more")
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.accent)
                .cursor_pointer()
                .on_click(cx.listener(|area, _, _, cx| {
                    if let Some(extras) = area.extras() {
                        extras.all_summaries = true;
                        extras.summaries = None;
                    }
                    area.read_extras(cx);
                    cx.notify();
                }))
                .child(format!(
                    "Look for summaries from {rest} more job{}",
                    if rest == 1 { "" } else { "s" }
                ))
        });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .children(file)
        .children(summaries)
        .children(more)
}

/// Whether the run has artifacts to list beside its jobs.
pub fn artifact_count(area: &HostArea) -> usize {
    area.runs
        .open
        .as_ref()
        .and_then(|opened| opened.extras.artifacts.as_ref())
        .map_or(0, Vec::len)
}

pub fn artifacts(area: &HostArea, cx: &mut Context<HostArea>) -> Div {
    let t = Tokens::get(cx).clone();
    let Some(extras) = area.runs.open.as_ref().map(|opened| &opened.extras) else {
        return div();
    };
    div()
        .flex()
        .flex_col()
        .children(
            extras
                .artifacts
                .iter()
                .flatten()
                .enumerate()
                .map(|(index, artifact)| {
                    let saving = extras.saving.as_deref() == Some(artifact.id.as_str());
                    let saved = artifact.clone();
                    div()
                        .h(px(28.))
                        .px(SPACE_3)
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .child(ui::icon(Icon::Download, 13.).text_color(t.text_tertiary))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(artifact.name.clone()),
                        )
                        .child(ui::meta(bytes(artifact.size_bytes), cx))
                        .child(if artifact.expired {
                            ui::meta("expired", cx).into_any_element()
                        } else {
                            div()
                                .id(("artifact-save", index))
                                .text_color(t.accent)
                                .cursor_pointer()
                                .when(!saving, |link| {
                                    link.on_click(cx.listener(move |area, _, _, cx| {
                                        area.save_artifact(&saved, cx)
                                    }))
                                })
                                .child(if saving { "Saving…" } else { "Download" })
                                .into_any_element()
                        })
                }),
        )
}

pub(crate) fn bytes(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The sheet that starts a workflow by hand.
pub fn dispatch_sheet(area: &HostArea, cx: &mut Context<HostArea>) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let dispatch = area.runs.dispatching.as_ref()?;
    let this = cx.entity().downgrade();
    let (yes, no) = (this.clone(), this);
    let start: Pressed = Rc::new(move |_, cx| {
        yes.update(cx, |area, cx| area.dispatch(cx)).ok();
    });
    let close: Pressed = Rc::new(move |_, cx| {
        no.update(cx, |area, cx| {
            area.runs.dispatching = None;
            cx.notify();
        })
        .ok();
    });
    let body = ui::dialog::marked_body()
        .pt(SPACE_3)
        .gap(SPACE_2H)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(ui::meta("Branch or tag", cx))
        .child(
            div()
                .px(SPACE_2H)
                .py(SPACE_1H)
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .font_family(theme::MONO)
                .child(Input::new(&dispatch.branch).appearance(false)),
        )
        .child(ui::meta("Its host says if an input is wrong.", cx))
        .child(
            div()
                .px(SPACE_2H)
                .py(SPACE_1H)
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .font_family(theme::MONO)
                .child(Textarea::new(&dispatch.inputs).appearance(false)),
        );
    let foot = div()
        .child(ui::dialog::button(
            "dispatch-cancel",
            "Cancel".into(),
            None,
            close.clone(),
            cx,
        ))
        .child(ui::dialog::button(
            "dispatch-start",
            if dispatch.busy {
                "Starting…".into()
            } else {
                "Run workflow".into()
            },
            Some(t.accent),
            start.clone(),
            cx,
        ));
    Some(
        ui::dialog::marked_sheet(
            Icon::Run,
            format!("Run {}", dispatch.workflow.name).into(),
            body.into_any_element(),
            foot,
            start,
            close,
            cx,
        )
        .into_any_element(),
    )
}
