//! A chat's model menu: the models its agent offers, searched from its own field.
//! Before the chat starts it also moves the chat to another agent or account, and
//! to a provider the person connected that pays for its turns instead.

use std::rc::Rc;

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, Anchor, AnyElement, App, AppContext, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::{ConfigChoice, ConfigPicker};

use crate::core;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

actions!(model_picker, [Up, Down, Pick, Close]);

const CONTEXT: &str = "ModelPicker";

pub fn install(cx: &mut App) {
    let context = Some("ModelPicker > Input");
    cx.bind_keys([
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("enter", Pick, context),
        KeyBinding::new("escape", Close, context),
    ]);
}

/// Where a chat that has not started moves to: an agent, the account it runs on, and the
/// model and provider it starts on when a provider pays for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Moving {
    pub provider: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
}

/// Sets one of the session's settings, by the setting's id, to a value.
pub type Set = Rc<dyn Fn(&str, &str, &mut Window, &mut App)>;
/// Moves the chat to another agent, account or provider.
pub type Move = Rc<dyn Fn(Moving, &mut Window, &mut App)>;

/// What the menu shows, as the chat that owns it says each time it draws.
#[derive(Clone)]
pub struct Picked {
    /// The chat's agent, such as `claude`.
    pub provider: String,
    /// The account it runs on.
    pub profile: Option<String>,
    pub model: Option<ConfigPicker>,
    /// The connected provider paying for its turns, when one is.
    pub model_provider: Option<String>,
    /// The chat has started, so it keeps its agent, account and provider.
    pub locked: bool,
    /// What the list says while the agent's models have not come yet.
    pub pending: Option<&'static str>,
    pub set: Set,
    pub moving: Move,
    /// The core of the host the chat runs on, whose providers it can pay with.
    pub target: core::Target,
}

/// A connected provider the agent can run on, with the models it serves.
#[derive(Clone)]
struct Route {
    id: String,
    label: String,
    models: Vec<String>,
    provider: Value,
}

/// The open menu: its search, the row the keys are on, and the providers that can pay.
pub struct Menu {
    bench: WeakEntity<Workbench>,
    picked: Picked,
    query: Entity<InputState>,
    chosen: usize,
    routes: Vec<Route>,
    close: Dismiss,
    _typing: Subscription,
}

/// Closes the popover the menu is in.
type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

/// Context, price per million tokens and tool calls, in a few words, as Settings words them.
fn served_label(route: &Route, model: &str) -> String {
    let name = model.strip_suffix("[1m]").unwrap_or(model);
    let facts = &route.provider["facts"][name];
    let check = &route.provider["checks"][name];
    let mut words = vec![model.to_owned()];
    if let Some(context) = facts["context"].as_f64() {
        words.push(format!("{} context", crate::home::compact(context)));
    }
    if let (Some(input), Some(output)) = (facts["input"].as_f64(), facts["output"].as_f64()) {
        words.push(format!("${input}/${output} per 1M"));
    }
    if !check.is_null() {
        words.push(
            if check["works"] == true {
                "works with agents"
            } else {
                "tool calls failed"
            }
            .into(),
        );
    } else if !facts.is_null() {
        words.push(
            if facts["toolCall"] == true {
                "lists tool calls"
            } else {
                "no tool calls"
            }
            .into(),
        );
    }
    words.join(" · ")
}

impl Menu {
    fn new(
        bench: &Entity<Workbench>,
        picked: Picked,
        close: Dismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search model…"));
        query.update(cx, |query, cx| query.focus(window, cx));
        let typing = cx.subscribe(&query, |menu, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                menu.chosen = 0;
                cx.notify();
            }
        });
        if !picked.locked {
            Self::read_routes(
                picked.target.clone(),
                picked.provider.clone(),
                picked.model_provider.clone(),
                cx,
            );
        }
        Self {
            bench: bench.downgrade(),
            picked,
            query,
            chosen: 0,
            routes: Vec::new(),
            close,
            _typing: typing,
        }
    }

    /// Reads the connected providers the agent can run on, asking each keyed one for its models.
    fn read_routes(
        target: core::Target,
        agent: String,
        current: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |menu, cx| {
            let Ok(providers) = target.request("modelProviders", json!({})).await else {
                return;
            };
            let mut routes = Vec::new();
            for provider in providers.as_array().into_iter().flatten() {
                let runs = provider["agents"]
                    .as_array()
                    .is_some_and(|agents| agents.iter().any(|each| each == agent.as_str()));
                if provider["connected"] != true || !runs {
                    continue;
                }
                let id = provider["id"].as_str().unwrap_or_default().to_owned();
                let models = match provider["models"].as_array() {
                    Some(models) => models.clone(),
                    None => target
                        .request("modelProviderModels", json!({ "id": id }))
                        .await
                        .ok()
                        .and_then(|served| served["models"].as_array().cloned())
                        .unwrap_or_default(),
                };
                let models: Vec<String> = models
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                if models.is_empty() && current.as_deref() != Some(id.as_str()) {
                    continue;
                }
                routes.push(Route {
                    label: provider["label"].as_str().unwrap_or_default().to_owned(),
                    models,
                    provider: provider.clone(),
                    id,
                });
            }
            let _ = menu.update(cx, |menu, cx| {
                menu.routes = routes;
                cx.notify();
            });
        })
        .detach();
    }

    fn route(&self) -> Option<&Route> {
        let current = self.picked.model_provider.as_deref()?;
        self.routes.iter().find(|route| route.id == current)
    }

    /// The models offered, those its provider serves after the agent's own, matched to the search.
    fn options(&self, cx: &App) -> Vec<ConfigChoice> {
        let listed = self
            .picked
            .model
            .as_ref()
            .map(|model| model.choices.clone())
            .unwrap_or_default();
        let served = self.route().map(|route| {
            route
                .models
                .iter()
                .filter(|model| !listed.iter().any(|choice| choice.value == **model))
                .map(|model| ConfigChoice {
                    value: model.clone(),
                    label: served_label(route, model),
                    description: None,
                })
                .collect::<Vec<_>>()
        });
        let query = self.query.read(cx).value().to_lowercase();
        listed
            .into_iter()
            .chain(served.into_iter().flatten())
            .filter(|choice| {
                format!(
                    "{} {} {}",
                    choice.label,
                    choice.value,
                    choice.description.as_deref().unwrap_or_default()
                )
                .to_lowercase()
                .contains(query.trim())
            })
            .collect()
    }

    fn pick(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        (self.close)(window, cx);
        if let Some(model) = &self.picked.model {
            if value != model.current {
                (self.picked.set)(&model.id, &value, window, cx);
            }
        }
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.options(cx).len() as isize;
        if count > 0 {
            self.chosen = (self.chosen as isize + by).rem_euclid(count) as usize;
        }
        cx.notify();
    }

    /// The agents a chat that has not started can move to, each on the account it runs on
    /// now, since the strip under the chat picks the account: the featured ones, its own, and
    /// those installed here.
    fn agents(&self, cx: &App) -> Vec<(String, Option<String>, String)> {
        let Some(bench) = self.bench.upgrade() else {
            return Vec::new();
        };
        let bench = bench.read(cx);
        let installed: Vec<String> = bench
            .shown()
            .map(|view| {
                view.workspace
                    .launchers
                    .iter()
                    .map(|launcher| launcher.provider.clone())
                    .collect()
            })
            .unwrap_or_default();
        bench
            .read("catalog")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|kind| !kind["chat"].is_null())
            .filter(|kind| {
                let provider = kind["type"].as_str().unwrap_or_default();
                kind["featured"] == true
                    || provider == self.picked.provider
                    || installed.iter().any(|each| each == provider)
            })
            .map(|kind| {
                let provider = kind["type"].as_str().unwrap_or_default().to_owned();
                let label = kind["label"].as_str().unwrap_or_default().to_owned();
                let account = bench
                    .accounts
                    .current(&provider)
                    .map(|profile| profile.id.clone());
                (provider, account, label)
            })
            .collect()
    }
}

/// A row of pressable choices over the list, as the agents and who pays.
fn chips(children: Vec<AnyElement>, t: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(4.))
        .p(px(6.))
        .border_b_1()
        .border_color(t.line)
        .children(children)
}

fn chip(
    id: SharedString,
    mark: Option<AnyElement>,
    label: String,
    pressed: bool,
    t: &Tokens,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(24.))
        .px(px(8.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .cursor_pointer()
        .text_size(crate::theme::ui_text(11.5))
        .map(|chip| match pressed {
            true => chip
                .border_color(t.border_selected)
                .bg(t.surface_chosen)
                .text_color(t.text_primary),
            false => chip
                .border_color(t.border)
                .text_color(t.text_secondary)
                .hover(|style| style.bg(t.surface_hover)),
        })
        .children(mark)
        .child(label)
        .on_click(move |_, window, cx| on(window, cx))
        .into_any_element()
}

impl Render for Menu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let options = self.options(cx);
        let current = self
            .picked
            .model
            .as_ref()
            .map(|model| model.current.clone())
            .unwrap_or_default();
        let this = cx.entity();
        let agents: Vec<AnyElement> = match self.picked.locked {
            true => Vec::new(),
            false => self
                .agents(cx)
                .into_iter()
                .map(|(provider, profile, label)| {
                    let pressed = provider == self.picked.provider;
                    let moving = self.picked.moving.clone();
                    let (close, going) = (
                        self.close.clone(),
                        Moving {
                            provider: provider.clone(),
                            profile: profile.clone(),
                            model: None,
                            model_provider: None,
                        },
                    );
                    chip(
                        SharedString::from(format!("model-agent-{provider}")),
                        Some(ui::mark(&provider, 14.)),
                        label,
                        pressed,
                        &t,
                        move |window, cx| {
                            if !pressed {
                                close(window, cx);
                                moving(going.clone(), window, cx);
                            }
                        },
                    )
                })
                .collect(),
        };
        let payers: Vec<AnyElement> = match self.picked.locked || self.routes.is_empty() {
            true => Vec::new(),
            false => {
                let account = self
                    .picked
                    .profile
                    .as_deref()
                    .and_then(|id| {
                        self.bench
                            .upgrade()?
                            .read(cx)
                            .accounts
                            .profile(id)
                            .map(|profile| profile.name.clone())
                    })
                    .unwrap_or_else(|| format!("{} account", self.picked.provider));
                let own = {
                    let (moving, close) = (self.picked.moving.clone(), self.close.clone());
                    let going = Moving {
                        provider: self.picked.provider.clone(),
                        profile: self.picked.profile.clone(),
                        model: None,
                        model_provider: None,
                    };
                    let pressed = self.picked.model_provider.is_none();
                    chip(
                        "model-paid-own".into(),
                        None,
                        account,
                        pressed,
                        &t,
                        move |window, cx| {
                            if !pressed {
                                close(window, cx);
                                moving(going.clone(), window, cx);
                            }
                        },
                    )
                };
                std::iter::once(own)
                    .chain(self.routes.iter().map(|route| {
                        let pressed =
                            self.picked.model_provider.as_deref() == Some(route.id.as_str());
                        let (moving, close) = (self.picked.moving.clone(), self.close.clone());
                        let going = Moving {
                            provider: self.picked.provider.clone(),
                            profile: self.picked.profile.clone(),
                            model: route.provider["defaults"][&self.picked.provider]
                                .as_str()
                                .map(str::to_owned),
                            model_provider: Some(route.id.clone()),
                        };
                        chip(
                            SharedString::from(format!("model-paid-{}", route.id)),
                            None,
                            route.label.clone(),
                            pressed,
                            &t,
                            move |window, cx| {
                                if !pressed {
                                    close(window, cx);
                                    moving(going.clone(), window, cx);
                                }
                            },
                        )
                    }))
                    .collect()
            }
        };
        let rows = options.iter().enumerate().map(|(at, option)| {
            let (this, value) = (this.clone(), option.value.clone());
            div()
                .id(("model-option", at))
                .flex()
                .items_center()
                .gap(px(9.))
                .py(px(7.))
                .px(px(10.))
                .rounded(theme::RADIUS_2)
                .cursor_pointer()
                .text_color(t.text_primary)
                .when(at == self.chosen, |row| row.bg(t.surface_hover))
                .when(option.value == current, |row| row.text_color(t.accent))
                .on_hover({
                    let this = this.clone();
                    move |over: &bool, _, cx| {
                        if *over {
                            this.update(cx, |menu, cx| {
                                menu.chosen = at;
                                cx.notify();
                            });
                        }
                    }
                })
                .on_click(move |_, window, cx| {
                    let value = value.clone();
                    this.update(cx, |menu, cx| menu.pick(value, window, cx))
                })
                .child(ui::mark(&self.picked.provider, 14.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(option.label.clone()),
                )
                .when(option.value == current, |row| {
                    row.child(ui::icon(Icon::Check, 14.))
                })
        });
        let empty = options.is_empty();
        let none_offered = self
            .picked
            .model
            .as_ref()
            .is_none_or(|model| model.choices.is_empty());
        div()
            .key_context(format!("{CONTEXT} Modal").as_str())
            .on_action(cx.listener(|menu, _: &Up, _, cx| menu.step(-1, cx)))
            .on_action(cx.listener(|menu, _: &Down, _, cx| menu.step(1, cx)))
            .on_action(cx.listener(|menu, _: &Pick, window, cx| {
                if let Some(option) = menu.options(cx).get(menu.chosen).cloned() {
                    menu.pick(option.value, window, cx);
                }
            }))
            .on_action(cx.listener(|menu, _: &Close, window, cx| (menu.close)(window, cx)))
            .w(px(340.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.surface_overlay)
            .border_1()
            .border_color(t.border)
            .rounded(theme::RADIUS_3)
            .shadow(t.shadow_float())
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(t.text_primary)
                    .child(Input::new(&self.query).appearance(false).bordered(false)),
            )
            .when(!agents.is_empty(), |menu| menu.child(chips(agents, &t)))
            .when(!payers.is_empty(), |menu| menu.child(chips(payers, &t)))
            .child(
                div()
                    .id("model-options")
                    .p(px(5.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px(px(10.))
                                .py(px(8.))
                                .text_size(crate::theme::ui_text(12.))
                                .text_color(t.text_tertiary)
                                .child(match (none_offered, self.picked.pending) {
                                    (true, Some(pending)) => pending,
                                    (true, None) => "This agent offers no models to pick",
                                    (false, _) => "No matches",
                                }),
                        )
                    }),
            )
    }
}

/// Wraps `trigger`, the chat's model, in the menu it opens; `picked` says what the menu shows
/// and what choosing in it does.
pub fn around(
    trigger: impl Selectable + IntoElement + 'static,
    key: &str,
    picked: Picked,
    this: &Entity<Workbench>,
) -> Popover {
    let this = this.clone();
    let key = SharedString::from(format!("model-picker-{key}"));
    Popover::new(key.clone())
        .anchor(Anchor::BottomLeft)
        .offset(px(12.))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let popover = cx.entity();
            let close: Dismiss = Rc::new(move |window, cx| {
                popover.update(cx, |popover, cx| popover.dismiss(window, cx));
            });
            let shown = picked.clone();
            let menu = window.use_keyed_state(key, cx, |window, cx| {
                Menu::new(&this, picked, close, window, cx)
            });
            menu.update(cx, |menu, _| menu.picked = shown);
            menu
        })
}
