//! The connectors built in: the services agents reach for the person, each with whether it is
//! signed in and as whom, its switch and its sign-in, and the few settings its tools read that
//! only the person changes.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_view::plugins::Listed;

use crate::core::{outcome, Ask, Target};
use crate::icons::Icon;
use crate::settings::index::Page::Connectors;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_0, SPACE_2, SPACE_3, SPACE_4, SPACE_5, TEXT_SM, TEXT_XS};
use crate::toast;
use crate::ui::{self, dialog, sign_in};
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(Connectors, "Connectors", "aws bitbucket database postgres postgresql mysql sqlite sql github gitlab jira linear rundeck signoz slack vercel sign in sign out login logout token account enable disable switch off turn on integrations plugins connectors"),
];

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
    /// The connector whose settings are open over the page.
    configuring: Option<String>,
    aws: Option<Aws>,
    rundeck: Option<Lists>,
    databases: Vec<Database>,
}

/// A sign-in on its way: the token form's boxes, and the browser's, while it runs.
struct Signing {
    plugin: String,
    /// The boxes of every way in, each under its field's name.
    fields: Vec<(String, Entity<InputState>)>,
    _typing: Vec<Subscription>,
    /// The form's second way in is showing.
    other_way: bool,
    /// The names of the form's ticked boxes.
    ticked: Vec<String>,
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
                rows::row_with(
                    Some(ui::picture(mark(&plugin.id), 16., ink).into_any_element()),
                    plugin.name.clone(),
                    Some(standing(plugin, fields)),
                    controls(plugin, fields, this),
                    cx,
                )
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

/// The connector has settings of its own to open, read since the page opened.
fn has_settings(plugin: &Listed, fields: &Fields) -> bool {
    plugin.enabled
        && match plugin.id.as_str() {
            AWS => fields.aws.is_some(),
            RUNDECK => fields.rundeck.is_some(),
            DATABASE => !fields.databases.is_empty(),
            _ => false,
        }
}

/// The connector's settings button, Sign in or Sign out, and its switch.
fn controls(plugin: &Listed, fields: &Fields, this: &Entity<Workbench>) -> AnyElement {
    let id = plugin.id.clone();
    let configure = has_settings(plugin, fields).then(|| {
        let this = this.clone();
        let id = id.clone();
        ui::IconButton::new(
            SharedString::from(format!("connector-settings-{id}")),
            Icon::Settings,
            "Settings",
        )
        .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let id = id.clone();
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.configuring = Some(id));
            })
        })
    });
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
        .children(configure)
        .children(button)
        .child(toggle)
        .into_any_element()
}

/// One setting in a connector's settings: its name and what it does, then its control.
fn setting(
    label: impl Into<SharedString>,
    about: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(heading(label.into(), about, t))
        .child(control)
        .into_any_element()
}

fn heading(label: SharedString, about: Option<SharedString>, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .child(
            div()
                .text_size(theme::ui_text(TEXT_SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(label),
        )
        .children(about.map(|about| {
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(about)
        }))
}

/// The settings a connector's tools read.
fn settings(
    plugin: &Listed,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    match plugin.id.as_str() {
        AWS => aws_settings(fields, this, cx),
        RUNDECK => fields.rundeck.as_ref().map_or_else(Vec::new, |lists| {
            let wide = |input: &Entity<InputState>| div().w_full().child(ui::Field::new(input));
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
    let t = Tokens::get(cx);
    let about = format!("{} · {}", database.engine, database.address);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_start()
                .gap(SPACE_2)
                .child(
                    heading(database.name.clone().into(), Some(about.into()), t)
                        .flex_1()
                        .min_w_0(),
                )
                .child(remove),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_4)
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
                )),
        )
        .into_any_element()
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
    let mut fields: Vec<(String, Entity<InputState>)> = Vec::new();
    let other_fields = form.iter().flat_map(|form| &form.other_way);
    for field in
        (form.iter().flat_map(|form| &form.fields)).chain(other_fields.flat_map(|way| &way.fields))
    {
        if typed_in(&fields, &field.name).is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(field.placeholder.clone())
                    .masked(field.secret)
            });
            typing.push(super::follow_typing(bench, &input, cx));
            fields.push((field.name.clone(), input));
        }
    }
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
        other_way: false,
        ticked: Vec::new(),
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

/// The box a field of the form is typed in.
fn typed_in<'a>(
    fields: &'a [(String, Entity<InputState>)],
    name: &str,
) -> Option<&'a Entity<InputState>> {
    let (_, input) = fields.iter().find(|(field, _)| field == name)?;
    Some(input)
}

fn switch_way(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| {
        if let Some(signing) = &mut fields.signing {
            signing.other_way = !signing.other_way;
            signing.refused = None;
        }
    });
}

fn tick(bench: &mut Workbench, name: String, on: bool, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| {
        if let Some(signing) = &mut fields.signing {
            signing.ticked.retain(|ticked| *ticked != name);
            if on {
                signing.ticked.push(name);
            }
        }
    });
}

/// Sends what the way in showing holds to the method it names.
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
    let (method, way) = form.way(signing.other_way);
    let mut typed: Map<String, Value> = way
        .iter()
        .filter_map(|field| {
            let typed = typed_in(&signing.fields, &field.name)?.read(cx).value();
            let value = match field.exact {
                true => typed.to_string(),
                false => typed.trim().to_owned(),
            };
            let value = (!value.is_empty()).then_some(Value::String(value));
            Some((field.name.clone(), value.unwrap_or(Value::Null)))
        })
        .collect();
    for check in &form.checks {
        let on = signing.ticked.contains(&check.name);
        typed.insert(check.name.clone(), Value::Bool(on));
    }
    let signed = ask::<Value>(bench, &signing.plugin, method, typed.into());
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
    fields: Vec<(String, Entity<InputState>)>,
    other_way: bool,
    ticked: Vec<String>,
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
        let Some(signing) = fields.signing.as_ref() else {
            return settings_dialog(bench, this, cx);
        };
        let profiles = fields.aws.as_ref().map(|aws| aws.profiles.clone());
        (
            Shown {
                plugin: signing.plugin.clone(),
                fields: signing.fields.clone(),
                other_way: signing.other_way,
                ticked: signing.ticked.clone(),
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
        let (_, way) = form.way(signing.other_way);
        let boxes: Vec<_> = way
            .iter()
            .filter_map(|field| Some((field, typed_in(&signing.fields, &field.name)?)))
            .collect();
        for (field, input) in &boxes {
            let typed = sign_in::Field::new(field.label.clone(), input);
            screen = screen.child(if field.secret { typed.mono() } else { typed });
        }
        for check in &form.checks {
            let name = check.name.clone();
            let this = this.clone();
            screen = screen.child(
                div().w_full().flex().child(
                    Checkbox::new(SharedString::from(format!(
                        "connector-check-{}",
                        check.name
                    )))
                    .label(check.label.clone())
                    .checked(signing.ticked.contains(&check.name))
                    .on_click(move |on: &bool, _, cx| {
                        let (name, on) = (name.clone(), *on);
                        this.update(cx, |bench, cx| tick(bench, name, on, cx))
                    }),
                ),
            );
        }
        let filled = boxes
            .iter()
            .all(|(_, input)| !input.read(cx).value().trim().is_empty());
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
    let server = typed_in(&signing.fields, "host")
        .map(|input| input.read(cx).value().to_string())
        .unwrap_or_default();
    let page = form.and_then(|form| form.token_page(&server));
    let switch = form
        .and_then(|form| form.other_way.as_ref())
        .map(|way| match signing.other_way {
            true => way.back.clone(),
            false => way.offer.clone(),
        });
    screen = screen.child(
        sign_in::links()
            .when_some(switch, |links, words| {
                links.child(
                    sign_in::link("connector-other-way", words, cx).on_click(on(this, switch_way)),
                )
            })
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

fn close_settings(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.configuring = None);
}

/// The open connector's settings, over the page.
fn settings_dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let (plugin, items) = {
        let fields = super::state(bench, cx)?.connectors.as_ref()?;
        let id = fields.configuring.as_deref()?;
        let plugin = bench.reads.plugins.all().find(|plugin| plugin.id == id)?;
        if !has_settings(plugin, fields) {
            return None;
        }
        (plugin, settings(plugin, fields, this, cx))
    };
    let t = Tokens::get(cx).clone();
    let count = items.len();
    let body = div()
        .id("connector-settings")
        .max_h(px(480.))
        .overflow_y_scroll()
        .pl(px(dialog::GLYPH_INSET))
        .pr(SPACE_5)
        .pb(SPACE_2)
        .flex()
        .flex_col()
        .children(items.into_iter().enumerate().map(|(at, item)| {
            div()
                .py(SPACE_3)
                .when(at + 1 < count, |item| {
                    item.border_b_1().border_color(t.line_soft)
                })
                .child(item)
        }))
        .into_any_element();
    let close: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, close_settings))
    };
    let foot = dialog::hinted_foot(dialog::escape("close", cx), cx).child(dialog::button(
        "connector-settings-done",
        "Done".into(),
        Some(t.accent),
        close.clone(),
        cx,
    ));
    Some(
        dialog::marked_sheet(
            mark(&plugin.id),
            format!("{} settings", plugin.name).into(),
            body,
            foot,
            close.clone(),
            close,
            cx,
        )
        .into_any_element(),
    )
}
