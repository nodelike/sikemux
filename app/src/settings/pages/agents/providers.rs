//! The models agents may run on besides their makers' accounts: coding plans,
//! providers paid by the token, models served on this host, and what
//! helpers run on.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window,
};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_agents::launch::providers::Helper;
use sikemux_agents::launch::providers::Kind;
use sikemux_calls::call;
use sikemux_calls::model_providers::Check;
use sikemux_calls::model_providers::CheckParams;
use sikemux_calls::model_providers::ConnectParams;
use sikemux_calls::model_providers::HelperParams;
use sikemux_calls::model_providers::ModelProvider;
use sikemux_calls::model_providers::ProviderParams;
use sikemux_calls::params::NoParams;
use sikemux_calls::vault::{
    OrgEntryParams, OrgModelKeyAdd, OrgVaultStatus, ShownOrgModelKey, ShownOrgVault,
};
use sikemux_calls::Call;

use crate::core;
use crate::core::Target;
use crate::settings::rows::{self, Choice};
use crate::settings::Page;
use crate::settings::Settings;
use crate::theme::{
    Tokens, SPACE_0, SPACE_1, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM,
};
use crate::ui;
use crate::ui::numbers::compact;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const HELPERS: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(
        crate::settings::index::Page::Helpers,
        "Helpers",
        "cheap model workers scouts reviewers subagents spawn paid by provider",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Helpers,
        "Helpers",
        "Agent",
        "helper worker scout reviewer subagent cheap model provider",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const MODELS: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(crate::settings::index::Page::Plans, "Coding plans", "zai glm zhipu minimax kimi code alibaba qwen volcengine ark xiaomi mimo stepfun tencent china opencode go synthetic ollama subscription plan key claude codex"),
    crate::settings::index::section(crate::settings::index::Page::Providers, "API providers", "openrouter baseten together fireworks groq cerebras deepseek moonshot kimi alibaba qwen dashscope volcengine ark doubao xiaomi mimo stepfun tencent hunyuan china mistral vercel api key hosted open models opencode pi omp claude codex"),
    crate::settings::index::section(crate::settings::index::Page::LocalModels, "Local models", "ollama lm studio lmstudio local offline runner claude codex"),
];

pub struct Fields {
    providers: Vec<ModelProvider>,
    orgs: Vec<ShownOrgVault>,
    /// The org a key is being added for, while its provider and key are being chosen.
    org_adding: Option<OrgAdding>,
    helper: Option<Helper>,
    /// The provider whose key is being typed into `key`.
    editing: Option<String>,
    key: Entity<InputState>,
    busy: Option<String>,
    failure: Option<(String, String)>,
    checking: bool,
    helper_model: Entity<InputState>,
    /// The helper's agent and who pays for it, made again whenever the helper changes.
    helper_choices: Option<(Option<Helper>, Choice, Option<Choice>)>,
    _entering: Vec<Subscription>,
}

struct OrgAdding {
    org: String,
    kind: Kind,
    /// The providers offered, in the order `choice` lists them.
    offered: Vec<String>,
    picked: usize,
    choice: Choice,
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("Paste the API key or its op:// reference"))
                .masked(true)
        });
        let helper_model = cx.new(|cx| InputState::new(window, cx));
        let entering = vec![
            super::super::follow_typing(bench, &key, cx),
            cx.subscribe(&key, |bench, _, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    match fields(bench, cx).is_some_and(|fields| fields.org_adding.is_some()) {
                        true => add_org_key(bench, cx),
                        false => connect(bench, cx),
                    }
                }
            }),
            cx.subscribe(&helper_model, |bench, model, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let model = model.read(cx).value().trim().to_owned();
                    save_model(bench, model, cx);
                }
            }),
        ];
        Self {
            providers: Vec::new(),
            orgs: Vec::new(),
            org_adding: None,
            helper: None,
            editing: None,
            key,
            busy: None,
            failure: None,
            checking: false,
            helper_model,
            helper_choices: None,
            _entering: entering,
        }
    }
}

/// Changes the providers and helper as the pages show them, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_fields(bench, cx, |fields, cx| change(&mut fields.providers, cx))
}

/// The providers and helper, while an agent page is open.
fn fields<'a>(bench: &Workbench, cx: &'a App) -> Option<&'a Fields> {
    Some(&super::fields(bench, cx)?.providers)
}

pub fn read(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let here = core::Target::here(&asks);
        let providers = here
            .request_typed::<call::ModelProviders>(NoParams {})
            .await;
        let helper = here.request_typed::<call::ModelHelper>(NoParams {}).await;
        let orgs = here.request_typed::<call::OrgVaults>(NoParams {}).await;
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                fields.providers = providers.unwrap_or_default();
                fields.orgs = orgs.unwrap_or_default();
                fields.helper = helper.ok().flatten();
                fields.checking = false;
                fields.helper_choices = None;
            });
        });
    })
    .detach();
}

/// Runs `method` on provider `id`, then reads the providers again; a failure stays on its row.
fn change<C: Call + 'static>(
    bench: &mut Workbench,
    id: String,
    params: C::Params,
    cx: &mut Context<Workbench>,
) where
    C::Params: Serialize,
    C::Answer: DeserializeOwned + 'static,
{
    change_fields(bench, cx, |fields, _| {
        fields.busy = Some(id.clone());
        fields.failure = None;
    });
    let asked = core::Target::here(&bench.asks).request_typed::<C>(params);
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                fields.busy = None;
                match answer {
                    Ok(_) => {
                        fields.editing = None;
                        fields.org_adding = None;
                    }
                    Err(message) => fields.failure = Some((id, message)),
                }
            });
            read(bench, cx);
        });
    })
    .detach();
}

fn connect(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some((id, key)) = fields(bench, cx).and_then(|fields| {
        let key = fields.key.read(cx).value().trim().to_owned();
        Some((fields.editing.clone()?, key)).filter(|(_, key)| !key.is_empty())
    }) else {
        return;
    };
    change::<call::ModelProviderConnect>(bench, id.clone(), ConnectParams { id, key }, cx);
}

fn edit(
    bench: &mut Workbench,
    id: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    change_fields(bench, cx, |fields, cx| {
        fields.editing = id;
        fields.org_adding = None;
        fields.failure = None;
        fields.key.update(cx, |key, cx| {
            key.set_value("", window, cx);
            key.focus(window, cx);
        });
    });
}

/// The model a check asks about: the provider's coding model, else the first a local runner has.
fn checked_model(provider: &ModelProvider) -> Option<String> {
    let served = provider.models.as_ref().and_then(|models| models.first());
    provider.choices.model.as_ref().or(served).cloned()
}

/// Which of Claude Code and Codex a chat can move onto the provider, in words.
fn chats_on(bench: &Workbench, provider: &ModelProvider) -> String {
    let agents: Vec<String> = (provider.agents.iter())
        .map(|agent| bench.agent_name(agent).to_owned())
        .collect();
    match agents.split_last() {
        None => String::new(),
        Some((last, [])) => t!(
            "{agent} chats can run on it from the model picker.",
            agent = last
        ),
        Some((last, rest)) => t!(
            "{agents} and {last} chats can run on it from the model picker.",
            agents = rest.join(", "),
            last = last
        ),
    }
}

/// Context, price per million tokens and tool calls, in a few words. A check outranks
/// what models.dev says.
fn model_words(provider: &ModelProvider, model: &str) -> Vec<String> {
    let name = model.strip_suffix("[1m]").unwrap_or(model);
    let facts = provider.choices.facts.get(name);
    let mut words = Vec::new();
    if let Some(facts) = facts {
        words.push(t!("{size} context", size = compact(facts.context as f64)));
        if let (Some(input), Some(output)) = (facts.input, facts.output) {
            words.push(t!(
                "${input}/${output} per 1M",
                input = input,
                output = output
            ));
        }
    }
    if let Some(check) = provider.checks.get(name) {
        words.push(check_words(check).into());
    } else if let Some(facts) = facts {
        words.push(
            if facts.tool_call {
                t!("lists tool calls")
            } else {
                t!("no tool calls")
            }
            .into(),
        );
    }
    words
}

fn check_words(check: &Check) -> &'static str {
    match check.works {
        true => t!("works with agents"),
        false => t!("tool calls failed"),
    }
}

/// The model it starts on with what models.dev says of it, and each model checked so far.
fn model_lines(provider: &ModelProvider) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(model) = provider.choices.model.as_deref() {
        let words = [vec![model.to_owned()], model_words(provider, model)].concat();
        lines.push(t!("Starts on {model}", model = words.join(" · ")));
    }
    for (model, check) in &provider.checks {
        let day = chrono::DateTime::from_timestamp_millis(check.at as i64)
            .map(|at| {
                at.with_timezone(&chrono::Local)
                    .format("%b %-d, %Y")
                    .to_string()
            })
            .unwrap_or_default();
        lines.push(t!(
            "{model} {result}, {day}",
            model = model,
            result = check_words(check),
            day = day
        ));
    }
    lines
}

fn button(
    id: String,
    label: &'static str,
    variant: ui::Variant,
    disabled: bool,
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> ui::Button {
    let this = this.clone();
    let run = std::rc::Rc::new(run);
    ui::Button::new(SharedString::from(id), label, variant)
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let run = run.clone();
            this.update(cx, |bench, cx| run(bench, window, cx))
        })
}

fn check_button(
    provider: &ModelProvider,
    busy: bool,
    this: &Entity<Workbench>,
) -> Option<ui::Button> {
    let model = checked_model(provider)?;
    let id = provider.id.clone();
    let label = if busy {
        t!("Checking…")
    } else {
        t!("Check tool calls")
    };
    Some(button(
        format!("provider-check-{id}"),
        label,
        ui::Variant::Outline,
        busy,
        this,
        move |bench, _, cx| {
            change::<call::ModelProviderCheck>(
                bench,
                id.clone(),
                CheckParams {
                    id: id.clone(),
                    model: model.clone(),
                },
                cx,
            )
        },
    ))
}

fn lines(about: Vec<String>, failed: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .text_color(if failed { t.danger } else { t.text_tertiary })
        .children(about)
        .into_any_element()
}

fn keyed_row(
    bench: &Workbench,
    fields: &Fields,
    provider: &ModelProvider,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = provider.id.clone();
    let label = provider.label.clone();
    let connected = provider.connected;
    let editing = fields.editing.as_deref() == Some(id.as_str());
    let busy = fields.busy.as_deref() == Some(id.as_str());
    let failed = fields
        .failure
        .as_ref()
        .filter(|(at, _)| *at == id)
        .map(|(_, message)| message.clone());
    let about = match (&failed, connected) {
        (Some(message), _) => Some(lines(vec![message.clone()], true, cx)),
        (None, true) => {
            let first = format!(
                "{} {}",
                t!("Connected. Agents started from now on can use it."),
                chats_on(bench, provider)
            );
            let added = provider
                .custom
                .then_some(provider.openai.as_str())
                .map(|url| {
                    let url = url.split_once("://").map_or(url, |(_, rest)| rest);
                    t!("Added by you, at {url}", url = url)
                });
            Some(lines(
                [
                    added.into_iter().collect(),
                    vec![first.trim().to_owned()],
                    model_lines(provider),
                ]
                .concat(),
                false,
                cx,
            ))
        }
        (None, false) => None,
    };
    let sign_up = provider.sign_up_url.clone();
    let custom = provider.custom;
    let word = if custom {
        t!("Remove")
    } else {
        t!("Disconnect")
    };
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .when(connected, |controls| {
            let disconnect = id.clone();
            controls
                .children(check_button(provider, busy, this))
                .child(button(
                    format!("provider-disconnect-{id}"),
                    word,
                    ui::Variant::Outline,
                    busy,
                    this,
                    move |bench, _, cx| {
                        let params = ProviderParams {
                            id: disconnect.clone(),
                        };
                        match custom {
                            true => change::<call::ModelProviderRemove>(
                                bench,
                                disconnect.clone(),
                                params,
                                cx,
                            ),
                            false => change::<call::ModelProviderDisconnect>(
                                bench,
                                disconnect.clone(),
                                params,
                                cx,
                            ),
                        }
                    },
                ))
        })
        .when(!connected && editing, |controls| {
            let empty = fields.key.read(cx).value().trim().is_empty();
            controls
                .child(div().w(px(220.)).child(ui::Field::new(&fields.key)))
                .child(button(
                    format!("provider-cancel-{id}"),
                    t!("Cancel"),
                    ui::Variant::Outline,
                    busy,
                    this,
                    |bench, window, cx| edit(bench, None, window, cx),
                ))
                .child(button(
                    format!("provider-connect-{id}"),
                    if busy {
                        t!("Checking…")
                    } else {
                        t!("Connect")
                    },
                    ui::Variant::Primary,
                    busy || empty,
                    this,
                    |bench, _, cx| connect(bench, cx),
                ))
        })
        .when(!connected && !editing, |controls| {
            let open = id.clone();
            controls
                .child(button(
                    format!("provider-sign-up-{id}"),
                    t!("Get a key"),
                    ui::Variant::Outline,
                    false,
                    this,
                    move |_, _, cx| cx.open_url(&sign_up),
                ))
                .child(button(
                    format!("provider-add-{id}"),
                    t!("Add key"),
                    ui::Variant::Outline,
                    false,
                    this,
                    move |bench, window, cx| edit(bench, Some(open.clone()), window, cx),
                ))
        });
    row(label, about, controls, &t)
}

/// A provider's row: its name, the lines under it, and its buttons.
fn row(
    label: String,
    about: Option<AnyElement>,
    controls: impl IntoElement,
    t: &Tokens,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .px(SPACE_4)
        .py(SPACE_2H)
        .border_b(crate::theme::border())
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
                .children(about),
        )
        .child(controls)
        .into_any_element()
}

fn local_row(
    bench: &Workbench,
    fields: &Fields,
    provider: &ModelProvider,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = provider.id.clone();
    let label = provider.label.clone();
    let connected = provider.connected;
    let busy = fields.busy.as_deref() == Some(id.as_str());
    let models: Vec<&str> = (provider.models.iter().flatten())
        .map(String::as_str)
        .collect();
    let about = match (connected, models.is_empty()) {
        (false, _) => {
            let host = (provider.openai.split("://").nth(1))
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            vec![t!("Nothing is answering at {host}.", host = host)]
        }
        (true, true) => vec![t!("Running, with no models downloaded yet.").into()],
        (true, false) => [
            vec![format!(
                "{} {}",
                t!("Running with {models}.", models = models.join(", ")),
                chats_on(bench, provider)
            )
            .trim()
            .to_owned()],
            model_lines(provider),
        ]
        .concat(),
    };
    let failed = fields
        .failure
        .as_ref()
        .filter(|(at, _)| *at == id)
        .map(|(_, message)| message.clone());
    let about = match failed {
        Some(message) => lines(vec![message], true, cx),
        None => lines(about, false, cx),
    };
    let sign_up = provider.sign_up_url.clone();
    let checking = fields.checking;
    let custom = provider.custom;
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .when(!connected && !custom, |controls| {
            controls.child(button(
                format!("runner-download-{id}"),
                t!("Download"),
                ui::Variant::Outline,
                false,
                this,
                move |_, _, cx| cx.open_url(&sign_up),
            ))
        })
        .when(connected, |controls| {
            controls.children(check_button(provider, busy, this))
        })
        .when(!custom, |controls| {
            controls.child(button(
                format!("runner-again-{id}"),
                if checking {
                    t!("Checking…")
                } else {
                    t!("Check again")
                },
                ui::Variant::Outline,
                checking,
                this,
                |bench, _, cx| {
                    change_fields(bench, cx, |fields, _| fields.checking = true);
                    read(bench, cx);
                },
            ))
        })
        .when(custom, |controls| {
            let removing = id.clone();
            controls.child(button(
                format!("runner-remove-{id}"),
                t!("Remove"),
                ui::Variant::Outline,
                busy,
                this,
                move |bench, _, cx| {
                    change::<call::ModelProviderRemove>(
                        bench,
                        removing.clone(),
                        ProviderParams {
                            id: removing.clone(),
                        },
                        cx,
                    )
                },
            ))
        });
    row(label, Some(about), controls, &t)
}

/// The kind of page a catalogue provider's keys live on.
fn kind_of(fields: &Fields, provider: &str) -> Option<Kind> {
    (fields.providers.iter())
        .find(|own| own.id == provider && own.org.is_none())
        .map(|own| own.kind)
}

fn start_org_add(
    bench: &mut Workbench,
    org: String,
    kind: Kind,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(fields) = fields(bench, cx) else {
        return;
    };
    let held: Vec<&str> = (fields.orgs.iter())
        .filter(|vault| vault.org == org)
        .flat_map(|vault| vault.model_keys.iter().map(|key| key.provider.as_str()))
        .collect();
    let offered: Vec<(String, String)> = (fields.providers.iter())
        .filter(|provider| provider.kind == kind && provider.org.is_none() && !provider.custom)
        .filter(|provider| !held.contains(&provider.id.as_str()))
        .map(|provider| (provider.id.clone(), provider.label.clone()))
        .collect();
    let choice = Choice::new(
        offered.iter().map(|(_, label)| label.clone()).collect(),
        0,
        |bench, at, cx| {
            change_fields(bench, cx, |fields, _| {
                if let Some(adding) = &mut fields.org_adding {
                    adding.picked = at;
                }
            });
        },
        window,
        cx,
    );
    change_fields(bench, cx, |fields, cx| {
        fields.editing = None;
        fields.failure = None;
        fields.org_adding = Some(OrgAdding {
            org,
            kind,
            offered: offered.into_iter().map(|(id, _)| id).collect(),
            picked: 0,
            choice,
        });
        fields.key.update(cx, |key, cx| {
            key.set_value("", window, cx);
            key.focus(window, cx);
        });
    });
}

fn add_org_key(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(params) = fields(bench, cx).and_then(|fields| {
        let adding = fields.org_adding.as_ref()?;
        let key = fields.key.read(cx).value().trim().to_owned();
        Some(OrgModelKeyAdd {
            org: adding.org.clone(),
            provider: adding.offered.get(adding.picked)?.clone(),
            key,
        })
        .filter(|params| !params.key.is_empty())
    }) else {
        return;
    };
    let id = format!("org-add-{}", params.org);
    change::<call::OrgModelKeyAdd>(bench, id, params, cx);
}

fn org_key_row(
    bench: &Workbench,
    fields: &Fields,
    vault: &ShownOrgVault,
    key: &ShownOrgModelKey,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let provider = (fields.providers.iter()).find(|provider| provider.id == key.route);
    let label = provider.map_or_else(
        || format!("{} ({})", key.label, vault.name),
        |provider| provider.label.clone(),
    );
    let busy = fields.busy.as_deref() == Some(key.route.as_str());
    let failed = (fields.failure.as_ref())
        .filter(|(at, _)| *at == key.route)
        .map(|(_, message)| message.clone());
    let models = provider.map(model_lines).unwrap_or_default();
    let about = match (failed, vault.active, vault.writes) {
        (Some(message), _, _) => lines(vec![message], true, cx),
        (None, false, _) => lines(vec![t!("Paused").into()], false, cx),
        (None, true, true) => {
            let chats = provider.map(|provider| chats_on(bench, provider));
            let first = t!("Connected for everyone in {org}.", org = vault.name);
            let first = format!("{first} {}", chats.unwrap_or_default());
            lines([vec![first.trim().to_owned()], models].concat(), false, cx)
        }
        (None, true, false) => {
            let first = t!(
                "{org}'s admins change the key of {provider}.",
                org = vault.name,
                provider = label
            );
            lines([vec![first], models].concat(), false, cx)
        }
    };
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .when(vault.active && vault.writes, |controls| {
            let params = OrgEntryParams {
                org: vault.org.clone(),
                id: key.id.clone(),
            };
            let route = key.route.clone();
            controls
                .children(provider.and_then(|provider| check_button(provider, busy, this)))
                .child(button(
                    format!("org-key-remove-{route}"),
                    t!("Remove"),
                    ui::Variant::Outline,
                    busy,
                    this,
                    move |bench, _, cx| {
                        let params = OrgEntryParams {
                            org: params.org.clone(),
                            id: params.id.clone(),
                        };
                        change::<call::OrgModelKeyRemove>(bench, route.clone(), params, cx)
                    },
                ))
        })
        .when(vault.active && !vault.writes, |controls| {
            controls.child(ui::icon(crate::icons::Icon::Lock, 14.).text_color(t.text_tertiary))
        });
    row(label, Some(about), controls, &t)
}

fn org_add_row(
    fields: &Fields,
    vault: &ShownOrgVault,
    kind: Kind,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let adding = (fields.org_adding.as_ref())
        .filter(|adding| adding.org == vault.org && adding.kind == kind);
    let Some(adding) = adding else {
        let this = this.clone();
        let org = vault.org.clone();
        return div()
            .flex()
            .px(SPACE_4)
            .py(SPACE_2H)
            .child(
                ui::Button::new(
                    SharedString::from(format!("org-key-add-{org}")),
                    t!("Add a key for {org}", org = vault.name),
                    ui::Variant::Outline,
                )
                .icon(crate::icons::Icon::Plus)
                .on_click(move |_, window, cx| {
                    let org = org.clone();
                    this.update(cx, |bench, cx| start_org_add(bench, org, kind, window, cx))
                }),
            )
            .into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let id = format!("org-add-{}", vault.org);
    let busy = fields.busy.as_deref() == Some(id.as_str());
    let failed = (fields.failure.as_ref())
        .filter(|(at, _)| *at == id)
        .map(|(_, message)| lines(vec![message.clone()], true, cx));
    let empty = fields.key.read(cx).value().trim().is_empty() || adding.offered.is_empty();
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(div().w(px(160.)).child(adding.choice.view()))
        .child(div().w(px(220.)).child(ui::Field::new(&fields.key)))
        .child(button(
            format!("org-key-cancel-{}", vault.org),
            t!("Cancel"),
            ui::Variant::Outline,
            busy,
            this,
            |bench, window, cx| edit(bench, None, window, cx),
        ))
        .child(button(
            format!("org-key-connect-{}", vault.org),
            if busy { t!("Checking…") } else { t!("Add") },
            ui::Variant::Primary,
            busy || empty,
            this,
            |bench, _, cx| add_org_key(bench, cx),
        ));
    row(
        t!("Add a key for {org}", org = vault.name),
        failed,
        controls,
        &t,
    )
}

/// A group's name in spaced capitals over its card, after the org's mark when it is an org's.
fn grouped(
    mark: Option<AnyElement>,
    name: &str,
    after: Option<AnyElement>,
    card: AnyElement,
    cx: &App,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .px(SPACE_4)
                .children(mark)
                .child(ui::caps(name, cx))
                .children(after),
        )
        .child(card)
        .into_any_element()
}

/// An org's keys of `kind`, while there are any to show or its admin may add one.
fn org_group(
    bench: &Workbench,
    fields: &Fields,
    vault: &ShownOrgVault,
    kind: Kind,
    this: &Entity<Workbench>,
    cx: &App,
) -> Option<AnyElement> {
    let keys: Vec<&ShownOrgModelKey> = (vault.model_keys.iter())
        .filter(|key| kind_of(fields, &key.provider) == Some(kind))
        .collect();
    let waiting = vault.status == OrgVaultStatus::Waiting;
    let adds = vault.writes && !waiting;
    if keys.is_empty() && !waiting && !adds {
        return None;
    }
    let sub = match (vault.active, vault.writes) {
        (false, _) => t!(
            "{org}'s plan has lapsed, so its keys are paused and no chat can pick them. Nothing was deleted: they come back when an owner renews.",
            org = vault.name
        ),
        (true, true) => t!(
            "Keys {org}'s admins added. A chat runs on one only when it's picked; it never stands in for your own. You're an admin.",
            org = vault.name
        ),
        (true, false) => t!(
            "Keys {org}'s admins added. A chat runs on one only when it's picked; it never stands in for your own. You're a member.",
            org = vault.name
        ),
    };
    let note = |words: Vec<String>, failed: bool| {
        div()
            .px(SPACE_4)
            .py(SPACE_2H)
            .child(lines(words, failed, cx))
    };
    let body = div()
        .when(waiting, |body| {
            body.child(note(
                vec![t!(
                    "Not open on this host yet. It opens once another of {org}'s hosts that's online hands this one the vault.",
                    org = vault.name
                )],
                false,
            ))
        })
        .children(vault.failed.clone().map(|failed| note(vec![failed], true)))
        .children(keys.iter().map(|key| org_key_row(bench, fields, vault, key, this, cx)))
        .when(adds, |body| body.child(org_add_row(fields, vault, kind, this, cx)));
    let card = rows::section("", None, Some(sub.into()), false, body, cx);
    let mark = ui::org_mark(&vault.name, &vault.org, 14.).into_any_element();
    let lapsed = (!vault.active).then(|| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .text_color(Tokens::get(cx).warn)
            .child(ui::plan_lapsed_mark(10., cx))
            .child(t!("Plan lapsed"))
            .into_any_element()
    });
    Some(grouped(Some(mark), &vault.name, lapsed, card, cx))
}

fn save_helper(bench: &mut Workbench, helper: Option<Helper>, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.helper = helper.clone());
    Target::here(&bench.asks).call_typed::<call::SetModelHelper>(HelperParams { helper });
}

fn save_model(bench: &mut Workbench, model: String, cx: &mut Context<Workbench>) {
    let Some(mut helper) = fields(bench, cx).and_then(|fields| fields.helper.clone()) else {
        return;
    };
    if helper.model.as_deref().unwrap_or_default() == model {
        return;
    }
    helper.model = Some(model).filter(|model| !model.is_empty());
    save_helper(bench, Some(helper), cx);
}

/// The agents a chat can run as, by id and name.
fn chat_agents(bench: &Workbench) -> Vec<(String, String)> {
    bench
        .reads
        .catalog
        .agents()
        .filter(|kind| kind.chat.is_some())
        .map(|kind| (kind.kind.clone(), kind.label.clone()))
        .collect()
}

/// Whether the helper's dropdowns were made for another helper than the one chosen now.
pub(super) fn stale(fields: &Fields) -> bool {
    (fields.helper_choices.as_ref()).is_none_or(|(made_for, _, _)| *made_for != fields.helper)
}

/// Makes the helper's dropdowns again for the helper as it is now.
pub(in crate::settings::pages) fn rebuild_choices(
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(helper) = fields(bench, cx).map(|fields| fields.helper.clone()) else {
        return;
    };
    let agents = chat_agents(bench);
    let agent = helper.as_ref().map(|helper| helper.agent.clone());
    let chosen = agent
        .as_ref()
        .and_then(|agent| agents.iter().position(|(kind, _)| kind == agent))
        .map_or(0, |at| at + 1);
    let labels: Vec<String> = std::iter::once(t!("As the agent that starts them").to_owned())
        .chain(agents.iter().map(|(_, label)| label.clone()))
        .collect();
    let kinds: Vec<String> = agents.iter().map(|(kind, _)| kind.clone()).collect();
    let agent_choice = Choice::new(
        labels,
        chosen,
        move |bench, at, cx| {
            let helper = at
                .checked_sub(1)
                .and_then(|at| kinds.get(at))
                .map(|agent| Helper {
                    agent: agent.clone(),
                    provider: None,
                    model: None,
                });
            save_helper(bench, helper, cx);
        },
        window,
        cx,
    );
    let paid = agent.map(|agent| {
        let routes: Vec<(String, String)> = fields(bench, cx)
            .map(|fields| {
                fields
                    .providers
                    .iter()
                    .filter(|provider| provider.connected)
                    .filter(|provider| provider.agents.contains(&agent))
                    .map(|provider| (provider.id.clone(), provider.label.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let current = helper
            .as_ref()
            .and_then(|helper| helper.provider.as_deref());
        let chosen = current
            .and_then(|current| routes.iter().position(|(id, _)| id == current))
            .map_or(0, |at| at + 1);
        let labels: Vec<String> =
            std::iter::once(t!("{agent} account", agent = bench.agent_name(&agent)))
                .chain(routes.iter().map(|(_, label)| label.clone()))
                .collect();
        let ids: Vec<String> = routes.iter().map(|(id, _)| id.clone()).collect();
        Choice::new(
            labels,
            chosen,
            move |bench, at, cx| {
                let Some(mut helper) = fields(bench, cx).and_then(|fields| fields.helper.clone())
                else {
                    return;
                };
                helper.provider = at.checked_sub(1).and_then(|at| ids.get(at)).cloned();
                save_helper(bench, Some(helper), cx);
            },
            window,
            cx,
        )
    });
    let model = helper
        .as_ref()
        .and_then(|helper| helper.model.clone())
        .unwrap_or_default();
    let route = fields(bench, cx).and_then(|fields| {
        let helper = helper.as_ref()?;
        let route = fields
            .providers
            .iter()
            .find(|each| Some(&each.id) == helper.provider.as_ref())?;
        route.choices.defaults.get(&helper.agent).cloned()
    });
    let placeholder = route.unwrap_or_else(|| t!("The agent's own").into());
    change_fields(bench, cx, |fields, cx| {
        fields.helper_model.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value(model, window, cx);
        });
        fields.helper_choices = Some((helper, agent_choice, paid));
    });
}

fn helpers(bench: &Workbench, fields: &Fields, cx: &App) -> AnyElement {
    let Some((_, agent, paid)) = &fields.helper_choices else {
        return div().into_any_element();
    };
    let _ = bench;
    div()
        .child(rows::row(t!("Agent"), None, agent.view(), cx))
        .children(
            paid.as_ref()
                .map(|paid| rows::row(t!("Paid by"), None, paid.view(), cx)),
        )
        .when(fields.helper.is_some(), |body| {
            body.child(rows::row(
                t!("Model"),
                None,
                div()
                    .w(px(220.))
                    .child(ui::Field::new(&fields.helper_model)),
                cx,
            ))
        })
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    page: Page,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let of_kind = |kind: Kind| -> Vec<&ModelProvider> {
        let mut providers: Vec<&ModelProvider> = fields
            .providers
            .iter()
            .filter(|provider| provider.kind == kind && provider.org.is_none())
            .collect();
        providers.sort_by_key(|provider| !provider.custom);
        providers
    };
    let connected = |providers: &[&ModelProvider]| {
        providers
            .iter()
            .filter(|provider| provider.connected)
            .count()
    };
    let keyed = |kind: Kind, title: &'static str, sub: &'static str, cx: &App| {
        let providers = of_kind(kind);
        let count = connected(&providers);
        let adds = (kind == Kind::Api).then(|| {
            let this = this.clone();
            div().flex().px(SPACE_4).py(SPACE_2H).child(
                ui::Button::new("provider-add-own", t!("Add provider"), ui::Variant::Outline)
                    .icon(crate::icons::Icon::Plus)
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| {
                            let here = core::Target::here(&bench.asks);
                            bench.add_provider_to(here, None, window, cx)
                        })
                    }),
            )
        });
        let body = div()
            .children(
                providers
                    .iter()
                    .map(|provider| keyed_row(bench, fields, provider, this, cx)),
            )
            .children(adds);
        rows::section(
            title,
            (count > 0).then(|| t!("{count} connected", count = count).into()),
            Some(sub.into()),
            lit(title),
            body,
            cx,
        )
    };
    let with_orgs = |kind: Kind, own: AnyElement| -> Vec<AnyElement> {
        let orgs: Vec<AnyElement> = (fields.orgs.iter())
            .filter_map(|vault| org_group(bench, fields, vault, kind, this, cx))
            .collect();
        match orgs.is_empty() {
            true => vec![own],
            false => std::iter::once(grouped(None, t!("Yours"), None, own, cx))
                .chain(orgs)
                .collect(),
        }
    };
    let section = match page {
        Page::Plans => return with_orgs(Kind::Plan, keyed(
            Kind::Plan,
        n!("Coding plans"),
        t!("Monthly subscriptions made for coding agents. Once its key is saved here, a plan's models show in every chat's model picker, run by an agent that can use them. Keys stay in your Keychain."),
        cx,
    )),
        Page::Providers => return with_orgs(Kind::Api, keyed(
            Kind::Api,
        n!("API providers"),
        t!("Hosted models you pay the provider for by the token. Once its key is saved here, a provider's models show in every chat's model picker, run by an agent that can use them. Keys stay in your Keychain."),
        cx,
    )),
        Page::LocalModels => {
    let local = of_kind(Kind::Local);
    let running = connected(&local);
    rows::section(
        n!("Local models"),
        (running > 0).then(|| t!("{count} running", count = running).into()),
        Some(t!("Models served on this host by Ollama or LM Studio. They need no key, and nothing leaves this host.").into()),
        lit("Local models"),
        div().children(local.iter().map(|provider| local_row(bench, fields, provider, this, cx))),
        cx,
    )
        }
        _ => rows::section(
        n!("Helpers"),
        None,
        Some(t!("Agents that other agents start without naming a model run as set here, and Claude Code's own subagents take this model when their chat runs where helpers do. The agent you build with keeps the model you pick.").into()),
        lit("Helpers"),
        helpers(bench, fields, cx),
        cx,
    ),
    };
    vec![section]
}
