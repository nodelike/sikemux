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
use sikemux_core::launch::providers::{Helper, Kind};
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::model_providers::{
    Check, CheckParams, ConnectParams, HelperParams, ModelProvider, ProviderParams,
};

use crate::core;
use crate::settings::rows::{self, Choice};
use crate::settings::Page;
use crate::settings::Settings;
use crate::theme::{Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM};
use crate::ui;
use crate::ui::numbers::compact;
use crate::workbench::Workbench;

pub struct Fields {
    providers: Vec<ModelProvider>,
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

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Paste the API key")
                .masked(true)
        });
        let helper_model = cx.new(|cx| InputState::new(window, cx));
        let entering = vec![
            super::super::follow_typing(bench, &key, cx),
            cx.subscribe(&key, |bench, _, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    connect(bench, cx)
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
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                fields.providers = providers.unwrap_or_default();
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
                    Ok(_) => fields.editing = None,
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
        Some((last, [])) => format!("{last} chats can run on it from the model picker."),
        Some((last, rest)) => format!(
            "{} and {last} chats can run on it from the model picker.",
            rest.join(", ")
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
        words.push(format!("{} context", compact(facts.context as f64)));
        if let (Some(input), Some(output)) = (facts.input, facts.output) {
            words.push(format!("${input}/${output} per 1M"));
        }
    }
    if let Some(check) = provider.checks.get(name) {
        words.push(check_words(check).into());
    } else if let Some(facts) = facts {
        words.push(
            if facts.tool_call {
                "lists tool calls"
            } else {
                "no tool calls"
            }
            .into(),
        );
    }
    words
}

fn check_words(check: &Check) -> &'static str {
    match check.works {
        true => "works with agents",
        false => "tool calls failed",
    }
}

/// The model it starts on with what models.dev says of it, and each model checked so far.
fn model_lines(provider: &ModelProvider) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(model) = provider.choices.model.as_deref() {
        let words = [vec![model.to_owned()], model_words(provider, model)].concat();
        lines.push(format!("Starts on {}", words.join(" · ")));
    }
    for (model, check) in &provider.checks {
        let day = chrono::DateTime::from_timestamp_millis(check.at as i64)
            .map(|at| {
                at.with_timezone(&chrono::Local)
                    .format("%b %-d, %Y")
                    .to_string()
            })
            .unwrap_or_default();
        lines.push(format!("{model} {}, {day}", check_words(check)));
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
        "Checking…"
    } else {
        "Check tool calls"
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
                "Connected. Agents started from now on can use it. {}",
                chats_on(bench, provider)
            );
            let added = provider
                .custom
                .then_some(provider.openai.as_str())
                .map(|url| {
                    let url = url.split_once("://").map_or(url, |(_, rest)| rest);
                    format!("Added by you, at {url}")
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
    let word = if custom { "Remove" } else { "Disconnect" };
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
                    "Cancel",
                    ui::Variant::Outline,
                    busy,
                    this,
                    |bench, window, cx| edit(bench, None, window, cx),
                ))
                .child(button(
                    format!("provider-connect-{id}"),
                    if busy { "Checking…" } else { "Connect" },
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
                    "Get a key",
                    ui::Variant::Outline,
                    false,
                    this,
                    move |_, _, cx| cx.open_url(&sign_up),
                ))
                .child(button(
                    format!("provider-add-{id}"),
                    "Add key",
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
            vec![format!("Nothing is answering at {host}.")]
        }
        (true, true) => vec!["Running, with no models downloaded yet.".into()],
        (true, false) => [
            vec![format!(
                "Running with {}. {}",
                models.join(", "),
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
                "Download",
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
                    "Checking…"
                } else {
                    "Check again"
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
                "Remove",
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

fn save_helper(bench: &mut Workbench, helper: Option<Helper>, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.helper = helper.clone());
    bench.call_typed::<call::SetModelHelper>(HelperParams { helper });
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
    let labels: Vec<String> = std::iter::once("As the agent that starts them".to_owned())
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
            std::iter::once(format!("{} account", bench.agent_name(&agent).to_owned()))
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
    let placeholder = route.unwrap_or_else(|| "The agent's own".into());
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
        .child(rows::row("Agent", None, agent.view(), cx))
        .children(
            paid.as_ref()
                .map(|paid| rows::row("Paid by", None, paid.view(), cx)),
        )
        .when(fields.helper.is_some(), |body| {
            body.child(rows::row(
                "Model",
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
            .filter(|provider| provider.kind == kind)
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
                ui::Button::new("provider-add-own", "Add provider", ui::Variant::Outline)
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
            (count > 0).then(|| format!("{count} connected").into()),
            Some(sub.into()),
            lit(title),
            body,
            cx,
        )
    };
    let section = match page {
        Page::Plans => keyed(
            Kind::Plan,
        "Coding plans",
        "Monthly subscriptions made for coding agents. Once its key is saved here, a plan's models show in every chat's model picker, run by an agent that can use them. Keys stay in your Keychain.",
        cx,
    ),
        Page::Providers => keyed(
            Kind::Api,
        "API providers",
        "Hosted models you pay the provider for by the token. Once its key is saved here, a provider's models show in every chat's model picker, run by an agent that can use them. Keys stay in your Keychain.",
        cx,
    ),
        Page::LocalModels => {
    let local = of_kind(Kind::Local);
    let running = connected(&local);
    rows::section(
        "Local models",
        (running > 0).then(|| format!("{running} running").into()),
        Some("Models served on this host by Ollama or LM Studio. They need no key, and nothing leaves this host.".into()),
        lit("Local models"),
        div().children(local.iter().map(|provider| local_row(bench, fields, provider, this, cx))),
        cx,
    )
        }
        _ => rows::section(
        "Helpers",
        None,
        Some("Agents that other agents start without naming a model run as set here, and Claude Code's own subagents take this model when their chat runs where helpers do. The agent you build with keeps the model you pick.".into()),
        lit("Helpers"),
        helpers(bench, fields, cx),
        cx,
    ),
    };
    vec![section]
}
