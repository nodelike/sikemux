//! A run as it goes: its steps and the output streamed in as it comes.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_5};

impl Rundeck {
    pub(super) fn execution(
        &mut self,
        host: &mut Host,
        job: &JobRef,
        id: u64,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let page = host
            .view(
                "executionView",
                ExecutionRequest {
                    execution_id: id,
                    job_id: job.job_id.clone(),
                    project: job.project.clone(),
                },
                cx,
            )
            .read::<ExecutionPage>();
        if self.run.as_ref().is_none_or(|run| run.id != id) {
            self.run = Some(Output::new(id));
            self.follow_output(host, cx);
        }
        let page = match page {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
            Ok(Some(page)) => page,
        };
        let t = Tokens::get(cx).clone();
        let watching = self.watching.contains_key(&id);
        let (abort_job, watch_job, again_job) = (job.clone(), job.clone(), job.clone());
        let again = page
            .can_run_again
            .then(|| (page.branch.clone(), page.repeat_options.clone()));
        let status = match (page.live, page.total) {
            (true, total) if total > 0 => format!(
                "{} · step {} of {total}",
                capitalised(&page.status),
                (page.completed + 1).min(total)
            ),
            _ => capitalised(&page.status),
        };
        let tools =
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    Button::new(
                        "rundeck-run-again",
                        format!("{} again", page.run_word),
                        Variant::Outline,
                    )
                    .icon(Icon::Fetch)
                    .disabled(again.is_none())
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some((branch, given)) = &again {
                            rd.push(Level::Deploy {
                                job: again_job.clone(),
                                branch: branch.clone(),
                                given: given.clone(),
                            });
                            cx.notify();
                        }
                    })),
                )
                .when(page.live, |tools| {
                    tools
                        .child(
                            Button::new(
                                "rundeck-notify",
                                if watching { "Will notify" } else { "Notify me" },
                                Variant::Outline,
                            )
                            .icon(Icon::Clock)
                            .disabled(watching)
                            .on_click(listen::<Rundeck, ClickEvent>(
                                cx,
                                move |rd, host, _, _, cx| {
                                    rd.watch(host, &watch_job, id, cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Button::new(
                                "rundeck-abort",
                                if self.aborting == Some(id) {
                                    "Aborting…"
                                } else {
                                    "Abort"
                                },
                                Variant::Outline,
                            )
                            .icon(Icon::Stop)
                            .disabled(self.aborting == Some(id))
                            .on_click(listen::<Rundeck, ClickEvent>(
                                cx,
                                move |_, host, _, _, cx| {
                                    abort(host, &abort_job, id, cx);
                                },
                            )),
                        )
                })
                .children(page.permalink.clone().map(|url| {
                    IconButton::new("rundeck-run-open", Icon::Link, "Open in Rundeck")
                        .on_click(move |_, _, cx| cx.open_url(&url))
                }))
                .into_any_element();
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            format!("#{id}"),
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(folder_chip(
                        &job.project,
                        job.group.as_deref(),
                        page.prod,
                        cx,
                    ))
                    .child(status_dot(Some(page.status_kind), cx))
                    .child(
                        ui::text(status, 12., FontWeight::MEDIUM)
                            .text_color(status_ink(page.status_kind, &t)),
                    )
                    .into_any_element(),
            ),
            tools,
            cx,
        );
        let elapsed = run_secs(page.started_at.as_deref(), page.ended_at.as_deref());
        let left = match (page.live, page.usual_secs, elapsed) {
            (true, Some(usual), Some(elapsed)) => Some(usual as f64 - elapsed),
            _ => None,
        };
        let progress = (!page.steps.is_empty()).then(|| {
            div().flex().flex_col().gap(SPACE_1H).child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .child(
                        ui::text(
                            page.percent
                                .map(|percent| format!("{percent}%"))
                                .unwrap_or_else(|| "—".into()),
                            20.,
                            FontWeight::SEMIBOLD,
                        )
                        .text_color(status_ink(page.status_kind, &t)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap(SPACE_0)
                            .children(page.steps.iter().map(|step| {
                                div()
                                    .flex_1()
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(step_ink(step.state, &t))
                            })),
                    )
                    .child(ui::meta(
                        format!(
                            "{} of {} steps{}",
                            page.completed,
                            page.total,
                            match left {
                                Some(left) if left > 0. => format!(" · about {} left", took(left)),
                                Some(_) => " · taking longer than usual".into(),
                                None => String::new(),
                            }
                        ),
                        cx,
                    )),
            )
        });
        let metas = div()
            .flex()
            .gap(SPACE_2)
            .flex_wrap()
            .children(
                page.branch
                    .clone()
                    .map(|branch| meta_chip(Icon::Git, branch, cx)),
            )
            .child(meta_chip(
                Icon::User,
                page.user.clone().unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(meta_chip(
                Icon::Clock,
                page.started_at
                    .as_deref()
                    .map(stamp_of)
                    .unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(meta_chip(
                Icon::Timer,
                elapsed.map(took).unwrap_or_else(|| "—".into()),
                cx,
            ));
        let steps = self.steps(&page.steps, cx);
        let output = self.output(&page, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .px(SPACE_5)
                    .py(SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(SPACE_2H)
                    .border_b_1()
                    .border_color(t.border)
                    .children(progress)
                    .child(metas),
            )
            .child(div().flex_1().min_h_0().flex().child(steps).child(output))
            .into_any_element()
    }

    fn steps(&self, steps: &[RunStep], cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let run = self.run.as_ref().expect("the run's output is kept");
        let lines = run.lines.len();
        let all = ui::Row::new("rundeck-step-all", "All output")
            .mark(
                22.,
                ui::icon(Icon::LogLines, 13.).text_color(t.text_tertiary),
            )
            .chosen(run.step.is_none())
            .status(Some(ui::meta(lines.to_string(), cx)))
            .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                if let Some(run) = &mut rd.run {
                    run.step = None;
                }
                cx.notify();
            }));
        div()
            .id("rundeck-steps")
            .w(px(200.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(t.border)
            .px(SPACE_1H)
            .py(SPACE_2)
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(div().px(SPACE_2).pb(SPACE_1).child(ui::caps("Steps", cx)))
            .child(all)
            .when(steps.is_empty(), |list| {
                list.child(div().px(SPACE_2).child(ui::meta("Waiting for steps…", cx)))
            })
            .children(steps.iter().enumerate().map(|(at, step)| {
                let key = step.key.clone();
                let chosen = run.step.as_deref() == Some(step.key.as_str());
                let mark = match step.state {
                    StepState::Ok => ui::icon(Icon::Check, 11.)
                        .text_color(t.live)
                        .into_any_element(),
                    StepState::Fail => ui::icon(Icon::Close, 11.)
                        .text_color(t.danger)
                        .into_any_element(),
                    state => div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(step_ink(state, &t))
                        .into_any_element(),
                };
                let dur = run_secs(step.started_at.as_deref(), step.ended_at.as_deref())
                    .filter(|_| step.started_at.is_some())
                    .map(took)
                    .unwrap_or_default();
                ui::Row::new(("rundeck-step", at), step.name.clone())
                    .mark(22., mark)
                    .chosen(chosen)
                    .status(Some(ui::meta(dur, cx)))
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some(run) = &mut rd.run {
                            run.step = if run.step.as_deref() == Some(key.as_str()) {
                                None
                            } else {
                                Some(key.clone())
                            };
                        }
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn output(&self, page: &ExecutionPage, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let run = self.run.as_ref().expect("the run's output is kept");
        let shown: Rc<Vec<OutputLine>> = Rc::new(
            run.lines
                .iter()
                .filter(|line| {
                    run.step
                        .as_deref()
                        .is_none_or(|key| step_matches(line.step_ctx.as_deref(), key))
                        && match run.shown {
                            LinesShown::All => true,
                            LinesShown::NoDebug => {
                                !matches!(level_of(line).as_str(), "DEBUG" | "VERBOSE")
                            }
                            LinesShown::Problems => {
                                matches!(level_of(line).as_str(), "ERROR" | "WARN")
                            }
                        }
                })
                .cloned()
                .collect(),
        );
        let errors = run
            .lines
            .iter()
            .filter(|line| level_of(line) == "ERROR")
            .count();
        if run.follow && !shown.is_empty() {
            run.scroll
                .scroll_to_item(shown.len() - 1, ScrollStrategy::Bottom);
        }
        let chosen = [LinesShown::All, LinesShown::NoDebug, LinesShown::Problems]
            .iter()
            .position(|shown| *shown == run.shown)
            .unwrap_or_default();
        let step_label = run.step.as_ref().and_then(|key| {
            page.steps
                .iter()
                .position(|step| step.key == *key)
                .map(|at| format!("Step {} · {}", at + 1, page.steps[at].name))
        });
        let streaming = if run.completed || !page.live {
            "Done"
        } else {
            "Live"
        };
        let count = shown.len();
        let (ink, dim, warn, danger) = (t.text_secondary, t.text_tertiary, t.warn, t.danger);
        let list = if shown.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::meta(
                    if run.step.is_some() {
                        "No output for this step yet."
                    } else {
                        "No output yet."
                    },
                    cx,
                ))
                .into_any_element()
        } else {
            let lines = shown.clone();
            uniform_list("rundeck-output", count, move |range, _, cx| {
                range
                    .map(|at| {
                        let line = &lines[at];
                        let level = level_of(line);
                        let text_ink = match level.as_str() {
                            "ERROR" => danger,
                            "WARN" => warn,
                            "DEBUG" | "VERBOSE" => dim,
                            _ => ink,
                        };
                        div()
                            .h(px(LOG_ROW))
                            .px(SPACE_3)
                            .flex()
                            .items_center()
                            .gap(SPACE_2H)
                            .child(
                                ui::mono(line.time.clone().unwrap_or_default(), 11., cx)
                                    .w(px(64.))
                                    .flex_none(),
                            )
                            .child(
                                ui::mono(
                                    line.step_ctx
                                        .as_deref()
                                        .and_then(|ctx| ctx.split(['/', '@']).next())
                                        .unwrap_or_default()
                                        .to_owned(),
                                    11.,
                                    cx,
                                )
                                .w(px(24.))
                                .flex_none(),
                            )
                            .child(output_text(line, text_ink, cx))
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&run.scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let banners = [
            match &run.stream {
                Stream::Flowing => None,
                Stream::Erred(error) => Some(banner(format!("Log stream: {error}"), t.warn, cx)),
                Stream::GaveUp(error) => Some(banner(
                    format!(
                        "The log stream stopped: {}",
                        error.as_deref().unwrap_or("too many errors")
                    ),
                    t.danger,
                    cx,
                )),
            },
            (run.dropped > 0).then(|| {
                banner(
                    format!(
                        "{} earlier lines aren't shown; Rundeck has them all.",
                        run.dropped
                    ),
                    t.text_tertiary,
                    cx,
                )
            }),
        ];
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_2H)
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        ui::text(
                            step_label.unwrap_or_else(|| "All steps".into()),
                            12.,
                            FontWeight::MEDIUM,
                        )
                        .text_color(t.text_primary),
                    )
                    .child(div().flex_1())
                    .child(
                        Segmented::words(
                            "rundeck-lines-shown",
                            vec![
                                ("All".into(), None),
                                ("Hide debug".into(), None),
                                ("Problems".into(), (errors > 0).then_some(errors as u64)),
                            ],
                            chosen,
                        )
                        .on_select(listen::<Rundeck, usize>(
                            cx,
                            |rd, _, index, _, cx| {
                                if let Some(run) = &mut rd.run {
                                    run.shown = [
                                        LinesShown::All,
                                        LinesShown::NoDebug,
                                        LinesShown::Problems,
                                    ][*index];
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Toggle::new("rundeck-follow", "Follow", run.follow).on_click(listen::<
                            Rundeck,
                            ClickEvent,
                        >(
                            cx,
                            |rd, _, _, _, cx| {
                                if let Some(run) = &mut rd.run {
                                    run.follow = !run.follow;
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(ui::meta(streaming, cx).text_color(if streaming == "Live" {
                        t.live
                    } else {
                        t.text_tertiary
                    })),
            )
            .children(banners.into_iter().flatten())
            .child(list)
            .child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_1H)
                    .flex()
                    .border_t_1()
                    .border_color(t.border)
                    .child(ui::meta(
                        format!(
                            "{} · {errors} {}",
                            if count == run.lines.len() {
                                format!("{count} lines")
                            } else {
                                format!("{count} of {} lines", run.lines.len())
                            },
                            if errors == 1 { "error" } else { "errors" }
                        ),
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(ui::meta(
                        if run.follow {
                            "Following the newest output"
                        } else {
                            "Paused"
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// Streams the open run's output, picking up after the lines already read.
    fn follow_output(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(run) = &mut self.run else {
            return;
        };
        let stream = host.stream::<Rundeck>(
            "logs",
            json!({ "executionId": run.id, "offset": run.offset }),
            cx,
            |rd, _, streamed, _| {
                let Some(run) = &mut rd.run else {
                    return;
                };
                match streamed {
                    Streamed::Item(item) => {
                        let tick = match serde_json::from_value::<OutputTick>(item) {
                            Ok(tick) => tick,
                            Err(error) => {
                                eprintln!("sikemux: a Rundeck run's output did not read: {error}");
                                return;
                            }
                        };
                        let lines = Rc::make_mut(&mut run.lines);
                        lines.extend(tick.entries);
                        if lines.len() > LOG_LINES_KEPT {
                            let over = lines.len() - LOG_LINES_KEPT;
                            lines.drain(..over);
                            run.dropped += over;
                        }
                        run.offset = Some(tick.offset);
                        run.completed |= tick.completed;
                        run.stream = match (tick.failed, tick.error) {
                            (true, error) => Stream::GaveUp(error),
                            (false, Some(error)) => Stream::Erred(error),
                            (false, None) => Stream::Flowing,
                        };
                    }
                    Streamed::Ended(failure) => {
                        if let Some(failure) = failure {
                            let error = failure.message.to_string();
                            run.stream = match run.stream {
                                Stream::GaveUp(_) => Stream::GaveUp(Some(error)),
                                _ => Stream::Erred(error),
                            };
                        }
                    }
                }
            },
        );
        run._stream = Some(stream);
    }

    /// Tells the person when the run ends, with what the plugin says of how it went.
    pub(super) fn watch(
        &mut self,
        host: &Host,
        job: &JobRef,
        id: u64,
        cx: &mut Context<PluginPane>,
    ) {
        if self.watching.contains_key(&id) {
            return;
        }
        let task = host.stream::<Rundeck>(
            "watch",
            json!({
                "executionId": id,
                "job": { "jobId": job.job_id, "name": job.name, "group": job.group, "project": job.project },
            }),
            cx,
            move |rd, host, streamed, cx| match streamed {
                Streamed::Item(item) => {
                    if item["terminal"] != Value::Bool(true) {
                        return;
                    }
                    if let Some(task) = rd.watching.remove(&id) {
                        task.detach();
                    }
                    host.read_again(None);
                    match serde_json::from_value::<RunNotice>(item["notice"].clone()) {
                        Ok(notice) => tell(host, &notice, cx),
                        Err(error) => {
                            eprintln!("sikemux: how Rundeck run {id} ended did not read: {error}")
                        }
                    }
                }
                Streamed::Ended(_) => {
                    if let Some(task) = rd.watching.remove(&id) {
                        task.detach();
                    }
                }
            },
        );
        self.watching.insert(id, task);
    }
}

impl Output {
    fn new(id: u64) -> Self {
        Self {
            id,
            lines: Rc::new(Vec::new()),
            dropped: 0,
            offset: None,
            completed: false,
            stream: Stream::Flowing,
            step: None,
            shown: LinesShown::All,
            follow: true,
            scroll: UniformListScrollHandle::new(),
            _stream: None,
        }
    }
}

fn abort(host: &Host, job: &JobRef, id: u64, cx: &mut Context<PluginPane>) {
    let pane = cx.weak_entity();
    let name = job.name.clone();
    host.confirm(
        format!("Abort execution #{id}?"),
        format!("{name} will be stopped where it is."),
        "abort",
        true,
        move |yes, cx| {
            if !yes {
                return;
            }
            let _ = pane.update(cx, |pane, cx| {
                let stopped = pane.host.call("abort", json!({ "executionId": id }));
                if let Some((rd, _)) = pane.parts::<Rundeck>() {
                    rd.aborting = Some(id);
                }
                cx.notify();
                after::<Rundeck, _>(
                    stopped,
                    |rd, host, done, cx| {
                        rd.aborting = None;
                        match done {
                            Ok(result) if result["abort"]["status"] == "failed" => {
                                let reason = result["abort"]["reason"]
                                    .as_str()
                                    .unwrap_or("no reason given");
                                let failure = Failure {
                                    category: "rundeck".into(),
                                    message: format!("Rundeck refused to abort: {reason}").into(),
                                    status: None,
                                };
                                host.said(&failure, cx);
                            }
                            Ok(_) => {}
                            Err(failure) => host.said(&failure.titled("Abort"), cx),
                        }
                    },
                    cx,
                );
            });
        },
        cx,
    );
}

/// Says how a run ended over the window.
fn tell(host: &Host, notice: &RunNotice, cx: &mut Context<PluginPane>) {
    let text = match notice.body.is_empty() {
        true => notice.title.clone(),
        false => format!("{} — {}", notice.title, notice.body),
    };
    let note = if notice.ok {
        toast::success(text)
    } else {
        toast::error(text)
    };
    host.notify(note, cx);
}

fn level_of(line: &OutputLine) -> String {
    line.level.as_deref().unwrap_or_default().to_uppercase()
}

/// Whether a line's step context is the step `key`, or one of its parts or nodes.
fn step_matches(ctx: Option<&str>, key: &str) -> bool {
    let ctx = ctx.unwrap_or_default();
    ctx == key || ctx.starts_with(&format!("{key}/")) || ctx.starts_with(&format!("{key}@"))
}

/// A line of output in the styles its terminal codes set, as the plugin read them.
fn output_text(line: &OutputLine, ink: Hsla, cx: &App) -> Div {
    let text = div().flex_1().min_w_0().flex().overflow_hidden();
    if line.runs.is_empty() {
        return text.child(
            ui::mono(line.log.clone().unwrap_or_default(), 11.5, cx)
                .text_color(ink)
                .truncate(),
        );
    }
    let t = Tokens::get(cx);
    text.children(line.runs.iter().map(|run| {
        let mut part = ui::mono(run.text.clone(), 11.5, cx)
            .flex_none()
            .text_color(run.fg.map_or(ink, |fg| term_ink(fg, t)))
            .when(run.bold, |part| part.font_weight(FontWeight::BOLD))
            .when(run.dim, |part| part.opacity(0.7))
            .when(run.italic, |part| part.italic())
            .when(run.underline, |part| part.underline());
        if let Some(bg) = run.bg {
            part = part.bg(term_ink(bg, t));
        }
        part
    }))
}

/// A terminal colour in the theme's terminal palette, or the xterm colour it numbers.
fn term_ink(colour: TermColor, t: &Tokens) -> Hsla {
    let [r, g, b] = match colour {
        TermColor::Rgb { r, g, b } => [r, g, b],
        TermColor::Palette { index } if index < 16 => t.terminal.ansi[index as usize],
        TermColor::Palette { index } if index < 232 => {
            let step = |level: u8| if level == 0 { 0 } else { 55 + level * 40 };
            let at = index - 16;
            [step(at / 36), step(at / 6 % 6), step(at % 6)]
        }
        TermColor::Palette { index } => {
            let grey = 8 + (index - 232) * 10;
            [grey, grey, grey]
        }
    };
    gpui_kit::rgb((r as u32) << 16 | (g as u32) << 8 | b as u32).into()
}
