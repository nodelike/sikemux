//! A folder's jobs, each with its last runs.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3};

impl Rundeck {
    pub(super) fn jobs(
        &mut self,
        jobs: Option<Result<Option<Rc<Jobs>>, Failure>>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Some(jobs) = jobs else {
            return crate::plugin_pane::empty(
                "Pick a Rundeck project from the list on the left.",
                cx,
            );
        };
        let jobs = match jobs {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
            Ok(Some(jobs)) => jobs,
        };
        let t = Tokens::get(cx).clone();
        let filter = self.filter_input(window, cx);
        let query = filter.read(cx).value().trim().to_lowercase();
        let compare = self.compare && !jobs.compare.is_empty();
        let by_id: HashMap<&str, &JobRow> = jobs
            .jobs
            .iter()
            .map(|job| (job.job_id.as_str(), job))
            .collect();
        let shown = |job: &JobRow| {
            (query.is_empty() || job.name.to_lowercase().contains(&query))
                && match self.jobs_shown {
                    JobsShown::All => true,
                    JobsShown::Running => job.latest.as_ref().is_some_and(|run| run.live),
                    JobsShown::Failed => job
                        .latest
                        .as_ref()
                        .is_some_and(|run| run.status_kind == StatusKind::Failed),
                }
        };
        let sections: Vec<(&Section, Vec<&JobRow>)> =
            (if compare { &jobs.compare } else { &jobs.list })
                .iter()
                .map(|section| {
                    let rows = section
                        .jobs
                        .iter()
                        .filter_map(|id| by_id.get(id.as_str()).copied())
                        .filter(|job| shown(job))
                        .collect::<Vec<_>>();
                    (section, rows)
                })
                .filter(|(_, rows)| !rows.is_empty())
                .collect();
        let total = jobs.jobs.len() as u64;
        let picker = if compare {
            ui::meta(
                format!(
                    "{} {} in more than one folder",
                    jobs.shared,
                    if jobs.shared == 1 { "job" } else { "jobs" }
                ),
                cx,
            )
            .into_any_element()
        } else {
            let chosen = [JobsShown::All, JobsShown::Running, JobsShown::Failed]
                .iter()
                .position(|shown| *shown == self.jobs_shown)
                .unwrap_or_default();
            Segmented::words(
                "rundeck-jobs-shown",
                vec![
                    ("All".into(), Some(total)),
                    ("Running".into(), Some(jobs.running as u64)),
                    ("Failed".into(), Some(jobs.failed as u64)),
                ],
                chosen,
            )
            .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
                rd.jobs_shown = [JobsShown::All, JobsShown::Running, JobsShown::Failed][*index];
                cx.notify();
            }))
            .into_any_element()
        };
        let layout = (!jobs.compare.is_empty()).then(|| {
            Segmented::words(
                "rundeck-layout",
                vec![("List".into(), None), ("Compare folders".into(), None)],
                usize::from(compare),
            )
            .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
                rd.compare = *index == 1;
                cx.notify();
            }))
        });
        let head = header(
            crumbs(&jobs.project, jobs.group.as_deref(), cx),
            jobs.title.clone(),
            Some(picker),
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .children(layout)
                .child(
                    div()
                        .w(px(200.))
                        .child(ui::Field::new(&filter).icon(Icon::Search)),
                )
                .child(refresh("rundeck-jobs-refresh", "jobsView", cx))
                .into_any_element(),
            cx,
        );
        let mut banners = Vec::new();
        if let Some(error) = &jobs.error {
            banners.push(banner(error.clone(), t.warn, cx));
        }
        if jobs.partial {
            let timed_out = match jobs.timed_out {
                0 => String::new(),
                1 => ": 1 job timed out".into(),
                count => format!(": {count} jobs timed out"),
            };
            banners.push(banner(
                format!("Rundeck was slow to answer{timed_out}. Those rows show no recent runs; refresh to try again."),
                t.warn,
                cx,
            ));
        }
        let body = if jobs.jobs.is_empty() {
            crate::plugin_pane::empty(
                match &jobs.group {
                    Some(group) => format!("No jobs in {group}."),
                    None => format!("No jobs in {}.", jobs.project),
                },
                cx,
            )
        } else if sections.is_empty() {
            crate::plugin_pane::empty("No jobs match.", cx)
        } else if compare {
            compare_grid(&sections, jobs.loose, cx)
        } else {
            let many = sections.len() > 1;
            let mut rows = Vec::new();
            for (section, jobs_in) in &sections {
                if many || section.segment.is_some() {
                    let (project, path) = (self.project.clone(), section.path.clone());
                    let link = section.segment.is_some();
                    rows.push(
                        div()
                            .id(SharedString::from(format!(
                                "rundeck-section-{}",
                                section.path
                            )))
                            .px(SPACE_3)
                            .pt(SPACE_3)
                            .pb(SPACE_1)
                            .when(link, |head| {
                                head.cursor_pointer()
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        move |rd, _, _, _, cx| {
                                            rd.pick(&project, Some(path.clone()));
                                            cx.notify();
                                        },
                                    ))
                            })
                            .child(
                                ui::caps(&section.label, cx)
                                    .when(section.prod, |caps| caps.text_color(t.warn)),
                            )
                            .into_any_element(),
                    );
                }
                for job in jobs_in {
                    rows.push(self.job_row(job, &jobs.project, cx));
                }
            }
            div()
                .id("rundeck-jobs")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(SPACE_1H)
                .pb(SPACE_3)
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .children(banners)
            .child(body)
            .into_any_element()
    }

    fn filter_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter jobs"));
        self.typing
            .push(cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()));
        self.filter = Some(filter.clone());
        filter
    }

    fn job_row(&self, job: &JobRow, project: &str, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let hover = t.surface_hover;
        let job_ref = JobRef {
            project: project.to_owned(),
            job_id: job.job_id.clone(),
            name: job.name.clone(),
            group: job.group.clone(),
        };
        let (open, deploy) = (job_ref.clone(), job_ref);
        let branch = job
            .deployed
            .as_ref()
            .and_then(|run| run.branch.clone())
            .or_else(|| job.latest.as_ref().and_then(|run| run.branch.clone()));
        let when = job
            .latest
            .as_ref()
            .and_then(|run| run.ended_at.as_deref().or(run.started_at.as_deref()))
            .map(crate::ui::time::ago_written)
            .unwrap_or_else(|| "—".into());
        let took_for = job
            .latest
            .as_ref()
            .and_then(|run| run_secs(run.started_at.as_deref(), run.ended_at.as_deref()))
            .map(took)
            .unwrap_or_else(|| "—".into());
        let tags = [
            job.scheduled.then(|| tag("scheduled", t.text_tertiary, cx)),
            (!job.enabled).then(|| tag("disabled", t.text_disabled, cx)),
            job.error.as_ref().map(|error| {
                tag(
                    if error == "timed out" {
                        "timed out"
                    } else {
                        "error"
                    },
                    t.warn,
                    cx,
                )
            }),
        ];
        div()
            .id(SharedString::from(format!("rundeck-job-{}", job.job_id)))
            .px(SPACE_2)
            .py(SPACE_1H)
            .rounded(theme::RADIUS_2)
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .when(!job.enabled, |row| row.opacity(0.6))
            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                rd.push(Level::Job(open.clone()));
                cx.notify();
            }))
            .child(status_dot(
                job.latest.as_ref().map(|run| run.status_kind),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .child(
                                ui::text(job.name.clone(), 12.5, FontWeight::MEDIUM)
                                    .text_color(t.text_primary)
                                    .min_w_0()
                                    .truncate(),
                            )
                            .children(tags.into_iter().flatten()),
                    )
                    .child(
                        ui::meta(
                            match &job.latest {
                                Some(run) => format!(
                                    "{} · {when} · took {took_for}",
                                    run.user.clone().unwrap_or_else(|| "—".into())
                                ),
                                None => "Never run".into(),
                            },
                            cx,
                        )
                        .truncate(),
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_none()
                    .overflow_hidden()
                    .child(branch_chip(job.branch.as_deref(), cx)),
            )
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(job.enabled, |slot| {
                        slot.child(
                            Button::new(
                                SharedString::from(format!("rundeck-start-{}", job.job_id)),
                                job.run_word.clone(),
                                Variant::Outline,
                            )
                            .icon(Icon::Run)
                            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                                rd.push(Level::Deploy {
                                    job: deploy.clone(),
                                    branch: branch.clone(),
                                    given: BTreeMap::new(),
                                });
                                cx.stop_propagation();
                                cx.notify();
                            })),
                        )
                    }),
            )
            .into_any_element()
    }
}
