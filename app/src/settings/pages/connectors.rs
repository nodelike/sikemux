//! The connectors built in: the services agents reach for the person, each with whether it is
//! signed in and as whom, its switch and its sign-in, and the few settings its tools read that
//! only the person changes.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_plugin_api::Listed;

use crate::core::{outcome, Ask, Target};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_2};
use crate::toast;
use crate::ui::{self, dialog, sign_in};
use crate::workbench::Workbench;

const AWS: &str = "sikemux.aws";
const RUNDECK: &str = "sikemux.rundeck";
const DATABASE: &str = "sikemux.database";

/// What the page keeps while it shows: the sign-in open over it, and what the connectors with
/// settings of their own last answered.
#[derive(Default)]
pub struct Fields {
    /// The plugins moved since the settings below were read, so they are read again.
    pub(super) stale: bool,
    signing: Option<Signing>,
    aws: Option<Aws>,
    rundeck: Option<Lists>,
    databases: Vec<Database>,
}

/// A sign-in on its way: the token form's boxes, and the browser's, while it runs.
struct Signing {
    plugin: String,
    fields: Vec<Entity<InputState>>,
    _typing: Vec<Subscription>,
    /// The AWS profile to sign in with.
    profile: Option<String>,
    busy: bool,
    refused: Option<SharedString>,
    /// The browser sign-in, which dropping stops.
    browser: Option<Task<()>>,
}

#[derive(Default)]
struct Aws {
    profiles: Vec<String>,
    /// The profile the sign-in is checked as.
    account: Option<String>,
    /// The profile every agent starts on, when the person picked one.
    agents: Option<String>,
}

/// Rundeck's lists, as the person types them.
struct Lists {
    production: Entity<InputState>,
    branches: Entity<InputState>,
    _typing: [Subscription; 2],
}

#[derive(Deserialize)]
struct Database {
    name: String,
    engine: String,
    address: String,
    writable: bool,
    production: bool,
}

#[derive(Deserialize)]
struct Profile {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RundeckLists {
    branch_options: Vec<String>,
    prod_envs: Vec<String>,
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    super::change_pages(bench, cx, |pages, _| {
        pages.connectors = Some(Fields::default())
    });
    read(bench, window, cx);
}

fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<crate::settings::Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.connectors.as_mut(), change)
}

/// Asks `plugin` for `method`, and reads its answer.
fn ask<T: serde::de::DeserializeOwned + 'static>(
    bench: &Workbench,
    plugin: &str,
    method: &str,
    params: Value,
) -> impl std::future::Future<Output = Result<T, SharedString>> + use<T> {
    let asked = Target::here(&bench.asks).request(
        "plugin",
        json!({ "plugin": plugin, "method": method, "params": params }),
    );
    async move { outcome(&asked.await.map_err(SharedString::from)?) }
}

/// Reads what the connectors with settings of their own say now.
pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.stale = false);
    let on = |id: &str| {
        bench
            .reads
            .plugins
            .all()
            .any(|plugin| plugin.id == id && plugin.enabled)
    };
    if on(AWS) {
        let profiles = ask::<Vec<Profile>>(bench, AWS, "profiles", Value::Null);
        let status = ask::<Value>(bench, AWS, "status", Value::Null);
        let picks = ask::<Value>(bench, AWS, "agentProfilePicks", Value::Null);
        cx.spawn(async move |bench, cx| {
            let aws = Aws {
                profiles: (profiles.await.unwrap_or_default().into_iter())
                    .map(|profile| profile.name)
                    .collect(),
                account: status
                    .await
                    .ok()
                    .and_then(|status| text(&status["profile"])),
                agents: picks.await.ok().and_then(|picks| text(&picks["default"])),
            };
            let _ = bench.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.aws = Some(aws));
            });
        })
        .detach();
    }
    if on(RUNDECK) && bench.reads.plugins.signed_in(RUNDECK) == Some(true) {
        let lists = ask::<RundeckLists>(bench, RUNDECK, "lists", Value::Null);
        cx.spawn_in(window, async move |bench, cx| {
            let Ok(lists) = lists.await else {
                return;
            };
            let _ = bench.update_in(cx, |bench, window, cx| {
                let made = rundeck_lists(&lists, window, cx);
                change_fields(bench, cx, |fields, _| fields.rundeck = Some(made));
            });
        })
        .detach();
    }
    if on(DATABASE) {
        let saved = ask::<Vec<Database>>(bench, DATABASE, "agentDatabases", Value::Null);
        cx.spawn(async move |bench, cx| {
            let saved = saved.await.unwrap_or_default();
            let _ = bench.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.databases = saved);
            });
        })
        .detach();
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Rundeck's two lists as boxes, each saved once the person leaves it or presses Return, so a
/// list half typed never gates a run.
fn rundeck_lists(lists: &RundeckLists, window: &mut Window, cx: &mut Context<Workbench>) -> Lists {
    let mut field = |key: &'static str, words: &[String], placeholder: &'static str| {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(words.join(", "))
        });
        let saving = cx.subscribe(&input, move |bench, input, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let words: Vec<String> = (input.read(cx).value().split(','))
                .map(|word| word.trim().to_owned())
                .filter(|word| !word.is_empty())
                .collect();
            let saved = ask::<Value>(bench, RUNDECK, "setLists", json!({ key: words }));
            cx.spawn(async move |bench, cx| {
                if let Err(error) = saved.await {
                    let _ = bench.update(cx, |bench, cx| {
                        bench.notify(toast::error(format!("Rundeck: {error}")), cx)
                    });
                }
            })
            .detach();
        });
        (input, saving)
    };
    let (production, saved_production) =
        field("prodEnvs", &lists.prod_envs, "prod, production, live");
    let (branches, saved_branches) =
        field("branchOptions", &lists.branch_options, "branch, git_branch");
    Lists {
        production,
        branches,
        _typing: [saved_production, saved_branches],
    }
}

/// Switches `id` on or off by sending the core every plugin that is off.
fn switch(bench: &Workbench, id: &str, on: bool) {
    let mut off: Vec<String> = bench
        .reads
        .plugins
        .all()
        .filter(|plugin| !plugin.enabled && plugin.id != id)
        .map(|plugin| plugin.id.clone())
        .collect();
    if !on {
        off.push(id.to_owned());
    }
    let _ = bench.asks.send(Ask::Call {
        method: "pluginSetDisabled",
        params: json!({ "ids": off }),
    });
}

/// The connector's mark, as the rest of the app draws it.
fn mark(id: &str) -> Icon {
    id.strip_prefix("sikemux.")
        .and_then(Icon::named)
        .unwrap_or(Icon::Plug)
}

/// It is signed in to some other way than a token form or the browser, or not at all.
fn has_sign_in(plugin: &Listed) -> bool {
    plugin.id == AWS || plugin.token_form.is_some() || plugin.browser_sign_in
}

/// How the connector stands, in the line under its name.
fn standing(plugin: &Listed, fields: &Fields) -> SharedString {
    if !plugin.enabled {
        return "Off".into();
    }
    if plugin.id == DATABASE {
        return match fields.databases.len() {
            0 => "No databases saved yet. Agents propose them for you to approve.".into(),
            1 => "1 database saved".into(),
            saved => format!("{saved} databases saved").into(),
        };
    }
    match (plugin.signed_in, &plugin.signed_in_as, &plugin.problem) {
        (Some(true), Some(who), _) => format!("Signed in as {who}").into(),
        (Some(true), None, _) => "Signed in".into(),
        (Some(false), _, Some(problem)) => problem.clone().into(),
        _ => "Not signed in".into(),
    }
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let plugins = &bench.reads.plugins;
    let Some(fields) = &pages.connectors else {
        return Vec::new();
    };
    let ink = Tokens::get(cx).text_secondary;
    let body = match plugins.is_empty() {
        true => rows::note("No connectors in this build.", cx),
        false => div()
            .children(plugins.all().map(|plugin| {
                let entry = rows::row_with(
                    Some(ui::picture(mark(&plugin.id), 16., ink).into_any_element()),
                    plugin.name.clone(),
                    Some(standing(plugin, fields)),
                    controls(plugin, this),
                    cx,
                );
                div()
                    .child(entry)
                    .children(settings(plugin, fields, this, cx))
                    .into_any_element()
            }))
            .into_any_element(),
    };
    vec![rows::section(
        "Connectors",
        None,
        Some("Sign in once here and every agent's tools use it. A connector switched off leaves agents' tools, and costs nothing until it is back on.".into()),
        lit("Connectors"),
        body,
        cx,
    )]
}

/// The connector's Sign in or Sign out, and its switch.
fn controls(plugin: &Listed, this: &Entity<Workbench>) -> AnyElement {
    let id = plugin.id.clone();
    let signed_in = plugin.signed_in == Some(true);
    let button = (plugin.enabled && has_sign_in(plugin)).then(|| {
        let (label, signing_out) = match signed_in && plugin.sign_out {
            true => ("Sign out", true),
            false if signed_in => ("Sign in again", false),
            false => ("Sign in", false),
        };
        let this = this.clone();
        let id = id.clone();
        ui::Button::new(
            SharedString::from(format!("connector-sign-{id}")),
            label,
            ui::Variant::Outline,
        )
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| match signing_out {
                true => sign_out(bench, &id, cx),
                false => open_sign_in(bench, &id, window, cx),
            })
        })
    });
    let toggle = {
        let this = this.clone();
        Switch::new(SharedString::from(format!("plugin-on-{id}")))
            .checked(plugin.enabled)
            .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                let on = *on;
                this.update(cx, |bench, _| switch(bench, &id, on))
            })
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(button)
        .child(toggle)
        .into_any_element()
}

/// A row under a connector's own, lined up with its name.
fn setting(
    label: impl Into<SharedString>,
    about: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    rows::row_with(
        Some(div().w(px(16.)).flex_none().into_any_element()),
        label,
        about,
        control,
        cx,
    )
    .into_any_element()
}

/// The rows of the settings a connector's tools read, while it is on.
fn settings(
    plugin: &Listed,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    if !plugin.enabled {
        return Vec::new();
    }
    match plugin.id.as_str() {
        AWS => aws_settings(fields, this, cx),
        RUNDECK => fields.rundeck.as_ref().map_or_else(Vec::new, |lists| {
            let wide = |input: &Entity<InputState>| div().w(px(260.)).child(ui::Field::new(input));
            vec![
                setting(
                    "Production words",
                    Some("A job whose project or top folder holds one of these is production, and agents ask you before running it.".into()),
                    wide(&lists.production),
                    cx,
                ),
                setting(
                    "Branch options",
                    Some("The job options that name the branch a run deploys.".into()),
                    wide(&lists.branches),
                    cx,
                ),
            ]
        }),
        DATABASE => fields
            .databases
            .iter()
            .map(|database| database_row(database, this, cx))
            .collect(),
        _ => Vec::new(),
    }
}

fn aws_settings(fields: &Fields, this: &Entity<Workbench>, cx: &App) -> Vec<AnyElement> {
    let Some(aws) = &fields.aws else {
        return Vec::new();
    };
    if aws.profiles.is_empty() {
        return vec![setting(
            "No profiles yet",
            Some("Add one to ~/.aws/config, then open this page again.".into()),
            div(),
            cx,
        )];
    }
    let picks = |automatic: Option<&str>| {
        automatic
            .map(|label| ui::Pick::new("", label))
            .into_iter()
            .chain(
                aws.profiles
                    .iter()
                    .map(|name| ui::Pick::new(name.clone(), name.clone())),
            )
            .collect::<Vec<_>>()
    };
    let pick = |method: &'static str| {
        let this = this.clone();
        move |key: &SharedString, window: &mut Window, cx: &mut App| {
            let profile = (!key.is_empty()).then(|| key.to_string());
            this.update(cx, |bench, cx| {
                let saved = ask::<Value>(bench, AWS, method, json!({ "profile": profile }));
                cx.spawn_in(window, async move |bench, cx| {
                    let said = saved.await;
                    let _ = bench.update_in(cx, |bench, window, cx| {
                        if let Err(error) = said {
                            bench.notify(toast::error(format!("AWS: {error}")), cx);
                        }
                        read(bench, window, cx);
                    });
                })
                .detach();
            })
        }
    };
    let account = aws.account.clone().unwrap_or_default();
    let agents = aws.agents.clone().unwrap_or_default();
    vec![
        setting(
            "Signed in with",
            Some("The profile whose sign-in this page checks.".into()),
            ui::select(
                "aws-account",
                match account.is_empty() {
                    true => "Pick a profile".to_owned(),
                    false => account.clone(),
                },
                picks(None),
                Some(account.into()),
                false,
                pick("setDefaultAccount"),
                cx,
            ),
            cx,
        ),
        setting(
            "Profile agents start on",
            Some("Set as AWS_PROFILE for every agent, unless your own shell sets one.".into()),
            ui::select(
                "aws-agents",
                match agents.is_empty() {
                    true => "Automatic".to_owned(),
                    false => agents.clone(),
                },
                picks(Some("Automatic")),
                Some(agents.into()),
                false,
                pick("pickAgentProfile"),
                cx,
            ),
            cx,
        ),
    ]
}

/// A saved database, with whether agents may change its data, whether it is production, and the
/// way to remove it.
fn database_row(database: &Database, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let name = database.name.clone();
    let remove = {
        let (this, name) = (this.clone(), name.clone());
        ui::IconButton::new(
            SharedString::from(format!("database-remove-{name}")),
            Icon::Trash,
            "Remove",
        )
        .on_click(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let params = json!({ "database": name });
            this.update(cx, |bench, cx| {
                change_database(bench, "remove", params, window, cx)
            })
        })
    };
    setting(
        database.name.clone(),
        Some(format!("{} · {}", database.engine, database.address).into()),
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(database_switch(
                &name,
                "Agents may change data",
                database.writable,
                "setAgentWrites",
                this,
                cx,
            ))
            .child(database_switch(
                &name,
                "Production",
                database.production,
                "setProduction",
                this,
                cx,
            ))
            .child(remove),
        cx,
    )
}

/// One of a saved database's switches, labelled, which `method` sets.
fn database_switch(
    name: &str,
    label: &'static str,
    checked: bool,
    method: &'static str,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let (this, database) = (this.clone(), name.to_owned());
    let switch = Switch::new(SharedString::from(format!("database-{method}-{name}")))
        .checked(checked)
        .on_change(move |on: &bool, window: &mut Window, cx: &mut App| {
            let params = json!({ "database": database, "on": *on });
            this.update(cx, |bench, cx| {
                change_database(bench, method, params, window, cx)
            })
        });
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(crate::theme::ui_text(crate::theme::TEXT_XS))
        .text_color(t.text_tertiary)
        .child(label)
        .child(switch)
        .into_any_element()
}

fn change_database(
    bench: &mut Workbench,
    method: &'static str,
    params: Value,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let changed = ask::<Value>(bench, DATABASE, method, params);
    cx.spawn_in(window, async move |bench, cx| {
        let said = changed.await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            if let Err(error) = said {
                bench.notify(toast::error(format!("Database: {error}")), cx);
            }
            read(bench, window, cx);
        });
    })
    .detach();
}

fn sign_out(bench: &mut Workbench, id: &str, cx: &mut Context<Workbench>) {
    let signed_out = ask::<Value>(bench, id, "signOut", json!({}));
    let name = (bench.reads.plugins.all())
        .find(|plugin| plugin.id == id)
        .map_or_else(|| id.to_owned(), |plugin| plugin.name.clone());
    cx.spawn(async move |bench, cx| {
        if let Err(error) = signed_out.await {
            let _ = bench.update(cx, |bench, cx| {
                bench.notify(toast::error(format!("{name}: {error}")), cx)
            });
        }
    })
    .detach();
}

fn open_sign_in(bench: &mut Workbench, id: &str, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(plugin) = bench
        .reads
        .plugins
        .all()
        .find(|plugin| plugin.id == id)
        .cloned()
    else {
        return;
    };
    let form = plugin.token_form.clone();
    let mut typing = Vec::new();
    let fields = (form.iter().flat_map(|form| &form.fields))
        .map(|field| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(field.placeholder.clone())
                    .masked(field.secret)
            });
            typing.push(super::follow_typing(bench, &input, cx));
            input
        })
        .collect();
    let profile = (plugin.id == AWS)
        .then(|| {
            super::state(bench, cx)?
                .connectors
                .as_ref()?
                .aws
                .as_ref()?
                .account
                .clone()
        })
        .flatten();
    let signing = Signing {
        plugin: plugin.id,
        fields,
        _typing: typing,
        profile,
        busy: false,
        refused: None,
        browser: None,
    };
    change_fields(bench, cx, |fields, _| fields.signing = Some(signing));
}

fn close_sign_in(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.signing = None);
}

/// Sends what the token form holds to the method it names.
fn sign_in_with_token(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(fields) = super::state(bench, cx).and_then(|pages| pages.connectors.as_ref()) else {
        return;
    };
    let Some(signing) = &fields.signing else {
        return;
    };
    let Some(form) = bench.reads.plugins.token_form(&signing.plugin) else {
        return;
    };
    let typed: Map<String, Value> = form
        .fields
        .iter()
        .zip(&signing.fields)
        .map(|(field, input)| {
            let value = input.read(cx).value().trim().to_owned();
            let value = (!value.is_empty()).then_some(Value::String(value));
            (field.name.clone(), value.unwrap_or(Value::Null))
        })
        .collect();
    let signed = ask::<Value>(bench, &signing.plugin, &form.method, typed.into());
    change_fields(bench, cx, |fields, _| {
        if let Some(signing) = &mut fields.signing {
            signing.busy = true;
            signing.refused = None;
        }
    });
    cx.spawn(async move |bench, cx| {
        let signed = signed.await.map(|_| ());
        let _ = bench.update(cx, |bench, cx| took_sign_in(bench, signed, cx));
    })
    .detach();
}

/// Starts the connector's sign-in in the browser: its own, or for AWS the profile's single
/// sign-on, opened in the browser Settings › Single sign-on names.
fn sign_in_with_browser(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(signing) = super::state(bench, cx)
        .and_then(|pages| pages.connectors.as_ref())
        .and_then(|fields| fields.signing.as_ref())
    else {
        return;
    };
    let plugin = signing.plugin.clone();
    let (method, params) = match plugin.as_str() {
        AWS => ("signIn", json!({ "profile": signing.profile })),
        _ => ("signInWithBrowser", json!({})),
    };
    let preferences = bench.preferences();
    let browser = (plugin == AWS && !preferences.cloud_browser.trim().is_empty()).then(|| {
        (
            preferences.cloud_browser.trim().to_owned(),
            preferences.cloud_browser_shortcut.clone(),
        )
    });
    let mut streaming = Target::here(&bench.asks).stream(
        "pluginStream",
        json!({ "plugin": plugin, "method": method, "params": params }),
    );
    let waiting = cx.spawn(async move |bench, cx| {
        let mut failed = None;
        while let Some(part) = streaming.parts.recv().await {
            for item in part.items {
                if let Some(url) = item["url"].as_str() {
                    open_page(url.to_owned(), browser.clone(), cx);
                }
            }
            if let Some(error) = part.error {
                let message = error["message"].as_str().map(str::to_owned);
                failed = Some(message.unwrap_or_else(|| error.to_string()));
            }
            if part.done {
                break;
            }
        }
        let signed = failed.map_or(Ok(()), |failed| Err(failed.into()));
        let _ = bench.update(cx, |bench, cx| took_sign_in(bench, signed, cx));
    });
    change_fields(bench, cx, |fields, _| {
        if let Some(signing) = &mut fields.signing {
            signing.busy = true;
            signing.refused = None;
            signing.browser = Some(waiting);
        }
    });
}

/// Opens a sign-in page in `browser`, the app and the shortcut that brings it forward, else in
/// the default browser.
fn open_page(url: String, browser: Option<(String, String)>, cx: &mut gpui_kit::AsyncApp) {
    #[cfg(target_os = "macos")]
    if let Some((app, shortcut)) = browser {
        cx.background_executor()
            .spawn(async move { crate::cloud_browser::open(&url, &app, &shortcut) })
            .detach();
        return;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = browser;
    cx.update(|cx| cx.open_url(&url));
}

/// The sign-in finished: the dialog closes, or says why the service refused it.
fn took_sign_in(
    bench: &mut Workbench,
    signed: Result<(), SharedString>,
    cx: &mut Context<Workbench>,
) {
    match signed {
        Ok(()) => close_sign_in(bench, cx),
        Err(refused) => {
            change_fields(bench, cx, |fields, _| {
                if let Some(signing) = &mut fields.signing {
                    signing.busy = false;
                    signing.browser = None;
                    signing.refused = Some(refused);
                }
            });
        }
    }
}

fn stop_browser(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| {
        if let Some(signing) = &mut fields.signing {
            signing.busy = false;
            signing.browser = None;
        }
    });
}

/// What the open sign-in shows, read out of the page's state before it is drawn.
struct Shown {
    plugin: String,
    fields: Vec<Entity<InputState>>,
    profile: Option<String>,
    busy: bool,
    refused: Option<SharedString>,
    browsing: bool,
}

/// A click that runs `run` on the workbench.
fn on(
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |_, _, cx| this.update(cx, |bench, cx| run(bench, cx))
}

/// The sign-in over the page while one is open: the browser's way in where the connector has
/// one, then its token form.
pub fn dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let (signing, profiles) = {
        let fields = super::state(bench, cx)?.connectors.as_ref()?;
        let signing = fields.signing.as_ref()?;
        let profiles = fields.aws.as_ref().map(|aws| aws.profiles.clone());
        (
            Shown {
                plugin: signing.plugin.clone(),
                fields: signing.fields.clone(),
                profile: signing.profile.clone(),
                busy: signing.busy,
                refused: signing.refused.clone(),
                browsing: signing.browser.is_some(),
            },
            profiles.unwrap_or_default(),
        )
    };
    let plugin = bench
        .reads
        .plugins
        .all()
        .find(|plugin| plugin.id == signing.plugin)?;
    let t = Tokens::get(cx).clone();
    let name = plugin.name.clone();
    let form = plugin.token_form.as_ref();
    let browser = plugin.browser_sign_in || plugin.id == AWS;
    let mut screen = sign_in::Screen::new(
        "connector-sign-in",
        ui::picture(mark(&plugin.id), 26., t.text_primary),
        format!("Sign in to {name}"),
    )
    .lede(form.map_or_else(
        || format!("Tools for your agents to use {name} for you."),
        |form| form.lede.clone(),
    ))
    .foot(sign_in::KEPT);
    if plugin.id == AWS {
        let chosen = signing.profile.clone().unwrap_or_default();
        let this = this.clone();
        screen = screen.child(ui::select(
            "connector-aws-profile",
            match chosen.is_empty() {
                true => "Pick a profile".to_owned(),
                false => chosen.clone(),
            },
            profiles
                .iter()
                .map(|name| ui::Pick::new(name.clone(), name.clone()))
                .collect(),
            Some(chosen.into()),
            signing.busy,
            move |key, _, cx| {
                let profile = key.to_string();
                this.update(cx, |bench, cx| {
                    change_fields(bench, cx, |fields, _| {
                        if let Some(signing) = &mut fields.signing {
                            signing.profile = Some(profile);
                        }
                    });
                })
            },
            cx,
        ));
    }
    if browser {
        screen = screen.child(match signing.browsing {
            true => sign_in::waiting(
                "connector-browser",
                "Finish signing in in your browser",
                on(this, stop_browser),
                cx,
            )
            .into_any_element(),
            false => ui::Button::new(
                "connector-browser",
                format!("Continue with {name}"),
                ui::Variant::Primary,
            )
            .wide()
            .icon(mark(&plugin.id))
            .disabled(signing.busy || (plugin.id == AWS && signing.profile.is_none()))
            .on_click(on(this, sign_in_with_browser))
            .into_any_element(),
        });
    }
    if let Some(form) = form {
        if browser {
            screen = screen.child(sign_in::or("or paste a token", cx));
        }
        for (field, input) in form.fields.iter().zip(&signing.fields) {
            let typed = sign_in::Field::new(field.label.clone(), input);
            screen = screen.child(if field.secret { typed.mono() } else { typed });
        }
        let filled = signing
            .fields
            .iter()
            .all(|input| !input.read(cx).value().trim().is_empty());
        screen = screen.child(
            ui::Button::new(
                "connector-token",
                if signing.busy && !signing.browsing {
                    "Signing in…"
                } else {
                    "Sign in"
                },
                match browser {
                    true => ui::Variant::Outline,
                    false => ui::Variant::Primary,
                },
            )
            .wide()
            .disabled(signing.busy || !filled)
            .on_click(on(this, sign_in_with_token)),
        );
    }
    match (&signing.refused, &plugin.problem) {
        (Some(refused), _) => screen = screen.child(sign_in::error(refused.clone(), cx)),
        (None, Some(problem)) => screen = screen.child(sign_in::note(problem.clone(), cx)),
        (None, None) => {}
    }
    let server = form
        .and_then(|form| {
            form.fields
                .iter()
                .zip(&signing.fields)
                .find(|(field, _)| field.name == "host")
        })
        .map(|(_, input)| input.read(cx).value().to_string())
        .unwrap_or_default();
    let page = form.and_then(|form| form.token_page(&server));
    screen = screen.child(
        sign_in::links()
            .when_some(page, |links, page| {
                links.child(
                    sign_in::link("connector-token-page", "Create a token", cx)
                        .on_click(move |_, _, cx| cx.open_url(&page)),
                )
            })
            .child(
                sign_in::link("connector-cancel", "Cancel", cx).on_click(on(this, close_sign_in)),
            ),
    );
    let close: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, close_sign_in))
    };
    let go: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, sign_in_with_token))
    };
    let body = div().px(px(4.)).pt(px(4.)).child(screen);
    Some(dialog::floating(px(440.), body, go, close, cx).into_any_element())
}
