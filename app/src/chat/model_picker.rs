//! A chat's model menu: the models its agent offers, searched from its own field, under its
//! maker's name, then those of every connected provider the agent can run. Before the chat
//! starts, the agents above the list move it to another agent, and a provider's model moves it
//! onto that provider.

use std::rc::Rc;

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, Anchor, AnyElement, App, AppContext, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::ConfigPicker;

use crate::agents::setup::Installing;
use crate::core;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_XS,
    TEXT_XS_H,
};
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

/// A connected provider, with the models it serves and the agents that can run it.
#[derive(Clone)]
struct Route {
    id: String,
    label: String,
    local: bool,
    models: Vec<String>,
    agents: Vec<String>,
    provider: Value,
}

impl Route {
    fn runs(&self, agent: &str) -> bool {
        self.agents.iter().any(|each| each == agent)
    }

    /// `model` as the agent names it once a chat runs on this provider.
    fn named(&self, model: &str, current: &str) -> bool {
        current == model || current == format!("sikemux-{}/{model}", self.id)
    }
}

/// What picking a row does.
#[derive(Clone)]
enum Act {
    /// Sets the model of the chat's own agent.
    Set(String),
    Move(Moving),
    AddProvider,
}

/// A row of the list.
#[derive(Clone)]
struct Row {
    group: usize,
    mark: Option<String>,
    label: String,
    meta: Option<String>,
    current: bool,
    act: Act,
}

/// An agent in the chip row: whether this host has it, and how its install is going.
struct Agent {
    provider: String,
    profile: Option<String>,
    label: String,
    installed: bool,
    installing: Option<Installing>,
}

/// The open menu: its search, the row the keys are on, and the connected providers.
pub struct Menu {
    bench: WeakEntity<Workbench>,
    picked: Picked,
    query: Entity<InputState>,
    chosen: usize,
    routes: Vec<Route>,
    close: Dismiss,
    scroll: ScrollHandle,
    /// The keys moved the chosen row, so the list scrolls it into view when it next draws.
    follow: bool,
    _typing: Subscription,
    _bench: Subscription,
}

/// Closes the popover the menu is in.
type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

/// Context, price per million tokens and tool calls, in a few words, as Settings words them.
fn facts(route: &Route, model: &str) -> Vec<String> {
    let name = model.strip_suffix("[1m]").unwrap_or(model);
    let facts = &route.provider["facts"][name];
    let check = &route.provider["checks"][name];
    let mut words = Vec::new();
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
    words
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
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
                menu.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
                cx.notify();
            }
        });
        Self::read_routes(picked.target.clone(), picked.model_provider.clone(), cx);
        Self {
            bench: bench.downgrade(),
            picked,
            query,
            chosen: 0,
            routes: Vec::new(),
            close,
            scroll: ScrollHandle::new(),
            follow: false,
            _typing: typing,
            _bench: cx.observe(bench, |_, _, cx| cx.notify()),
        }
    }

    /// Reads the connected providers, asking each that does not list its models for them.
    fn read_routes(target: core::Target, current: Option<String>, cx: &mut Context<Self>) {
        cx.spawn(async move |menu, cx| {
            let Ok(providers) = target.request("modelProviders", json!({})).await else {
                return;
            };
            let mut routes = Vec::new();
            for provider in providers.as_array().into_iter().flatten() {
                if provider["connected"] != true {
                    continue;
                }
                let id = provider["id"].as_str().unwrap_or_default().to_owned();
                let models = match provider["models"].as_array() {
                    Some(models) => Value::Array(models.clone()),
                    None => target
                        .request("modelProviderModels", json!({ "id": id }))
                        .await
                        .map(|served| served["models"].clone())
                        .unwrap_or_default(),
                };
                let models = strings(&models);
                if models.is_empty() && current.as_deref() != Some(id.as_str()) {
                    continue;
                }
                routes.push(Route {
                    label: provider["label"].as_str().unwrap_or_default().to_owned(),
                    local: provider["kind"] == "local",
                    models,
                    agents: strings(&provider["agents"]),
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

    /// The group headings and every row under them, matched to the search: the agent's own
    /// models under its maker, then those of each connected provider it runs, then the row that
    /// adds one.
    fn options(&self, cx: &App) -> (Vec<String>, Vec<Row>) {
        let Some(bench) = self.bench.upgrade() else {
            return (Vec::new(), Vec::new());
        };
        let bench = bench.read(cx);
        let own = self.picked.provider.as_str();
        let current = self
            .picked
            .model
            .as_ref()
            .map(|model| model.current.clone())
            .unwrap_or_default();
        let listed = self
            .picked
            .model
            .as_ref()
            .map(|model| model.choices.clone())
            .unwrap_or_default();
        let on_route = self.picked.model_provider.is_some();
        let paying = self
            .routes
            .iter()
            .find(|route| self.picked.model_provider.as_deref() == Some(route.id.as_str()))
            .filter(|_| self.picked.locked)
            .map(|route| route.label.clone());
        let maker = bench
            .reads
            .catalog
            .kind(own)
            .and_then(|kind| kind.maker.clone())
            .unwrap_or_else(|| bench.agent_name(own).to_owned());
        let mut groups = vec![paying.unwrap_or(maker)];
        let mut options: Vec<Row> = Vec::new();
        if self.picked.locked || !on_route {
            options.extend(
                listed
                    .iter()
                    .filter(|choice| self.picked.locked || !choice.value.starts_with("sikemux-"))
                    .map(|choice| Row {
                        group: 0,
                        mark: Some(own.to_owned()),
                        label: choice.label.clone(),
                        meta: choice.is_default.then(|| "Default".to_owned()),
                        current: choice.value == current,
                        act: Act::Set(choice.value.clone()),
                    }),
            );
        } else {
            let offered = bench
                .launchers()
                .iter()
                .find(|launcher| launcher.provider == own)
                .and_then(|launcher| launcher.model.clone())
                .map(|model| model.choices)
                .unwrap_or_default()
                .into_iter()
                .filter(|choice| !choice.value.starts_with("sikemux-"))
                .collect::<Vec<_>>();
            let going = |model: Option<String>| Moving {
                provider: own.to_owned(),
                profile: self.picked.profile.clone(),
                model,
                model_provider: None,
            };
            match offered.is_empty() {
                true => options.push(Row {
                    group: 0,
                    mark: Some(own.to_owned()),
                    label: "Default model".into(),
                    meta: None,
                    current: false,
                    act: Act::Move(going(None)),
                }),
                false => options.extend(offered.iter().map(|choice| Row {
                    group: 0,
                    mark: Some(own.to_owned()),
                    label: choice.label.clone(),
                    meta: choice.is_default.then(|| "Default".to_owned()),
                    current: false,
                    act: Act::Move(going(Some(choice.value.clone()))),
                })),
            }
        }
        if !self.picked.locked {
            for route in self.routes.iter().filter(|route| route.runs(own)) {
                let here = self.picked.model_provider.as_deref() == Some(route.id.as_str());
                groups.push(match route.local && !route.label.starts_with("Local") {
                    true => format!("{} · local", route.label),
                    false => route.label.clone(),
                });
                let group = groups.len() - 1;
                options.extend(route.models.iter().map(|model| {
                    let chosen = here && route.named(model, &current);
                    let set = here
                        .then(|| {
                            listed
                                .iter()
                                .find(|choice| route.named(model, &choice.value))
                        })
                        .flatten();
                    let act = match set {
                        Some(choice) => Act::Set(choice.value.clone()),
                        None => Act::Move(Moving {
                            provider: own.to_owned(),
                            profile: self.picked.profile.clone(),
                            model: Some(model.clone()),
                            model_provider: Some(route.id.clone()),
                        }),
                    };
                    let meta = facts(route, model);
                    Row {
                        group,
                        mark: Some(own.to_owned()),
                        label: model.clone(),
                        meta: (!meta.is_empty()).then(|| meta.join(" · ")),
                        current: chosen,
                        act,
                    }
                }));
            }
            groups.push(String::new());
            options.push(Row {
                group: groups.len() - 1,
                mark: None,
                label: "Add provider".into(),
                meta: None,
                current: false,
                act: Act::AddProvider,
            });
        }
        let query = self.query.read(cx).value().trim().to_lowercase();
        options.retain(|option| {
            matches!(option.act, Act::AddProvider)
                || format!(
                    "{} {} {}",
                    option.label,
                    option.meta.as_deref().unwrap_or_default(),
                    groups[option.group]
                )
                .to_lowercase()
                .contains(&query)
        });
        (groups, options)
    }

    fn pick(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        (self.close)(window, cx);
        match act {
            Act::Set(value) => {
                if let Some(model) = &self.picked.model {
                    if value != model.current {
                        (self.picked.set)(&model.id, &value, window, cx);
                    }
                }
            }
            Act::Move(going) => (self.picked.moving)(going, window, cx),
            Act::AddProvider => {
                let chat = (!self.picked.locked).then(|| self.picked.moving.clone());
                if let Some(bench) = self.bench.upgrade() {
                    bench.update(cx, |bench, cx| bench.add_provider(chat, window, cx));
                }
            }
        }
    }

    /// Whether the chat runs on this host, where an agent it moves to can be installed.
    fn here(&self) -> bool {
        matches!(self.picked.target.place(), crate::hosts::Place::This)
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.options(cx).1.len() as isize;
        if count > 0 {
            self.chosen = (self.chosen as isize + by).rem_euclid(count) as usize;
            self.follow = true;
        }
        cx.notify();
    }

    /// The agents a chat that has not started can move to, each on the account it runs on now,
    /// since the strip under the chat picks the account: every chat agent this host has or can
    /// install, and its own.
    fn agents(&self, cx: &App) -> Vec<Agent> {
        let Some(bench) = self.bench.upgrade() else {
            return Vec::new();
        };
        let bench = bench.read(cx);
        bench
            .reads
            .catalog
            .agents()
            .filter(|kind| kind.chat.is_some())
            .filter_map(|kind| {
                let provider = kind.kind.clone();
                let installed = provider == self.picked.provider || bench.has_agent(&provider);
                if !installed && kind.install.is_none() {
                    return None;
                }
                Some(Agent {
                    profile: bench
                        .accounts
                        .current(&provider)
                        .map(|profile| profile.id.clone()),
                    label: kind.label.clone(),
                    installing: bench.agent_setup.installing(&provider),
                    installed,
                    provider,
                })
            })
            .collect()
    }
}

fn install_agent(bench: &WeakEntity<Workbench>, provider: &str, cx: &mut App) {
    if let Some(bench) = bench.upgrade() {
        bench.update(cx, |bench, cx| {
            crate::agents::setup::install_behind(bench, provider, cx)
        });
    }
}

fn chip(
    id: SharedString,
    agent: &Agent,
    pressed: bool,
    t: &Tokens,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let state = match agent.installing {
        Some(Installing::Running) => Some(("Installing", t.text_tertiary)),
        Some(Installing::Failed) => Some(("Install failed", t.danger)),
        None => None,
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(24.))
        .px(SPACE_2)
        .rounded(theme::RADIUS_2)
        .border_1()
        .cursor_pointer()
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .map(|chip| match pressed {
            true => chip
                .border_color(gpui::transparent_black())
                .bg(t.surface_chosen)
                .text_color(t.text_primary),
            false => chip
                .border_color(t.border)
                .text_color(t.text_secondary)
                .hover(|style| style.bg(t.surface_hover)),
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .when(!agent.installed, |label| label.opacity(0.55))
                .child(ui::mark(&agent.provider, 14.))
                .child(agent.label.clone()),
        )
        .children(state.map(|(words, ink)| div().text_color(ink).child(words)))
        .on_click(move |_, window, cx| on(window, cx))
        .into_any_element()
}

impl Menu {
    fn row(
        &self,
        at: usize,
        row: Row,
        typed: &str,
        found: gpui::HighlightStyle,
        t: &Tokens,
        this: &Entity<Self>,
    ) -> AnyElement {
        let (menu, act) = (this.clone(), row.act.clone());
        let adds = matches!(row.act, Act::AddProvider);
        div()
            .id(("model-option", at))
            .flex()
            .items_center()
            .gap(px(9.))
            .py(px(7.))
            .px(SPACE_2H)
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_color(if adds {
                t.text_secondary
            } else {
                t.text_primary
            })
            .when(at == self.chosen, |row| row.bg(t.surface_hover))
            .on_hover({
                let menu = menu.clone();
                move |over: &bool, _, cx| {
                    if *over {
                        menu.update(cx, |menu, cx| {
                            menu.chosen = at;
                            cx.notify();
                        });
                    }
                }
            })
            .on_click(move |_, window, cx| {
                let act = act.clone();
                menu.update(cx, |menu, cx| menu.pick(act, window, cx))
            })
            .child(match &row.mark {
                Some(mark) => ui::mark(mark, 14.),
                None => ui::mark_box(14., ui::icon(Icon::Plus, 13.).text_color(t.text_tertiary))
                    .into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .child(ui::found_with(row.label.clone(), typed, found)),
            )
            .children(row.meta.map(|meta| {
                div()
                    .min_w_0()
                    .max_w(px(170.))
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_tertiary)
                    .child(meta)
            }))
            .when(row.current, |row| {
                row.child(ui::icon(Icon::Check, 14.).text_color(t.text_secondary))
            })
            .into_any_element()
    }
}

impl Render for Menu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (groups, options) = self.options(cx);
        let (typed, found) = (
            self.query.read(cx).value().trim().to_owned(),
            ui::found_mark(cx),
        );
        let this = cx.entity();
        let agents: Vec<AnyElement> = match self.picked.locked {
            true => Vec::new(),
            false => self
                .agents(cx)
                .into_iter()
                .map(|agent| {
                    let pressed = agent.provider == self.picked.provider;
                    let (moving, close, menu) =
                        (self.picked.moving.clone(), self.close.clone(), this.clone());
                    let going = Moving {
                        provider: agent.provider.clone(),
                        profile: agent.profile.clone(),
                        model: None,
                        model_provider: None,
                    };
                    let (installed, installing) = (agent.installed, agent.installing);
                    chip(
                        SharedString::from(format!("model-agent-{}", agent.provider)),
                        &agent,
                        pressed,
                        &t,
                        move |window, cx| {
                            let provider = going.provider.clone();
                            match (installed, installing) {
                                (true, _) if !pressed => {
                                    close(window, cx);
                                    moving(going.clone(), window, cx);
                                }
                                (true, _) => {}
                                (false, Some(Installing::Failed)) => {
                                    close(window, cx);
                                    let bench = menu.read(cx).bench.upgrade();
                                    if let Some(bench) = bench {
                                        bench.update(cx, |bench, cx| {
                                            crate::agents::setup::show_install(
                                                bench, &provider, window, cx,
                                            )
                                        });
                                    }
                                }
                                (false, _) if menu.read(cx).here() => {
                                    let bench = menu.read(cx).bench.clone();
                                    install_agent(&bench, &provider, cx)
                                }
                                (false, _) => {}
                            }
                        },
                    )
                })
                .collect(),
        };
        let empty_own = options.iter().all(|row| row.group != 0);
        let note = match (typed.is_empty(), self.picked.pending) {
            (false, _)
                if options
                    .iter()
                    .all(|row| matches!(row.act, Act::AddProvider)) =>
            {
                Some("No matches")
            }
            (true, _) if !empty_own || self.picked.model_provider.is_some() => None,
            (true, Some(pending)) => Some(pending),
            (true, None) => Some("This agent offers no models to pick"),
            (false, _) => None,
        };
        let footer = options
            .iter()
            .cloned()
            .enumerate()
            .find(|(_, row)| matches!(row.act, Act::AddProvider))
            .map(|(at, row)| self.row(at, row, &typed, found, &t, &this));
        let mut list: Vec<AnyElement> = Vec::new();
        let mut chosen_at = None;
        for (group, heading) in groups.iter().enumerate() {
            let rows: Vec<(usize, Row)> = options
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, row)| row.group == group && !matches!(row.act, Act::AddProvider))
                .collect();
            let noted = group == 0 && note.is_some();
            if rows.is_empty() && !noted {
                continue;
            }
            list.push(match heading.is_empty() {
                true => div()
                    .h(px(1.))
                    .mx(px(3.))
                    .my(px(5.))
                    .bg(t.line)
                    .into_any_element(),
                false => ui::text(heading.to_uppercase(), TEXT_3XS, FontWeight::MEDIUM)
                    .letter_spacing(0.08)
                    .text_color(t.text_tertiary)
                    .px(SPACE_2H)
                    .pt(px(if list.is_empty() { 4. } else { 9. }))
                    .pb(px(3.))
                    .into_any_element(),
            });
            if let Some(note) = note.filter(|_| noted) {
                list.push(
                    div()
                        .px(SPACE_2H)
                        .py(SPACE_1H)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(note)
                        .into_any_element(),
                );
            }
            for (at, row) in rows {
                if at == self.chosen {
                    chosen_at = Some(list.len());
                }
                list.push(self.row(at, row, &typed, found, &t, &this));
            }
        }
        if let Some(at) = chosen_at.filter(|_| std::mem::take(&mut self.follow)) {
            self.scroll.scroll_to_item(at);
        }
        div()
            .key_context(format!("{CONTEXT} Modal").as_str())
            .on_action(cx.listener(|menu, _: &Up, _, cx| menu.step(-1, cx)))
            .on_action(cx.listener(|menu, _: &Down, _, cx| menu.step(1, cx)))
            .on_action(cx.listener(|menu, _: &Pick, window, cx| {
                if let Some(option) = menu.options(cx).1.get(menu.chosen).cloned() {
                    menu.pick(option.act, window, cx);
                }
            }))
            .on_action(cx.listener(|menu, _: &Close, window, cx| (menu.close)(window, cx)))
            .w(px(360.))
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
                    .px(px(ui::PANEL_INSET + 10.))
                    .py(SPACE_2H)
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_primary)
                    .child(Input::new(&self.query).appearance(false).bordered(false)),
            )
            .when(!agents.is_empty(), |menu| {
                menu.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(SPACE_1)
                        .p(px(ui::PANEL_INSET + 2.))
                        .border_b_1()
                        .border_color(t.line)
                        .children(agents),
                )
            })
            .child(
                div()
                    .id("model-options")
                    .p(px(ui::PANEL_INSET))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(list),
            )
            .children(footer.map(|footer| {
                div()
                    .p(px(ui::PANEL_INSET))
                    .border_t_1()
                    .border_color(t.line)
                    .child(footer)
            }))
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
