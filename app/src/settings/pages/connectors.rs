//! External service connectors: search and category filters on top, Connected rows with
//! inline Configure and ⋯ menu, Available rows with Connect, and a quiet Connector sync footer.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_calls::call;
use sikemux_calls::plugins::{Disabled, PluginCall, StreamCall};
use sikemux_view::plugins::Listed;

use crate::connector_sign_in::{ask, mark, on, stand, Stand, AWS};
use crate::core::Target;
use crate::icons::Icon;
use crate::settings::index::Page::Connectors;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS,
    TEXT_3XS, TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, sign_in, Clicks, MenuRow, Tone};
use crate::workbench::Workbench;

pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Connectors,
        "Connector sync",
        "account vault sync recovery key sealed sign-ins hosts",
    ),
    section(
        Connectors,
        "Connectors",
        "aws bitbucket cloudflare dns zones workers pages database postgres postgresql mysql sqlite sql github gitlab forgejo gitea codeberg google play jira linear rundeck signoz slack vercel sign in sign out connect configure integrations plugins connectors",
    ),
];

const RUNDECK: &str = "sikemux.rundeck";
const DATABASE: &str = "sikemux.database";
const GITHUB: &str = "sikemux.github";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Category {
    #[default]
    All,
    Code,
    Cloud,
    Issues,
    Observability,
    Data,
}

impl Category {
    const ALL: [Category; 6] = [
        Category::All,
        Category::Code,
        Category::Cloud,
        Category::Issues,
        Category::Observability,
        Category::Data,
    ];

    fn label(self) -> &'static str {
        match self {
            Category::All => t!("All"),
            Category::Code => t!("Code"),
            Category::Cloud => t!("Cloud"),
            Category::Issues => t!("Issues & Chat"),
            Category::Observability => t!("Observability"),
            Category::Data => t!("Data"),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Category::All => "all",
            Category::Code => "code",
            Category::Cloud => "cloud",
            Category::Issues => "issues",
            Category::Observability => "obs",
            Category::Data => "data",
        }
    }
}

struct Meta {
    category: Category,
    summary: &'static str,
}

fn meta(id: &str) -> Option<Meta> {
    Some(match id {
        "sikemux.github" => Meta {
            category: Category::Code,
            summary: t!("Repos, pull requests, reviews and Actions runs."),
        },
        "sikemux.gitlab" => Meta {
            category: Category::Code,
            summary: t!("Merge requests, pipelines and issues."),
        },
        "sikemux.bitbucket" => Meta {
            category: Category::Code,
            summary: t!("Pull requests and Pipelines on Bitbucket Cloud."),
        },
        "sikemux.forgejo" => Meta {
            category: Category::Code,
            summary: t!("Self-hosted Forgejo, Gitea and Codeberg."),
        },
        "sikemux.aws" => Meta {
            category: Category::Cloud,
            summary: t!("Uses your AWS CLI profiles: logs, ECS, S3, Lambda."),
        },
        "sikemux.cloudflare" => Meta {
            category: Category::Cloud,
            summary: t!("DNS records, zones, Workers and Pages."),
        },
        "sikemux.vercel" => Meta {
            category: Category::Cloud,
            summary: t!("Deployments, build logs and project env."),
        },
        "sikemux.googleplay" => Meta {
            category: Category::Cloud,
            summary: t!("Release tracks, testers and review status."),
        },
        "sikemux.rundeck" => Meta {
            category: Category::Cloud,
            summary: t!("Run jobs and read their output."),
        },
        "sikemux.jira" => Meta {
            category: Category::Issues,
            summary: t!("Read and update issues, sprints and boards."),
        },
        "sikemux.linear" => Meta {
            category: Category::Issues,
            summary: t!("Issues, cycles and project updates."),
        },
        "sikemux.slack" => Meta {
            category: Category::Issues,
            summary: t!("Read channels and threads; post when you approve."),
        },
        "sikemux.signoz" => Meta {
            category: Category::Observability,
            summary: t!("Traces, logs and alerts from your SigNoz."),
        },
        "sikemux.database" => Meta {
            category: Category::Data,
            summary: t!("Postgres, MySQL and SQLite, read-only by default."),
        },
        _ => return None,
    })
}

pub struct Fields {
    pub(super) stale: bool,
    search: Entity<InputState>,
    _typing: Subscription,
    category: Category,
    /// The connector currently expanded inline for configuration.
    configuring: Option<String>,
    aws: Option<Aws>,
    rundeck: Option<Lists>,
    databases: Vec<Database>,
    github: Option<GithubApps>,
    organization: Option<Entity<InputState>>,
    making_app: Option<Task<()>>,
    pub(super) vault: super::account_vault::Fields,
}

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
    account: Option<String>,
    agents: Option<String>,
}

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
    let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search connectors")));
    let typing = super::follow_typing(bench, &search, cx);
    super::change_pages(bench, cx, |pages, _| {
        pages.connectors = Some(Fields {
            stale: false,
            search,
            _typing: typing,
            category: Category::All,
            configuring: None,
            aws: None,
            rundeck: None,
            databases: Vec::new(),
            github: None,
            organization: None,
            making_app: None,
            vault: super::account_vault::Fields::default(),
        });
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
    change: impl FnOnce(&mut super::account_vault::Fields, &mut Context<crate::settings::Settings>),
) {
    change_fields(bench, cx, |fields, cx| change(&mut fields.vault, cx));
}

pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.stale = false);
    super::account_vault::read(bench, window, cx);
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
                let note = match saved.await {
                    Ok(_) => toast::success(t!("Saved Rundeck settings")),
                    Err(error) => toast::error(t!("Rundeck: {error}", error = error)),
                };
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
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

fn switch(bench: &Workbench, id: &str, name: &str, on: bool, cx: &mut Context<Workbench>) {
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
    let note = if on {
        toast::success(t!("Agents can use {name}", name = name))
    } else {
        toast::info(t!("{name} hidden from agents", name = name))
    };
    bench.notify(note, cx);
}

fn has_sign_in(plugin: &Listed) -> bool {
    plugin.id == AWS || plugin.token_form.is_some() || plugin.browser_sign_in
}

fn is_connected(plugin: &Listed, fields: &Fields) -> bool {
    plugin.signed_in == Some(true)
        || (plugin.id == DATABASE && !fields.databases.is_empty())
        || !plugin.enabled
        || super::account_vault::has_sign_in(&plugin.id, &fields.vault)
}

fn looks_like_email(text: &str) -> bool {
    let text = text.trim();
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.contains(char::is_whitespace)
        && domain.contains('.')
        && !domain.contains(char::is_whitespace)
}

/// Formats a plugin's signed-in scope for display while stripping any email PII.
fn scope_label(raw: &str) -> Option<String> {
    let cleaned: Vec<String> = raw
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            if let Some((_, workspace)) = part.split_once(" in ") {
                let workspace = workspace.trim();
                if !workspace.is_empty() && !looks_like_email(workspace) {
                    return Some(workspace.to_owned());
                }
            }
            if let Some((_, host)) = part.split_once(" on ") {
                let host = host.trim();
                if !host.is_empty() && !looks_like_email(host) {
                    return Some(host.to_owned());
                }
            }
            let trimmed = match part.split_once(" (") {
                Some((head, tail)) => match tail.strip_suffix(')') {
                    Some(inner) if looks_like_email(inner) => head.trim(),
                    _ => part,
                },
                None => part,
            };
            (!trimmed.is_empty() && !looks_like_email(trimmed)).then(|| trimmed.to_owned())
        })
        .collect();
    (!cleaned.is_empty()).then(|| cleaned.join(", "))
}

fn scope(plugin: &Listed, fields: &Fields) -> Option<SharedString> {
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
        .as_deref()
        .filter(|_| plugin.signed_in == Some(true))
        .and_then(scope_label)
        .map(SharedString::from)
}

fn matches_category(plugin: &Listed, category: Category) -> bool {
    category == Category::All || meta(&plugin.id).is_some_and(|m| m.category == category)
}

fn matches_query(plugin: &Listed, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let meta = meta(&plugin.id);
    let summary = meta.as_ref().map_or("", |m| m.summary);
    let category = meta.as_ref().map_or("", |m| m.category.label());
    let haystack = format!("{} {} {} {}", plugin.name, plugin.id, summary, category).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

fn tile(plugin: &Listed, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .bg(t.surface_raised)
        .border(crate::theme::border())
        .border_color(t.border)
        .when(!plugin.enabled, |tile| tile.opacity(0.55))
        .child(ui::picture(mark(&plugin.id), 16., t.text_primary))
        .into_any_element()
}

fn chip(text: SharedString, mono: bool, warn: bool, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .max_w(px(200.))
        .truncate()
        .px(SPACE_1H)
        .py(px(1.))
        .rounded(theme::radius_1())
        .border(crate::theme::border())
        .border_color(if warn { t.warn.opacity(0.35) } else { t.border })
        .bg(if warn {
            t.warn.opacity(0.08)
        } else {
            t.surface_raised
        })
        .when(mono, |chip| chip.font_family(theme::mono_font()))
        .text_size(theme::ui_text(TEXT_2XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if warn { t.warn } else { t.text_secondary })
        .child(text)
}

fn local_pill(plugin_id: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .id(SharedString::from(format!(
            "connector-local-pill-{plugin_id}"
        )))
        .flex_none()
        .px(px(5.))
        .py(px(1.))
        .rounded(theme::radius_1())
        .border(crate::theme::border())
        .border_color(t.border)
        .text_size(theme::ui_text(TEXT_3XS))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_tertiary)
        .tooltip(ui::tooltip(t!("Kept on this host only")))
        .child(t!("THIS HOST"))
        .into_any_element()
}

fn toolbar(plugins: &[&Listed], fields: &Fields, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let pills = Category::ALL.into_iter().map(|cat| {
        let count = plugins
            .iter()
            .filter(|plugin| matches_category(plugin, cat))
            .count();
        let active = fields.category == cat;
        let picking = this.clone();
        div()
            .id(SharedString::from(format!("connector-cat-{}", cat.id())))
            .flex()
            .items_center()
            .gap(SPACE_1)
            .h(px(26.))
            .px(SPACE_2H)
            .rounded(theme::radius_2())
            .cursor_pointer()
            .text_size(theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if active {
                t.text_primary
            } else {
                t.text_secondary
            })
            .when(active, |pill| pill.bg(t.surface_chosen))
            .when(!active, |pill| {
                pill.hover(|style| style.bg(t.surface_hover).text_color(t.text_primary))
            })
            .clicks(move |_, _, cx| {
                picking.update(cx, |bench, cx| {
                    change_fields(bench, cx, |fields, _| fields.category = cat);
                })
            })
            .child(cat.label())
            .child(
                div()
                    .text_size(theme::ui_text(TEXT_2XS))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.text_tertiary)
                    .child(count.to_string()),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .child(ui::Field::new(&fields.search).icon(Icon::Search))
        .child(div().flex().flex_wrap().gap(SPACE_1).children(pills))
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
    let all: Vec<&Listed> = plugins.all().collect();
    if all.is_empty() {
        return vec![rows::section(
            n!("Connectors"),
            None,
            None,
            lit("Connectors"),
            rows::note(t!("No connectors in this build."), cx),
            cx,
        )];
    }

    let query = fields.search.read(cx).value().trim().to_lowercase();
    let visible: Vec<&Listed> = all
        .iter()
        .copied()
        .filter(|plugin| matches_category(plugin, fields.category) && matches_query(plugin, &query))
        .collect();
    let (connected, available): (Vec<&Listed>, Vec<&Listed>) = visible
        .into_iter()
        .partition(|plugin| is_connected(plugin, fields));

    let mut sections: Vec<AnyElement> = Vec::new();

    if let Some(banner) =
        super::account_vault::top_banner(&fields.vault, lit("Connector sync"), this, cx)
    {
        sections.push(banner);
    }

    sections.push(toolbar(&all, fields, this, cx));

    if !connected.is_empty() {
        let count = SharedString::from(connected.len().to_string());
        let body = div()
            .flex()
            .flex_col()
            .children(
                connected
                    .into_iter()
                    .map(|plugin| connected_row(bench, plugin, fields, this, cx)),
            )
            .into_any_element();
        sections.push(rows::section(
            n!("Connected"),
            Some(count),
            None,
            lit("Connectors"),
            body,
            cx,
        ));
    }

    if !available.is_empty() {
        let count = SharedString::from(available.len().to_string());
        let body = div()
            .flex()
            .flex_col()
            .children(
                available
                    .into_iter()
                    .map(|plugin| available_row(bench, plugin, this, cx)),
            )
            .into_any_element();
        sections.push(rows::section(
            n!("Available"),
            Some(count),
            None,
            false,
            body,
            cx,
        ));
    }

    if sections.len() == 1
        || (all
            .iter()
            .all(|p| !matches_category(p, fields.category) || !matches_query(p, &query)))
    {
        let msg: SharedString = if query.is_empty() {
            t!("No connectors in this category.").into()
        } else {
            t!(
                "No connector matches “{query}”.",
                query = fields.search.read(cx).value().trim()
            )
            .into()
        };
        sections.push(
            div()
                .py(px(28.))
                .text_center()
                .text_size(theme::ui_text(TEXT_SM))
                .text_color(Tokens::get(cx).text_tertiary)
                .child(msg)
                .into_any_element(),
        );
    }

    if let Some(footer) =
        super::account_vault::footer(&fields.vault, lit("Connector sync"), this, cx)
    {
        sections.push(footer);
    }

    sections
}

fn connected_row(
    _bench: &Workbench,
    plugin: &Listed,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let id = plugin.id.clone();
    let open = fields.configuring.as_deref() == Some(id.as_str());
    let locked = super::account_vault::is_locked(&id, &fields.vault);
    let kept_local = super::account_vault::is_kept_local(&id, &fields.vault);
    let dot_tone = if locked || (plugin.enabled && plugin.problem.is_some()) {
        Tone::Pending
    } else if !plugin.enabled {
        Tone::Idle
    } else {
        Tone::Ok
    };

    let lead = div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(ui::dot(dot_tone, cx))
        .child(tile(plugin, cx))
        .into_any_element();

    let name = div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(if plugin.enabled {
            t.text_primary
        } else {
            t.text_tertiary
        })
        .child(plugin.name.clone())
        .into_any_element();

    let trouble = (plugin.enabled && plugin.signed_in == Some(false))
        .then(|| plugin.problem.clone())
        .flatten()
        .map(|problem| rows::about_line(problem.into(), t.danger));

    let scope_chip = if locked {
        Some(chip(t!("Locked on this host").into(), false, true, cx).into_any_element())
    } else if !plugin.enabled {
        Some(chip(t!("Off for agents").into(), false, false, cx).into_any_element())
    } else {
        scope(plugin, fields).map(|text| chip(text, plugin.id == AWS, false, cx).into_any_element())
    };

    let configure_btn = {
        let this = this.clone();
        let id = id.clone();
        ui::Button::new(
            SharedString::from(format!("connector-configure-{id}")),
            if open { t!("Done") } else { t!("Configure") },
            ui::Variant::Ghost,
        )
        .pressed(open)
        .on_click(move |_, window, cx| {
            let id = id.clone();
            this.update(cx, |bench, cx| toggle_configure(bench, &id, window, cx))
        })
    };

    let more_menu = {
        let this = this.clone();
        let plugin_id = id.clone();
        let plugin_name = plugin.name.clone();
        let can_sign_out = plugin.sign_out;
        let can_sign_in = has_sign_in(plugin);
        let enabled = plugin.enabled;
        let local_state = super::account_vault::local_toggle_state(&id, &fields.vault);
        let trigger = ui::IconButton::new(
            SharedString::from(format!("connector-more-{id}")),
            Icon::More,
            t!("More"),
        );
        ui::menu_button(
            SharedString::from(format!("connector-menu-{id}")),
            Anchor::BottomRight,
            trigger,
            move |mut popup, _, cx| {
                let (cfg_this, cfg_id) = (this.clone(), plugin_id.clone());
                popup = popup.item(
                    MenuRow::new(if open {
                        t!("Hide settings")
                    } else {
                        t!("Configure…")
                    })
                    .icon(Icon::Settings)
                    .item(cx)
                    .on_click(move |_, window, cx| {
                        let id = cfg_id.clone();
                        cfg_this.update(cx, |bench, cx| toggle_configure(bench, &id, window, cx));
                    }),
                );
                if enabled && can_sign_in {
                    let (check_this, check_id, check_name) =
                        (this.clone(), plugin_id.clone(), plugin_name.clone());
                    popup = popup.item(
                        MenuRow::new(t!("Re-check sign-in"))
                            .icon(Icon::Refresh)
                            .item(cx)
                            .on_click(move |_, window, cx| {
                                let id = check_id.clone();
                                let name = check_name.clone();
                                check_this
                                    .update(cx, |bench, cx| recheck(bench, &id, &name, window, cx));
                            }),
                    );
                }
                if let Some(is_local) = local_state {
                    let (local_this, local_id) = (this.clone(), plugin_id.clone());
                    popup = popup.item(
                        MenuRow::new(t!("Keep on this host only"))
                            .icon(Icon::Pin)
                            .chosen(is_local)
                            .item(cx)
                            .on_click(move |_, _, cx| {
                                let id = local_id.clone();
                                local_this.update(cx, |bench, cx| {
                                    super::account_vault::toggle_keep_local(
                                        bench, &id, !is_local, cx,
                                    )
                                });
                            }),
                    );
                }
                if can_sign_out {
                    let (out_this, out_id, out_name) =
                        (this.clone(), plugin_id.clone(), plugin_name.clone());
                    popup = popup.separator().item(
                        MenuRow::new(t!("Sign out"))
                            .icon(Icon::LogOut)
                            .danger(true)
                            .item(cx)
                            .on_click(move |_, _, cx| {
                                let id = out_id.clone();
                                let name = out_name.clone();
                                out_this.update(cx, |bench, cx| {
                                    if !enabled {
                                        switch(bench, &id, &name, true, cx);
                                    }
                                    sign_out(bench, &id, cx);
                                });
                            }),
                    );
                } else if can_sign_in {
                    let (in_this, in_id) = (this.clone(), plugin_id.clone());
                    popup = popup.separator().item(
                        MenuRow::new(t!("Sign in again"))
                            .icon(Icon::LogIn)
                            .item(cx)
                            .on_click(move |_, window, cx| {
                                let id = in_id.clone();
                                in_this.update(cx, |bench, cx| {
                                    crate::connector_sign_in::open(bench, &id, window, cx)
                                });
                            }),
                    );
                }
                popup
            },
        )
    };

    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .children(scope_chip)
        .when(kept_local && !locked, |row| row.child(local_pill(&id, cx)))
        .child(configure_btn)
        .child(more_menu);

    let row = rows::row_parts(Some(lead), name, trouble, controls, cx);
    let asking = super::account_vault::asking(&id, &fields.vault, this, cx);
    let inline = open.then(|| inline_config(plugin, fields, this, cx));

    div()
        .flex()
        .flex_col()
        .child(row)
        .children(asking)
        .children(inline)
        .into_any_element()
}

fn available_row(
    bench: &Workbench,
    plugin: &Listed,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let id = plugin.id.clone();
    let lead = div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(div().w(px(7.)).flex_none())
        .child(tile(plugin, cx))
        .into_any_element();

    let desc = match &plugin.problem {
        Some(problem) if plugin.signed_in == Some(false) => {
            Some(rows::about_line(problem.clone().into(), t.danger))
        }
        _ => meta(&id)
            .map(|m| m.summary)
            .map(|summary| rows::about_line(summary.into(), t.text_tertiary)),
    };

    let busy = matches!(stand(bench, &id), Some(Stand::Busy));
    let action: AnyElement = if has_sign_in(plugin) {
        let this = this.clone();
        let id_btn = id.clone();
        let variant = if plugin.problem.is_some() && plugin.signed_in == Some(false) {
            ui::Variant::Primary
        } else {
            ui::Variant::Outline
        };
        let label = if busy {
            t!("Signing in…")
        } else {
            t!("Connect")
        };
        ui::Button::new(
            SharedString::from(format!("connector-sign-{id}")),
            label,
            variant,
        )
        .when(busy, |button| {
            button.lead(ui::working(12., t.text_secondary))
        })
        .disabled(busy)
        .on_click(move |_, window, cx| {
            let id = id_btn.clone();
            this.update(cx, |bench, cx| {
                crate::connector_sign_in::open(bench, &id, window, cx)
            })
        })
        .into_any_element()
    } else {
        div()
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
            .child(t!("Proposed by agents"))
            .into_any_element()
    };

    let name = div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_primary)
        .child(plugin.name.clone());

    rows::row_parts(Some(lead), name, desc, action, cx).into_any_element()
}

fn toggle_configure(
    bench: &mut Workbench,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let opening = change_fields(bench, cx, |fields, _| {
        if fields.configuring.as_deref() == Some(id) {
            fields.configuring = None;
            false
        } else {
            fields.configuring = Some(id.to_owned());
            true
        }
    });
    if opening == Some(true) {
        read(bench, window, cx);
    }
}

fn recheck(
    bench: &mut Workbench,
    id: &str,
    name: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let checked = ask::<Value>(bench, id, "status", Value::Null);
    let name = name.to_owned();
    cx.spawn_in(window, async move |bench, cx| {
        let answer = checked.await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            let note = match answer {
                Ok(_) => toast::success(t!("{name} re-checked · all good", name = name)),
                Err(error) => toast::error(format!("{name}: {error}")),
            };
            bench.notify(note, cx);
            read(bench, window, cx);
        });
    })
    .detach();
}

/// Inline settings panel expanded beneath a Connected row.
fn inline_config(
    plugin: &Listed,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let mut items: Vec<AnyElement> = Vec::new();

    // 1. Agents can use <X> toggle (moved here off the main row).
    {
        let this = this.clone();
        let id = plugin.id.clone();
        let name = plugin.name.clone();
        let toggle = Switch::new(SharedString::from(format!("plugin-on-{id}")))
            .checked(plugin.enabled)
            .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                let on = *on;
                this.update(cx, |bench, cx| switch(bench, &id, &name, on, cx))
            });
        items.push(inline_row(
            t!("Agents can use {name}", name = plugin.name),
            Some(t!("Off keeps you signed in but hides its tools from agents.").into()),
            toggle,
            cx,
        ));
    }

    // 2. Connector-specific settings (GitHub, AWS, Database, Rundeck).
    if plugin.enabled {
        items.extend(connector_settings(plugin, fields, this, cx));
    }

    // 3. Keep on this host only toggle (when connector sync is open for this plugin).
    if let Some(is_local) = super::account_vault::local_toggle_state(&plugin.id, &fields.vault) {
        let this = this.clone();
        let id = plugin.id.clone();
        let toggle = Switch::new(SharedString::from(format!("connector-local-{id}")))
            .checked(is_local)
            .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                let on = *on;
                this.update(cx, |bench, cx| {
                    super::account_vault::toggle_keep_local(bench, &id, on, cx)
                })
            });
        items.push(inline_row(
            t!("Keep on this host only"),
            Some(t!("Don’t sync this sign-in to your other hosts.").into()),
            toggle,
            cx,
        ));
    }

    // 4. Reconnect option if auth failed or connector doesn't support sign-out.
    if plugin.enabled
        && has_sign_in(plugin)
        && (!plugin.sign_out || plugin.signed_in == Some(false))
    {
        let this = this.clone();
        let id = plugin.id.clone();
        items.push(inline_row(
            t!("Connection"),
            Some(t!("Sign in again or switch account.").into()),
            ui::Button::new(
                SharedString::from(format!("connector-reauth-{id}")),
                t!("Sign in again"),
                ui::Variant::Outline,
            )
            .icon(Icon::LogIn)
            .on_click(move |_, window, cx| {
                let id = id.clone();
                this.update(cx, |bench, cx| {
                    crate::connector_sign_in::open(bench, &id, window, cx)
                })
            }),
            cx,
        ));
    }

    let count = items.len();
    div()
        .pl(px(56.))
        .pr(SPACE_4)
        .py(SPACE_1)
        .bg(t.surface_hover.opacity(0.35))
        .border_b(crate::theme::border())
        .border_color(t.line_soft)
        .flex()
        .flex_col()
        .children(items.into_iter().enumerate().map(|(at, item)| {
            div()
                .py(SPACE_2H)
                .when(at + 1 < count, |row| {
                    row.border_b(crate::theme::border())
                        .border_color(t.line_soft)
                })
                .child(item)
        }))
        .into_any_element()
}

fn inline_row(
    label: impl Into<SharedString>,
    about: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(heading(label.into(), about, t).flex_1().min_w_0())
        .child(div().flex_none().child(control))
        .into_any_element()
}

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

fn connector_settings(
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
                .tooltip(t!("Sikemux forgets this App on every host that has it. It stays on GitHub until you delete it there."))
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
            match failed {
                Some(failed) => {
                    bench.notify(toast::error(t!("GitHub: {error}", error = failed)), cx);
                }
                None => bench.notify(toast::success(t!("GitHub App created")), cx),
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
            match forgot {
                Ok(_) => bench.notify(toast::success(t!("Forgot GitHub App")), cx),
                Err(error) => bench.notify(toast::error(t!("GitHub: {error}", error = error)), cx),
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
                        match said {
                            Ok(_) => bench.notify(toast::success(t!("Updated AWS profile")), cx),
                            Err(error) => {
                                bench.notify(toast::error(t!("AWS: {error}", error = error)), cx)
                            }
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
        inline_row(
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
        inline_row(
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
                change_database(bench, "remove", params, t!("Removed database"), window, cx)
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
                change_database(
                    bench,
                    method,
                    params,
                    t!("Updated database setting"),
                    window,
                    cx,
                )
            })
        });
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_tertiary)
        .child(label)
        .child(switch)
        .into_any_element()
}

fn change_database(
    bench: &mut Workbench,
    method: &'static str,
    params: Value,
    done: &'static str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let changed = ask::<Value>(bench, DATABASE, method, params);
    cx.spawn_in(window, async move |bench, cx| {
        let said = changed.await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            match said {
                Ok(_) => bench.notify(toast::success(done), cx),
                Err(error) => {
                    bench.notify(toast::error(t!("Database: {error}", error = error)), cx)
                }
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
    let id = id.to_owned();
    cx.spawn(async move |bench, cx| {
        let result = signed_out.await;
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                if fields.configuring.as_deref() == Some(id.as_str()) {
                    fields.configuring = None;
                }
            });
            match result {
                Ok(_) => bench.notify(toast::success(t!("Signed out of {name}", name = name)), cx),
                Err(error) => bench.notify(toast::error(format!("{name}: {error}")), cx),
            }
        });
    })
    .detach();
}
