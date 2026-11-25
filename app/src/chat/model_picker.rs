//! A chat's model menu: the models its agent offers, searched from its own field, under its
//! maker's name, then those of every connected provider the agent can run. The agents above the
//! list move the chat to another agent, handing over what it said once it has begun, and before
//! it starts a provider's model moves it onto that provider.

use crate::ui::Clicks;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{Popover, Selectable};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    actions, div, px, Anchor, AnyElement, App, AppContext, Context, Entity, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window,
};
use sikemux_agents::agent_accounts::status::AgentStatus;
use sikemux_view::workspace::{ConfigChoice, ConfigPicker, RecentModel};

use crate::accounts;
use crate::agents::setup::{self, Installing};
use crate::core;
use crate::hosts::Place;
use crate::icons::{self, Icon};
use crate::theme::{
    self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui;
use crate::ui::list_picker::{self, Choose, Close, Dismiss, Next, Previous};
use crate::ui::sign_in;
use crate::workbench::Workbench;
use sikemux_agents::launch::providers::ProviderOrg;
use sikemux_calls::call;
use sikemux_calls::model_providers::{ModelRoute, RoutesParams};

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
/// Asks the agent for its models again.
pub type Retry = Rc<dyn Fn(&mut Window, &mut App)>;
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
    /// The chat's project, by id, which decides whether an org's providers are offered.
    pub project: String,
    /// The chat has started, so it keeps its agent, account and provider.
    pub locked: bool,
    /// No agent is running the chat now, so the menu stays open to hand it to another.
    pub idle: bool,
    /// What the list says while the agent's models have not come yet.
    pub pending: Option<&'static str>,
    /// Why the agent could not say its models, and the account to sign in to when that is why.
    pub unread: Option<Unread>,
    pub set: Set,
    pub moving: Move,
    /// Moves the chat onto another of its agent's accounts, while it runs on this host.
    pub account: Option<crate::account_picker::Pick>,
    /// The core of the host the chat runs on, whose providers it can pay with.
    pub target: core::Target,
}

#[derive(Clone)]
pub struct Unread {
    pub said: SharedString,
    pub sign_in: Option<String>,
    pub retry: Option<Retry>,
}

type Route = ModelRoute;

/// The mark of the provider a route runs on, an org's own key included.
fn route_mark(route: &Route) -> Icon {
    let provider = route
        .org
        .as_ref()
        .map_or(route.id.as_str(), |org| org.provider_in(&route.id));
    icons::model_provider(provider)
}

trait Named {
    /// `model` as the agent names it once a chat runs on this provider.
    fn named(&self, model: &str, current: &str) -> bool;
}

impl Named for Route {
    fn named(&self, model: &str, current: &str) -> bool {
        current == model || current == format!("sikemux-{}/{model}", self.id)
    }
}

/// The group of models new chats of the agent started on lately, drawn first.
const RECENT: usize = 0;
/// The group of the agent's own models.
const OWN: usize = 1;

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
    /// Signs in to the account the chat's agent is signed out of.
    SignIn(String),
    /// Asks the chat's agent for its models again, after it could not say.
    Retry,
}

#[derive(Clone)]
struct Row {
    group: usize,
    mark: Option<Icon>,
    label: String,
    meta: Option<String>,
    current: bool,
    act: Act,
}

/// An agent in the rail, and how its install is going.
struct Agent {
    provider: String,
    label: String,
    installing: Option<Installing>,
}

/// What an agent needs before a chat can run on it.
#[derive(Clone, PartialEq)]
enum Need {
    Install {
        broken: bool,
    },
    Installing,
    InstallFailed,
    /// Signing in, to the account when it has one.
    SignIn(Option<String>),
    Ready,
}

/// The open menu: its search, the row the keys are on, and the connected providers.
pub struct Menu {
    bench: WeakEntity<Workbench>,
    picked: Picked,
    query: Entity<InputState>,
    chosen: usize,
    routes: Vec<Route>,
    /// The agent `routes` were read for.
    routes_of: Option<String>,
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
    /// The agent picked in the rail while it is not ready, whose welcome card shows.
    viewing: Option<String>,
    /// The agent whose install this menu started, so its card says it installed.
    installed: Option<String>,
    /// What just finished, said in the search bar for a moment.
    said: Option<SharedString>,
    _saying: Option<Task<()>>,
    _typing: Subscription,
    _bench: Subscription,
}

/// Context, price per million tokens and tool calls, in a few words, as Settings words them.
fn facts(route: &Route, model: &str) -> Vec<String> {
    let name = model.strip_suffix("[1m]").unwrap_or(model);
    let facts = route.facts.get(name);
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
    if let Some(works) = route.works.get(name) {
        words.push(
            if *works {
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
        if !bench.read(cx).agent_setup.knows(picked.target.place()) {
            bench.update(cx, |bench, cx| crate::agents::setup::read(bench, cx));
        }
        Self {
            bench: bench.downgrade(),
            picked,
            query,
            chosen: 0,
            routes: Vec::new(),
            routes_of: None,
            close,
            scroll: ScrollHandle::new(),
            follow: false,
            older: false,
            asked_effort: None,
            named: None,
            viewing: None,
            installed: None,
            said: None,
            _saying: None,
            _typing: typing,
            _bench: cx.observe(bench, |_, _, cx| cx.notify()),
        }
    }

    /// Reads the connected providers the chat's agent runs on, when that agent changed.
    fn read_routes(&mut self, cx: &mut Context<Self>) {
        if self.routes_of.as_deref() == Some(self.picked.provider.as_str()) {
            return;
        }
        self.routes_of = Some(self.picked.provider.clone());
        let target = self.picked.target.clone();
        let params = RoutesParams {
            agent: self.picked.provider.clone(),
            current: self.picked.model_provider.clone(),
            project: Some(self.picked.project.clone()),
        };
        cx.spawn(async move |menu, cx| {
            let Ok(routes) = target.request_typed::<call::ModelRoutes>(params).await else {
                return;
            };
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
    fn options(&self, cx: &App) -> (Vec<(String, Option<ProviderOrg>)>, Vec<Row>) {
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
            .filter(|_| self.picked.locked);
        let maker = bench
            .reads
            .catalog
            .kind(own)
            .and_then(|kind| kind.maker.clone());
        let own_mark = match (paying, &maker) {
            (Some(route), _) => route_mark(route),
            (None, Some(maker)) => icons::model_provider(maker),
            (None, None) => icons::agent(own),
        };
        let mark_of = |choice: &ConfigChoice| match (&paying, &choice.maker) {
            (None, Some(maker)) => icons::model_provider(maker),
            _ => own_mark,
        };
        let paying = paying.map(|route| route.label.clone());
        let maker = maker.unwrap_or_else(|| bench.agent_name(own).to_owned());
        let mut groups = vec![
            (t!("Recent").to_owned(), None),
            (paying.unwrap_or(maker), None),
        ];
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
                        group: OWN,
                        mark: Some(mark_of(choice)),
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
                    group: OWN,
                    mark: Some(own_mark),
                    label: t!("Default model").into(),
                    meta: None,
                    current: false,
                    act: Act::Move(going(None)),
                }),
                false => options.extend(offered.iter().map(|choice| Row {
                    group: OWN,
                    mark: Some(mark_of(choice)),
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
        if let Some(account) = (self.picked.unread.as_ref())
            .and_then(|unread| unread.sign_in.clone())
            .filter(|_| !self.here())
        {
            options.push(Row {
                group: OWN,
                mark: None,
                label: t!("Sign in to {agent}", agent = bench.agent_name(own)),
                meta: None,
                current: false,
                act: Act::SignIn(account),
            });
        }
        if (self.picked.unread.as_ref()).is_some_and(|unread| unread.retry.is_some()) {
            options.push(Row {
                group: OWN,
                mark: None,
                label: t!("Try again").into(),
                meta: None,
                current: false,
                act: Act::Retry,
            });
        }
        if folded && query.is_empty() {
            options.push(Row {
                group: OWN,
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
            for route in &self.routes {
                let here = self.picked.model_provider.as_deref() == Some(route.id.as_str());
                let heading = match route.local && !route.label.starts_with("Local") {
                    true => t!("{route} · local", route = route.label),
                    false => route.label.clone(),
                };
                groups.push((heading, route.org.clone()));
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
                        mark: Some(route_mark(route)),
                        label: model.clone(),
                        meta: (!meta.is_empty()).then(|| meta.join(" · ")),
                        current: chosen,
                        act,
                    }
                }));
            }
        }
        options.retain(|option| {
            matches!(
                option.act,
                Act::AddProvider | Act::Fold | Act::Update | Act::SignIn(_) | Act::Retry
            ) || format!(
                "{} {} {}",
                option.label,
                option.meta.as_deref().unwrap_or_default(),
                groups
                    .get(option.group)
                    .map(|(heading, _)| heading.as_str())
                    .unwrap_or_default()
            )
            .to_lowercase()
            .contains(&query)
        });
        if query.is_empty() {
            let recent = self.recent(bench, &options);
            options.splice(0..0, recent);
        }
        (groups, options)
    }

    /// The rows of the models new chats of the agent started on lately, as they stand below.
    fn recent(&self, bench: &Workbench, options: &[Row]) -> Vec<Row> {
        let paying = (self.routes.iter())
            .find(|route| self.picked.model_provider.as_deref() == Some(route.id.as_str()));
        let used = |row: &Row, recent: &RecentModel| match &row.act {
            Act::Move(going) => {
                going.model.as_ref() == Some(&recent.model)
                    && going.model_provider == recent.provider
            }
            Act::Set(value) => {
                recent.provider == self.picked.model_provider
                    && paying.map_or(*value == recent.model, |route| {
                        route.named(&recent.model, value)
                    })
            }
            _ => false,
        };
        (bench.launchers().iter())
            .find(|launcher| launcher.provider == self.picked.provider)
            .map(|launcher| launcher.recent_models.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|recent| options.iter().find(|row| used(row, recent)))
            .map(|row| Row {
                group: RECENT,
                ..row.clone()
            })
            .collect()
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
            Act::SignIn(account) => {
                if let Some(bench) = self.bench.upgrade() {
                    bench.update(cx, |bench, cx| {
                        crate::accounts::sign_in(bench, &account, window, cx)
                    });
                }
            }
            Act::Retry => {
                if let Some(retry) =
                    (self.picked.unread.as_ref()).and_then(|unread| unread.retry.clone())
                {
                    retry(window, cx);
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
            group: OWN,
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
                    label: kind.label.clone(),
                    installing: bench.agent_setup.installing(&provider),
                    provider,
                })
            })
            .collect()
    }
}

/// An agent in the rail down the menu's left: its mark, lit while it is the one shown, and its
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
        .aria_label(agent.label.clone())
        .test_support()
        .relative()
        .flex_none()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .cursor_pointer()
        .map(|button| match pressed {
            true => button.bg(t.surface_chosen),
            false => button.hover(|style| style.bg(t.surface_hover)),
        })
        .child(ui::mark(&agent.provider, 16.))
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
        let adds = matches!(
            row.act,
            Act::AddProvider | Act::Fold | Act::Update | Act::SignIn(_) | Act::Retry
        );
        let key = nth.map(|nth| ui::keycap(numbered(nth), cx));
        div()
            .id(("model-option", at))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(30.))
            .px(SPACE_2H)
            .rounded(theme::radius_2())
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
            .child(match row.mark {
                Some(mark) => ui::picture(mark, 14., t.text_primary),
                None => ui::mark_box(
                    14.,
                    ui::icon(
                        match row.act {
                            Act::Fold => Icon::Chevron,
                            Act::Update => Icon::Download,
                            Act::SignIn(_) => Icon::User,
                            Act::Retry => Icon::Refresh,
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
            .border_b(crate::theme::border())
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
                    match self
                        .picked
                        .pending
                        .filter(|_| self.picked.effort.is_some() || self.picked.model.is_none())
                    {
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
                        .rounded(theme::radius_2())
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
            .border_l(crate::theme::border())
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

    /// The rail of agents down the left, each the chat can move to, and the tile that adds a
    /// model provider under them.
    fn rail(&self, this: &Entity<Self>, t: &Tokens, cx: &App) -> Option<AnyElement> {
        let agents = self.agents(cx);
        if agents.is_empty() {
            return None;
        }
        let shown = self.shown().to_owned();
        let buttons = agents.into_iter().map(|agent| {
            let pressed = agent.provider == shown;
            let menu = this.clone();
            let going = Moving {
                provider: agent.provider.clone(),
                profile: None,
                model: None,
                model_provider: None,
            };
            agent_button(&agent, pressed, t, move |window, cx| {
                if !pressed {
                    let going = going.clone();
                    menu.update(cx, |menu, cx| menu.show_agent(going, window, cx));
                }
            })
        });
        let add = {
            let menu = this.clone();
            div()
                .id("model-add-provider")
                .flex_none()
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::radius_2())
                .cursor_pointer()
                .aria_label(t!("Add provider"))
                .test_support()
                .hover(|style| style.bg(t.surface_hover))
                .child(ui::icon(Icon::Plus, 14.).text_color(t.text_tertiary))
                .tooltip(ui::tooltip(t!("Add provider")))
                .clicks(move |_, window, cx| {
                    menu.update(cx, |menu, cx| menu.pick(Act::AddProvider, window, cx))
                })
        };
        Some(
            div()
                .flex_none()
                .w(px(46.))
                .flex()
                .flex_col()
                .items_center()
                .border_r(crate::theme::border())
                .border_color(t.line)
                .child(
                    div()
                        .id("model-agents")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(3.))
                        .py(px(ui::PANEL_INSET + 2.))
                        .overflow_y_scroll()
                        .children(buttons),
                )
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .flex()
                        .justify_center()
                        .py(px(ui::PANEL_INSET + 2.))
                        .border_t(crate::theme::border())
                        .border_color(t.line)
                        .child(add),
                )
                .into_any_element(),
        )
    }

    /// The agent the menu shows: the one picked in the rail while it is not ready, else the
    /// chat's own.
    fn shown(&self) -> &str {
        self.viewing.as_deref().unwrap_or(&self.picked.provider)
    }

    /// Moves the chat to the agent picked in the rail when it can run chats, else shows the
    /// card that gets it ready.
    fn show_agent(&mut self, going: Moving, window: &mut Window, cx: &mut Context<Self>) {
        let own = going.provider == self.picked.provider;
        let need = match (self.here(), self.bench.upgrade()) {
            (true, Some(bench)) => self.need(bench.read(cx), &going.provider),
            _ => Need::Ready,
        };
        self.viewing = (need != Need::Ready && !own).then(|| going.provider.clone());
        if need == Need::Ready && !own {
            (self.picked.moving)(going, window, cx);
        }
        cx.notify();
    }

    /// What `provider` needs before a chat runs on it on this host.
    fn need(&self, bench: &Workbench, provider: &str) -> Need {
        match bench.agent_setup.installing(provider) {
            Some(Installing::Running) => return Need::Installing,
            Some(Installing::Failed) => return Need::InstallFailed,
            None => {}
        }
        if provider == self.picked.provider {
            return (self.picked.unread.as_ref())
                .and_then(|unread| unread.sign_in.clone())
                .map_or(Need::Ready, |account| Need::SignIn(Some(account)));
        }
        let status = (bench.agent_setup.on(&Place::This))
            .and_then(|found| found.iter().find(|agent| agent.kind == provider))
            .map(|agent| &agent.status);
        let missing = match status {
            Some(status) => matches!(status, AgentStatus::Missing | AgentStatus::Broken { .. }),
            None => !bench.has_agent(provider),
        };
        if missing {
            return Need::Install {
                broken: matches!(status, Some(AgentStatus::Broken { .. })),
            };
        }
        let profiles = bench.accounts.of(provider);
        let profile = bench.accounts.current(provider);
        let signed_out = if profiles.is_empty() {
            None
        } else if profiles.iter().any(|profile| {
            accounts::status(bench, profile)
                .and_then(|status| status.as_ref().ok())
                .is_some_and(|status| status.signed_in)
        }) {
            Some(false)
        } else if profiles.iter().all(|profile| {
            accounts::status(bench, profile)
                .and_then(|status| status.as_ref().ok())
                .is_some_and(|status| !status.signed_in)
        }) {
            Some(true)
        } else {
            None
        };
        match (signed_out, status) {
            (Some(true), _) | (None, Some(AgentStatus::SignedOut)) => {
                Need::SignIn(profile.map(|profile| profile.id.clone()))
            }
            _ => Need::Ready,
        }
    }

    /// The agent whose welcome card shows instead of the models, and what it needs.
    fn card(&self, cx: &App) -> Option<(String, Need)> {
        if !self.here() {
            return None;
        }
        let bench = self.bench.upgrade()?;
        let bench = bench.read(cx);
        let need = self.need(bench, self.shown());
        (need != Need::Ready).then(|| (self.shown().to_owned(), need))
    }

    /// The agent the welcome card showed, once it can run chats.
    fn settled(&self, cx: &App) -> Option<String> {
        let bench = self.bench.upgrade()?;
        let viewing = self.viewing.as_deref()?;
        (self.need(bench.read(cx), viewing) == Need::Ready).then(|| viewing.to_owned())
    }

    /// Hands the chat on once what the card waited on has happened.
    fn hand_back(&mut self, provider: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        self.viewing = None;
        self.installed = None;
        let label = bench.read(cx).agent_name(&provider).to_owned();
        if provider != self.picked.provider {
            let going = Moving {
                provider,
                profile: None,
                model: None,
                model_provider: None,
            };
            (self.picked.moving)(going, window, cx);
        }
        self.say(t!("{agent} is ready", agent = label).into(), cx);
        cx.notify();
    }

    /// Says `words` in the search bar for a few seconds.
    fn say(&mut self, words: SharedString, cx: &mut Context<Self>) {
        self.said = Some(words);
        self._saying = Some(cx.spawn(async move |menu, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = menu.update(cx, |menu, cx| {
                menu.said = None;
                cx.notify();
            });
        }));
    }

    /// Closes the menu and opens the add-account dialog for `provider` over the window,
    /// signing `account` in again when there is one.
    fn add_account(
        &mut self,
        provider: &str,
        account: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        (self.close)(window, cx);
        bench.update(cx, |bench, cx| {
            accounts::open_dialog(bench, provider, window, cx);
            if let Some(account) = account {
                accounts::sign_in_again(bench, account, window, cx);
            }
        });
    }

    /// The card that installs the agent and signs it in, where its models would be.
    fn welcome(&self, provider: &str, need: Need, this: &Entity<Self>, cx: &App) -> AnyElement {
        let Some(bench) = self.bench.upgrade() else {
            return div().into_any_element();
        };
        let t = Tokens::get(cx);
        let shown = bench.read(cx);
        let label = shown.agent_name(provider).to_owned();
        let about = (shown.reads.catalog.kind(provider)).map(|kind| kind.about.clone());
        let on = |run: fn(&mut Menu, &str, &mut Window, &mut Context<Menu>)| {
            let (menu, provider) = (this.clone(), provider.to_owned());
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                menu.update(cx, |menu, cx| run(menu, &provider, window, cx))
            }
        };
        let install = |menu: &mut Menu, provider: &str, _: &mut Window, cx: &mut Context<Menu>| {
            menu.installed = Some(provider.to_owned());
            if let Some(bench) = menu.bench.upgrade() {
                bench.update(cx, |bench, cx| setup::install_behind(bench, provider, cx));
            }
        };
        let title = match &need {
            Need::Install { .. } => t!("Get {agent}", agent = label),
            Need::Installing => t!("Installing {agent}", agent = label),
            Need::InstallFailed => t!("{agent} did not install", agent = label),
            Need::SignIn(_) | Need::Ready => t!("Sign in to {agent}", agent = label),
        };
        let screen = sign_in::Screen::new("model-welcome", ui::mark(provider, 34.), title)
            .when_some(about, |screen, about| screen.lede(about));
        let screen = match need {
            Need::Install { broken } => screen
                .child(
                    ui::Button::new(
                        "model-welcome-install",
                        if broken {
                            t!("Reinstall")
                        } else {
                            t!("Install")
                        },
                        ui::Variant::Primary,
                    )
                    .wide()
                    .on_click(on(install)),
                )
                .child(sign_in::note(
                    t!(
                        "Runs {agent}'s own installer on this computer.",
                        agent = label
                    ),
                    cx,
                )),
            Need::Installing => screen.child(sign_in::waiting(
                "model-welcome-installing",
                t!("Installing…"),
                on(|menu, provider, _, cx| {
                    menu.installed = None;
                    if let Some(bench) = menu.bench.upgrade() {
                        bench.update(cx, |bench, cx| setup::cancel_behind(bench, provider, cx));
                    }
                }),
                cx,
            )),
            Need::InstallFailed => screen
                .child(sign_in::error(
                    t!("The installer stopped before it finished."),
                    cx,
                ))
                .child(
                    ui::Button::new("model-welcome-retry", t!("Try again"), ui::Variant::Primary)
                        .wide()
                        .on_click(on(install)),
                )
                .child(sign_in::links().child(
                    sign_in::link("model-welcome-output", t!("See what it printed"), cx).clicks(
                        on(|menu, provider, window, cx| {
                            (menu.close)(window, cx);
                            if let Some(bench) = menu.bench.upgrade() {
                                bench.update(cx, |bench, cx| {
                                    setup::show_install(bench, provider, cx)
                                });
                            }
                        }),
                    ),
                )),
            Need::SignIn(account) => {
                let installed = self.installed.as_deref() == Some(provider);
                let go = match account {
                    Some(account) => {
                        let (menu, provider) = (this.clone(), provider.to_owned());
                        ui::Button::new(
                            "model-welcome-sign-in",
                            t!("Continue with {agent}", agent = label),
                            ui::Variant::Primary,
                        )
                        .wide()
                        .on_click(move |_, window, cx| {
                            menu.update(cx, |menu, cx| {
                                menu.add_account(&provider, Some(&account), window, cx)
                            })
                        })
                    }
                    None => ui::Button::new(
                        "model-welcome-sign-in",
                        t!("Sign in"),
                        ui::Variant::Primary,
                    )
                    .wide()
                    .on_click(on(|menu, provider, window, cx| {
                        (menu.close)(window, cx);
                        if let Some(bench) = menu.bench.upgrade() {
                            bench.update(cx, |bench, cx| {
                                sign_in_in_settings(bench, provider, window, cx)
                            });
                        }
                    })),
                };
                screen
                    .when(installed, |screen| {
                        screen.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_1H)
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .text_color(t.live)
                                .child(ui::icon(Icon::Check, 13.))
                                .child(t!("Installed")),
                        )
                    })
                    .child(go)
            }
            Need::Ready => screen,
        };
        screen.into_any_element()
    }

    /// The accounts the chat's agent has, the chat's own lit, and the chip that adds one.
    fn accounts_row(&self, this: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        let pick = self.picked.account.clone()?;
        let bench = self.bench.upgrade()?;
        let bench = bench.read(cx);
        let own = self.picked.provider.as_str();
        let profiles = bench.accounts.of(own);
        if profiles.is_empty() {
            return None;
        }
        let t = Tokens::get(cx);
        let on = (self.picked.profile.clone()).or_else(|| {
            bench
                .accounts
                .current(own)
                .map(|profile| profile.id.clone())
        });
        let chip = |id: SharedString| {
            div()
                .id(id)
                .flex_none()
                .h(px(24.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(theme::radius_2())
                .border(crate::theme::border())
                .border_color(t.line)
                .cursor_pointer()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
        };
        let chips = profiles.into_iter().map(|profile| {
            let status = accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
            let signed_out = status.is_some_and(|status| !status.signed_in);
            let chosen = on.as_deref() == Some(profile.id.as_str());
            let meta = match signed_out {
                true => Some(t!("Signed out").to_owned()),
                false => accounts::used(bench, profile).map(|used| format!("{}%", used.round())),
            };
            let (menu, pick, chosen_profile) = (this.clone(), pick.clone(), profile.clone());
            chip(SharedString::from(format!("model-account-{}", profile.id)))
                .map(|chip| match chosen {
                    true => chip.bg(t.surface_chosen),
                    false => chip.hover(|style| style.bg(t.surface_hover)),
                })
                .text_color(if signed_out {
                    t.text_tertiary
                } else {
                    t.text_primary
                })
                .child(accounts::avatar(bench.accounts.avatar(profile), 14.))
                .child(profile.shown_name(status))
                .children(meta.map(|meta| div().text_color(t.text_tertiary).child(meta)))
                .clicks(move |_, window, cx| {
                    if chosen {
                        return;
                    }
                    let profile = chosen_profile.clone();
                    match signed_out {
                        true => menu.update(cx, |menu, cx| {
                            let provider = menu.picked.provider.clone();
                            menu.add_account(&provider, Some(&profile.id), window, cx)
                        }),
                        false => {
                            pick(&profile, cx);
                            let name = profile.shown_name(None);
                            menu.update(cx, |menu, cx| {
                                menu.say(t!("On {name}", name = name).into(), cx)
                            });
                        }
                    }
                })
        });
        let add = {
            let menu = this.clone();
            chip("model-account-add".into())
                .text_color(t.text_secondary)
                .hover(|style| style.bg(t.surface_hover))
                .child(ui::icon(Icon::Plus, 12.).text_color(t.text_tertiary))
                .child(t!("Add account"))
                .clicks(move |_, window, cx| {
                    menu.update(cx, |menu, cx| {
                        let provider = menu.picked.provider.clone();
                        menu.add_account(&provider, None, window, cx)
                    })
                })
        };
        Some(
            div()
                .id("model-accounts")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(ui::PANEL_INSET + 2.))
                .py(px(7.))
                .border_b(crate::theme::border())
                .border_color(t.line)
                .overflow_x_scroll()
                .children(chips)
                .child(add)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(settled) = self.settled(cx) {
            cx.defer_in(window, move |menu, window, cx| {
                menu.hand_back(settled, window, cx)
            });
        }
        let t = Tokens::get(cx).clone();
        let (groups, options) = self.options(cx);
        let numbers = models(&options);
        let (typed, found) = (
            self.query.read(cx).value().trim().to_owned(),
            ui::found_mark(cx),
        );
        let this = cx.entity();
        let empty_own = options.iter().all(|row| row.group != OWN);
        let unread = (self.picked.unread.as_ref())
            .filter(|_| typed.is_empty())
            .map(|unread| unread.said.clone());
        let note = match (typed.is_empty(), self.picked.pending) {
            _ if unread.is_some() => unread,
            (false, _)
                if !options
                    .iter()
                    .any(|row| matches!(row.act, Act::Set { .. } | Act::Move { .. })) =>
            {
                Some(t!("No matches").into())
            }
            (true, _) if !empty_own || self.picked.model_provider.is_some() => None,
            (true, Some(pending)) => Some(pending.into()),
            (true, None) => Some(t!("This agent offers no models to pick").into()),
            (false, _) => None,
        };
        let speed = self.speed(&t);
        let mut list: Vec<AnyElement> = Vec::new();
        let mut chosen_at = None;
        for (group, (heading, org)) in groups.iter().enumerate() {
            let rows: Vec<(usize, Row)> = options
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, row)| row.group == group)
                .collect();
            let noted = group == OWN && note.is_some();
            if rows.is_empty() && !noted {
                continue;
            }
            if !heading.is_empty() {
                list.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .children(
                            org.as_ref()
                                .map(|org| ui::org_mark(&org.name, &org.id, 14.)),
                        )
                        .child(ui::caps(heading, cx))
                        .flex_none()
                        .px(SPACE_2H)
                        .pt(px(if list.is_empty() { 6. } else { 10. }))
                        .pb(px(4.))
                        .into_any_element(),
                );
            }
            if let Some(note) = note.clone().filter(|_| noted) {
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
        let card = self.card(cx);
        let said = self.said.clone().map(|said| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_color(t.live)
                .child(ui::icon(Icon::Check, 12.))
                .child(said)
        });
        let header = div()
            .flex_none()
            .h(px(38.))
            .px(px(ROW_INSET))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .border_b(crate::theme::border())
            .border_color(t.line)
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .text_color(t.text_primary)
            .child(ui::icon(Icon::Search, 13.).text_color(t.text_tertiary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.query).appearance(false).bordered(false)),
            )
            .children(said);
        let accounts = card
            .is_none()
            .then(|| self.accounts_row(&this, cx))
            .flatten();
        let showing_card = card.is_some();
        let body = match card {
            Some((provider, need)) => div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .id("model-welcome-card")
                        .h(px(398.))
                        .child(self.welcome(&provider, need, &this, cx)),
                )
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(header)
                        .children(accounts)
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
                        ),
                )
                .child(self.effort(&this, cx))
                .into_any_element(),
        };
        let footer = (speed.is_some() && !showing_card).then(|| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .py(SPACE_1H)
                .pl(px(ui::PANEL_INSET))
                .pr(SPACE_2)
                .border_t(crate::theme::border())
                .border_color(t.line)
                .child(div())
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
                if menu.card(cx).is_some() {
                    return;
                }
                let options = menu.options(cx).1;
                if let Some(option) = models(&options).get(*nth).and_then(|at| options.get(*at)) {
                    menu.pick(option.act.clone(), window, cx);
                }
            }))
            .on_action(cx.listener(|menu, _: &Choose, window, cx| {
                if menu.card(cx).is_some() {
                    return;
                }
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
            .border(crate::theme::border())
            .border_color(t.border)
            .rounded(theme::radius_3())
            .shadow(t.shadow_float())
            .child(div().flex().children(self.rail(&this, &t, cx)).child(body))
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
            let menu = window.use_keyed_state(key, cx, |window, cx| {
                Menu::new(&this, picked, close, window, cx)
            });
            menu.update(cx, |menu, cx| {
                menu.picked = shown;
                menu.read_routes(cx);
                let agent = this.read(cx).agent_name(menu.shown()).to_owned();
                menu.name_search(&agent, window, cx);
            });
            menu
        })
}

/// Signs `provider` in the way Settings does when it has no account to sign in again.
fn sign_in_in_settings(
    bench: &mut Workbench,
    provider: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let sheet = (bench.agent_setup.on(&Place::This))
        .and_then(|found| found.iter().find(|agent| agent.kind == provider))
        .and_then(|agent| setup::next_step(bench, agent))
        .map(|(_, sheet)| sheet);
    match sheet {
        Some(sheet) => setup::run(bench, sheet, cx),
        None => bench.open_settings(crate::settings::Page::Agents, window, cx),
    }
}
