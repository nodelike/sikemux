//! A job with its runs, and a project's activity.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_5};

impl Rundeck {
    pub(super) fn job(
        &mut self,
        host: &mut Host,
        job: &JobRef,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let page = host
            .view(
                "jobView",
                JobRequest {
                    job_id: job.job_id.clone(),
                    project: job.project.clone(),
                },
                cx,
            )
            .read::<JobPage>();
        let page = match page {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => {
                return crate::plugin_pane::failure(&failure.titled("Couldn't load runs"), cx)
            }
            Ok(Some(page)) => page,
        };
        let t = Tokens::get(cx).clone();
        let repeat = page.last_good.as_ref().map(|good| {
            (
                good.branch.clone(),
                page.repeat_options.clone(),
                good.execution_id,
            )
        });
        let (form_job, again_job) = (job.clone(), job.clone());
        let again_label = if page.runs_branch {
            match repeat.as_ref().and_then(|(branch, _, _)| branch.clone()) {
                Some(branch) => format!("Redeploy {branch}"),
                None => "Redeploy".into(),
            }
        } else {
            "Run again".into()
        };
        let tools = div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .children(page.permalink.clone().map(|url| {
                IconButton::new("rundeck-job-open", Icon::Link, "Open in Rundeck")
                    .on_click(move |_, _, cx| cx.open_url(&url))
            }))
            .child(
                Button::new("rundeck-again", again_label, Variant::Outline)
                    .icon(Icon::Fetch)
                    .disabled(repeat.is_none() || !page.execution_enabled)
                    .tooltip(match &repeat {
                        Some((_, _, id)) => format!("Prefill from #{id}"),
                        None => format!(
                            "No successful {} to repeat yet",
                            page.run_word.to_lowercase()
                        ),
                    })
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some((branch, given, _)) = &repeat {
                            rd.push(Level::Deploy {
                                job: again_job.clone(),
                                branch: branch.clone(),
                                given: given.clone(),
                            });
                            cx.notify();
                        }
                    })),
            )
            .child(
                Button::new("rundeck-start", page.run_word.clone(), Variant::Primary)
                    .icon(Icon::Run)
                    .disabled(!page.execution_enabled)
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        rd.push(Level::Deploy {
                            job: form_job.clone(),
                            branch: None,
                            given: BTreeMap::new(),
                        });
                        cx.notify();
                    })),
            )
            .child(refresh("rundeck-job-refresh", "jobView", cx))
            .into_any_element();
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            page.name.clone(),
            Some(folder_chip(
                &page.project,
                page.group.as_deref(),
                page.prod,
                cx,
            )),
            tools,
            cx,
        );
        let fact = |label: &'static str, value: String, under: Option<String>| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(ui::caps(label, cx))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(SPACE_1)
                        .child(ui::text(value, 13., FontWeight::MEDIUM).text_color(t.text_primary))
                        .children(under.map(|under| ui::meta(under, cx))),
                )
        };
        let good = page.last_good.as_ref();
        let mut facts = div().flex().gap(px(28.)).flex_wrap();
        if page.runs_branch {
            facts = facts.child(fact(
                "Live",
                good.and_then(|good| good.branch.clone())
                    .unwrap_or_else(|| "—".into()),
                None,
            ));
        }
        facts = facts
            .child(fact(
                if page.runs_branch {
                    "Deployed"
                } else {
                    "Last success"
                },
                good.and_then(|good| good.ended_at.as_deref())
                    .map(crate::ui::time::ago_written)
                    .unwrap_or_else(|| "—".into()),
                good.and_then(|good| good.user.clone())
                    .map(|user| format!("by {user}")),
            ))
            .child(fact(
                "Success",
                match page.finished {
                    0 => "—".into(),
                    finished => format!("{}%", page.succeeded * 100 / finished),
                },
                (page.finished > 0).then(|| format!("of {}", page.finished)),
            ))
            .child(fact(
                "Usual time",
                page.usual_secs
                    .map(|secs| took(secs as f64))
                    .unwrap_or_else(|| "—".into()),
                None,
            ))
            .children(
                page.fastest_secs
                    .zip(page.slowest_secs)
                    .map(|(fast, slow)| {
                        fact(
                            "Fastest · slowest",
                            format!("{} · {}", took(fast as f64), took(slow as f64)),
                            None,
                        )
                    }),
            );
        let tags = [
            (!page.execution_enabled).then(|| tag("executions disabled", t.warn, cx)),
            page.node_filter
                .as_ref()
                .map(|nodes| tag(&format!("nodes: {nodes}"), t.text_tertiary, cx)),
        ];
        let live = page.live.as_ref().map(|run| {
            let open = job.clone();
            let id = run.execution_id;
            live_card(run, page.usual_secs, cx).on_click(listen::<Rundeck, ClickEvent>(
                cx,
                move |rd, _, _, _, cx| {
                    rd.push(Level::Execution {
                        job: open.clone(),
                        id,
                    });
                    cx.notify();
                },
            ))
        });
        let history = if page.runs.is_empty() {
            crate::plugin_pane::empty(format!("{} hasn't run yet.", page.name), cx)
        } else {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_3H)
                .child(card(
                    if page.runs_branch {
                        "Deploy times"
                    } else {
                        "Run times"
                    },
                    format!("last {} runs", page.runs.len()),
                    times(&page.runs, page.usual_secs, job, cx),
                    cx,
                ))
                .child(card(
                    "History",
                    page.runs.len().to_string(),
                    history(&page.runs, false, job, cx),
                    cx,
                ))
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("rundeck-job")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_5)
                    .py(SPACE_3H)
                    .flex()
                    .flex_col()
                    .gap(SPACE_3H)
                    .children(page.description.clone().map(|description| {
                        ui::text(description, 12.5, FontWeight::NORMAL)
                            .text_color(t.text_secondary)
                            .whitespace_normal()
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(SPACE_1H)
                            .children(tags.into_iter().flatten()),
                    )
                    .child(facts)
                    .children(live)
                    .child(history),
            )
            .into_any_element()
    }

    pub(super) fn activity(
        &mut self,
        host: &mut Host,
        project: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let max = self.activity_max.max(ACTIVITY_PAGE);
        let page = host
            .view(
                "activityView",
                ActivityRequest {
                    project: project.to_owned(),
                    max,
                },
                cx,
            )
            .read::<ActivityPage>();
        let page = match page {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => {
                return crate::plugin_pane::failure(&failure.titled("Couldn't load runs"), cx)
            }
            Ok(Some(page)) => page,
        };
        let mut words = vec![
            ("All".into(), Some(page.runs.len() as u64)),
            ("Running".into(), Some(page.running as u64)),
            ("Failed".into(), Some(page.failed as u64)),
        ];
        let mut kinds = vec![RunsShown::All, RunsShown::Running, RunsShown::Failed];
        if page.me.is_some() {
            words.push(("Mine".into(), Some(page.mine as u64)));
            kinds.push(RunsShown::Mine);
        }
        let chosen = kinds
            .iter()
            .position(|kind| *kind == self.runs_shown)
            .unwrap_or_default();
        let picker = Segmented::words("rundeck-runs-shown", words, chosen)
            .on_select(listen::<Rundeck, usize>(cx, move |rd, _, index, _, cx| {
                rd.runs_shown = kinds.get(*index).copied().unwrap_or_default();
                cx.notify();
            }))
            .into_any_element();
        let shown: Vec<Run> = page
            .runs
            .iter()
            .filter(|run| match self.runs_shown {
                RunsShown::All => true,
                RunsShown::Running => run.live,
                RunsShown::Failed => run.status_kind == StatusKind::Failed,
                RunsShown::Mine => page.me.is_some() && run.user == page.me,
            })
            .cloned()
            .collect();
        let head = header(
            self.back(project, None, cx),
            "Activity",
            Some(picker),
            refresh("rundeck-activity-refresh", "activityView", cx),
            cx,
        );
        let more = page
            .total
            .is_some_and(|total| (page.runs.len() as u64) < total && max < ACTIVITY_MOST);
        let job = JobRef {
            project: project.to_owned(),
            job_id: String::new(),
            name: String::new(),
            group: None,
        };
        let body = if shown.is_empty() {
            crate::plugin_pane::empty(
                if page.runs.is_empty() {
                    format!("Nothing has run in {project} yet.")
                } else {
                    "No runs match this filter.".into()
                },
                cx,
            )
        } else {
            div()
                .id("rundeck-activity")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(SPACE_5)
                .py(SPACE_3H)
                .child(history(&shown, true, &job, cx))
                .when(more, |list| {
                    list.child(div().pt(SPACE_2H).flex().justify_center().child(
                        Button::new("rundeck-older", "Show older runs", Variant::Outline).on_click(
                            listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                                rd.activity_max = (max + ACTIVITY_PAGE).min(ACTIVITY_MOST);
                                cx.notify();
                            }),
                        ),
                    ))
                })
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// A way back to the folder's jobs, or up the stack.
    pub(super) fn back(
        &self,
        project: &str,
        group: Option<&str>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .child(
                IconButton::new("rundeck-back", Icon::Chevron, "Back")
                    .small()
                    .turned(std::f32::consts::PI)
                    .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                        rd.stack.pop();
                        rd.form = None;
                        cx.notify();
                    })),
            )
            .child(crumbs(project, group, cx))
            .into_any_element()
    }
}

/// Runs by day, newest first: each run's state, branch, who, when and how long against usual.
fn history(runs: &[Run], with_job: bool, job: &JobRef, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let mut days: Vec<(String, Vec<&Run>)> = Vec::new();
    for run in runs {
        let day = run
            .started_at
            .as_deref()
            .map(day_of)
            .unwrap_or_else(|| "Unknown date".into());
        match days.last_mut() {
            Some((last, runs)) if *last == day => runs.push(run),
            _ => days.push((day, vec![run])),
        }
    }
    div()
        .flex()
        .flex_col()
        .children(days.into_iter().map(|(day, runs)| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .gap(SPACE_1H)
                        .px(SPACE_3)
                        .pt(SPACE_2H)
                        .pb(SPACE_1)
                        .child(ui::caps(&day, cx))
                        .child(ui::meta(runs.len().to_string(), cx)),
                )
                .children(runs.into_iter().map(|run| {
                    let secs =
                        run_secs(run.started_at.as_deref(), run.ended_at.as_deref()).unwrap_or(0.);
                    let usual = run.usual_secs.map(|usual| usual as f64);
                    let scale = (usual.unwrap_or(secs) * 1.7).max(secs).max(1.);
                    let open = JobRef {
                        project: job.project.clone(),
                        job_id: run.job_id.clone().unwrap_or_else(|| job.job_id.clone()),
                        name: run.job_name.clone().unwrap_or_else(|| job.name.clone()),
                        group: if with_job {
                            run.job_group.clone()
                        } else {
                            job.group.clone()
                        },
                    };
                    let id = run.execution_id;
                    let clickable = !open.job_id.is_empty();
                    div()
                        .id(SharedString::from(format!("rundeck-run-{id}")))
                        .px(SPACE_3)
                        .py(px(5.))
                        .rounded(theme::RADIUS_2)
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .when(clickable, |row| {
                            row.cursor_pointer()
                                .hover(move |row| row.bg(hover))
                                .on_click(listen::<Rundeck, ClickEvent>(
                                    cx,
                                    move |rd, _, _, _, cx| {
                                        rd.push(Level::Execution {
                                            job: open.clone(),
                                            id,
                                        });
                                        cx.notify();
                                    },
                                ))
                        })
                        .child(status_dot(Some(run.status_kind), cx))
                        .child(ui::mono(format!("#{id}"), 11.5, cx).w(px(56.)).flex_none())
                        .when(with_job, |row| {
                            row.child(
                                ui::text(
                                    run.job_name.clone().unwrap_or_else(|| "—".into()),
                                    12.5,
                                    FontWeight::MEDIUM,
                                )
                                .text_color(t.text_primary)
                                .w(px(160.))
                                .flex_shrink(1.)
                                .min_w(px(60.))
                                .truncate(),
                            )
                        })
                        .child(
                            div()
                                .w(px(130.))
                                .flex_shrink(1.)
                                .min_w(px(40.))
                                .overflow_hidden()
                                .child(branch_chip(run.branch.as_deref(), cx)),
                        )
                        .child(
                            ui::meta(run.user.clone().unwrap_or_else(|| "—".into()), cx)
                                .w(px(84.))
                                .flex_shrink(1.)
                                .min_w(px(30.))
                                .truncate(),
                        )
                        .child(
                            ui::meta(
                                run.started_at.as_deref().map(clock_of).unwrap_or_default(),
                                cx,
                            )
                            .w(px(40.))
                            .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(48.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(t.line_soft)
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .rounded_full()
                                        .bg(status_ink(run.status_kind, &t))
                                        .w(gpui_kit::relative(((secs / scale).min(1.)) as f32)),
                                )
                                .children(usual.map(|usual| {
                                    div()
                                        .absolute()
                                        .top(px(-3.))
                                        .left(gpui_kit::relative((usual / scale) as f32))
                                        .w(px(1.5))
                                        .h(px(10.))
                                        .bg(t.text_tertiary)
                                })),
                        )
                        .child(
                            ui::meta(
                                if run.live {
                                    "running".into()
                                } else {
                                    took(secs)
                                },
                                cx,
                            )
                            .when(run.live, |meta| meta.text_color(t.warn))
                            .w(px(52.))
                            .flex_none()
                            .flex()
                            .justify_end(),
                        )
                }))
        }))
        .into_any_element()
}

/// One bar per run, oldest on the left, against the usual time.
fn times(
    runs: &[Run],
    usual: Option<u64>,
    job: &JobRef,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    const SLOTS: usize = 30;
    const HEIGHT: f32 = 96.;
    let t = Tokens::get(cx).clone();
    let shown: Vec<&Run> = runs.iter().take(SLOTS).rev().collect();
    let secs: Vec<f64> = shown
        .iter()
        .map(|run| run_secs(run.started_at.as_deref(), run.ended_at.as_deref()).unwrap_or(0.))
        .collect();
    let longest = secs
        .iter()
        .copied()
        .fold(60_f64.max(usual.unwrap_or(0) as f64 * 1.2), f64::max);
    let bars = shown.iter().zip(&secs).map(|(run, secs)| {
        let id = run.execution_id;
        let open = job.clone();
        let label = format!(
            "#{id}{} · {} · {}",
            run.branch
                .as_deref()
                .map(|branch| format!(" · {branch}"))
                .unwrap_or_default(),
            took(*secs),
            run.user.clone().unwrap_or_default()
        );
        div()
            .id(SharedString::from(format!("rundeck-bar-{id}")))
            .flex_1()
            .h_full()
            .flex()
            .items_end()
            .justify_center()
            .cursor_pointer()
            .tooltip(ui::tooltip(label))
            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                rd.push(Level::Execution {
                    job: open.clone(),
                    id,
                });
                cx.notify();
            }))
            .child(
                div()
                    .w(px(6.))
                    .rounded_t(px(2.))
                    .bg(status_ink(run.status_kind, &t))
                    .h(px((HEIGHT * (*secs / longest) as f32).max(3.))),
            )
    });
    let empty = (shown.len()..SLOTS).map(|_| div().flex_1());
    div()
        .relative()
        .h(px(HEIGHT))
        .flex()
        .gap(SPACE_0)
        .children(empty.map(IntoElement::into_any_element))
        .children(bars.map(IntoElement::into_any_element))
        .children(usual.map(|usual| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(HEIGHT * (usual as f64 / longest) as f32))
                .border_t_1()
                .border_color(t.text_tertiary)
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(-14.))
                        .child(ui::meta(format!("usual {}", took(usual as f64)), cx)),
                )
        }))
        .into_any_element()
}

/// The run of a job going on now, step by step; it opens the run.
fn live_card(
    run: &LiveRun,
    usual: Option<u64>,
    cx: &mut Context<PluginPane>,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let elapsed = since(run.started_at.as_deref()).unwrap_or(0.);
    let step = (run.total > 0).then(|| {
        format!(
            "step {} of {} · ",
            (run.completed + 1).min(run.total),
            run.total
        )
    });
    div()
        .id("rundeck-live-run")
        .p(SPACE_3)
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(div().size(px(7.)).rounded_full().bg(t.warn))
                .child(
                    ui::text(
                        match &run.branch {
                            Some(branch) => format!("Deploying {branch}"),
                            None => "Running".into(),
                        },
                        12.5,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                )
                .child(ui::meta(
                    format!(
                        "#{} · {}",
                        run.execution_id,
                        run.user.clone().unwrap_or_default()
                    ),
                    cx,
                ))
                .child(div().flex_1())
                .child(ui::meta(
                    format!(
                        "{}{}{}",
                        step.unwrap_or_default(),
                        took(elapsed),
                        usual
                            .map(|usual| format!(" of about {}", took(usual as f64)))
                            .unwrap_or_default()
                    ),
                    cx,
                )),
        )
        .when(!run.steps.is_empty(), |card| {
            card.child(
                div()
                    .flex()
                    .gap(SPACE_0)
                    .children(run.steps.iter().map(|step| {
                        div()
                            .flex_1()
                            .h(px(4.))
                            .rounded_full()
                            .bg(step_ink(step.state, &t))
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap(SPACE_0)
                    .children(run.steps.iter().map(|step| {
                        ui::meta(step.name.clone(), cx)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(step.state == StepState::Running, |meta| {
                                meta.text_color(t.text_primary)
                            })
                    })),
            )
        })
}
