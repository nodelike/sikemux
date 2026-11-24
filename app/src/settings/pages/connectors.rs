//! The connectors built in: the services agents reach for the person, each with whether it is
//! signed in and as whom, its switch and its sign-in, and the few settings its tools read that
//! only the person changes.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_calls::call;
use sikemux_calls::plugins::Disabled;
use sikemux_calls::plugins::PluginCall;
use sikemux_calls::plugins::StreamCall;
use sikemux_view::plugins::Listed;

use crate::connector_sign_in::{ask, mark, on, AWS};
use crate::core::Target;
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
    section(Connectors, "Account vault", "vault recovery key sealed sign-ins hosts"),
    section(Connectors, "Connectors", "aws bitbucket database postgres postgresql mysql sqlite sql github gitlab forgejo gitea codeberg jira linear rundeck signoz slack vercel sign in sign out login logout token account enable disable switch off turn on integrations plugins connectors"),
];

const RUNDECK: &str = "sikemux.rundeck";
const DATABASE: &str = "sikemux.database";
const GITHUB: &str = "sikemux.github";

/// What the page keeps while it shows: the settings open over it, and what the connectors with
/// settings of their own last answered.
#[derive(Default)]
pub struct Fields {
    /// The plugins moved since the settings below were read, so they are read again.
    pub(super) stale: bool,
    /// The connector whose settings are open over the page.
    configuring: Option<String>,
    aws: Option<Aws>,
    rundeck: Option<Lists>,
    databases: Vec<Database>,
    github: Option<GithubApps>,
    /// The organization to make an App for, empty for the person's own account.
    organization: Option<Entity<InputState>>,
    /// GitHub's page for making an App is open, which dropping stops.
    making_app: Option<Task<()>>,
    vault: super::vault::Fields,
}

/// The GitHub Apps agents act as, as the GitHub connector last said.
#[derive(Deserialize)]
struct GithubApps {
    apps: Vec<GithubApp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubApp {
    id: String,
    app: String,
    owner: String,
    organization: bool,
    repositories: usize,
    install_url: String,
    problem: Option<String>,
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

pub(super) fn change_vault(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut super::vault::Fields, &mut Context<crate::settings::Settings>),
) {
    change_fields(bench, cx, |fields, cx| change(&mut fields.vault, cx));
}

/// Reads what the connectors with settings of their own say now.
pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.stale = false);
    super::vault::read(bench, window, cx);
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
                    .and_then(|status| status.get("profile").and_then(text)),
                agents: picks
                    .await
                    .ok()
                    .and_then(|picks| picks.get("default").and_then(text)),
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
    if on(GITHUB) {
        if change_fields(bench, cx, |fields, _| fields.organization.is_none()) == Some(true) {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Organization")));
            change_fields(bench, cx, |fields, _| fields.organization = Some(input));
        }
        read_github(bench, cx);
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
                        bench.notify(toast::error(t!("Rundeck: {error}", error = error)), cx)
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
    Target::here(&bench.asks).call_typed::<call::PluginSetDisabled>(Disabled { ids: off });
}

fn has_sign_in(plugin: &Listed) -> bool {
    plugin.id == AWS || plugin.token_form.is_some() || plugin.browser_sign_in
}

/// Who the connector is signed in as.
fn account(plugin: &Listed, fields: &Fields) -> Option<SharedString> {
    if !plugin.enabled {
        return None;
    }
    if plugin.id == DATABASE {
        return match fields.databases.len() {
            0 => None,
            1 => Some(t!("1 database").into()),
            saved => Some(t!("{count} databases", count = saved).into()),
        };
    }
    plugin
        .signed_in_as
        .clone()
        .filter(|_| plugin.signed_in == Some(true))
        .map(SharedString::from)
}

/// What the person should know about the connector, under its name: only what needs them.
fn trouble(plugin: &Listed, fields: &Fields) -> Option<SharedString> {
    if !plugin.enabled {
        return None;
    }
    if plugin.id == DATABASE && fields.databases.is_empty() {
        return Some(t!("No databases saved yet. Agents propose them for you to approve.").into());
    }
    (plugin.signed_in == Some(false))
        .then(|| plugin.problem.clone().map(SharedString::from))
        .flatten()
}

/// The connector's name, its account beside it, and anything wrong under it.
fn summary(plugin: &Listed, fields: &Fields, cx: &App) -> (AnyElement, Option<AnyElement>) {
    let t = Tokens::get(cx);
    let name = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .min_w_0()
        .when(!plugin.enabled, |name| name.text_color(t.text_tertiary))
        .child(div().flex_none().child(plugin.name.clone()))
        .children(account(plugin, fields).map(|account| {
            div()
                .min_w_0()
                .truncate()
                .px(SPACE_2)
                .py(px(1.))
                .rounded_full()
                .bg(t.surface_hover)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(account)
        }))
        .into_any_element();
    let trouble = trouble(plugin, fields).map(|line| {
        let wrong = plugin.problem.is_some() && plugin.id != DATABASE;
        div()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(if wrong { t.danger } else { t.text_tertiary })
            .child(line)
            .into_any_element()
    });
    (name, trouble)
}

/// The connector's mark on a small tile.
fn tile(plugin: &Listed, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .bg(t.surface_hover)
        .border(crate::theme::border())
        .border_color(t.line_soft)
        .when(!plugin.enabled, |tile| tile.opacity(0.5))
        .child(ui::picture(mark(&plugin.id), 16., t.text_secondary))
        .into_any_element()
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
    let body = match plugins.is_empty() {
        true => rows::note(t!("No connectors in this build."), cx),
        false => div()
            .children(plugins.all().flat_map(|plugin| {
                let (name, trouble) = summary(plugin, fields, cx);
                let row = rows::row_parts(
                    Some(tile(plugin, cx)),
                    name,
                    trouble,
                    controls(plugin, fields, this),
                    cx,
                )
                .into_any_element();
                std::iter::once(row).chain(super::vault::under(&plugin.id, &fields.vault, this, cx))
            }))
            .into_any_element(),
    };
    let vault = super::vault::section(&fields.vault, lit("Account vault"), this, cx);
    let connectors = rows::section(n!("Connectors"), None, None, lit("Connectors"), body, cx);
    vault.into_iter().chain([connectors]).collect()
}

/// The connector has settings of its own to open, read since the page opened.
fn has_settings(plugin: &Listed, fields: &Fields) -> bool {
    plugin.enabled
        && match plugin.id.as_str() {
            AWS => fields.aws.is_some(),
            RUNDECK => fields.rundeck.is_some(),
            DATABASE => !fields.databases.is_empty(),
            GITHUB => fields.github.is_some(),
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
            t!("Settings"),
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
            true => (t!("Sign out"), true),
            false if signed_in => (t!("Sign in again"), false),
            false => (t!("Sign in"), false),
        };
        let (glyph, variant) = match (signing_out, plugin.problem.is_some() && !signed_in) {
            (true, _) => (Icon::LogOut, ui::Variant::Ghost),
            (false, true) => (Icon::LogIn, ui::Variant::Primary),
            (false, false) => (Icon::LogIn, ui::Variant::Outline),
        };
        let this = this.clone();
        let id = id.clone();
        ui::Button::new(
            SharedString::from(format!("connector-sign-{id}")),
            label,
            variant,
        )
        .icon(glyph)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| match signing_out {
                true => sign_out(bench, &id, cx),
                false => crate::connector_sign_in::open(bench, &id, window, cx),
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
        .children(super::vault::keep_button(&plugin.id, &fields.vault, this))
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
                    t!("Production words"),
                    Some(t!("A job whose project or top folder holds one of these is production, and agents ask you before running it.").into()),
                    wide(&lists.production),
                    cx,
                ),
                setting(
                    t!("Branch options"),
                    Some(t!("The job options that name the branch a run deploys.").into()),
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
        GITHUB => github_settings(fields, this, cx),
        _ => Vec::new(),
    }
}

/// The agents' own GitHub Apps: a way to make one for the person or an organization, then a row
/// for each, with where it is installed and the ways to install it on more repositories or
/// forget it.
fn github_settings(fields: &Fields, this: &Entity<Workbench>, cx: &App) -> Vec<AnyElement> {
    let Some(github) = &fields.github else {
        return Vec::new();
    };
    let about = match github.apps.is_empty() {
        true => t!("Agents push to GitHub as you today. Create a GitHub App to give them their own identity, which cannot change your repositories' settings or secrets."),
        false => t!("Agents act as the App installed on the repository they work in. Create another for an organization's repositories."),
    };
    let about = t!(
        "{about} Name an organization to make its App. Only its owners and App managers can, so ask one of them if you are neither.",
        about = about
    );
    let control = match (&fields.making_app, &fields.organization) {
        (Some(_), _) => sign_in::waiting(
            "github-app-making",
            t!("Finish making the App on GitHub"),
            on(this, stop_making_app),
            cx,
        )
        .into_any_element(),
        (None, organization) => {
            let named = organization.clone();
            let this = this.clone();
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .children(
                    organization
                        .as_ref()
                        .map(|input| div().w(px(200.)).child(ui::Field::new(input))),
                )
                .child(
                    ui::Button::new(
                        "github-app-make",
                        t!("Create GitHub App"),
                        ui::Variant::Primary,
                    )
                    .icon(mark(GITHUB))
                    .on_click(move |_, _, cx| {
                        let organization = (named.as_ref())
                            .map(|input| input.read(cx).value().trim().to_owned())
                            .filter(|name| !name.is_empty());
                        this.update(cx, |bench, cx| make_app(bench, organization, cx));
                    }),
                )
                .into_any_element()
        }
    };
    let mut rows = vec![setting(
        t!("Agent identity"),
        Some(about.into()),
        control,
        cx,
    )];
    rows.extend(github.apps.iter().map(|app| github_app(app, this, cx)));
    rows
}

fn github_app(app: &GithubApp, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let owner = match app.organization {
        true => t!("Owned by the {owner} organization.", owner = app.owner),
        false => t!("Owned by {owner}.", owner = app.owner),
    };
    let installed = match app.repositories {
        0 => t!("Not installed on any repository yet.").to_owned(),
        1 => t!("Installed on 1 repository.").to_owned(),
        count => t!("Installed on {count} repositories.", count = count),
    };
    let about = match &app.problem {
        Some(problem) => t!(
            "{owner} {installed} GitHub said: {problem}",
            owner = owner,
            installed = installed,
            problem = problem
        ),
        None => t!("{owner} {installed}", owner = owner, installed = installed),
    };
    let install = app.install_url.clone();
    let id = app.id.clone();
    let forget = {
        let this = this.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let id = id.clone();
            this.update(cx, |bench, cx| forget_app(bench, id, cx));
        }
    };
    setting(
        app.app.clone(),
        Some(about.into()),
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(
                ui::Button::new(
                    SharedString::from(format!("github-app-install:{}", app.id)),
                    t!("Install on more repositories"),
                    ui::Variant::Outline,
                )
                .icon(Icon::ExternalLink)
                .on_click(move |_, _, cx| cx.open_url(&install)),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("github-app-forget:{}", app.id)),
                    t!("Forget"),
                    ui::Variant::Ghost,
                )
                .tooltip(t!("Sikemux forgets this App on every computer that has it. It stays on GitHub until you delete it there."))
                .on_click(forget),
            ),
        cx,
    )
}

fn read_github(bench: &Workbench, cx: &mut Context<Workbench>) {
    let status = ask::<GithubApps>(bench, GITHUB, "appStatus", Value::Null);
    cx.spawn(async move |bench, cx| {
        let Ok(status) = status.await else {
            return;
        };
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.github = Some(status));
        });
    })
    .detach();
}

/// Opens GitHub's page for making an App for the person or `organization`, then its page for
/// picking repositories.
fn make_app(bench: &mut Workbench, organization: Option<String>, cx: &mut Context<Workbench>) {
    let call = PluginCall {
        plugin: GITHUB.to_owned(),
        method: "createApp".to_owned(),
        params: json!({ "organization": organization }),
    };
    let mut streaming = Target::here(&bench.asks)
        .stream::<call::PluginStream>(|stream| StreamCall { stream, call });
    let waiting = cx.spawn(async move |bench, cx| {
        let mut failed = None;
        while let Some(part) = streaming.parts.recv().await {
            for item in part.items {
                if let Some(url) = item.get("url").and_then(Value::as_str) {
                    cx.update(|cx| cx.open_url(url));
                }
            }
            if let Some(error) = part.error {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                failed = Some(message.unwrap_or_else(|| error.to_string()));
            }
            if part.done {
                break;
            }
        }
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.making_app = None);
            if let Some(failed) = failed {
                bench.notify(toast::error(t!("GitHub: {error}", error = failed)), cx);
            }
            read_github(bench, cx);
        });
    });
    change_fields(bench, cx, |fields, _| fields.making_app = Some(waiting));
}

fn stop_making_app(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.making_app = None);
}

fn forget_app(bench: &mut Workbench, id: String, cx: &mut Context<Workbench>) {
    let forgot = ask::<Value>(bench, GITHUB, "forgetApp", json!({ "app": id }));
    cx.spawn(async move |bench, cx| {
        let forgot = forgot.await;
        let _ = bench.update(cx, |bench, cx| {
            if let Err(error) = forgot {
                bench.notify(toast::error(t!("GitHub: {error}", error = error)), cx);
            }
            read_github(bench, cx);
        });
    })
    .detach();
}

fn aws_settings(fields: &Fields, this: &Entity<Workbench>, cx: &App) -> Vec<AnyElement> {
    let Some(aws) = &fields.aws else {
        return Vec::new();
    };
    if aws.profiles.is_empty() {
        return vec![setting(
            t!("No profiles yet"),
            Some(t!("Add one to ~/.aws/config, then open this page again.").into()),
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
                            bench.notify(toast::error(t!("AWS: {error}", error = error)), cx);
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
            t!("Signed in with"),
            Some(t!("The profile whose sign-in this page checks.").into()),
            ui::select(
                "aws-account",
                match account.is_empty() {
                    true => t!("Pick a profile").to_owned(),
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
            t!("Profile agents start on"),
            Some(t!("Set as AWS_PROFILE for every agent, unless your own shell sets one.").into()),
            ui::select(
                "aws-agents",
                match agents.is_empty() {
                    true => t!("Automatic").to_owned(),
                    false => agents.clone(),
                },
                picks(Some(t!("Automatic"))),
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
            t!("Remove"),
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
                    t!("Agents may change data"),
                    database.writable,
                    "setAgentWrites",
                    this,
                    cx,
                ))
                .child(database_switch(
                    &name,
                    t!("Production"),
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
                bench.notify(toast::error(t!("Database: {error}", error = error)), cx);
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

fn close_settings(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.configuring = None);
}

pub fn dialog(
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
                    item.border_b(crate::theme::border())
                        .border_color(t.line_soft)
                })
                .child(item)
        }))
        .into_any_element();
    let close: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, close_settings))
    };
    let foot = dialog::hinted_foot(dialog::escape(t!("close"), cx), cx).child(dialog::button(
        "connector-settings-done",
        t!("Done").into(),
        Some(t.accent),
        close.clone(),
        cx,
    ));
    Some(
        dialog::marked_sheet(
            mark(&plugin.id),
            t!("{name} settings", name = plugin.name).into(),
            body,
            foot,
            close.clone(),
            close,
            cx,
        )
        .into_any_element(),
    )
}
