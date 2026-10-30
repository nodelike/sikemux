//! Skills every agent Sikemux starts can use, kept only in Sikemux, and the
//! person's own skills in each agent's global folder, which can move in.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement,
    PathPromptOptions, SharedString, Styled, Subscription, Window,
};
use serde_json::{json, Value};

use crate::core;
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq)]
enum Form {
    Create,
    Install,
}

pub struct Fields {
    overview: Option<Value>,
    form: Option<Form>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    body: Entity<TextareaState>,
    source: Entity<InputState>,
    subfolder: Entity<InputState>,
    busy: bool,
    /// What failed, and on which row: the open form, or one skill's Remove or Move.
    failure: Option<(String, String)>,
    moving: Option<String>,
    _typing: Vec<Subscription>,
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("deploy-preview"));
        let description = cx.new(|cx| {
            InputState::new(window, cx).placeholder("What it does and when an agent should use it")
        });
        let body = cx
            .new(|cx| TextareaState::new(window, cx).placeholder("The instructions, in markdown"));
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("A folder with a SKILL.md, or a Git URL")
        });
        let subfolder = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Folder inside the repository, if the skill is not at its root")
        });
        let typing = vec![
            super::super::follow_typing(bench, &name, cx),
            super::super::follow_typing(bench, &description, cx),
            super::super::follow_typing(bench, &source, cx),
        ];
        Self {
            overview: None,
            form: None,
            name,
            description,
            body,
            source,
            subfolder,
            busy: false,
            failure: None,
            moving: None,
            _typing: typing,
        }
    }
}

/// Changes the skills page's list and forms, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_fields(bench, cx, |fields, cx| change(&mut fields.skills, cx))
}

pub fn read(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let overview = core::request(&asks, "skillsOverview", json!({})).await;
        let _ = bench.update(cx, |bench, cx| {
            if let Ok(overview) = overview {
                change_fields(bench, cx, |fields, _| fields.overview = Some(overview));
            }
        });
    })
    .detach();
}

/// Opens a form, or closes it, with its fields empty.
fn show(
    bench: &mut Workbench,
    form: Option<Form>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    change_fields(bench, cx, |fields, cx| {
        fields.form = form;
        fields.failure = None;
        for input in [
            &fields.name,
            &fields.description,
            &fields.source,
            &fields.subfolder,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        fields
            .body
            .update(cx, |input, cx| input.set_value("", window, cx));
        let first = match form {
            Some(Form::Create) => Some(fields.name.clone()),
            Some(Form::Install) => Some(fields.source.clone()),
            None => None,
        };
        if let Some(first) = first {
            first.update(cx, |input, cx| input.focus(window, cx));
        }
    });
}

/// Runs `method`; a failure stays on the row `at`, and `after` runs once it worked.
fn run(
    bench: &mut Workbench,
    at: String,
    method: &'static str,
    params: Value,
    after: impl FnOnce(&mut Fields) + 'static,
    cx: &mut Context<Workbench>,
) {
    change_fields(bench, cx, |fields, _| {
        fields.busy = true;
        fields.failure = None;
    });
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let answer = core::request(&asks, method, params).await;
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                fields.busy = false;
                match answer {
                    Ok(_) => after(fields),
                    Err(message) => fields.failure = Some((at, message)),
                }
            });
            read(bench, cx);
        });
    })
    .detach();
}

fn submit(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(fields) = super::fields(bench, cx).map(|agents| &agents.skills) else {
        return;
    };
    let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
    let (method, params) = match fields.form {
        Some(Form::Create) => (
            "createSkill",
            json!({
                "name": text(&fields.name),
                "description": fields.description.read(cx).value().to_string(),
                "body": fields.body.read(cx).value().to_string(),
            }),
        ),
        Some(Form::Install) => {
            let subfolder = text(&fields.subfolder);
            (
                "installSkill",
                json!({ "source": text(&fields.source), "subfolder": Some(subfolder).filter(|folder| !folder.is_empty()) }),
            )
        }
        None => return,
    };
    run(
        bench,
        "form".into(),
        method,
        params,
        |fields| fields.form = None,
        cx,
    );
}

fn choose_folder(_: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(path) = paths
            .first()
            .map(|path| path.to_string_lossy().into_owned())
        else {
            return;
        };
        let _ = bench.update_in(cx, |bench, window, cx| {
            change_fields(bench, cx, |fields, cx| {
                fields
                    .source
                    .update(cx, |input, cx| input.set_value(path, window, cx))
            });
        });
    })
    .detach();
}

type Run = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

fn button(
    id: String,
    label: &'static str,
    variant: ui::Variant,
    disabled: bool,
    this: &Entity<Workbench>,
    run: Run,
) -> ui::Button {
    let this = this.clone();
    ui::Button::new(SharedString::from(id), label, variant)
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let run = run.clone();
            this.update(cx, |bench, cx| run(bench, window, cx))
        })
}

fn about(text: impl Into<SharedString>, failed: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .text_color(if failed { t.danger } else { t.text_tertiary })
        .child(text.into())
        .into_any_element()
}

/// A skill's row: its name, a line about it, and its buttons.
fn row(label: String, line: AnyElement, controls: impl IntoElement, t: &Tokens) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .px(SPACE_4)
        .py(SPACE_2H)
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(label),
                )
                .child(line),
        )
        .child(div().flex().gap(SPACE_2).child(controls))
}

pub fn render(
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let failed = |at: &str| {
        fields
            .failure
            .as_ref()
            .filter(|(on, _)| on == at)
            .map(|(_, message)| message.clone())
    };
    let skills: Vec<&Value> = fields
        .overview
        .as_ref()
        .and_then(|overview| overview["skills"].as_array())
        .into_iter()
        .flatten()
        .collect();
    let clashes: Vec<&str> = fields
        .overview
        .as_ref()
        .and_then(|overview| overview["clashes"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let library = fields
        .overview
        .as_ref()
        .and_then(|overview| overview["library"].as_str())
        .unwrap_or_default();
    let busy = fields.busy;

    let listed = skills.iter().map(|skill| {
        let name = skill["name"].as_str().unwrap_or_default().to_owned();
        let description = skill["description"].as_str().unwrap_or_default();
        let path = PathBuf::from(skill["path"].as_str().unwrap_or_default());
        let line = match failed(&format!("remove:{name}")) {
            Some(message) => about(message, true, cx),
            None if clashes.contains(&name.as_str()) => about(
                format!("{description} One of your own skills has this name too."),
                false,
                cx,
            ),
            None => about(description.to_owned(), false, cx),
        };
        let removing = name.clone();
        let controls = div()
            .flex()
            .gap(SPACE_2)
            .child(button(
                format!("skill-reveal-{name}"),
                ui::SHOW_IN_FILES,
                ui::Variant::Outline,
                false,
                this,
                Rc::new(move |_, _, cx| cx.reveal_path(&path)),
            ))
            .child(button(
                format!("skill-remove-{name}"),
                "Remove",
                ui::Variant::Outline,
                busy,
                this,
                Rc::new(move |bench, _, cx| {
                    let name = removing.clone();
                    run(
                        bench,
                        format!("remove:{name}"),
                        "removeSkill",
                        json!({ "name": name }),
                        |_| {},
                        cx,
                    )
                }),
            ));
        row(name, line, controls, &t).into_any_element()
    });

    let form: AnyElement = match fields.form {
        Some(Form::Create) => {
            let ready = !fields.name.read(cx).value().trim().is_empty()
                && !fields.description.read(cx).value().trim().is_empty();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .p(SPACE_4)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child("New skill"),
                )
                .children(failed("form").map(|message| about(message, true, cx)))
                .child(ui::Field::new(&fields.name))
                .child(ui::Field::new(&fields.description))
                .child(
                    div()
                        .border_1()
                        .border_color(t.line)
                        .rounded(theme::RADIUS_2)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .child(Textarea::new(&fields.body).h(px(140.)).bordered(false)),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(SPACE_2)
                        .child(button(
                            "skill-cancel".into(),
                            "Cancel",
                            ui::Variant::Outline,
                            busy,
                            this,
                            Rc::new(|bench, window, cx| show(bench, None, window, cx)),
                        ))
                        .child(button(
                            "skill-create".into(),
                            "Create",
                            ui::Variant::Primary,
                            busy || !ready,
                            this,
                            Rc::new(|bench, _, cx| submit(bench, cx)),
                        )),
                )
                .into_any_element()
        }
        Some(Form::Install) => {
            let source = fields.source.read(cx).value().trim().to_owned();
            let repository = !source.is_empty() && !source.starts_with('/');
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .p(SPACE_4)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child("Install a skill"),
                )
                .children(failed("form").map(|message| about(message, true, cx)))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_2)
                        .child(div().flex_1().child(ui::Field::new(&fields.source).mono()))
                        .child(button(
                            "skill-choose".into(),
                            "Choose folder",
                            ui::Variant::Outline,
                            false,
                            this,
                            Rc::new(choose_folder),
                        )),
                )
                .when(repository, |form| {
                    form.child(ui::Field::new(&fields.subfolder).mono())
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(SPACE_2)
                        .child(button(
                            "skill-cancel".into(),
                            "Cancel",
                            ui::Variant::Outline,
                            busy,
                            this,
                            Rc::new(|bench, window, cx| show(bench, None, window, cx)),
                        ))
                        .child(button(
                            "skill-install".into(),
                            if busy { "Installing…" } else { "Install" },
                            ui::Variant::Primary,
                            busy || source.is_empty(),
                            this,
                            Rc::new(|bench, _, cx| submit(bench, cx)),
                        )),
                )
                .into_any_element()
        }
        None => {
            let line = match (fields.overview.is_some(), skills.is_empty()) {
                (false, _) => String::new(),
                (true, true) => format!("None yet. They are kept in {library}"),
                (true, false) => format!("Kept in {library}"),
            };
            row(
                "Add a skill".into(),
                about(line, false, cx),
                div()
                    .flex()
                    .gap(SPACE_2)
                    .child(button(
                        "skill-install-open".into(),
                        "Install…",
                        ui::Variant::Outline,
                        false,
                        this,
                        Rc::new(|bench, window, cx| show(bench, Some(Form::Install), window, cx)),
                    ))
                    .child(button(
                        "skill-new".into(),
                        "New skill",
                        ui::Variant::Primary,
                        false,
                        this,
                        Rc::new(|bench, window, cx| show(bench, Some(Form::Create), window, cx)),
                    )),
                &t,
            )
            .into_any_element()
        }
    };

    let skills_section = rows::section(
        "Skills",
        (!skills.is_empty()).then(|| format!("{} in Sikemux", skills.len()).into()),
        Some("Skills every agent Sikemux starts can use, in a terminal or a chat. They live only in Sikemux: the same agents run elsewhere never see them. Agents can make them too, with the skills tool. A new skill loads in the next session.".into()),
        lit("Skills"),
        div().children(listed).child(form),
        cx,
    );

    let own: Vec<(String, String, Value)> = fields
        .overview
        .as_ref()
        .and_then(|overview| overview["own"].as_array())
        .into_iter()
        .flatten()
        .flat_map(|own| {
            let agent = own["agent"].as_str().unwrap_or_default().to_owned();
            let folder = own["folder"].as_str().unwrap_or_default().to_owned();
            own["skills"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |skill| (agent.clone(), folder.clone(), skill))
        })
        .collect();
    let own_section = (!own.is_empty()).then(|| {
        let rows_of_own = own.iter().map(|(agent, folder, skill)| {
            let name = skill["name"].as_str().unwrap_or_default().to_owned();
            let path = skill["path"].as_str().unwrap_or_default().to_owned();
            let moving = fields.moving.as_deref() == Some(path.as_str());
            let line = match failed(&format!("move:{path}")) {
                Some(message) => about(message, true, cx),
                None if moving => about(format!("Moves it out of {folder}, so {agent} outside Sikemux no longer has it."), false, cx),
                None => about(format!("{agent} · {}", skill["description"].as_str().unwrap_or_default()), false, cx),
            };
            let controls: AnyElement = match moving {
                true => {
                    let moved = path.clone();
                    div()
                        .flex()
                        .gap(SPACE_2)
                        .child(button(format!("skill-move-cancel-{path}"), "Cancel", ui::Variant::Outline, false, this, Rc::new(|bench, _, cx| {
                            change_fields(bench, cx, |fields, _| fields.moving = None);
                        })))
                        .child(button(format!("skill-move-{path}"), "Move", ui::Variant::Primary, busy, this, Rc::new(move |bench, _, cx| {
                            let path = moved.clone();
                            run(bench, format!("move:{path}"), "adoptSkill", json!({ "path": path }), |fields| fields.moving = None, cx)
                        })))
                        .into_any_element()
                }
                false => {
                    let asked = path.clone();
                    button(format!("skill-move-ask-{path}"), "Move into Sikemux", ui::Variant::Outline, false, this, Rc::new(move |bench, _, cx| {
                        change_fields(bench, cx, |fields, _| fields.moving = Some(asked.clone()));
                    }))
                    .into_any_element()
                }
            };
            row(name, line, controls, &t)
        });
        rows::section(
            "Your own skills",
            None,
            Some("Skills in each agent's global folder. Agents load these inside and outside Sikemux, and Sikemux leaves them alone unless you move one in.".into()),
            false,
            div().children(rows_of_own),
            cx,
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(28.))
        .child(skills_section)
        .children(own_section)
        .into_any_element()
}
