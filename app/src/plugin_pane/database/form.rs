//! The connection form, as the plugin hands it: a new connection or a saved one to edit, tried
//! and saved through the plugin, which says what a filled-in one still needs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_database_views::{Engine, ProfileDraft, ProfileForm};

use super::{Database, Host, PluginPane};
use crate::hosts::Place;
use crate::icons::Icon;
use crate::plugin_pane::Failure;
use crate::plugin_pane::{after, listen};
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_5, SPACE_6, TEXT_XS,
};
use crate::ui::{self, Button, IconButton, Pick, Variant};

/// A connection an agent proposed, which the core opens a new connection's form on.
#[derive(Deserialize)]
pub(super) struct Proposed {
    profile: ProfileDraft,
    #[serde(default)]
    password: Option<String>,
}

/// The form the plugin sent, and the password an agent's proposal carried.
pub(super) struct Arrived {
    choices: Rc<ProfileForm>,
    password: Option<String>,
}

impl Arrived {
    pub(super) fn into_form(self, window: &mut Window, cx: &mut Context<PluginPane>) -> Form {
        let form = Form::new(self.choices, window, cx);
        if let Some(password) = self.password {
            form.password
                .update(cx, |input, cx| input.set_value(password, window, cx));
        }
        form
    }
}

/// What the last Test or Save said.
enum Said {
    Tested(String),
    Failed(SharedString),
}

/// A form as the person left it, so it is still there when its pane is made again, as it is
/// after another project has been in front.
struct Kept {
    choices: Rc<ProfileForm>,
    draft: ProfileDraft,
    password: String,
    url: String,
    forget_password: bool,
}

thread_local! {
    static KEPT: RefCell<HashMap<(Place, String), Kept>> = RefCell::default();
}

fn kept_key(host: &Host) -> (Place, String) {
    (host.place().clone(), host.slot.clone())
}

/// The form this pane's slot left open, filled in as it was left.
pub(super) fn restore(
    host: &Host,
    window: &mut Window,
    cx: &mut Context<PluginPane>,
) -> Option<Form> {
    let kept = KEPT.with_borrow_mut(|kept| kept.remove(&kept_key(host)))?;
    let mut form = Form::new(kept.choices, window, cx);
    form.fill(&kept.draft, window, cx);
    form.forget_password = kept.forget_password;
    for (input, value) in [(&form.password, kept.password), (&form.url, kept.url)] {
        input.update(cx, |input, cx| input.set_value(value, window, cx));
    }
    Some(form)
}

/// Keeps the form open in this pane's slot as it stands, or forgets it once it is closed.
pub(super) fn keep(host: &Host, form: Option<&Form>, cx: &App) {
    let key = kept_key(host);
    let Some(form) = form else {
        KEPT.with_borrow_mut(|kept| kept.remove(&key));
        return;
    };
    let read = |input: &Entity<InputState>| input.read(cx).value().to_string();
    let now = Kept {
        choices: form.choices.clone(),
        draft: form.draft(cx),
        password: read(&form.password),
        url: read(&form.url),
        forget_password: form.forget_password,
    };
    KEPT.with_borrow_mut(|kept| kept.insert(key, now));
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
    url: Entity<InputState>,
    /// Why the URL typed could not be read.
    url_failed: Option<SharedString>,
    busy: Option<&'static str>,
    said: Option<Said>,
}

impl Form {
    pub(super) fn new(
        choices: Rc<ProfileForm>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Self {
        let draft = &choices.draft;
        let hint = match choices.has_password {
            true => "Saved in the Keychain",
            false => "None",
        };
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint));
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("postgres://user:password@host:5432/database")
        });
        cx.subscribe_in(&url, window, |pane, url, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                let typed = url.read(cx).value().trim().to_owned();
                from_url(pane, typed, window, cx);
            }
        })
        .detach();
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
            url,
            url_failed: None,
            busy: None,
            said: None,
            choices: choices.clone(),
        };
        form.hint(window, cx);
        form
    }

    /// Picks another engine, moving the port and user to its usual ones unless they were typed.
    fn switch(&mut self, engine: Engine, window: &mut Window, cx: &mut App) {
        let choice = |engine: Engine| {
            self.choices
                .engines
                .iter()
                .find(|each| each.engine == engine)
                .cloned()
        };
        if let (Some(was), Some(now)) = (choice(self.engine), choice(engine)) {
            let port = |choice: &sikemux_plugin_database_views::EngineChoice| {
                choice.default_port.map(|port| port.to_string())
            };
            for (input, was, now) in [
                (&self.port, port(&was), port(&now)),
                (&self.user, Some(was.user_hint), Some(now.user_hint)),
            ] {
                let typed = input.read(cx).value().to_string();
                let (Some(was), Some(now)) = (was, now) else {
                    continue;
                };
                if (typed.is_empty() || typed == was) && !now.is_empty() {
                    input.update(cx, |input, cx| input.set_value(now, window, cx));
                }
            }
        }
        self.engine = engine;
        self.said = None;
        self.hint(window, cx);
    }

    /// The fields' hints for the engine picked.
    fn hint(&self, window: &mut Window, cx: &mut App) {
        let Some(engine) = self
            .choices
            .engines
            .iter()
            .find(|each| each.engine == self.engine)
        else {
            return;
        };
        let port = engine
            .default_port
            .map(|port| port.to_string())
            .unwrap_or_default();
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

    /// Puts `draft` into the fields, keeping the saved connection the form edits.
    fn fill(&mut self, draft: &ProfileDraft, window: &mut Window, cx: &mut App) {
        self.engine = draft.engine;
        self.tls = draft.tls.clone();
        self.read_only = draft.read_only;
        self.agent_writes = draft.agent_writes;
        for (input, value) in [
            (&self.name, &draft.name),
            (&self.host, &draft.host),
            (&self.port, &draft.port),
            (&self.database, &draft.database),
            (&self.user, &draft.user),
            (&self.path, &draft.path),
        ] {
            if input.read(cx).value().as_ref() != value.as_str() {
                input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
            }
        }
        self.hint(window, cx);
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
pub(super) fn open(
    database: &mut Database,
    host: &Host,
    id: Option<String>,
    proposed: Option<Proposed>,
    cx: &mut Context<PluginPane>,
) {
    database.form = None;
    database.form_loading = true;
    let call = host.call("form", json!({ "id": id }));
    after::<Database, _>(
        call,
        |database, host, done, cx| {
            database.form_loading = false;
            match done.map(serde_json::from_value::<ProfileForm>) {
                Ok(Ok(mut form)) => {
                    let password = proposed.and_then(|proposed| {
                        form.draft = proposed.profile;
                        proposed.password
                    });
                    database.form_arrived = Some(Arrived {
                        choices: Rc::new(form),
                        password,
                    });
                }
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

/// Fills the fields from the connection URL `typed`, as the plugin reads it.
fn from_url(
    pane: &mut PluginPane,
    typed: String,
    window: &mut Window,
    cx: &mut Context<PluginPane>,
) {
    let Some((database, host)) = pane.parts::<Database>() else {
        return;
    };
    let Some(form) = &mut database.form else {
        return;
    };
    form.url_failed = None;
    cx.notify();
    if typed.is_empty() {
        return;
    }
    let call = host.call(
        "fromUrl",
        json!({ "url": typed, "profile": form.draft(cx) }),
    );
    cx.spawn_in(window, async move |pane, cx| {
        let read = call.await;
        let _ = pane.update_in(cx, |pane, window, cx| {
            let Some((database, _)) = pane.parts::<Database>() else {
                return;
            };
            let Some(form) = &mut database.form else {
                return;
            };
            if form.url.read(cx).value().trim() != typed {
                return;
            }
            match read {
                Ok(read) => {
                    let draft = serde_json::from_value::<ProfileDraft>(read["profile"].clone());
                    match draft {
                        Ok(draft) => form.fill(&draft, window, cx),
                        Err(error) => form.url_failed = Some(error.to_string().into()),
                    }
                    if let Some(password) = read["password"].as_str() {
                        let password = password.to_owned();
                        form.password
                            .update(cx, |input, cx| input.set_value(password, window, cx));
                    }
                    form.said = None;
                }
                Err(failure) => form.url_failed = Some(failure.message),
            }
            cx.notify();
        });
    })
    .detach();
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
        move |database, host, done, cx| {
            let Some(form) = &mut database.form else {
                return;
            };
            if let ("save", Ok(_)) = (kind, &done) {
                return show_saved(database, host, cx);
            }
            form.busy = None;
            match done {
                Ok(tested) => {
                    let version = tested["version"].as_str().unwrap_or_default();
                    let millis = tested["millis"].as_u64().unwrap_or_default();
                    form.said = Some(Said::Tested(format!(
                        "Connected to {version} in {millis} ms"
                    )));
                }
                Err(failure) => form.said = Some(Said::Failed(failure.message)),
            }
        },
        cx,
    );
    cx.notify();
}

/// Closes the form once the explorer, read again from the plugin itself, holds what was saved,
/// since the explorer the core keeps can still be the one from before.
fn show_saved(database: &mut Database, host: &Host, cx: &mut Context<PluginPane>) {
    let params = json!({ "open": database.open_nodes() });
    let read = host.call("explorerView", params.clone());
    after::<Database, _>(
        read,
        move |database, host, read, _| {
            if let Ok(explorer) = read {
                host.put_view("explorerView", &params, explorer);
            }
            database.form = None;
        },
        cx,
    );
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
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = pane.update_in(cx, |pane, window, cx| {
            if let Some((database, _)) = pane.parts::<Database>() {
                if let Some(form) = &database.form {
                    form.path
                        .update(cx, |input, cx| input.set_value(file, window, cx));
                    if form.name.read(cx).value().trim().is_empty() {
                        form.name
                            .update(cx, |input, cx| input.set_value(name, window, cx));
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
    let engines = div()
        .flex()
        .gap(SPACE_2H)
        .children(form.choices.engines.iter().map(|each| {
            let engine = each.engine;
            let chosen = engine == form.engine;
            div()
                .id(SharedString::from(format!("db-form-engine-{}", each.label)))
                .flex_1()
                .min_w_0()
                .py(SPACE_3H)
                .flex()
                .flex_col()
                .items_center()
                .gap(SPACE_2)
                .rounded(RADIUS_2)
                .border_1()
                .cursor_pointer()
                .map(|tile| match chosen {
                    true => tile.bg(t.surface_chosen).border_color(t.border_strong),
                    false => tile
                        .border_color(t.border)
                        .hover(|tile| tile.bg(t.surface_hover)),
                })
                .child(logo_mark(engine, 34.))
                .child(
                    ui::text(each.label.clone(), 12.5, FontWeight::MEDIUM).text_color(
                        match chosen {
                            true => t.text_primary,
                            false => t.text_secondary,
                        },
                    ),
                )
                .on_click(listen::<Database, ClickEvent>(
                    cx,
                    move |database, _, _, window, cx| {
                        if let Some(form) = &mut database.form {
                            if form.engine != engine {
                                form.switch(engine, window, cx);
                            }
                        }
                        cx.notify();
                    },
                ))
        }));
    let labelled = |label: &'static str, input: &Entity<InputState>| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(ui::meta(label, cx))
            .child(ui::Field::new(input))
    };
    let mut fields = div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .child(labelled("Name", &form.name));
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
                    .map(|mode| {
                        Pick::new(mode.value.clone(), mode.label.clone()).hint(mode.detail.clone())
                    })
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
                        .gap(SPACE_2H)
                        .child(labelled("Host", &form.host))
                        .child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .child(labelled("Port", &form.port)),
                        ),
                )
                .child(labelled("Database", &form.database))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_2H)
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
                        .gap(SPACE_1)
                        .child(ui::meta("Encryption", cx))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_2H)
                                .child(encryption)
                                .child(ui::meta(tls_detail, cx)),
                        ),
                )
        }
        false => {
            fields.child(
                div()
                    .flex()
                    .items_end()
                    .gap(SPACE_2)
                    .child(labelled("Database file", &form.path))
                    .child(
                        IconButton::new("db-form-choose", Icon::Folder, "Choose…").on_click(
                            listen::<Database, _>(cx, |_, _, _, window, cx| choose(window, cx)),
                        ),
                    ),
            )
        }
    };
    let toggles = div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
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
            .px(SPACE_3)
            .py(SPACE_2)
            .rounded(RADIUS_2)
            .bg(ink.opacity(0.12))
            .text_color(ink)
            .text_size(crate::theme::ui_text(TEXT_XS))
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
        .px(SPACE_5)
        .py(SPACE_3H)
        .border_t_1()
        .border_color(t.border)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(removing)
        .child(div().flex_1())
        .child(
            Button::new("db-form-cancel", "Cancel", Variant::Outline).on_click(
                listen::<Database, _>(cx, |database, _, _, _, cx| {
                    database.form = None;
                    cx.notify();
                }),
            ),
        )
        .child(
            Button::new(
                "db-form-test",
                if busy == Some("test") {
                    "Connecting…"
                } else {
                    "Test"
                },
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
                if busy == Some("save") {
                    "Saving…"
                } else {
                    "Save"
                },
                Variant::Primary,
            )
            .disabled(busy.is_some())
            .on_click(listen::<Database, _>(cx, |database, host, _, _, cx| {
                run(database, host, "save", cx)
            })),
        );
    let subtitle = match server {
        true => "A PostgreSQL or MySQL server. Passwords stay in the Keychain.",
        false => "A SQLite file on this host.",
    };
    let header = div()
        .px(SPACE_5)
        .pt(px(18.))
        .pb(SPACE_3H)
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(logo_mark(form.engine, 30.))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(ui::text(title, 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(ui::meta(subtitle, cx)),
        );
    let rule = || div().flex_1().h(px(1.)).bg(t.border);
    let by_url = div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(ui::meta("Connection URL", cx))
        .child(ui::Field::new(&form.url))
        .children(
            form.url_failed
                .clone()
                .map(|failed| ui::meta(failed, cx).text_color(t.danger)),
        );
    let or = div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(rule())
        .child(ui::meta("or", cx))
        .child(rule());
    let body = div()
        .px(SPACE_5)
        .pb(px(18.))
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .child(by_url)
        .child(or)
        .child(engines)
        .child(fields)
        .child(toggles)
        .children(said);
    div()
        .id("db-form")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .min_h_full()
                .px(SPACE_6)
                .py(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    ui::panel(cx)
                        .w_full()
                        .max_w(px(540.))
                        .flex_none()
                        .child(header)
                        .child(body)
                        .child(actions),
                ),
        )
        .into_any_element()
}

fn logo_mark(engine: Engine, size: f32) -> Div {
    ui::mark_box(size, ui::picture(logo(engine), size, gpui_kit::white()))
}

fn logo(engine: Engine) -> Icon {
    match engine {
        Engine::Postgres => Icon::Postgresql,
        Engine::Mysql => Icon::Mysql,
        Engine::Sqlite => Icon::Sqlite,
    }
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
