//! The connection form, as the plugin hands it: a new connection or a saved one to edit, tried
//! and saved through the plugin, which says what a filled-in one still needs.

use std::rc::Rc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputState, InputEvent};
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement,
    IntoElement,
    ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde_json::json;
use sikemux_wire::database::{Engine, ProfileDraft, ProfileForm};

use super::{Database, Host, PluginPane};
use crate::plugin_pane::Failure;
use crate::icons::Icon;
use crate::plugin_pane::{after, listen};
use crate::theme::{Tokens, RADIUS_2};
use crate::ui::{self, Button, IconButton, Pick, Segmented, Variant};

/// What the last Test or Save said.
enum Said {
    Tested(String),
    Failed(SharedString),
}

pub(super) struct Form {
    choices: Rc<ProfileForm>,
    engine: Engine,
    tls: String,
    read_only: bool,
    agent_writes: bool,
    forget_password: bool,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    database: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    path: Entity<InputState>,
    busy: Option<&'static str>,
    said: Option<Said>,
}

impl Form {
    pub(super) fn new(choices: Rc<ProfileForm>, window: &mut Window, cx: &mut Context<PluginPane>) -> Self {
        let draft = &choices.draft;
        let hint = match choices.has_password {
            true => "Saved in the Keychain",
            false => "None",
        };
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint));
        let mut field = |value: &str| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_owned()));
            cx.subscribe(&input, |pane, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some((database, _)) = pane.parts::<Database>() {
                        if let Some(form) = &mut database.form {
                            form.said = None;
                        }
                    }
                    cx.notify();
                }
            })
            .detach();
            input
        };
        let form = Self {
            engine: draft.engine,
            tls: draft.tls.clone(),
            read_only: draft.read_only,
            agent_writes: draft.agent_writes,
            forget_password: false,
            name: field(&draft.name),
            host: field(&draft.host),
            port: field(&draft.port),
            database: field(&draft.database),
            user: field(&draft.user),
            password,
            path: field(&draft.path),
            busy: None,
            said: None,
            choices: choices.clone(),
        };
        form.hint(window, cx);
        form
    }

    /// The fields' hints for the engine picked.
    fn hint(&self, window: &mut Window, cx: &mut App) {
        let Some(engine) = self.choices.engines.iter().find(|each| each.engine == self.engine) else {
            return;
        };
        let port = engine.default_port.map(|port| port.to_string()).unwrap_or_default();
        for (input, hint) in [
            (&self.name, engine.name_hint.clone()),
            (&self.host, "localhost".to_owned()),
            (&self.port, port),
            (&self.database, engine.database_hint.clone()),
            (&self.user, engine.user_hint.clone()),
            (&self.path, engine.path_hint.clone()),
        ] {
            input.update(cx, |input, cx| input.set_placeholder(hint, window, cx));
        }
    }

    fn draft(&self, cx: &App) -> ProfileDraft {
        let read = |input: &Entity<InputState>| input.read(cx).value().to_string();
        ProfileDraft {
            id: self.choices.draft.id.clone(),
            name: read(&self.name),
            engine: self.engine,
            host: read(&self.host),
            port: read(&self.port),
            database: read(&self.database),
            user: read(&self.user),
            tls: self.tls.clone(),
            path: read(&self.path),
            read_only: self.read_only,
            agent_writes: self.agent_writes && !self.read_only,
        }
    }

    /// Typed text replaces the saved password; ticking "forget" clears it; otherwise the saved
    /// one stays.
    fn password(&self, cx: &App) -> Option<String> {
        let typed = self.password.read(cx).value().to_string();
        match (self.forget_password, typed.is_empty()) {
            (true, _) => Some(String::new()),
            (false, true) => None,
            (false, false) => Some(typed),
        }
    }
}

/// Asks the plugin for the form of the saved connection `id`, or of a new one.
pub(super) fn open(database: &mut Database, host: &Host, id: Option<String>, cx: &mut Context<PluginPane>) {
    database.form = None;
    database.form_loading = true;
    let call = host.call("form", json!({ "id": id }));
    after::<Database, _>(
        call,
        |database, host, done, cx| {
            database.form_loading = false;
            match done.map(serde_json::from_value::<ProfileForm>) {
                Ok(Ok(form)) => database.form_arrived = Some(Rc::new(form)),
                Ok(Err(error)) => {
                    let failure = Failure {
                        category: "core".into(),
                        message: format!("unreadable form: {error}").into(),
                        status: None,
                    };
                    host.said(&failure, cx);
                }
                Err(failure) => host.said(&failure, cx),
            }
        },
        cx,
    );
    cx.notify();
}

fn run(database: &mut Database, host: &Host, kind: &'static str, cx: &mut Context<PluginPane>) {
    let Some(form) = &mut database.form else {
        return;
    };
    if form.busy.is_some() {
        return;
    }
    form.busy = Some(kind);
    form.said = None;
    let params = json!({ "profile": form.draft(cx), "password": form.password(cx) });
    let call = host.call(kind, params);
    after::<Database, _>(
        call,
        move |database, _, done, _| {
            let Some(form) = &mut database.form else {
                return;
            };
            form.busy = None;
            match (kind, done) {
                ("save", Ok(_)) => database.form = None,
                (_, Ok(tested)) => {
                    let version = tested["version"].as_str().unwrap_or_default();
                    let millis = tested["millis"].as_u64().unwrap_or_default();
                    form.said = Some(Said::Tested(format!("Connected to {version} in {millis} ms")));
                }
                (_, Err(failure)) => form.said = Some(Said::Failed(failure.message)),
            }
        },
        cx,
    );
    cx.notify();
}

/// Fills the file and, when it has none yet, the name from a file the person picks.
fn choose(window: &mut Window, cx: &mut Context<PluginPane>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.spawn_in(window, async move |pane, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(path) = paths.first() else {
            return;
        };
        let file = path.to_string_lossy().into_owned();
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = pane.update_in(cx, |pane, window, cx| {
            if let Some((database, _)) = pane.parts::<Database>() {
                if let Some(form) = &database.form {
                    form.path.update(cx, |input, cx| input.set_value(file, window, cx));
                    if form.name.read(cx).value().trim().is_empty() {
                        form.name.update(cx, |input, cx| input.set_value(name, window, cx));
                    }
                }
            }
            cx.notify();
        });
    })
    .detach();
}

pub(super) fn page(database: &Database, cx: &mut Context<PluginPane>) -> AnyElement {
    let Some(form) = &database.form else {
        return super::super::loading(cx);
    };
    let t = Tokens::get(cx).clone();
    let saved = form.choices.draft.id.clone();
    let title = match &saved {
        Some(_) => form.choices.draft.name.clone(),
        None => "New connection".to_owned(),
    };
    let server = form.engine != Engine::Sqlite;
    let at = form
        .choices
        .engines
        .iter()
        .position(|each| each.engine == form.engine)
        .unwrap_or(0);
    let engines = Segmented::words(
        "db-form-engine",
        form.choices
            .engines
            .iter()
            .map(|each| (SharedString::from(each.label.clone()), None))
            .collect(),
        at,
    )
    .on_select(listen::<Database, usize>(cx, |database, _, picked, window, cx| {
        if let Some(form) = &mut database.form {
            if let Some(engine) = form.choices.engines.get(*picked).map(|each| each.engine) {
                form.engine = engine;
                form.said = None;
                form.hint(window, cx);
            }
        }
        cx.notify();
    }));
    let labelled = |label: &'static str, input: &Entity<InputState>| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(ui::meta(label, cx))
            .child(ui::Field::new(input))
    };
    let mut fields = div().flex().flex_col().gap(px(10.)).child(labelled("Name", &form.name));
    fields = match server {
        true => {
            let tls_detail = form
                .choices
                .tls_modes
                .iter()
                .find(|mode| mode.value == form.tls)
                .map(|mode| mode.detail.clone())
                .unwrap_or_default();
            let encryption = ui::select(
                "db-form-tls",
                form.choices
                    .tls_modes
                    .iter()
                    .find(|mode| mode.value == form.tls)
                    .map_or(form.tls.clone(), |mode| mode.label.clone()),
                form.choices
                    .tls_modes
                    .iter()
                    .map(|mode| Pick::new(mode.value.clone(), mode.label.clone()).hint(mode.detail.clone()))
                    .collect(),
                Some(form.tls.clone().into()),
                false,
                on_pick(cx),
                cx,
            );
            fields
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(labelled("Host", &form.host))
                        .child(div().w(px(110.)).flex_none().child(labelled("Port", &form.port))),
                )
                .child(labelled("Database", &form.database))
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(labelled("User", &form.user))
                        .child(labelled("Password", &form.password)),
                )
                .children(form.choices.has_password.then(|| {
                    Checkbox::new("db-form-forget")
                        .label("Forget the saved password")
                        .checked(form.forget_password)
                        .on_click(listen::<Database, bool>(cx, |database, _, on, _, cx| {
                            if let Some(form) = &mut database.form {
                                form.forget_password = *on;
                            }
                            cx.notify();
                        }))
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(ui::meta("Encryption", cx))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(encryption)
                                .child(ui::meta(tls_detail, cx)),
                        ),
                )
        }
        false => fields.child(
            div()
                .flex()
                .items_end()
                .gap(px(8.))
                .child(labelled("Database file", &form.path))
                .child(
                    IconButton::new("db-form-choose", Icon::Folder, "Choose…")
                        .on_click(listen::<Database, _>(cx, |_, _, _, window, cx| choose(window, cx))),
                ),
        ),
    };
    let toggles = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            Checkbox::new("db-form-read-only")
                .label("Read only: open connections read-only, a guard against accidental changes")
                .checked(form.read_only)
                .on_click(listen::<Database, bool>(cx, |database, _, on, _, cx| {
                    if let Some(form) = &mut database.form {
                        form.read_only = *on;
                        form.agent_writes = form.agent_writes && !*on;
                    }
                    cx.notify();
                })),
        )
        .children((!form.read_only).then(|| {
            Checkbox::new("db-form-agents")
                .label("Let agents change data here. Otherwise their queries can only read")
                .checked(form.agent_writes)
                .on_click(listen::<Database, bool>(cx, |database, _, on, _, cx| {
                    if let Some(form) = &mut database.form {
                        form.agent_writes = *on;
                    }
                    cx.notify();
                }))
        }));
    let said = form.said.as_ref().map(|said| {
        let (text, ink) = match said {
            Said::Tested(text) => (SharedString::from(text.clone()), t.live),
            Said::Failed(text) => (text.clone(), t.danger),
        };
        div()
            .px(px(12.))
            .py(px(8.))
            .rounded(RADIUS_2)
            .bg(ink.opacity(0.12))
            .text_color(ink)
            .text_size(crate::theme::ui_text(12.))
            .child(text)
    });
    let busy = form.busy;
    let removing = saved.clone().map(|id| {
        let name = form.choices.draft.name.clone();
        Button::new("db-form-remove", "Remove", Variant::Outline).on_click(listen::<Database, _>(
            cx,
            move |database, _, _, _, cx| {
                database.removing = Some((id.clone(), name.clone()));
                cx.notify();
            },
        ))
    });
    let actions = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .children(removing)
        .child(div().flex_1())
        .child(
            Button::new("db-form-cancel", "Cancel", Variant::Outline).on_click(listen::<Database, _>(
                cx,
                |database, _, _, _, cx| {
                    database.form = None;
                    cx.notify();
                },
            )),
        )
        .child(
            Button::new(
                "db-form-test",
                if busy == Some("test") { "Connecting…" } else { "Test" },
                Variant::Outline,
            )
            .disabled(busy.is_some())
            .on_click(listen::<Database, _>(cx, |database, host, _, _, cx| {
                run(database, host, "test", cx)
            })),
        )
        .child(
            Button::new(
                "db-form-save",
                if busy == Some("save") { "Saving…" } else { "Save" },
                Variant::Primary,
            )
            .disabled(busy.is_some())
            .on_click(listen::<Database, _>(cx, |database, host, _, _, cx| {
                run(database, host, "save", cx)
            })),
        );
    div()
        .id("db-form")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .max_w(px(560.))
                .px(px(20.))
                .py(px(16.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(ui::text(title, 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(engines)
                .child(fields)
                .child(toggles)
                .children(said)
                .child(actions),
        )
        .into_any_element()
}

fn on_pick(cx: &Context<PluginPane>) -> impl Fn(&SharedString, &mut Window, &mut App) + 'static {
    listen::<Database, SharedString>(cx, |database, _, picked, _, cx| {
        if let Some(form) = &mut database.form {
            form.tls = picked.to_string();
            form.said = None;
        }
        cx.notify();
    })
}
