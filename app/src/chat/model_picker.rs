//! A chat's model menu: the models its agent offers, searched from its own field, under its
//! maker's name, then those of every connected provider the agent can run. The agents above the
//! list move the chat to another agent, handing over what it said once it has begun, and before
//! it starts a provider's model moves it onto that provider.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, Anchor, AnyElement, App, AppContext, Context, Entity, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use sikemux_view::workspace::{ConfigChoice, ConfigPicker};

use crate::agents::setup::Installing;
use crate::core;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui;
use crate::ui::list_picker::{self, Choose, Close, Dismiss, Next, Previous};
use crate::workbench::Workbench;
use sikemux_agents::launch::providers::Kind;
use sikemux_calls::call;
use sikemux_calls::model_providers::ModelProvider;
use sikemux_calls::model_providers::ProviderParams;
use sikemux_calls::params::NoParams;

const CONTEXT: &str = "ModelPicker";

actions!(model_picker, [LessEffort, MoreEffort]);

/// Picks the model at this place in the list, from ⌘1 to ⌘4.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(name = "model_picker.pickNth", no_json)]
struct PickNth(usize);

/// How many of the first models take a number key.
const NUMBERED: usize = 4;

/// Where a column's words start: the search field and the effort's header line up with the rows'
/// marks, which sit inside the list's inset and their own padding.
const ROW_INSET: f32 = ui::PANEL_INSET + 10.;

pub fn install(cx: &mut App) {
    let context = Some("ModelPicker > Input");
    list_picker::bind("ModelPicker > Input", list_picker::Tab::Nothing, cx);
    let numbers = if cfg!(target_os = "macos") {
        ["cmd-1", "cmd-2", "cmd-3", "cmd-4"]
    } else {
        ["ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4"]
    };
    cx.bind_keys([
        KeyBinding::new("left", LessEffort, context),
        KeyBinding::new("right", MoreEffort, context),
    ]);
    cx.bind_keys(
        numbers
            .into_iter()
            .enumerate()
            .map(|(at, keys)| KeyBinding::new(keys, PickNth(at), context)),
    );
}

/// What a number key is drawn as beside the model it picks.
fn numbered(at: usize) -> String {
    match cfg!(target_os = "macos") {
        true => format!("⌘{}", at + 1),
        false => format!("Ctrl+{}", at + 1),
    }
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
    /// How hard the model thinks, when its agent says.
    pub effort: Option<ConfigPicker>,
    /// Standard speed or the agent's faster tiers for the model, while the session runs one.
    pub speed: Option<ConfigPicker>,
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
    provider: Rc<ModelProvider>,
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
    /// Shows or folds away the older releases of the agent's models.
    Fold,
    /// Updates the chat's agent on its host, for the models its newest release brings.
    Update,
}

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
    /// The older releases of the agent's models show, which start folded away.
    older: bool,
    /// The effort just picked, shown as current until the session says it took.
    asked_effort: Option<String>,
    /// The agent the search field names.
    named: Option<String>,
    _typing: Subscription,
    _bench: Subscription,
}

/// Context, price per million tokens and tool calls, in a few words, as Settings words them.
fn facts(route: &Route, model: &str) -> Vec<String> {
    let name = model.strip_suffix("[1m]").unwrap_or(model);
    let facts = route.provider.choices.facts.get(name);
    let mut words = Vec::new();
    if let Some(facts) = facts {
        let context = facts.context as f64;
        words.push(t!(
            "{size} context",
            size = crate::ui::numbers::compact(context)
        ));
        if let (Some(input), Some(output)) = (facts.input, facts.output) {
            words.push(t!(
                "${input}/${output} per 1M",
                input = input,
                output = output
            ));
        }
    }
    if let Some(check) = route.provider.checks.get(name) {
        words.push(
            if check.works {
                t!("works with agents")
            } else {
                t!("tool calls failed")
            }
            .into(),
        );
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

impl Menu {
    fn new(
        bench: &Entity<Workbench>,
        picked: Picked,
        close: Dismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        query.update(cx, |query, cx| query.focus(window, cx));
        let typing = cx.subscribe(&query, |menu, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                menu.chosen = 0;
                menu.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
                cx.notify();
            }
        });
        Self::read_routes(picked.target.clone(), picked.model_provider.clone(), cx);
        if !bench.read(cx).agent_setup.knows(picked.target.place()) {
            bench.update(cx, |bench, cx| crate::agents::setup::read(bench, cx));
        }
        Self {
            bench: bench.downgrade(),
            picked,
            query,
            chosen: 0,
            routes: Vec::new(),
            close,
            scroll: ScrollHandle::new(),
            follow: false,
            older: false,
            asked_effort: None,
            named: None,
            _typing: typing,
            _bench: cx.observe(bench, |_, _, cx| cx.notify()),
        }
    }

    /// Reads the connected providers, asking each that does not list its models for them.
    fn read_routes(target: core::Target, current: Option<String>, cx: &mut Context<Self>) {
        cx.spawn(async move |menu, cx| {
            let Ok(providers) = target
                .request_typed::<call::ModelProviders>(NoParams {})
                .await
            else {
                return;
            };
            let mut routes = Vec::new();
            for provider in providers {
                if !provider.connected {
                    continue;
                }
                let id = provider.id.clone();
                let models = match &provider.models {
                    Some(models) => models.clone(),
                    None => target
                        .request_typed::<call::ModelProviderModels>(ProviderParams {
                            id: id.clone(),
                        })
                        .await
                        .map(|served| served.models)
                        .unwrap_or_default(),
                };
                if models.is_empty() && current.as_deref() != Some(id.as_str()) {
                    continue;
                }
                routes.push(Route {
                    label: provider.label.clone(),
                    local: provider.kind == Kind::Local,
                    models,
                    agents: provider.agents.clone(),
                    provider: Rc::new(provider),
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
        let query = self.query.read(cx).value().trim().to_lowercase();
        let unfolded = self.older || !query.is_empty();
        let mut folded = false;
        let mut shows = |choice: &ConfigChoice| {
            let hidden = choice.superseded && choice.value != current;
            folded |= hidden;
            unfolded || !hidden
        };
        if self.picked.locked || !on_route {
            options.extend(
                listed
                    .iter()
                    .filter(|choice| self.picked.locked || !choice.value.starts_with("sikemux-"))
                    .filter(|choice| shows(choice))
                    .map(|choice| Row {
                        group: 0,
                        mark: Some(own.to_owned()),
                        label: choice.label.clone(),
                        meta: choice.is_default.then(|| t!("Default").to_owned()),
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
                .filter(|choice| shows(choice))
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
                    label: t!("Default model").into(),
                    meta: None,
                    current: false,
                    act: Act::Move(going(None)),
                }),
                false => options.extend(offered.iter().map(|choice| Row {
                    group: 0,
                    mark: Some(own.to_owned()),
                    label: choice.label.clone(),
                    meta: choice.is_default.then(|| t!("Default").to_owned()),
                    current: false,
                    act: Act::Move(going(Some(choice.value.clone()))),
                })),
            }
        }
        if let Some(update) = self.update_row(bench) {
            options.push(update);
        }
        if folded && query.is_empty() {
            options.push(Row {
                group: 0,
                mark: None,
                label: if self.older {
                    t!("Show less")
                } else {
                    t!("Show more")
                }
                .into(),
                meta: None,
                current: false,
                act: Act::Fold,
            });
        }
        if !self.picked.locked {
            for route in self.routes.iter().filter(|route| route.runs(own)) {
                let here = self.picked.model_provider.as_deref() == Some(route.id.as_str());
                groups.push(match route.local && !route.label.starts_with("Local") {
                    true => t!("{route} · local", route = route.label),
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
                label: t!("Add provider").into(),
                meta: None,
                current: false,
                act: Act::AddProvider,
            });
        }
        options.retain(|option| {
            matches!(option.act, Act::AddProvider | Act::Fold | Act::Update)
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
        if matches!(act, Act::Fold) {
            self.older = !self.older;
            cx.notify();
            return;
        }
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
            Act::Fold => {}
            Act::Update => {
                let (place, kind) = (
                    self.picked.target.place().clone(),
                    self.picked.provider.clone(),
                );
                if let Some(bench) = self.bench.upgrade() {
                    bench.update(cx, |bench, cx| {
                        crate::agents::setup::update(bench, place, vec![kind], cx)
                    });
                }
            }
            Act::AddProvider => {
                let chat = (!self.picked.locked).then(|| self.picked.moving.clone());
                if let Some(bench) = self.bench.upgrade() {
                    bench.update(cx, |bench, cx| bench.add_provider(chat, window, cx));
                }
            }
        }
    }

    /// "Update Claude to use Fable 5.2", while the chat's agent on its host has a newer release
    /// that brings models, or is updating to one.
    fn update_row(&self, bench: &Workbench) -> Option<Row> {
        let agent = bench
            .agent_setup
            .on(self.picked.target.place())?
            .iter()
            .find(|agent| agent.kind == self.picked.provider)?;
        let label = match agent.updating_now() {
            true => t!("Updating {agent}…", agent = agent.label),
            false if agent.unlocks.is_empty() || agent.update_to().is_none() => return None,
            false => t!(
                "Update {agent} to use {models}",
                agent = agent.label,
                models = agent.unlocks.join(", ")
            ),
        };
        Some(Row {
            group: 0,
            mark: None,
            label,
            meta: agent.latest.clone(),
            current: false,
            act: Act::Update,
        })
    }

    /// The switch between the model's usual speed and the faster tiers its agent offers.
    fn speed(&self, t: &Tokens) -> Option<AnyElement> {
        let speed = self.picked.speed.as_ref()?;
        let chosen = speed
            .choices
            .iter()
            .position(|choice| choice.value == speed.current)
            .unwrap_or_default();
        let words = speed
            .choices
            .iter()
            .map(|choice| (SharedString::from(choice.label.clone()), None))
            .collect();
        let mut switch = ui::Segmented::words("model-speed", words, chosen);
        for (at, choice) in speed.choices.iter().enumerate() {
            if choice.label.to_lowercase().contains("fast") {
                switch = switch.glyph(at, Icon::Bolt);
            }
        }
        let (set, picker) = (self.picked.set.clone(), speed.clone());
        let switch = switch.on_select(move |at, window, cx| {
            if let Some(choice) = picker
                .choices
                .get(*at)
                .filter(|choice| choice.value != picker.current)
            {
                set(&picker.id, &choice.value, window, cx);
            }
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_secondary)
                        .child(t!("Speed")),
                )
                .child(switch)
                .into_any_element(),
        )
    }

    /// Whether the chat runs on this host, where an agent it moves to can be installed.
    fn here(&self) -> bool {
        matches!(self.picked.target.place(), crate::hosts::Place::This)
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.options(cx).1.len();
        if count > 0 {
            self.chosen = list_picker::stepped(self.chosen, by, count);
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

/// An agent in the rail down the menu's left: its mark, lit while the chat runs on it, and its
/// name and how its install goes on hover.
fn agent_button(
    agent: &Agent,
    pressed: bool,
    t: &Tokens,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let tip = match agent.installing {
        Some(Installing::Running) => t!("{agent} · Installing", agent = agent.label),
        Some(Installing::Failed) => t!("{agent} · Install failed", agent = agent.label),
        None => agent.label.clone(),
    };
    div()
        .id(SharedString::from(format!(
            "model-agent-{}",
            agent.provider
        )))
        .relative()
        .flex_none()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .map(|button| match pressed {
            true => button.bg(t.surface_chosen),
            false => button.hover(|style| style.bg(t.surface_hover)),
        })
        .child(
            div()
                .when(!agent.installed, |mark| mark.opacity(0.55))
                .when(agent.installing == Some(Installing::Running), |mark| {
                    mark.opacity(0.3)
                })
                .child(ui::mark(&agent.provider, 16.)),
        )
        .when(agent.installing == Some(Installing::Failed), |button| {
            button.child(
                div()
                    .absolute()
                    .top(px(4.))
                    .right(px(4.))
                    .size(px(6.))
                    .rounded_full()
                    .bg(t.danger),
            )
        })
        .tooltip(ui::tooltip(tip))
        .clicks(move |_, window, cx| on(window, cx))
        .into_any_element()
}

/// Five bars rising left to right, `lit` of them in the accent, as each step of effort shows.
fn strength(lit: usize, t: &Tokens) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_end()
        .gap(px(1.5))
        .h(px(11.))
        .children((0..5).map(|bar| {
            div()
                .w(px(2.5))
                .h(px(3. + 2. * bar as f32))
                .rounded(px(1.))
                .bg(if bar < lit { t.accent } else { t.border_strong })
        }))
}

impl Menu {
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        at: usize,
        row: Row,
        nth: Option<usize>,
        typed: &str,
        found: gpui::HighlightStyle,
        this: &Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let (menu, act) = (this.clone(), row.act.clone());
        let adds = matches!(row.act, Act::AddProvider | Act::Fold | Act::Update);
        let key = nth.map(|nth| ui::keycap(numbered(nth), cx));
        div()
            .id(("model-option", at))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(30.))
            .px(SPACE_2H)
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_color(if adds {
                t.text_secondary
            } else {
                t.text_primary
            })
            .map(|line| match (row.current, at == self.chosen) {
                (true, _) => line.bg(t.surface_chosen),
                (false, true) => line.bg(t.surface_hover),
                (false, false) => line,
            })
            .map(|row| list_picker::hover_chooses(row, at, &menu, |list, at| list.chosen = at))
            .clicks(move |_, window, cx| {
                let act = act.clone();
                menu.update(cx, |menu, cx| menu.pick(act, window, cx))
            })
            .child(match &row.mark {
                Some(mark) => ui::mark(mark, 14.),
                None => ui::mark_box(
                    14.,
                    ui::icon(
                        match row.act {
                            Act::Fold => Icon::Chevron,
                            Act::Update => Icon::Download,
                            _ => Icon::Plus,
                        },
                        13.,
                    )
                    .text_color(t.text_tertiary),
                )
                .into_any_element(),
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .child(ui::found_with(row.label.clone(), typed, found)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_tertiary)
                    .children(row.meta),
            )
            .when(row.current, |row| {
                row.child(ui::icon(Icon::Check, 14.).text_color(t.accent))
            })
            .children(key)
            .into_any_element()
    }
}

/// Where the rows that pick a model sit among all the rows, the first of them taking ⌘1 to ⌘4.
fn models(options: &[Row]) -> Vec<usize> {
    options
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row.act, Act::Set(_) | Act::Move(_)))
        .map(|(at, _)| at)
        .take(NUMBERED)
        .collect()
}

impl Menu {
    /// The effort shown as current: the one just picked until the session takes it.
    fn effort_now(&self) -> Option<String> {
        let effort = self.picked.effort.as_ref()?;
        Some(
            self.asked_effort
                .clone()
                .filter(|asked| *asked != effort.current)
                .unwrap_or_else(|| effort.current.clone()),
        )
    }

    fn set_effort(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(effort) = self.picked.effort.clone() else {
            return;
        };
        if Some(&value) == self.effort_now().as_ref() {
            return;
        }
        (self.picked.set)(&effort.id, &value, window, cx);
        self.asked_effort = Some(value);
        cx.notify();
    }

    /// Steps the effort one choice up or down, from the arrow keys while nothing is typed.
    fn step_effort(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.query.read(cx).value().is_empty() {
            cx.propagate();
            return;
        }
        let Some(effort) = self.picked.effort.clone() else {
            return;
        };
        let now = self.effort_now();
        let at = effort
            .choices
            .iter()
            .position(|choice| Some(&choice.value) == now.as_ref());
        let to = match at {
            Some(at) => at
                .checked_add_signed(by)
                .filter(|to| *to < effort.choices.len()),
            None => (!effort.choices.is_empty()).then_some(0),
        };
        if let Some(choice) = to.and_then(|to| effort.choices.get(to)) {
            self.set_effort(choice.value.clone(), window, cx);
        }
    }

    /// The column of efforts the model can think at, each led by its strength bars.
    fn effort(&self, this: &Entity<Self>, cx: &App) -> AnyElement {
        let t = Tokens::get(cx);
        let model = self
            .picked
            .model
            .as_ref()
            .filter(|model| !model.current.is_empty())
            .map(|model| crate::chat::composer::chosen(model, t!("Model")));
        let header = div()
            .flex_none()
            .h(px(38.))
            .px(px(ROW_INSET))
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .border_b_1()
            .border_color(t.line)
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .text_color(t.text_secondary)
            .child(ui::icon(Icon::Gauge, 14.).text_color(t.text_tertiary))
            .child(t!("Effort"))
            .child(div().flex_1())
            .children(model.map(|model| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_tertiary)
                    .child(model)
            }));
        let choices = self
            .picked
            .effort
            .as_ref()
            .map(|effort| effort.choices.clone())
            .unwrap_or_default();
        let now = self.effort_now();
        let steps = choices.len();
        let body: Vec<AnyElement> = match steps {
            0 => vec![div()
                .px(SPACE_2H)
                .py(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(
                    match self.picked.pending.filter(|_| self.picked.effort.is_some()) {
                        Some(_) => t!("Effort shows once the agent starts"),
                        None => t!("This model has no effort to pick"),
                    },
                )
                .into_any_element()],
            _ => choices
                .into_iter()
                .enumerate()
                .map(|(at, choice)| {
                    let current = Some(&choice.value) == now.as_ref();
                    let lit = ((at + 1) * 5).div_ceil(steps);
                    let (menu, value) = (this.clone(), choice.value.clone());
                    div()
                        .id(("model-effort", at))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .h(px(30.))
                        .px(SPACE_2H)
                        .rounded(theme::RADIUS_2)
                        .cursor_pointer()
                        .text_size(crate::theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_primary)
                        .map(|row| match current {
                            true => row.bg(t.surface_chosen),
                            false => row.hover(|style| style.bg(t.surface_hover)),
                        })
                        .clicks(move |_, window, cx| {
                            let value = value.clone();
                            menu.update(cx, |menu, cx| menu.set_effort(value, window, cx))
                        })
                        .child(strength(lit, t))
                        .child(div().flex_1().min_w_0().truncate().child(choice.label))
                        .when(current, |row| {
                            row.child(ui::icon(Icon::Check, 14.).text_color(t.accent))
                        })
                        .into_any_element()
                })
                .collect(),
        };
        div()
            .w(px(EFFORT_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(t.line)
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(ui::PANEL_INSET))
                    .children(body),
            )
            .into_any_element()
    }

    /// The rail of agents down the left, each the chat can move to.
    fn rail(&self, this: &Entity<Self>, t: &Tokens, cx: &App) -> Option<AnyElement> {
        let agents = self.agents(cx);
        if agents.is_empty() {
            return None;
        }
        let buttons = agents.into_iter().map(|agent| {
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
            agent_button(&agent, pressed, t, move |window, cx| {
                let provider = going.provider.clone();
                match (installed, installing) {
                    (true, _) if !pressed => {
                        if menu.read(cx).picked.locked {
                            close(window, cx);
                        }
                        moving(going.clone(), window, cx);
                    }
                    (true, _) => {}
                    (false, Some(Installing::Failed)) => {
                        close(window, cx);
                        let bench = menu.read(cx).bench.upgrade();
                        if let Some(bench) = bench {
                            bench.update(cx, |bench, cx| {
                                crate::agents::setup::show_install(bench, &provider, window, cx)
                            });
                        }
                    }
                    (false, _) if menu.read(cx).here() => {
                        let bench = menu.read(cx).bench.clone();
                        install_agent(&bench, &provider, cx)
                    }
                    (false, _) => {}
                }
            })
        });
        Some(
            div()
                .id("model-agents")
                .flex_none()
                .w(px(46.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .py(px(ui::PANEL_INSET + 2.))
                .border_r_1()
                .border_color(t.line)
                .overflow_y_scroll()
                .children(buttons)
                .into_any_element(),
        )
    }

    /// Puts the agent's name in the search field, as the menu follows a chat that moves.
    fn name_search(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.named.as_deref() == Some(agent) {
            return;
        }
        self.named = Some(agent.to_owned());
        let hint = t!("Search {agent}'s models…", agent = agent);
        self.query
            .update(cx, |query, cx| query.set_placeholder(hint, window, cx));
    }
}

/// How wide the menu's effort column is.
const EFFORT_WIDTH: f32 = 184.;

impl Render for Menu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (groups, options) = self.options(cx);
        let numbers = models(&options);
        let (typed, found) = (
            self.query.read(cx).value().trim().to_owned(),
            ui::found_mark(cx),
        );
        let this = cx.entity();
        let empty_own = options.iter().all(|row| row.group != 0);
        let note = match (typed.is_empty(), self.picked.pending) {
            (false, _)
                if options
                    .iter()
                    .all(|row| matches!(row.act, Act::AddProvider)) =>
            {
                Some(t!("No matches"))
            }
            (true, _) if !empty_own || self.picked.model_provider.is_some() => None,
            (true, Some(pending)) => Some(pending),
            (true, None) => Some(t!("This agent offers no models to pick")),
            (false, _) => None,
        };
        let add = options
            .iter()
            .cloned()
            .enumerate()
            .find(|(_, row)| matches!(row.act, Act::AddProvider))
            .map(|(at, row)| self.row(at, row, None, &typed, found, &this, cx));
        let speed = self.speed(&t);
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
            if !heading.is_empty() {
                list.push(
                    ui::caps(heading, cx)
                        .flex_none()
                        .px(SPACE_2H)
                        .pt(px(if list.is_empty() { 6. } else { 10. }))
                        .pb(px(4.))
                        .into_any_element(),
                );
            }
            if let Some(note) = note.filter(|_| noted) {
                list.push(
                    div()
                        .flex_none()
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
                let nth = numbers.iter().position(|model| *model == at);
                list.push(self.row(at, row, nth, &typed, found, &this, cx));
            }
        }
        if let Some(at) = chosen_at.filter(|_| std::mem::take(&mut self.follow)) {
            self.scroll.scroll_to_item(at);
        }
        let middle = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(38.))
                    .px(px(ROW_INSET))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_primary)
                    .child(ui::icon(Icon::Search, 13.).text_color(t.text_tertiary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.query).appearance(false).bordered(false)),
                    ),
            )
            .child(
                div()
                    .id("model-options")
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(ui::PANEL_INSET))
                    .min_h(px(300.))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(list),
            );
        let footer = (add.is_some() || speed.is_some()).then(|| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .py(SPACE_1H)
                .pl(px(ui::PANEL_INSET))
                .pr(SPACE_2)
                .border_t_1()
                .border_color(t.line)
                .child(div().children(add))
                .children(speed)
        });
        div()
            .key_context(format!("{CONTEXT} Modal").as_str())
            .on_action(cx.listener(|menu, _: &Previous, _, cx| menu.step(-1, cx)))
            .on_action(cx.listener(|menu, _: &Next, _, cx| menu.step(1, cx)))
            .on_action(
                cx.listener(|menu, _: &LessEffort, window, cx| menu.step_effort(-1, window, cx)),
            )
            .on_action(
                cx.listener(|menu, _: &MoreEffort, window, cx| menu.step_effort(1, window, cx)),
            )
            .on_action(cx.listener(|menu, PickNth(nth): &PickNth, window, cx| {
                let options = menu.options(cx).1;
                if let Some(option) = models(&options).get(*nth).and_then(|at| options.get(*at)) {
                    menu.pick(option.act.clone(), window, cx);
                }
            }))
            .on_action(cx.listener(|menu, _: &Choose, window, cx| {
                if let Some(option) = menu.options(cx).1.get(menu.chosen).cloned() {
                    menu.pick(option.act, window, cx);
                }
            }))
            .on_action(cx.listener(|menu, _: &Close, window, cx| (menu.close)(window, cx)))
            .w(px(600.))
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
                    .flex()
                    .children(self.rail(&this, &t, cx))
                    .child(middle)
                    .child(self.effort(&this, cx)),
            )
            .children(footer)
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
            let close = list_picker::dismiss(popover);
            let shown = picked.clone();
            let agent = this.read(cx).agent_name(&shown.provider).to_owned();
            let menu = window.use_keyed_state(key, cx, |window, cx| {
                Menu::new(&this, picked, close, window, cx)
            });
            menu.update(cx, |menu, cx| {
                menu.picked = shown;
                menu.name_search(&agent, window, cx);
            });
            menu
        })
}
