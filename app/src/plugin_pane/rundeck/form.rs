//! The form that starts a job, with its options and deploy plan.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_5};

impl Rundeck {
    pub(super) fn deploy(
        &mut self,
        host: &mut Host,
        level: Level,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Level::Deploy { job, branch, given } = &level else {
            return crate::plugin_pane::empty("", cx);
        };
        let read = host
            .view(
                "deployView",
                DeployRequest {
                    job_id: job.job_id.clone(),
                    project: job.project.clone(),
                    name: job.name.clone(),
                    group: job.group.clone(),
                    branch: branch.clone(),
                    given: given.clone(),
                },
                cx,
            )
            .read::<DeployForm>();
        let form_data = match read {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => {
                return crate::plugin_pane::failure(&failure.titled("Couldn't load the job"), cx)
            }
            Ok(Some(form)) => form,
        };
        if self.form.as_ref().is_none_or(|form| form.of != level) {
            self.form = Some(Form::new(level.clone(), &form_data, window, cx));
            self.recheck(host, cx);
        }
        let urls = self
            .form
            .as_ref()
            .map(|form| form.check.values_urls.clone())
            .unwrap_or_default();
        self.read_remote(host, urls, cx);
        let t = Tokens::get(cx).clone();
        let form = self.form.as_ref().expect("made just now");
        let target = form_data
            .branch_key
            .as_ref()
            .map(|key| {
                form.values
                    .get(key)
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let plan = (form_data.runs_branch && !target.is_empty()).then(|| {
            host.view("planView", plan_request(host, &form_data, &target, cx), cx)
                .read::<Plan>()
        });
        let plan_ready = !form_data.runs_branch
            || matches!(&plan, Some(Ok(Some(plan))) if plan.target_branch == target);
        let verb = match (form_data.runs_branch, target.is_empty()) {
            (true, false) => format!("Deploy {target}"),
            (true, true) => "Deploy".into(),
            (false, _) => "Run".into(),
        };
        let label = if form.busy {
            "Starting…".into()
        } else if form_data.prod {
            format!("{verb} to production")
        } else {
            verb
        };
        let run_word = form_data.run_word.to_lowercase();
        let errors = if form.attempted {
            form.check.errors.clone()
        } else {
            BTreeMap::new()
        };
        let plan_view = form_data
            .runs_branch
            .then(|| plan_card(plan.clone(), &target, cx));
        let mut picks = branch_picks(&form_data, plan.as_ref(), &target, cx);
        let options = div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .when(form_data.options.is_empty(), |list| {
                list.child(ui::meta("This job takes no options.", cx))
            })
            .children(form_data.options.iter().map(|option| {
                let hint = match form_data.branch_key.as_deref() == Some(option.name.as_str()) {
                    true => picks.take(),
                    false => None,
                };
                self.option_field(
                    option,
                    errors.get(&option.name).cloned(),
                    hint,
                    &form_data,
                    cx,
                )
            }))
            .child(self.advanced(form_data.node_filter.as_deref(), cx));
        let banners = [
            form_data.options_error.as_ref().map(|error| {
                banner(
                    format!("Couldn't read job options: {error}. Only the branch is sent."),
                    t.text_tertiary,
                    cx,
                )
            }),
            (!form_data.execution_enabled).then(|| {
                banner(
                    "Executions are disabled for this job in Rundeck.".into(),
                    t.warn,
                    cx,
                )
            }),
            form.error.clone().map(|error| banner(error, t.danger, cx)),
        ];
        let start_data = form_data.clone();
        let can_start = !form.busy && form_data.execution_enabled && plan_ready;
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            format!(
                "{} {}",
                if form_data.runs_branch {
                    "Deploy"
                } else {
                    "Run"
                },
                job.name
            ),
            Some(folder_chip(
                &form_data.project,
                form_data.group.as_deref(),
                form_data.prod,
                cx,
            )),
            div().into_any_element(),
            cx,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("rundeck-form")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_5)
                    .py(SPACE_3H)
                    .flex()
                    .flex_col()
                    .gap(SPACE_3H)
                    .children(banners.into_iter().flatten())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(SPACE_3H)
                            .items_start()
                            .child(div().flex_1().min_w(px(320.)).child(card(
                                "Options",
                                form_data.description.clone().unwrap_or_default(),
                                options.into_any_element(),
                                cx,
                            )))
                            .children(
                                plan_view.map(|plan| div().flex_1().min_w(px(320.)).child(plan)),
                            ),
                    )
                    .when(form_data.prod, |page| {
                        page.child(banner(
                            format!(
                                "This {run_word}s to production ({}). You'll be asked to confirm.",
                                form_data.env
                            ),
                            t.warn,
                            cx,
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .children(form_data.usual_secs.map(|usual| {
                                ui::meta(format!("Usually takes {}", took(usual as f64)), cx)
                            }))
                            .child(div().flex_1())
                            .child(
                                Button::new("rundeck-cancel", "Cancel", Variant::Outline)
                                    .disabled(form.busy)
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        |rd, _, _, _, cx| {
                                            rd.stack.pop();
                                            rd.form = None;
                                            cx.notify();
                                        },
                                    )),
                            )
                            .child(
                                Button::new("rundeck-submit", label, Variant::Primary)
                                    .icon(Icon::Run)
                                    .disabled(!can_start)
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        move |rd, host, _, _, cx| {
                                            rd.submit(host, &start_data, cx);
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn option_field(
        &self,
        option: &JobOption,
        error: Option<String>,
        hint: Option<AnyElement>,
        form_data: &DeployForm,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let form = self.form.as_ref().expect("the form is open");
        let value = form.values.get(&option.name).cloned().unwrap_or_default();
        let remote_url = form.check.values_urls.get(&option.name).cloned();
        let label = option.label.clone().unwrap_or_else(|| option.name.clone());
        let choices: Option<Vec<OptionChoice>> = match (&option.values_url, &option.values) {
            (Some(_), _) => remote_url.as_ref().and_then(|_| {
                form.remote
                    .get(&option.name)
                    .cloned()
                    .flatten()
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|value| OptionChoice {
                                name: value.clone(),
                                value,
                            })
                            .collect()
                    })
            }),
            (None, Some(values)) if !values.is_empty() => Some(
                values
                    .iter()
                    .map(|value| OptionChoice {
                        name: value.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            ),
            _ => None,
        };
        let name = option.name.clone();
        let input: AnyElement = if option.kind == OptionKind::File {
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::meta("file options must be run from Rundeck", cx))
                .children(form_data.permalink.clone().map(|url| {
                    Button::new("rundeck-file-open", "Open in Rundeck", Variant::Outline)
                        .on_click(move |_, _, cx| cx.open_url(&url))
                }))
                .into_any_element()
        } else if option.enforced && option.multivalued && choices.is_some() {
            let picked: HashSet<String> = value
                .split(option.delimiter.as_str())
                .map(|part| part.trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect();
            let choices = choices.clone().unwrap_or_default();
            let delimiter = option.delimiter.clone();
            div()
                .flex()
                .flex_wrap()
                .gap(SPACE_1H)
                .children(choices.iter().enumerate().map(|(at, choice)| {
                    let on = picked.contains(&choice.value);
                    let (name, value, all, delimiter, picked) = (
                        name.clone(),
                        choice.value.clone(),
                        choices
                            .iter()
                            .map(|choice| choice.value.clone())
                            .collect::<Vec<_>>(),
                        delimiter.clone(),
                        picked.clone(),
                    );
                    Toggle::new(
                        SharedString::from(format!("rundeck-multi-{name}-{at}")),
                        choice.name.clone(),
                        on,
                    )
                    .on_click(listen::<Rundeck, ClickEvent>(
                        cx,
                        move |rd, host, _, _, cx| {
                            let next: Vec<String> = all
                                .iter()
                                .filter(|candidate| {
                                    if **candidate == value {
                                        !on
                                    } else {
                                        picked.contains(*candidate)
                                    }
                                })
                                .cloned()
                                .collect();
                            rd.set_value(host, &name, next.join(&delimiter), cx);
                        },
                    ))
                }))
                .into_any_element()
        } else if option.enforced && choices.is_some() {
            let choices = choices.clone().unwrap_or_default();
            let known = choices.iter().any(|choice| choice.value == value);
            let mut picks = Vec::new();
            if !(option.required && known) {
                picks.push(ui::Pick::new("", "—"));
            }
            if !value.is_empty() && !known {
                picks.push(ui::Pick::new(value.clone(), value.clone()).hint("not allowed"));
            }
            picks.extend(choices.iter().map(|choice| {
                let pick = ui::Pick::new(choice.value.clone(), choice.name.clone());
                if choice.name != choice.value {
                    pick.hint(choice.value.clone())
                } else {
                    pick
                }
            }));
            ui::select(
                SharedString::from(format!("rundeck-option-{name}")),
                if value.is_empty() {
                    "—".to_owned()
                } else {
                    value.clone()
                },
                picks,
                Some(value.clone().into()),
                false,
                listen::<Rundeck, SharedString>(cx, move |rd, host, picked, _, cx| {
                    rd.set_value(host, &name, picked.to_string(), cx);
                }),
                cx,
            )
        } else {
            match form.inputs.get(&option.name) {
                Some(input) => ui::Field::new(input).into_any_element(),
                None => div().into_any_element(),
            }
        };
        let remote_failed = option.values_url.is_some()
            && form.remote.get(&option.name).is_some_and(Option::is_none);
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(
                        ui::text(label.clone(), 12., FontWeight::MEDIUM).text_color(t.text_primary),
                    )
                    .when(option.required, |head| {
                        head.child(ui::text("*", 12., FontWeight::MEDIUM).text_color(t.danger))
                    })
                    .when(
                        option
                            .label
                            .as_deref()
                            .is_some_and(|label| label != option.name),
                        |head| head.child(ui::mono(option.name.clone(), 10.5, cx)),
                    ),
            )
            .child(input)
            .children(hint)
            .children(
                option
                    .description
                    .clone()
                    .map(|description| ui::meta(description, cx).whitespace_normal()),
            )
            .when(remote_failed, |field| {
                field.child(
                    ui::meta(
                        "Couldn't load the allowed values. Type a value instead.",
                        cx,
                    )
                    .text_color(t.warn),
                )
            })
            .children(error.map(|error| ui::meta(error, cx).text_color(t.danger)))
            .into_any_element()
    }

    fn advanced(&self, default_filter: Option<&str>, cx: &mut Context<PluginPane>) -> AnyElement {
        let form = self.form.as_ref().expect("the form is open");
        let toggle = Button::new("rundeck-advanced", "Advanced", Variant::Outline)
            .pressed(form.advanced)
            .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                if let Some(form) = &mut rd.form {
                    form.advanced = !form.advanced;
                }
                cx.notify();
            }));
        let labelled = |label: &'static str, field: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(ui::meta(label, cx))
                .child(field)
        };
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(toggle)
                    .when(!form.advanced, |row| {
                        row.child(ui::meta("debug output, nodes, schedule, run as", cx))
                    }),
            )
            .when(form.advanced, |page| {
                page.child(
                    Toggle::new("rundeck-debug", "Debug output", form.debug).on_click(listen::<
                        Rundeck,
                        ClickEvent,
                    >(
                        cx,
                        |rd, _, _, _, cx| {
                            if let Some(form) = &mut rd.form {
                                form.debug = !form.debug;
                            }
                            cx.notify();
                        },
                    )),
                )
                .child(labelled(
                    "Node filter",
                    ui::Field::new(&form.node_filter).mono().into_any_element(),
                ))
                .child(labelled(
                    "Run later (YYYY-MM-DD HH:MM, your time)",
                    ui::Field::new(&form.run_at).into_any_element(),
                ))
                .child(labelled(
                    "Run as user",
                    ui::Field::new(&form.as_user).into_any_element(),
                ))
                .children(
                    default_filter
                        .map(|filter| ui::meta(format!("The job's own nodes: {filter}"), cx)),
                )
            })
            .into_any_element()
    }

    fn set_value(
        &mut self,
        host: &mut Host,
        name: &str,
        value: String,
        cx: &mut Context<PluginPane>,
    ) {
        if let Some(form) = &mut self.form {
            form.values.insert(name.to_owned(), value);
        }
        self.recheck(host, cx);
        cx.notify();
    }

    /// Has the plugin check the form again, as it now stands.
    fn recheck(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let call = host.call(
            "checkRun",
            CheckRequest {
                options: form.options.clone(),
                values: form.values.clone(),
                remote: form.remote.clone(),
                job_id: None,
                project: None,
            },
        );
        form.checking = Some(cx.spawn(async move |pane, cx| {
            let checked = call.await;
            let _ = pane.update(cx, |pane, cx| {
                let Some((rd, _)) = pane.parts::<Rundeck>() else {
                    return;
                };
                let check = checked
                    .ok()
                    .and_then(|value| serde_json::from_value::<RunCheck>(value).ok());
                if let (Some(form), Some(check)) = (&mut rd.form, check) {
                    if check.errors.is_empty() && form.error.as_deref() == Some(FIX_OPTIONS) {
                        form.error = None;
                    }
                    form.check = check;
                }
                cx.notify();
            });
        }));
    }

    /// Reads each option's values from its URL, as the plugin filled the URL in.
    fn read_remote(
        &mut self,
        host: &mut Host,
        urls: BTreeMap<String, String>,
        cx: &mut Context<PluginPane>,
    ) {
        let mut moved = false;
        for (name, url) in urls {
            let read = host
                .view("optionValues", json!({ "url": url }), cx)
                .read::<Vec<OptionChoice>>();
            let values = match read {
                Ok(None) => continue,
                Ok(Some(choices)) => {
                    Some(choices.iter().map(|choice| choice.value.clone()).collect())
                }
                Err(_) => None,
            };
            if let Some(form) = &mut self.form {
                if form.remote.get(&name) != Some(&values) {
                    form.remote.insert(name, values);
                    moved = true;
                }
            }
        }
        if moved {
            self.recheck(host, cx);
        }
    }

    fn submit(
        &mut self,
        host: &mut Host,
        form_data: &Rc<DeployForm>,
        cx: &mut Context<PluginPane>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.attempted = true;
        form.error = None;
        let run_at = form.run_at.read(cx).value().trim().to_owned();
        let run_at_time = match run_at.is_empty() {
            true => None,
            false => match local_to_iso(&run_at) {
                Some(iso) => Some(iso),
                None => {
                    form.error = Some("Run later needs a full date and time.".into());
                    cx.notify();
                    return;
                }
            },
        };
        let filter = form.node_filter.read(cx).value().trim().to_owned();
        let as_user = form.as_user.read(cx).value().trim().to_owned();
        let own_nodes =
            filter.is_empty() || Some(filter.as_str()) == form_data.node_filter.as_deref();
        let filter = (!own_nodes).then_some(filter);
        let request = json!({
            "loglevel": if form.debug { Some("DEBUG") } else { None },
            "filter": filter,
            "runAtTime": run_at_time,
            "asUser": (!as_user.is_empty()).then_some(as_user),
        });
        form.busy = true;
        cx.notify();
        let ready = host.call(
            "checkRun",
            CheckRequest {
                options: form_data.options.clone(),
                values: form.values.clone(),
                remote: form.remote.clone(),
                job_id: Some(form_data.job_id.clone()),
                project: Some(form_data.project.clone()),
            },
        );
        let form_data = form_data.clone();
        after::<Rundeck, _>(
            ready,
            move |rd, host, ready, cx| {
                let ready = match ready.map(serde_json::from_value::<RunCheck>) {
                    Ok(Ok(ready)) => ready,
                    Ok(Err(error)) => return rd.failed(error.to_string()),
                    Err(failure) => return rd.failed(failure.message.to_string()),
                };
                if !ready.errors.is_empty() {
                    if let Some(form) = &mut rd.form {
                        form.check.errors = ready.errors.clone();
                    }
                    return rd.failed(FIX_OPTIONS.into());
                }
                rd.confirm_and_start(host, form_data, ready, request, cx);
            },
            cx,
        );
    }

    fn failed(&mut self, error: String) {
        if let Some(form) = &mut self.form {
            form.busy = false;
            form.error = Some(error);
        }
    }

    /// Asks before production and before a second run, pushes the checkout when the plan says,
    /// then starts the job and opens the run.
    fn confirm_and_start(
        &mut self,
        host: &mut Host,
        form_data: Rc<DeployForm>,
        ready: RunCheck,
        request: Value,
        cx: &mut Context<PluginPane>,
    ) {
        let target = form_data
            .branch_key
            .as_ref()
            .and_then(|key| ready.send.get(key).cloned())
            .unwrap_or_default();
        let run_word = form_data.run_word.to_lowercase();
        let mut questions = Vec::new();
        if form_data.prod {
            questions.push((
                if form_data.runs_branch {
                    format!("Deploy {target} to production?")
                } else {
                    format!("Run {} in production?", form_data.name)
                },
                format!(
                    "Project: {}\nJob: {}\nEnvironment: {}",
                    form_data.project, form_data.service, form_data.env
                ),
                if form_data.runs_branch {
                    "deploy to production"
                } else {
                    "run in production"
                }
                .to_owned(),
                true,
            ));
        }
        if ready.running == Some(true) {
            questions.push((
                "A run is already in progress".to_owned(),
                format!(
                    "{} is already running. Start another {run_word} anyway?",
                    form_data.name
                ),
                format!("{run_word} anyway"),
                false,
            ));
        }
        let push = form_data
            .runs_branch
            .then(|| {
                host.view("planView", plan_request(host, &form_data, &target, cx), cx)
                    .read::<Plan>()
                    .ok()
                    .flatten()
            })
            .flatten()
            .and_then(|plan| plan.push_first.clone());
        let mut options = request;
        options["jobId"] = json!(form_data.job_id);
        options["options"] = json!(ready.send);
        self.ask_then_start(host, questions, push, options, form_data, target, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn ask_then_start(
        &mut self,
        host: &mut Host,
        mut questions: Vec<(String, String, String, bool)>,
        push: Option<String>,
        request: Value,
        form_data: Rc<DeployForm>,
        target: String,
        cx: &mut Context<PluginPane>,
    ) {
        if questions.is_empty() {
            return self.start(host, push, request, form_data, target, cx);
        }
        let (title, body, yes, destructive) = questions.remove(0);
        let pane = cx.weak_entity();
        host.confirm(
            title,
            body,
            yes,
            destructive,
            move |yes, cx| {
                let (questions, push, request, form_data, target) = (
                    questions.clone(),
                    push.clone(),
                    request.clone(),
                    form_data.clone(),
                    target.clone(),
                );
                let _ = pane.update(cx, |pane, cx| {
                    if let Some((rd, host)) = pane.parts::<Rundeck>() {
                        if yes {
                            rd.ask_then_start(
                                host, questions, push, request, form_data, target, cx,
                            );
                        } else if let Some(form) = &mut rd.form {
                            form.busy = false;
                        }
                        cx.notify();
                    }
                });
            },
            cx,
        );
    }

    fn start(
        &mut self,
        host: &mut Host,
        push: Option<String>,
        request: Value,
        form_data: Rc<DeployForm>,
        target: String,
        cx: &mut Context<PluginPane>,
    ) {
        let pushed = push.as_ref().map(|repo| host.push(repo));
        let run = {
            let started = host.call("run", request);
            async move {
                if let Some(pushed) = pushed {
                    pushed
                        .await
                        .map_err(|failure| failure.titled("Nothing was run: the push failed"))?;
                }
                started.await
            }
        };
        let pushing = push.is_some();
        after::<Rundeck, _>(
            run,
            move |rd, host, started, cx| {
                let id = match started {
                    Ok(result) => result["id"].as_u64(),
                    Err(failure) => {
                        let message = if pushing {
                            format!(
                                "{target} was pushed, but Rundeck rejected the run: {}",
                                failure.message
                            )
                        } else {
                            failure.message.to_string()
                        };
                        return rd.failed(message);
                    }
                };
                let Some(id) = id else {
                    return rd
                        .failed("Rundeck started the run but did not say which it is.".into());
                };
                let job = JobRef {
                    project: form_data.project.clone(),
                    job_id: form_data.job_id.clone(),
                    name: form_data.name.clone(),
                    group: form_data.group.clone(),
                };
                rd.form = None;
                rd.stack.pop();
                rd.push(Level::Execution {
                    job: job.clone(),
                    id,
                });
                rd.watch(host, &job, id, cx);
            },
            cx,
        );
    }
}

const FIX_OPTIONS: &str = "Fix the highlighted options first.";

impl Form {
    fn new(
        of: Level,
        data: &DeployForm,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Self {
        let mut inputs = BTreeMap::new();
        for option in &data.options {
            if option.kind == OptionKind::File {
                continue;
            }
            let value = data.values.get(&option.name).cloned().unwrap_or_default();
            let placeholder = if option.is_date {
                option.date_format.clone().unwrap_or_else(|| "date".into())
            } else if option.multivalued {
                format!("values separated by {}", option.delimiter)
            } else {
                String::new()
            };
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(option.secure)
                    .default_value(value)
            });
            let name = option.name.clone();
            cx.subscribe(&input, move |pane, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let value = input.read(cx).value().to_string();
                if let Some((rd, host)) = pane.parts::<Rundeck>() {
                    rd.set_value(host, &name, value, cx);
                }
            })
            .detach();
            inputs.insert(option.name.clone(), input);
        }
        let mut field = |placeholder: &str, value: String, cx: &mut Context<PluginPane>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        Self {
            of,
            options: data.options.clone(),
            values: data.values.clone(),
            inputs,
            remote: BTreeMap::new(),
            check: RunCheck {
                errors: BTreeMap::new(),
                send: BTreeMap::new(),
                values_urls: BTreeMap::new(),
                running: None,
            },
            checking: None,
            advanced: false,
            debug: false,
            node_filter: field(
                data.node_filter.as_deref().unwrap_or("job's own nodes"),
                data.node_filter.clone().unwrap_or_default(),
                cx,
            ),
            run_at: field("2026-10-23 18:30", String::new(), cx),
            as_user: field("you", String::new(), cx),
            attempted: false,
            busy: false,
            error: None,
        }
    }
}

/// Branches worth one click: the checkout's, the live one, and whatever ran lately.
fn branch_picks(
    form: &DeployForm,
    plan: Option<&Result<Option<Rc<Plan>>, Failure>>,
    target: &str,
    cx: &mut Context<PluginPane>,
) -> Option<AnyElement> {
    let key = form.branch_key.clone()?;
    let plan = plan.and_then(|plan| plan.as_ref().ok().cloned().flatten());
    let mut picks: Vec<(String, Option<&'static str>)> = Vec::new();
    let mut add = |name: Option<String>, note: Option<&'static str>| {
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return;
        };
        match picks.iter_mut().find(|(known, _)| *known == name) {
            Some((_, known_note)) => {
                if known_note.is_none() {
                    *known_note = note;
                }
            }
            None => picks.push((name, note)),
        }
    };
    add(
        plan.as_ref().and_then(|plan| plan.current_branch.clone()),
        Some("checkout"),
    );
    add(
        plan.as_ref().and_then(|plan| plan.deployed_branch.clone()),
        Some("live"),
    );
    for branch in &form.recent_branches {
        add(Some(branch.clone()), None);
    }
    picks.truncate(6);
    if picks.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(SPACE_1H)
            .children(picks.into_iter().enumerate().map(|(at, (name, note))| {
                let (key, picked) = (key.clone(), name.clone());
                Toggle::new(
                    SharedString::from(format!("rundeck-pick-{at}")),
                    match note {
                        Some(note) => format!("{name} · {note}"),
                        None => name.clone(),
                    },
                    name == target,
                )
                .on_click(listen::<Rundeck, ClickEvent>(
                    cx,
                    move |rd, host, _, window, cx| {
                        if let Some(input) = rd
                            .form
                            .as_ref()
                            .and_then(|form| form.inputs.get(&key))
                            .cloned()
                        {
                            let picked = picked.clone();
                            input.update(cx, |input, cx| input.set_value(picked, window, cx));
                        }
                        rd.set_value(host, &key, picked.clone(), cx);
                    },
                ))
            }))
            .into_any_element(),
    )
}

/// What deploying the branch would do, as the plugin worked it out.
fn plan_card(
    plan: Option<Result<Option<Rc<Plan>>, Failure>>,
    target: &str,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let body = match plan {
        None => ui::meta("Pick a branch to see what the deploy will do.", cx)
            .whitespace_normal()
            .into_any_element(),
        Some(Ok(None)) => ui::meta("Working out the plan…", cx).into_any_element(),
        Some(Err(failure)) => ui::meta(failure.message.to_string(), cx)
            .text_color(t.danger)
            .into_any_element(),
        Some(Ok(Some(plan))) if plan.target_branch != target => {
            ui::meta("Checking…", cx).into_any_element()
        }
        Some(Ok(Some(plan))) => div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(SPACE_0)
                            .child(ui::meta("Live now", cx))
                            .child(match &plan.deployed_branch {
                                Some(branch) => branch_chip(Some(branch), cx),
                                None => ui::meta("Nothing yet", cx).into_any_element(),
                            }),
                    )
                    .child(ui::meta("→", cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(SPACE_0)
                            .child(ui::meta("Will deploy", cx))
                            .child(branch_chip(Some(&plan.target_branch), cx)),
                    ),
            )
            .children(plan.checks.iter().map(|check| {
                let (icon, ink) = match (check.tone, check.pushes) {
                    (_, true) => (Icon::ArrowUp, t.warn),
                    (Tone::Ok, _) => (Icon::Check, t.live),
                    (Tone::Info, _) => (Icon::InfoCircle, t.text_tertiary),
                    (Tone::Warn, _) => (Icon::Warning, t.warn),
                    (Tone::Danger, _) => (Icon::Warning, t.danger),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(ui::icon(icon, 13.).text_color(ink))
                    .child(
                        ui::text(check.text.clone(), 12., FontWeight::NORMAL)
                            .text_color(t.text_secondary)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    )
                    .children(check.value.clone().map(|value| ui::mono(value, 11., cx)))
            }))
            .children(
                plan.detail
                    .clone()
                    .map(|detail| ui::meta(detail, cx).whitespace_normal()),
            )
            .into_any_element(),
    };
    card("Plan", String::new(), body, cx)
}

/// What the plan of deploying `target` with the job is asked, with the checkout in front.
fn plan_request(host: &Host, form_data: &DeployForm, target: &str, cx: &App) -> PlanRequest {
    PlanRequest {
        job_id: form_data.job_id.clone(),
        project: form_data.project.clone(),
        service: form_data.service.clone(),
        name: form_data.name.clone(),
        target_branch: target.to_owned(),
        checkout: host.watched_project(cx),
    }
}
