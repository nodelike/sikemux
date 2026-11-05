use crate::keyboard::Keyboard;
use crate::keyboard::TakesKeys;
use sikemux_wire::changed::Topic;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;

use gpui_kit::component::input::{AnyInputState, InputEvent, InputState, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, deferred, div, px, AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context,
    DragMoveEvent, Entity, ExternalPaths, FocusHandle, Focusable, Image, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, Subscription, Task, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::layout::{PaneKind, ProjectLayout, Tab};
use sikemux_view::prefs::{MidTurnSend, Preferences, TabRailPage, TabRailScope, WindowView};
use sikemux_wire::protocol::{
    AgentFace, AgentGroup, AgentStatus, LauncherInfo, NoticeGoal, PermissionMode, ProjectCodeHost,
    ProjectInfo, RemoteStatus, SessionId, SessionInfo,
};
use tokio::sync::mpsc;

use crate::battery::Battery;
use crate::chat::{Answering, Chat, ComposeStage, Composing, Question, Sent, START};
use crate::core::{Ask, ChatAsk, Link, Target};
use crate::files::FileTree;
use crate::finder::ProjectFiles;
use crate::git::GitPane;
use crate::hosts::Place;
use crate::motion;
use crate::overlays::{Layer, Overlays};
use crate::reads::{Read, Reads};
use crate::search::SearchPane;
use crate::theme::{self, Tokens, SPACE_1, SPACE_2H, TEXT_SM};
use crate::toast;
use crate::{agents, home, rail, tabs, terminal};
use sikemux_core::git::status::DiscoveredRepo;
use sikemux_core::server::{
    AccountUsage, PortOwner, ProjectPort, RunView, TaskView, TrustQuestion,
};

/// A click in the app's own window takes the keyboard back from a page holding it. On
/// Linux a page is a window of another connection, which X would otherwise leave it with.
#[cfg(target_os = "linux")]
fn keyboard_back() {
    sikemux_browser::give_keyboard_back();
}

#[cfg(not(target_os = "linux"))]
fn keyboard_back() {}

/// How long a tab asked to close stays closing in the window if the core never says it
/// went, such as when its closing failed.
const CLOSE_SETTLES: Duration = Duration::from_secs(2);
/// How long a peeking rail stays after the pointer leaves it.
/// How long a chat stays attached once its tab leaves the front, so flicking between tabs
/// keeps them live while a long-hidden one stops streaming every change to the app.
const CHAT_RESTS_AFTER: Duration = Duration::from_secs(60);
const PEEK_CLOSE: Duration = Duration::from_millis(180);
/// How close to the window's edge the pointer brings out a closed rail.
const PEEK_REACH: f32 = 28.;
/// How far past its own width a peeking rail starts and ends, out of sight beyond the window.
const PEEK_GAP: f32 = 10.;
/// The handle between a docked rail and the stage.
const RESIZER_WIDTH: f32 = 10.;

/// The decoded pictures a long chat full of screenshots keeps loaded: about a dozen at full size.
const PICTURE_BUDGET: usize = 256 * 1024 * 1024;

/// The one store: what the core last said, and what only this window decides.
/// The longest a space's name may be, as on the Mac.
pub(crate) const MAX_SPACE_NAME: usize = 40;

pub(crate) mod actions;
mod feed;

pub struct Workbench {
    pub connection: crate::boot::Connection,
    pub reads: Reads,
    /// The person's preferences, once the core has said them.
    pub(crate) preferences: Option<Preferences>,
    /// Dictation, heard into the chat or terminal holding the keyboard.
    pub voice: Entity<crate::voice::Dictation>,
    _voice_heard: [Subscription; 4],
    pub keys: Keyboard,
    window_active: bool,
    /// The agents last told to the core as on screen, and whether the window was in front.
    on_screen: Option<(Vec<String>, bool)>,
    /// The project the per-project watches were last made for, by path.
    pub(crate) watched_project: Option<String>,
    /// The host whose core those watches went to.
    watched_place: crate::hosts::Place,
    pub battery: Option<Battery>,
    pub agent_setup: crate::agents::setup::Setup,
    pub overlays: Overlays,
    /// The question over the window while a phone waits to connect.
    pub pairing: crate::pairing::Pairing,
    /// Whether this window has looked for what kept running while Sikemux was closed.
    pub kept_running_told: bool,
    /// Each waiting `sikemux open --wait`, with the tabs it opened that are still open.
    pub(crate) open_waits: Vec<(sikemux_wire::protocol::CallId, HashMap<String, bool>)>,
    /// Each open project settings tab's state, by the project's folder.
    pub project_settings: HashMap<String, crate::project_settings::ProjectSettings>,
    pub shown: crate::experience::Shown,
    pub(crate) picture: crate::picture::AccountPicture,
    /// From asking the core to sign in until the browser is done or gives up.
    pub signing_in: bool,
    /// Where remote access stands, once the core has said.
    pub remote: Option<RemoteStatus>,
    pub accounts: crate::accounts::Accounts,
    /// The number the Dock icon's badge shows.
    pub badge: usize,
    /// A tab being dragged along its strip, and where it would land.
    pub reorder: Option<crate::drag::Reorder>,
    /// The project or host in front, and the one in front before it.
    pub place: Option<Strip>,
    pub last_place: Option<Strip>,
    pub rails: crate::rail::Rails,
    pub(crate) strip: crate::tabs::StripMotion,
    /// Tabs asked to close that the layout still lists, and when, so a second click or ⌘W
    /// before the core's answer does not ask for a tab it has already closed.
    closing_tabs: std::cell::RefCell<HashMap<String, std::time::Instant>>,
    /// The card beside the agent row the pointer rests on.
    pub agent_card: crate::agents::card::Card,
    /// The task each project's Run button starts, when the person chose one, by project path.
    pub picked_tasks: HashMap<String, String>,
    /// Where the stage was last drawn, for what rises over the stage alone.
    pub stage_frame: Rc<Cell<Bounds<Pixels>>>,
    pub run_cards: crate::test_card::RunCards,
    /// The space whose projects the rail shows, else every project.
    pub space: Option<String>,
    pub git: crate::git::GitTab,
    pub chats: crate::chat::Chats,
    pub run_clock: Entity<crate::tabs::RunClock>,
    pub files: FileTree,
    /// The file tree's, which its arrow keys and F2 move and act in.
    pub files_focus: FocusHandle,
    /// The agent the project picker moves to the project picked, while it does.
    pub moving: Option<String>,
    /// The command deck's keys, the last used first.
    pub recent_commands: Vec<SharedString>,
    /// When a refused file listing is asked again.
    pub(crate) files_retry: crate::retry::Retry,
    /// The Git pane takes the keyboard once it shows, as when ⌘⌥3 opens it.
    pub project_files: ProjectFiles,
    /// The kind of agent the rail was switched to, such as "claude"; its account is the one the
    /// person chose for that kind.
    pub launcher_choice: Option<String>,
    /// The plan limits are to be read once the core lists its agents, as the app starts.
    usage_wanted: bool,
    pub pinch: Option<crate::pinch::Pinch>,
    pub calendar: crate::home::Calendar,
    /// The rail's filter of recent chats, while it shows.
    pub filter: Option<crate::recent::Filter>,
    /// The front project's saved chats paged or searched to beyond the recent ones.
    pub older: Option<crate::recent::OlderChats>,
    pub asks: mpsc::UnboundedSender<Ask>,
    pub terminals: terminal::Terminals,
    pub editors: Entity<crate::editor::Editors>,
    /// The Search pane of the project whose Search tab was last in front.
    pub search: Option<Entity<SearchPane>>,
    pub board: Option<Entity<crate::board::BoardPane>>,
    /// The card to open beside the board of the project at the folder named, once it shows.
    pub(crate) board_card: Option<(String, u64)>,
    /// The goals, made the first time the window shows them, and the window hearing them move,
    /// which its rails show.
    pub goals: Option<Entity<crate::goals::GoalsPane>>,
    goals_moved: Option<Subscription>,
    /// What the window shows, as the person last left it.
    pub window_view: WindowView,
    /// The mod panes of the strip in front, by view.
    pub mod_panes: HashMap<String, Entity<crate::mod_pane::ModPane>>,
    /// Settings, while they take the stage's place.
    pub settings: Entity<crate::settings::Settings>,
    pub feeds: crate::hosts::Feeds,
    /// The stage's tabs while they move between one another.
    pub(crate) track: crate::track::Slide<String>,
    /// The runs the person started here, whose terminal opens once the run has one.
    pub runs_to_show: HashSet<String>,
    /// The agent in front whose worktree its header reads, by id and folder, and where it is.
    pub watched_worktree: Option<(String, String, crate::hosts::Place)>,
    pub(crate) trust: Trust,
    /// The skills agents saved in their own folders that the person was told of, by path.
    pub(crate) skills_offered: HashSet<String>,
    /// The window the store draws, for what is said in it from outside a draw.
    pub(crate) window: AnyWindowHandle,
    pub parts: Parts,
    pub panes: crate::panes::Panes,
    pub desks: Entity<crate::desk::Desks>,
    /// What the core was last told the desks hold.
    pub(crate) desks_sent: Option<sikemux_view::goals::DesksParams>,
    /// Telling the core what the desks hold, once they settle.
    pub(crate) desks_telling: Option<Task<()>>,
    _desks_watched: Vec<Subscription>,
    _desk_ears: crate::desk::DeskEars,
    pub sims: Entity<crate::sim::pane::Simulators>,
    _sim_news: Subscription,
    pub(crate) _older: Option<Task<()>>,
}

/// Whose tabs a strip holds: a project's, those of an SSH host shown instead of one, or
/// the terminals of no project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Strip {
    Project(String),
    Host(String),
    Terminals,
}

impl Strip {
    /// `params` naming this strip, as the core's tab calls take it.
    pub(crate) fn with(&self, mut params: Value) -> Value {
        let (key, id) = match self {
            Strip::Project(id) => ("project", json!(id)),
            Strip::Host(alias) => ("host", json!(alias)),
            Strip::Terminals => ("terminals", json!(true)),
        };
        params[key] = id;
        params
    }
}

/// A side of the window a rail lives on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// How wide a rail can be dragged, and how wide it starts.
#[derive(Clone, Copy)]
pub struct RailWidth {
    pub min: f32,
    pub max: f32,
    pub initial: f32,
}

/// An agent's title while it is edited in place, in a field holding it.
pub struct Retitling {
    pub agent: String,
    /// Edited in its tab rail row rather than its header.
    pub in_rail: bool,
    pub input: Entity<InputState>,
    title: String,
    _events: Subscription,
}

/// A rail's handle while it is dragged.
struct DraggedRail(Edge);

impl Render for DraggedRail {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// How often the Limits panel reads the plan limits again while it shows, as the Mac app does.
const LIMITS_EVERY: Duration = Duration::from_secs(5 * 60);

/// An agent as the rails and tabs show it.
#[derive(Clone)]
pub struct AgentRow {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub name: Option<String>,
    pub status: AgentStatus,
    pub subagents: u32,
    pub tasks: u32,
    pub project: String,
    /// The face it wears, else none and its agent's mark is drawn.
    pub face: Option<AgentFace>,
    pub group: AgentGroup,
    /// Mid-turn or waiting on an answer, so stopping it now loses work.
    pub busy: bool,
    /// It still has subagents or shells running.
    pub background: bool,
    /// When it last worked, in Unix milliseconds; 0 when the core does not know.
    pub active_at: u64,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    pub next_trigger: Option<u64>,
    /// When it was last named with `@` or messaged by another agent, in Unix milliseconds.
    pub called_at: Option<u64>,
    /// Its place on its lead's team, which the rows after a lead belong to.
    pub team: Option<TeamLink>,
}

pub use sikemux_view::inbox::TeamLink;

impl AgentRow {
    /// Its face, or its agent's mark while it wears none, `size` across.
    pub fn mark(&self, size: f32, cx: &App) -> AnyElement {
        crate::ui::agent_mark(
            &self.id,
            &self.provider,
            self.face.as_ref(),
            self.group,
            size,
            cx,
        )
    }
}

impl Workbench {
    pub fn new(link: Link, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Link { mut updates, asks } = link;
        let _ = asks.send(Ask::Remote(crate::core::RemoteAsk::Status));
        cx.defer_in(window, |bench, _, cx| crate::accounts::read(bench, cx));
        cx.set_decoded_image_budget(PICTURE_BUDGET);
        // The window's close button hides the app as ⌘H does, so the Dock, a link or
        // `sikemux open` brings the window back as it was.
        #[cfg(target_os = "macos")]
        window.on_window_should_close(cx, |_, cx| {
            cx.hide();
            false
        });
        actions::install(asks.clone(), cx);
        crate::hang_watch::start(window, cx);
        crate::ui::grain::animate(window, cx, |bench| {
            bench.battery.is_some_and(|battery| !battery.charging)
        });
        crate::ui::clock::follow(window, cx);
        let (power_changed, mut power) = mpsc::unbounded_channel();
        crate::battery::watch(move || {
            let _ = power_changed.send(());
        });
        cx.spawn(async move |this, cx| {
            while power.recv().await.is_some() {
                let read = |bench: &mut Self, cx: &mut Context<Self>| {
                    bench.battery = crate::battery::read();
                    cx.notify();
                };
                if this.update(cx, read).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                let mut batch = vec![update];
                while let Ok(update) = updates.try_recv() {
                    batch.push(update);
                }
                if this
                    .update(cx, |this, cx| {
                        batch.into_iter().for_each(|update| this.apply(update, cx))
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let here = Target::here(&asks);
        for read in Read::FROM_THE_START {
            here.watch(read, json!({}));
        }
        let _ = asks.send(Ask::Follow(Topic::Git));
        let _ = asks.send(Ask::Follow(Topic::Files));
        let _ = asks.send(Ask::Follow(Topic::Lsp));
        let _ = asks.send(Ask::Follow(Topic::Boards));
        let _ = asks.send(Ask::Follow(Topic::Memory));
        let _ = asks.send(Ask::Follow(Topic::GoalSummary));
        let _ = asks.send(Ask::Follow(Topic::AgentSignIn));
        let _ = asks.send(Ask::Follow(Topic::AgentStatus));
        let _ = asks.send(Ask::Follow(Topic::AgentAccounts));
        let _ = asks.send(Ask::Follow(Topic::Mods));
        for topic in [Topic::Tasks, Topic::Runs, Topic::Usage, Topic::Questions] {
            let _ = asks.send(Ask::Follow(topic));
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Self::until_next_minute())
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(LIMITS_EVERY).await;
            let read = this.update_in(cx, |bench, window, _| {
                if window.is_visible() && bench.limits_shown() {
                    bench.read_usage();
                }
            });
            if read.is_err() {
                break;
            }
        })
        .detach();
        let voice = cx.new(|_| crate::voice::Dictation::default());
        let voice_heard = [
            cx.subscribe_in(&voice, window, |bench, _, heard, window, cx| {
                bench.dictated(heard, window, cx)
            }),
            cx.observe_window_activation(window, |bench, window, cx| {
                bench.window_active = window.is_window_active();
                crate::hang_watch::in_front(bench.window_active);
                bench.publish_on_screen();
                match window.is_window_active() {
                    true => crate::accounts::check_on_return(bench, cx),
                    false => bench.voice.update(cx, |voice, cx| voice.abandon_hold(cx)),
                }
                crate::terminal::keep_blinking(bench, cx);
            }),
            cx.observe_window_visibility(window, |bench, _, _, cx| {
                crate::terminal::keep_blinking(bench, cx)
            }),
            cx.observe(&voice, |bench, _, cx| {
                if bench.settings.read(cx).is_open() {
                    bench.parts.redraw_stage(cx);
                }
            }),
        ];
        let sims = cx.new(crate::sim::pane::Simulators::new);
        let desks = cx.new(|cx| crate::desk::Desks::new(window, cx));
        let mut bench = Self {
            connection: crate::boot::Connection::default(),
            reads: Reads::default(),
            preferences: None,
            voice,
            _voice_heard: voice_heard,
            keys: Keyboard::new(window, cx),
            window_active: window.is_window_active(),
            on_screen: None,
            watched_project: None,
            watched_place: crate::hosts::Place::This,
            battery: crate::battery::read(),
            agent_setup: Default::default(),
            overlays: Overlays::default(),
            project_settings: HashMap::new(),
            kept_running_told: false,
            open_waits: Vec::new(),
            pairing: crate::pairing::Pairing::new(cx),
            shown: Default::default(),
            picture: crate::picture::AccountPicture::default(),
            signing_in: false,
            remote: None,
            accounts: Default::default(),
            badge: 0,
            reorder: None,
            place: None,
            last_place: None,
            rails: crate::rail::Rails {
                side_open: true,
                tab_open: true,
                side_width: rail::SIDE_RAIL.initial,
                tab_width: agents::TAB_RAIL.initial,
                resizers: [cx.focus_handle(), cx.focus_handle()],
                peeking: None,
                unpeek: None,
                page: TabRailPage::Agents,
                keeping: None,
                all_projects: false,
                goal_focus: None,
                all_harnesses: false,
                folded_teams: HashSet::new(),
                seen: Default::default(),
                scroll: Default::default(),
                glide: Default::default(),
                frame: Rc::default(),
                slide: crate::track::Slide::default(),
            },
            strip: Default::default(),
            closing_tabs: Default::default(),
            agent_card: crate::agents::card::Card::default(),
            picked_tasks: HashMap::new(),
            stage_frame: Rc::default(),
            run_cards: Default::default(),
            space: None,
            feeds: Default::default(),
            track: crate::track::Slide::default(),
            runs_to_show: HashSet::new(),
            watched_worktree: None,
            trust: Trust::default(),
            skills_offered: HashSet::new(),
            chats: crate::chat::Chats::default(),
            git: crate::git::GitTab {
                pane: None,
                picked: None,
                listing: false,
                behind: false,
                wanted_commit: None,
            },
            run_clock: cx.new(|_| crate::tabs::RunClock::default()),
            files: FileTree::default(),
            files_focus: cx.focus_handle(),
            moving: None,
            recent_commands: Vec::new(),
            files_retry: Default::default(),
            project_files: ProjectFiles::default(),
            launcher_choice: None,
            usage_wanted: false,
            pinch: None,
            calendar: crate::home::Calendar::default(),
            filter: None,
            older: None,
            asks,
            terminals: terminal::Terminals::default(),
            editors: cx.new(|_| crate::editor::Editors::default()),
            search: None,
            board: None,
            board_card: None,
            goals: None,
            goals_moved: None,
            window_view: WindowView::default(),
            mod_panes: HashMap::new(),
            settings: cx.new(|_| crate::settings::Settings::default()),
            window: window.window_handle(),
            parts: Parts::new(cx),
            panes: crate::panes::Panes::default(),
            _desk_ears: crate::desk::listen(&desks, window, cx),
            desks,
            desks_sent: None,
            desks_telling: None,
            _desks_watched: Vec::new(),
            _sim_news: cx.subscribe(&sims, Self::hear_sims),
            sims,
            _older: None,
        };
        bench.parts.stage_follows(&bench.sims, cx);
        bench.parts.stage_follows(&bench.desks, cx);
        bench._desks_watched = vec![
            cx.observe(&bench.desks, |bench, _, cx| bench.desks_moved(cx)),
            cx.observe(&bench.sims, |bench, _, cx| bench.desks_moved(cx)),
        ];
        bench.parts.stage_follows(&bench.editors, cx);
        bench.parts.stage_follows(&bench.settings, cx);
        bench.keys.focus.focus(window, cx);
        bench.wait_patiently(cx);
        crate::handover::follow(&mut bench, cx);
        bench
    }

    /// Whether the Limits panel is on screen: the tab rail open on its agents page, for one
    /// agent.
    fn limits_shown(&self) -> bool {
        self.rails.tab_open
            && self.rails.page == TabRailPage::Agents
            && !self.rails.all_harnesses
            && self.launcher().is_some()
    }

    /// Reads the plan limits again: each lookup reaches the agent's service on the person's
    /// sign-in, so it runs as the app starts, every few minutes while the Limits panel shows,
    /// and when the person asks.
    pub fn read_usage(&self) {
        self.call("readAgentUsages", json!({}));
    }

    pub fn ask_chat(&self, ask: ChatAsk) {
        self.chat_target(ask.agent()).ask(Ask::Chat(ask));
    }

    /// Where `agent`'s chat asks: the core of the host the agent runs on.
    pub fn chat_target(&self, agent: &str) -> Target {
        self.chats.open.get(agent).map_or_else(
            || Target::new(&self.asks, self.place()),
            |chat| chat.target.clone(),
        )
    }

    /// Holds the chat of the agent the tab in front shows, attaching to it the first time.
    fn open_chat(&mut self, cx: &mut Context<Self>) {
        let Some(AgentRow {
            id: agent,
            provider,
            ..
        }) = self.agent()
        else {
            return;
        };
        if let Some(chat) = self.chats.open.get_mut(&agent) {
            if std::mem::take(&mut chat.resting) {
                let since = chat.mark.clone();
                self.ask_chat(ChatAsk::Attach { agent, since });
            }
            return;
        }
        let mut chat = Chat::new(
            agent.clone(),
            &provider,
            cx.weak_entity(),
            Target::new(&self.asks, self.place()),
            cx,
        );
        chat.rescale(self.preferences().chat_text_scale as f32, cx);
        self.chats.open.insert(agent.clone(), chat);
        self.ask_chat(ChatAsk::Attach { agent, since: None });
    }

    /// Lets go of the chats out of sight once they have stayed so for `CHAT_RESTS_AFTER`.
    fn rest_chats_later(&mut self, cx: &mut Context<Self>) {
        let front = self.agent().map(|agent| agent.id);
        let anyone = self
            .chats
            .open
            .iter()
            .any(|(agent, chat)| !chat.resting && Some(agent) != front.as_ref());
        if !anyone {
            self.chats.resting = None;
            return;
        }
        if self
            .chats
            .resting
            .as_ref()
            .is_some_and(|(counted_from, _)| *counted_from == front)
        {
            return;
        }
        let task = cx.spawn(async move |bench, cx| {
            cx.background_executor().timer(CHAT_RESTS_AFTER).await;
            let _ = bench.update(cx, |bench, cx| {
                bench.rest_chats();
                bench.rest_chats_later(cx);
            });
        });
        self.chats.resting = Some((front, task));
    }

    fn rest_chats(&mut self) {
        self.chats.resting = None;
        let front = self.agent().map(|agent| agent.id);
        let resting: Vec<String> = self
            .chats
            .open
            .iter_mut()
            .filter(|(agent, chat)| {
                Some(*agent) != front.as_ref() && !chat.resting && chat.can_rest()
            })
            .map(|(agent, chat)| {
                chat.resting = true;
                agent.clone()
            })
            .collect();
        for agent in resting {
            self.ask_chat(ChatAsk::Detach { agent });
        }
    }

    /// Makes the composer of the chat in front, unless the goals cover it, and of the goals' Home
    /// while they show, which needs the window.
    fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.window_view == WindowView::Goals {
            if self.chats.start.is_none() {
                let mut composing = self.new_composing(START, window, cx);
                composing.draft.update(cx, |draft, cx| {
                    draft.set_placeholder("What should get done?", window, cx)
                });
                if let Some(drafts) = self.feeds.this.drafts.as_ref() {
                    composing.start_from(drafts, START, window, cx);
                }
                self.chats.start = Some(composing);
                self.watch_drafts();
            }
            if let Some(start) = self.chats.start.as_mut() {
                start.take_arrived(window, cx);
            }
        }
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let covered = self.window_view == WindowView::Goals
            && !(self.goals.as_ref()).is_some_and(|goals| goals.read(cx).wants_chat(&agent));
        if covered {
            return;
        }
        if !(self.chats.open.get(&agent)).is_some_and(|chat| chat.composer.is_none()) {
            return;
        }
        let mut composing = self.new_composing(&agent, window, cx);
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let drafts = match crate::chat::draft::is_elsewhere(chat) {
            true => self.feeds.front_mut().and_then(|feed| feed.drafts.as_ref()),
            false => self.feeds.this.drafts.as_ref(),
        };
        if let Some(drafts) = drafts {
            composing.start_from(drafts, &agent, window, cx);
        }
        chat.composer = Some(composing);
        self.watch_drafts();
        if let Some(text) = self.chats.deliveries.remove(&agent) {
            self.deliver(&agent, text, window, cx);
        }
    }

    /// A composer for `owner`, an agent's chat or the goals' Home, sending what it holds on
    /// Return and keeping it as the person types.
    fn new_composing(
        &mut self,
        owner: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Composing {
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(if owner == crate::chat::START { 1 } else { 2 }, 12)
                .submit_on_enter(true)
        });
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Type the answer")
        });
        let sending = {
            let owner = owner.to_owned();
            let was_empty = Cell::new(true);
            cx.subscribe_in(
                &draft,
                window,
                move |bench, draft, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary,
                    } => bench.send(&owner, *secondary, window, cx),
                    InputEvent::Change => {
                        let empty = draft.read(cx).value().is_empty();
                        if was_empty.replace(empty) != empty {
                            if !empty && owner != START {
                                bench.began_writing(&owner);
                            }
                            cx.notify();
                        }
                        if let Some(composing) = bench.composing_mut(&owner) {
                            composing.drop_untagged(window, cx);
                        }
                        bench.offer(&owner, cx);
                        bench.typed_draft(&owner, cx);
                        if owner == START {
                            bench.start_typed(cx);
                        }
                    }
                    _ => {}
                },
            )
        };
        let keeping = {
            let owner = owner.to_owned();
            cx.subscribe_in(
                &secret,
                window,
                move |bench, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        bench.keep_secret(&owner, window, cx);
                    }
                },
            )
        };
        let mic = {
            let (voice, toggle) = (
                self.voice.clone(),
                crate::chat::dictate(owner, cx.weak_entity()),
            );
            let owner = owner.to_owned();
            cx.new(|cx| crate::voice::MicButton::new(voice, owner, toggle, cx))
        };
        let bench = cx.weak_entity();
        let goals = self.goals.clone().filter(|_| owner == START);
        let view = cx.new(|cx| crate::chat::ComposerView {
            bench,
            owner: owner.to_owned(),
            agent: None,
            covered: false,
            _follows: goals.map(|goals| cx.observe(&goals, |_, _, cx| cx.notify())),
        });
        Composing {
            draft,
            view,
            secret,
            mic,
            hint: SharedString::default(),
            attachments: Vec::new(),
            menu: None,
            context: Vec::new(),
            tracked: None,
            tracked_context: HashMap::new(),
            refusal: None,
            stashed: None,
            stage: ComposeStage::Writing,
            synced: json!({ "text": "" }),
            read_rev: 0,
            arrived: None,
            _keeping: Task::ready(()),
            _heard: [sending, keeping],
        }
    }

    /// Hands `agent` text from elsewhere in the app, such as a terminal's selection, and brings
    /// its tab to the front: typed into its CLI when it runs in a terminal, else at the end of
    /// its chat's draft after a blank line, with the draft focused.
    pub fn deliver(
        &mut self,
        agent: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.this_view().and_then(|view| {
            let saved = view
                .workspace
                .agents
                .iter()
                .find(|saved| saved.id == agent)?;
            Some(saved.project.clone())
        });
        if let Some(project) = project {
            self.call(
                "openTab",
                json!({ "project": project, "kind": "agent", "target": agent }),
            );
        }
        if let Some((place, session)) = self.agent_terminal(agent) {
            let key = self.terminals.key(&place, session);
            crate::terminal::hand_to(self, key, text, cx);
            if let Some(focus) = self.terminals.focus(key) {
                window.focus(focus, cx);
            }
            return;
        }
        let Some(draft) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.draft.clone()))
        else {
            self.chats
                .deliveries
                .entry(agent.to_owned())
                .and_modify(|held| *held = format!("{held}\n\n{text}"))
                .or_insert(text);
            return;
        };
        append(&draft, &text, window, cx);
        draft.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Stops the turn running in `agent`'s chat. The messages waiting behind it come back
    /// to the draft rather than going out after it.
    pub fn stop(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let waiting = chat.waiting().join("\n\n");
        if let Some(composing) = chat.composer.as_ref().filter(|_| !waiting.is_empty()) {
            append(&composing.draft, &waiting, window, cx);
        }
        self.ask_chat(ChatAsk::Cancel {
            agent: agent.to_owned(),
        });
        cx.notify();
    }

    /// Keeps what the empty draft of the chat in front says in step with how the chat stands.
    fn hint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        self.send_after_worktree(&agent, window, cx);
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let mut returned = false;
        if let Some(composing) = chat.composer.as_mut() {
            let empty = composing.draft.read(cx).value().trim().is_empty();
            let returned_held = match std::mem::take(&mut composing.stage) {
                ComposeStage::Returned(held) => Some(held),
                stage => {
                    composing.stage = stage;
                    None
                }
            };
            if let Some(held) = returned_held.filter(|_| empty) {
                composing
                    .draft
                    .update(cx, |draft, cx| draft.set_value(held.content, window, cx));
                composing.attachments = held.attachments;
                composing.context = held.context;
                composing.tracked_context = held.tracked_context;
                returned = true;
            }
            composing.take_arrived(window, cx);
        }
        if returned {
            self.keep_draft(&agent, cx);
        }
        self.focus_draft(&agent, window, cx);
        let steer_first = self.preferences().mid_turn_send == MidTurnSend::Steer;
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let hint = SharedString::from(chat.hint(&agent, self.feeds.questions(), steer_first));
        let Some(composing) = chat
            .composer
            .as_mut()
            .filter(|composing| composing.hint != hint)
        else {
            return;
        };
        composing.hint = hint.clone();
        composing
            .draft
            .update(cx, |draft, cx| draft.set_placeholder(hint, window, cx));
    }

    /// Puts the keys in the draft of the chat in front when it comes to the front and again
    /// once its session is ready, unless the person is typing somewhere other than a draft or
    /// the window itself.
    fn focus_draft(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let shown = (agent.to_owned(), chat.ready());
        if self.chats.focused.as_ref() == Some(&shown) {
            return;
        }
        let Some(draft) = chat
            .composer
            .as_ref()
            .map(|composing| composing.draft.clone())
        else {
            return;
        };
        self.chats.focused = Some(shown);
        let held = window.focused(cx);
        let free = held.as_ref().is_none_or(|held| {
            *held == self.keys.focus
                || self.chats.open.values().any(|chat| {
                    chat.composer
                        .as_ref()
                        .is_some_and(|composing| composing.draft.read(cx).focus_handle(cx) == *held)
                })
        });
        if free {
            draft.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Sends what the person wrote: the answer to a question it answers, else words for
    /// the agent, which join a running turn or wait behind it as the person prefers,
    /// and ⌘ does the other. ⌘ with nothing written sends the questions' answers, else
    /// steers in the messages waiting.
    pub fn send(
        &mut self,
        agent: &str,
        primary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waits = self
            .voice
            .update(cx, |voice, cx| voice.send_after_words(agent, primary, cx));
        if waits {
            return;
        }
        if agent == START {
            return self.start_from_home(window, cx);
        }
        let preparing = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
            .is_some_and(|composing| matches!(composing.stage, ComposeStage::Worktree { .. }));
        if preparing || self.hold_for_install(agent, cx) {
            return;
        }
        if self.starts_in_worktree(agent, cx) {
            let text = self
                .chats
                .open
                .get(agent)
                .and_then(|chat| Some(chat.composer.as_ref()?.draft.read(cx).value().to_string()))
                .unwrap_or_default();
            return self.begin_worktree(agent, text, cx);
        }
        let Some(composing) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
        else {
            return;
        };
        let draft = composing.draft.clone();
        let outgoing = composing.outgoing(cx);
        let text = draft.read(cx).value().trim().to_owned();
        if text.is_empty() && outgoing.0.is_empty() {
            if primary {
                self.answer(
                    agent,
                    |chat, open| {
                        chat.send_answers(open)
                            .map(Into::into)
                            .or_else(|| chat.steer_waiting(agent).map(Into::into))
                    },
                    cx,
                );
                cx.notify();
            }
            return;
        }
        let unknown = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.unknown_command(&text))
            .map(|name| format!("/{name} is not available in this session"));
        if let Some(refused) = unknown {
            if let Some(composing) = self
                .chats
                .open
                .get_mut(agent)
                .and_then(|chat| chat.composer.as_mut())
            {
                composing.refusal = Some(refused.into());
            }
            return cx.notify();
        }
        let sent = Sent {
            content: draft.read(cx).content(),
            attachments: composing.attachments.clone(),
            context: composing.context.clone(),
            tracked_context: composing.tracked_context.clone(),
        };
        let steers = self.preferences().mid_turn_send == MidTurnSend::Steer;
        self.answer(
            agent,
            |chat, open| {
                if let Some(composing) = chat.composer.as_mut() {
                    composing.stage = ComposeStage::Sent(sent);
                }
                chat.sent(agent, text, outgoing, primary == steers, open)
            },
            cx,
        );
        draft.update(cx, |draft, cx| draft.set_value("", window, cx));
        self.keep_draft(agent, cx);
        cx.notify();
    }

    pub fn keep_secret(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.secret.clone()))
        else {
            return;
        };
        let text = field.read(cx).value().to_string();
        let sending = self
            .chats
            .open
            .get(agent)
            .is_some_and(Chat::sending_answers);
        if text.is_empty() || sending {
            return;
        }
        self.answer(
            agent,
            |chat, open| chat.keep_secret(text, open).map(Into::into),
            cx,
        );
        field.update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }

    /// Changes the chat of `agent` with its open questions to hand, and asks the core what that asks.
    pub fn answer(
        &mut self,
        agent: &str,
        change: impl FnOnce(&mut Chat, &[&Question]) -> Option<Answering>,
        cx: &mut Context<Self>,
    ) {
        let open: Vec<&Question> = self
            .feeds
            .questions()
            .filter(|question| question.asks(agent))
            .collect();
        if let Some(ask) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| change(chat, &open))
        {
            self.ask_answering(agent, ask, cx);
        }
    }

    /// Lets go of the questions `keep` turns down, of this host and of the host in front.
    pub fn keep_questions(&mut self, keep: impl Fn(&Question) -> bool) {
        self.feeds.this.questions.retain(&keep);
        if let Some(front) = self.feeds.front_mut() {
            front.questions.retain(keep);
        }
    }

    /// Whether the agent asks before it acts, kept for its next session and changed in the one running.
    pub fn switch_mode(&self, agent: &str, mode: PermissionMode) {
        self.chat_target(agent).call(
            "updateAgent",
            json!({ "agentId": agent, "patch": { "permissionMode": mode } }),
        );
        self.ask_chat(ChatAsk::Mode {
            agent: agent.to_owned(),
            mode,
        });
    }

    /// Lets go of the chats whose agents no longer have a tab. A draft let go of gives the
    /// keyboard back to the workbench, and the chat takes it again if it comes back.
    fn close_chats(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.shown() else {
            return;
        };
        let tabs = view
            .layout
            .projects
            .values()
            .flat_map(|layout| &layout.tabs);
        let shown: Vec<&String> = tabs.filter_map(|tab| shows(tab, PaneKind::Agent)).collect();
        let gone: Vec<String> = self
            .chats
            .open
            .keys()
            .filter(|agent| !shown.contains(agent))
            .cloned()
            .collect();
        let closed = !gone.is_empty();
        for agent in gone {
            if let Some(composing) = self
                .chats
                .open
                .remove(&agent)
                .and_then(|chat| chat.composer)
            {
                self.release_keys(composing.draft.read(cx).focus_handle(cx), cx);
            }
            self.chats.focused.take_if(|(shown, _)| *shown == agent);
            self.ask_chat(ChatAsk::Detach { agent });
        }
        if closed {
            self.watch_drafts();
        }
    }

    /// Asks for the rows after the last the chat holds, when those were dropped.
    pub fn ask_later(&mut self, agent: &str) {
        if let Some(ask) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.later(agent))
        {
            self.ask_chat(ask);
        }
    }

    /// Brings `agent`'s chat to its latest rows.
    pub fn show_latest(&mut self, agent: &str, cx: &mut Context<Self>) {
        if let Some(ask) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.show_latest(agent, cx))
        {
            self.ask_chat(ask);
        }
    }

    /// Asks for the turns before the earliest the chat holds.
    pub fn ask_earlier(&mut self, agent: &str) {
        if let Some(ask) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.earlier(agent))
        {
            self.ask_chat(ask);
        }
    }

    /// The core the panes of the project in front ask: this host's, or the chosen account
    /// host's.
    pub fn target(&self) -> Target {
        Target::new(&self.asks, self.place())
    }

    /// The name of the account host the panes ask while it is out of reach, else none.
    pub(crate) fn out_of_reach(&self) -> Option<SharedString> {
        let crate::hosts::Place::Account(key) = self.place() else {
            return None;
        };
        if self.feeds.host(&key).is_some_and(|feed| feed.reached) {
            return None;
        }
        let named = crate::hosts::HostChoice::Account(key.clone());
        let label = self
            .host_choices()
            .into_iter()
            .find(|option| option.choice == named)
            .map_or_else(|| SharedString::from(key), |option| option.label);
        Some(label)
    }

    /// Tells the Git pane whether its host is in reach, and its repository's code host.
    fn tell_reach(&self, cx: &mut Context<Self>) {
        let away = self.out_of_reach();
        if let Some(pane) = &self.git.pane {
            let host = self.code_host_of(&pane.read(cx).repo);
            pane.update(cx, |pane, cx| {
                pane.set_out_of_reach(away, cx);
                pane.set_code_host(host, cx);
            });
        }
    }

    /// The code host of the project holding `repo`, with the account chosen for it there.
    fn code_host_of(&self, repo: &str) -> Option<(ProjectCodeHost, Option<String>)> {
        let project = self
            .projects()
            .iter()
            .filter(|project| Path::new(repo).starts_with(&project.path))
            .max_by_key(|project| project.path.as_os_str().len())?;
        let host = project.code_host.clone()?;
        let account = project
            .code_hosts
            .get(&host.host)
            .and_then(|choice| choice.account.clone());
        Some((host, account))
    }

    /// The repository the Git views of the project in front open on: one picked from the
    /// list, else the core's pick, the folder when it is one or the one chosen among those
    /// inside it. None while the person has to pick.
    pub(crate) fn git_repo(&self) -> Option<String> {
        let folder = self.watched_project.as_ref()?;
        if self.git.listing {
            return None;
        }
        if let Some((at, repo)) = &self.git.picked {
            if at == folder {
                return Some(repo.clone());
            }
        }
        let found = self.reads.git_project_repo.as_ref()?;
        match &found.repo {
            Some(repo) => Some(repo.clone()),
            None if found.repos.is_some() => None,
            None => Some(folder.clone()),
        }
    }

    /// The project folder the Git views follow.
    pub(crate) fn git_folder(&self) -> Option<&str> {
        self.watched_project.as_deref()
    }

    /// The repositories directly inside the project folder, while it is not one itself.
    pub(crate) fn git_repos(&self) -> Option<&Vec<DiscoveredRepo>> {
        self.reads.git_project_repo.as_ref()?.repos.as_ref()
    }

    /// Opens `repo` in the project's Git views until the project changes.
    pub(crate) fn pick_git_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git.picked = self.watched_project.clone().map(|folder| (folder, repo));
        self.git.listing = false;
        self.follow_git_repo(was, cx);
    }

    /// Shows the repositories inside the project folder again.
    pub(crate) fn list_git_repos(&mut self, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git.listing = true;
        self.follow_git_repo(was, cx);
    }

    /// Points the git overview at the repository shown, when it moved from `was`.
    pub(crate) fn follow_git_repo(&mut self, was: Option<String>, cx: &mut Context<Self>) {
        let repo = self.git_repo();
        if let Some(repo) = repo.as_ref().filter(|repo| {
            Some(*repo) != was.as_ref() || self.reads.git_overview.as_ref().is_none()
        }) {
            let target = self.target();
            target.watch_folder(Read::GitOverview, json!({ "repo": repo }), repo.clone());
            let draft = format!("git:{repo}");
            target.watch(Read::GitDraft, json!({ "keys": [draft] }));
        }
        self.parts.redraw_stage(cx);
    }

    /// Files dropped where nothing takes them go to the terminal holding the keys, else to the
    /// chat in front.
    fn take_stray_drop(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = crate::terminal::focused(self) {
            return crate::terminal::take_dropped(self, id, paths);
        }
        let into = match self.window_view {
            WindowView::Goals => match self.chat_in_goals(cx) {
                Some(agent) => Some(agent),
                None => Some(START.to_owned()),
            },
            WindowView::Workbench => self.chats.focused.clone().map(|(agent, _)| agent),
        };
        if let Some(into) = into {
            self.attach(&into, paths.to_vec(), window, cx);
        }
    }

    /// Whether anything is over the whole window: a phone waiting to pair, a command's terminal,
    /// or the welcome, the release notes or runtime diagnostics.
    pub(crate) fn over_shown(&self) -> bool {
        self.overlays.in_layer(Layer::Over) || self.someone_waits()
    }

    /// The clock and every "ago" move each minute, which draws the whole window again. What
    /// counts seconds, such as a running tool call or the Run chip, counts them itself.
    fn until_next_minute() -> Duration {
        let now = chrono::Local::now();
        let into_second = Duration::from_nanos(u64::from(now.timestamp_subsec_nanos()));
        Duration::from_secs(60 - u64::from(now.second())) - into_second
    }

    /// The run of the project in front's tasks the person started that is going, and the last.
    pub fn person_runs(&self) -> (Option<&RunView>, Option<&RunView>) {
        let Some(path) = self.watched_project.as_deref() else {
            return (None, None);
        };
        let mine: Vec<&RunView> = (self.reads.runs.iter())
            .filter(|run| run.run.project == path && run.agent_id.is_none())
            .collect();
        (
            mine.iter()
                .rev()
                .find(|run| run.run.status.is_active())
                .copied(),
            mine.last().copied(),
        )
    }

    /// The port the task run `execution` listens on, when it listens on one.
    pub fn port_of_run(&self, execution: &str) -> Option<&ProjectPort> {
        self.reads.ports.iter().find(|port| {
            matches!(&port.listening.owner, PortOwner::Task { execution_id, .. } if execution_id == execution)
        })
    }

    /// Counts the seconds on the Run chip while the person's task runs with no port to show.
    pub(crate) fn time_run(&mut self, cx: &mut Context<Self>) {
        let run = self.person_runs().0;
        let counting = run.is_some_and(|run| self.port_of_run(&run.run.execution_id).is_none());
        let started_at = run.and_then(|run| run.run.started_at).map(|at| at as i64);
        self.run_clock
            .update(cx, |clock, cx| clock.set(counting, started_at, cx));
    }

    /// Makes `task` the one the project's Run button starts.
    pub fn pick_task(&mut self, task: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.watched_project.clone() {
            self.picked_tasks.insert(project, task.to_owned());
            cx.notify();
        }
    }

    /// Opens the Add a process dialog for the project whose processes the Run menu lists.
    pub fn add_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.watched_project.clone() {
            self.overlays.adding_task = Some(crate::add_task::AddTask::new(project, window, cx));
            cx.notify();
        }
    }

    /// Opens the task dialog on the person's own `task` of `project`, to change it.
    pub fn edit_task(
        &mut self,
        project: &str,
        task: &TaskView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.adding_task = Some(crate::add_task::AddTask::edit(
            project.to_owned(),
            task,
            window,
            cx,
        ));
        cx.notify();
    }

    /// Brings forward the project's tab showing `target`, opening one when none does.
    pub fn show(&self, kind: &str, target: &str, title: Option<&str>) {
        if let Some(project) = self.project() {
            let tab =
                json!({ "project": project.id, "kind": kind, "target": target, "title": title });
            self.call_in_front("openTab", tab);
        }
    }

    /// The path of the project whose tasks, runs and ports the store watches.
    /// Whether the person chose the rails' Compact density.
    pub fn rail_compact(&self) -> bool {
        self.preferences().rail_density == sikemux_view::prefs::RailDensity::Compact
    }

    pub fn window_handle(&self) -> AnyWindowHandle {
        self.window
    }

    pub fn stop_run(&self, execution: &str) {
        self.call_in_front("taskStop", json!({ "executionId": execution }));
    }

    pub fn picture(&self) -> Option<Arc<Image>> {
        self.picture.image()
    }

    /// The person's preferences, once the core has said them.
    pub fn preferences_read(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    pub fn preferences(&self) -> &Preferences {
        static UNREAD: std::sync::LazyLock<Preferences> =
            std::sync::LazyLock::new(Preferences::default);
        self.preferences.as_ref().unwrap_or(&UNREAD)
    }

    /// Shows `note` in the window's notifications.
    pub fn notify(&self, note: Notification, cx: &mut Context<Self>) {
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| window.push_notification(note, cx));
        });
    }

    /// Shows `note` for `shown`, longer than a notification hides itself after, then takes it away.
    pub fn notify_for<T: 'static>(
        &self,
        note: Notification,
        shown: Duration,
        cx: &mut Context<Self>,
    ) {
        let window = self.window;
        self.notify(note.id::<T>().autohide(false), cx);
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(shown).await;
            let _ = window.update(cx, |_, window, cx| window.remove_notification::<T>(cx));
        })
        .detach();
    }

    pub fn call(&self, method: &'static str, params: Value) {
        Target::here(&self.asks).call(method, params);
    }

    /// The view of the host in front: another host's own, while one on the account is chosen,
    /// else this host's.
    pub fn shown(&self) -> Option<&DeviceView> {
        match self.this_view()?.layout.chosen_host.as_ref() {
            Some(key) => self.feeds.host(key)?.view.as_ref(),
            None => self.this_view(),
        }
    }

    /// The view of the host at `place`, as its core sends it.
    pub fn view_at(&self, place: &Place) -> Option<&DeviceView> {
        self.feeds.at(place)?.view.as_ref()
    }

    /// This host's view, as its core sends it, whichever host is in front.
    pub fn this_view(&self) -> Option<&DeviceView> {
        self.feeds.this.view.as_ref()
    }

    /// One of the core's methods on the host in front, for what the person did there.
    pub fn call_in_front(&self, method: &'static str, params: Value) {
        self.target().call(method, params);
    }

    pub fn projects(&self) -> &[ProjectInfo] {
        self.shown()
            .map_or(&[], |view| view.workspace.projects.as_slice())
    }

    /// The project in front, as the core's layout says.
    pub fn project(&self) -> Option<&ProjectInfo> {
        let active = self.shown()?.layout.active_project.as_deref();
        let projects = self.projects();
        active
            .and_then(|id| projects.iter().find(|project| project.id == id))
            .or(projects.first())
    }

    pub fn launchers(&self) -> &[LauncherInfo] {
        self.shown()
            .map_or(&[], |view| view.workspace.launchers.as_slice())
    }

    /// The agents the host has that run only in their own terminal, such as Pi.
    pub fn terminal_launchers(&self) -> &[LauncherInfo] {
        self.shown()
            .map_or(&[], |view| view.workspace.terminal_launchers.as_slice())
    }

    /// Whether the host in front has the agent `provider`.
    pub fn has_agent(&self, provider: &str) -> bool {
        self.launchers().iter().any(|launcher| {
            launcher.provider == provider && launcher.status.as_deref() != Some("missing")
        })
    }

    /// Whether agents of `provider` have the built-in chat, as the core's catalog says; any agent
    /// does until the catalog is read.
    pub fn has_chat(&self, provider: &str) -> bool {
        self.reads
            .catalog
            .kind(provider)
            .is_none_or(|kind| kind.chat.is_some())
    }

    /// What a new chat started from the rail starts with.
    pub fn launcher(&self) -> Option<&LauncherInfo> {
        let launchers = self.launchers();
        let chosen = |id: &str| launchers.iter().find(|launcher| launcher.id == id);
        let of_kind = |kind: &str| self.launcher_of_kind(kind);
        let last_kind = self.preferences().last_agent_type.as_deref();
        self.launcher_choice
            .as_deref()
            .and_then(of_kind)
            .or_else(|| {
                let project = self.project()?.launcher.as_deref().and_then(chosen)?;
                of_kind(&project.provider)
            })
            .or_else(|| last_kind.and_then(of_kind))
            .or_else(|| of_kind(&launchers.first()?.provider))
    }

    /// The launcher that starts the kind of agent on the account the person chose for it.
    pub fn launcher_of_kind(&self, kind: &str) -> Option<&LauncherInfo> {
        let launchers = self.launchers();
        self.accounts
            .current(kind)
            .and_then(|account| {
                let id = format!("{kind}:{}", account.id);
                launchers.iter().find(|launcher| launcher.id == id)
            })
            .or_else(|| launchers.iter().find(|launcher| launcher.id == kind))
            .or_else(|| launchers.iter().find(|launcher| launcher.provider == kind))
    }

    /// What agents of `provider` are called, such as "Claude", whichever account they run on.
    pub fn agent_name<'a>(&'a self, provider: &'a str) -> &'a str {
        self.reads.catalog.label(provider)
    }

    /// The launcher's account's plan usage as the core last read it, or why it has none.
    pub fn usage_of(&self, launcher: &str) -> Option<&AccountUsage> {
        (self.reads.usage.iter()).find(|account| account.launcher == launcher)
    }

    /// The most of any window of its plan the launcher's account has used.
    pub fn usage_peak(&self, launcher: &str) -> Option<f64> {
        let usage = self.usage_of(launcher)?.usage.as_ref()?;
        (usage.windows.iter())
            .map(|window| window.used_percent.clamp(0., 100.))
            .reduce(f64::max)
    }

    /// The file the tab in front shows, when it is an editor.
    pub fn front_file(&self) -> Option<PathBuf> {
        let (kind, target) = crate::tabs::first_view(&self.tab()?.pane)?;
        (kind == PaneKind::Files).then(|| PathBuf::from(target))
    }

    /// Brings the file's editor tab to the front, opening one when none shows it.
    pub fn open_file(&mut self, path: &Path, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::File(path.to_owned()), cx);
        self.show_file(path, false);
    }

    /// Opens the file's editor to look at, in place of the one opened that way last.
    pub fn preview_file(&self, path: &Path) {
        self.show_file(path, true);
    }

    fn show_file(&self, path: &Path, preview: bool) {
        if let Some(project) = self.project() {
            self.call_in_front(
                "openTab",
                json!({
                    "project": project.id,
                    "kind": "files",
                    "target": path.to_string_lossy(),
                    "preview": preview,
                }),
            );
        }
    }

    /// Opens settings on `page`, as a link to one of them does.
    pub fn open_settings(
        &mut self,
        page: crate::settings::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::open(self, page, window, cx);
    }

    /// A terminal tab in the project in front, on the shell the core keeps ready there.
    /// Starts a chat with the agent the rail is set to, in the project in front.
    /// Starts a chat in the project in front, or in the one picked when no project is in front,
    /// or on an SSH host's strip opens its Install Sikemux pane.
    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.strip() {
            Some(Strip::Host(alias)) => self.open_install_pane(&alias),
            Some(Strip::Project(project)) => self.start_chat_in(&project, window, cx),
            _ => self.pick_project(crate::picker::ThenPicked::StartChat, window, cx),
        }
    }

    /// Starts a chat in `project` with the agent the rail starts with, or offers every agent when
    /// none is installed.
    pub fn start_chat_in(&mut self, project: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.launcher() {
            Some(launcher) => {
                self.target()
                    .start_chat(launcher.id.clone(), project.to_owned(), None, None)
            }
            None => self.toggle_agent_palette(window, cx),
        }
    }

    /// Keeps a rail open, or closes it; a peeking rail that is kept stays where it is.
    pub fn toggle_rail(&mut self, edge: Edge, cx: &mut Context<Self>) {
        let open = match edge {
            Edge::Start => &mut self.rails.side_open,
            Edge::End => &mut self.rails.tab_open,
        };
        *open = !*open;
        self.rails.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    /// Swaps an agent's title in its header, or its tab rail row when `in_rail`, for a field
    /// holding it, which Enter or clicking away saves and Escape leaves unchanged.
    pub fn retitle(
        &mut self,
        agent: &str,
        in_rail: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(title) = self.this_view().and_then(|view| {
            view.workspace
                .agents
                .iter()
                .find(|kept| kept.id == agent)
                .map(|kept| kept.title.clone())
        }) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title.clone()));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        let events = cx.subscribe(&input, |bench, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                bench.finish_retitle(true, cx);
            }
        });
        self.overlays.retitling = Some(Retitling {
            agent: agent.to_owned(),
            in_rail,
            input,
            title,
            _events: events,
        });
        self.parts.redraw_stage(cx);
        self.parts.redraw(&self.parts.rail, cx);
    }

    pub fn finish_retitle(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(retitling) = self.overlays.retitling.take() else {
            return;
        };
        let title: String = retitling
            .input
            .read(cx)
            .value()
            .trim()
            .chars()
            .take(72)
            .collect();
        if save && !title.is_empty() && title != retitling.title {
            self.call_in_front(
                "updateAgent",
                json!({ "agentId": retitling.agent, "patch": { "title": title, "renamed": true } }),
            );
        }
        self.parts.redraw_stage(cx);
        self.parts.redraw(&self.parts.rail, cx);
    }

    /// Puts the core's next trust question to the person, each once, while nothing else is asked.
    pub(crate) fn ask_trust(&mut self, cx: &mut Context<Self>) {
        let free = self.overlays.asking.is_none();
        let Some((host, ask)) = self.trust.next(&self.feeds, free) else {
            return;
        };
        let id = ask.id.clone();
        let on = host.as_ref().and_then(|host| {
            self.host_choices()
                .into_iter()
                .find(|option| option.choice == crate::hosts::HostChoice::Account(host.clone()))
                .map(|option| option.label)
        });
        let commands: Vec<String> = (ask.commands.iter())
            .map(|command| format!("{}: {}", command.label, command.command))
            .collect();
        let body: Vec<String> = [ask.body.clone(), commands.join("\n")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        #[cfg(feature = "drive")]
        crate::drive::trace(format_args!("trust question set"));
        let title = match on {
            Some(name) => format!("{} on {name}", ask.title),
            None => ask.title.clone(),
        };
        self.confirm(
            title,
            body.join("\n\n"),
            ask.confirm_label,
            false,
            move |bench, trust, _, _| {
                let answer = Ask::Call {
                    method: "answerTrust",
                    params: json!({ "id": id, "trust": trust }),
                };
                let _ = bench.asks.send(match &host {
                    Some(host) => Ask::On(host.clone(), Box::new(answer)),
                    None => answer,
                });
            },
            cx,
        );
    }

    /// Removes an agent's worktree once the person knows what that loses: uncommitted
    /// changes, and commits only its branch has, which a merged pull request already keeps.
    pub fn remove_worktree(
        &mut self,
        agent: &str,
        pull_merged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = agent.to_owned();
        let cost = self
            .chat_target(&agent)
            .request("worktreeRemoval", json!({ "agentId": agent }));
        cx.spawn_in(window, async move |bench, cx| {
            let cost = cost.await;
            let _ = bench.update_in(cx, |bench, window, cx| match cost {
                Ok(cost) => bench.confirm_removal(agent, &cost, pull_merged, window, cx),
                Err(error) => window.push_notification(
                    toast::error(format!("Could not remove the worktree: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn confirm_removal(
        &mut self,
        agent: String,
        cost: &Value,
        pull_merged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cost["sharedWith"]
            .as_array()
            .is_some_and(|shared| !shared.is_empty())
        {
            return window.push_notification(
                "Another agent or project still uses this worktree. Close it before removing the worktree.",
                cx,
            );
        }
        if cost["busy"] == true {
            return window
                .push_notification("Stop the agent's turn before removing its worktree", cx);
        }
        let branch = cost["branch"].as_str().unwrap_or_default().to_owned();
        let path = cost["path"].as_str().unwrap_or_default().to_owned();
        let dirty = cost["dirtyFiles"].as_u64().unwrap_or(0);
        let unmerged = !pull_merged && cost["unmergedCommits"].as_u64().unwrap_or(0) > 0;
        let named = branch.clone();
        let then_branch = move |bench: &mut Workbench,
                                discard: bool,
                                window: &mut Window,
                                cx: &mut Context<Workbench>| {
            let (agent, named) = (agent.clone(), named.clone());
            if !unmerged {
                return bench.finish_removal(agent, named, discard, false, window, cx);
            }
            bench.confirm(
                format!("Delete branch {named} too?"),
                "It has commits that are not in the branch it started from. Deleting it loses any that were not pushed.",
                "Delete branch",
                true,
                move |bench, delete, window, cx| {
                    bench.finish_removal(agent.clone(), named.clone(), discard, delete, window, cx)
                },
                cx,
            );
        };
        if dirty == 0 {
            return then_branch(self, false, window, cx);
        }
        let changes = if dirty == 1 { "change" } else { "changes" };
        self.confirm(
            format!("Remove worktree {branch}?"),
            format!("{dirty} uncommitted {changes} in {path} will be lost."),
            "Remove",
            true,
            move |bench, remove, window, cx| {
                if remove {
                    then_branch(bench, true, window, cx);
                }
            },
            cx,
        );
    }

    fn finish_removal(
        &self,
        agent: String,
        branch: String,
        discard: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params =
            json!({ "agentId": agent, "discardChanges": discard, "deleteBranch": delete_branch });
        let removing = self
            .chat_target(&agent)
            .request("removeAgentWorktree", params);
        cx.spawn_in(window, async move |_, cx| {
            let removed = removing.await;
            let _ = cx.update(|window, cx| {
                window.push_notification(
                    match removed {
                        Ok(answer) if answer["branchDeleted"] == true => {
                            toast::success(format!("Removed worktree and branch {branch}"))
                        }
                        Ok(_) => toast::success(format!("Removed worktree {branch}")),
                        Err(error) => {
                            toast::error(format!("Could not remove worktree {branch}: {error}"))
                        }
                    },
                    cx,
                )
            });
        })
        .detach();
    }

    /// The terminal an agent runs in, and the host it is on, while it runs in one rather than
    /// as a chat.
    pub fn agent_terminal(&self, agent: &str) -> Option<(Place, SessionId)> {
        let place = self.chat_target(agent).place().clone();
        let session = self.kept_agent(agent)?.pty_id?;
        Some((place, session))
    }

    /// Whether the agent runs in its CLI in a terminal, there now or starting again.
    pub fn in_terminal(&self, agent: &str) -> bool {
        self.kept_agent(agent)
            .is_some_and(|kept| kept.terminal || kept.pty_id.is_some())
    }

    /// Whether the agent is asleep with its terminal ended, to start again on its session.
    pub fn terminal_asleep(&self, agent: &str) -> bool {
        self.kept_agent(agent)
            .is_some_and(|kept| kept.terminal && kept.asleep && kept.pty_id.is_none())
    }

    /// The agent as the core of the host it runs on keeps it.
    fn kept_agent(&self, agent: &str) -> Option<&sikemux_wire::protocol::WorkspaceAgent> {
        self.view_at(self.chat_target(agent).place())?
            .workspace
            .agents
            .iter()
            .find(|kept| kept.id == agent)
    }

    /// Moves an agent between its chat and its own CLI in a terminal, carrying what was
    /// typed and not sent.
    pub fn switch_view(&self, agent: &str, terminal: bool) {
        let to = if terminal { "terminal" } else { "chat" };
        self.chat_target(agent)
            .call("switchAgentView", json!({ "agentId": agent, "to": to }));
    }

    pub fn rail_width(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Start => self.rails.side_width,
            Edge::End => self.rails.tab_width,
        }
    }

    /// Sets a rail's width, within what that rail allows, and draws it at once.
    fn resize_rail(&mut self, edge: Edge, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        let (kept, range) = match edge {
            Edge::Start => (&mut self.rails.side_width, rail::SIDE_RAIL),
            Edge::End => (&mut self.rails.tab_width, agents::TAB_RAIL),
        };
        let width = width.clamp(range.min, range.max).round();
        if *kept != width {
            *kept = width;
            window.refresh();
            self.keep_rails(cx);
        }
    }

    /// Moves the edge between a rail and the stage `by` points to the right.
    fn nudge_rail(&mut self, edge: Edge, by: f32, window: &mut Window, cx: &mut Context<Self>) {
        let outward = match edge {
            Edge::Start => by,
            Edge::End => -by,
        };
        self.resize_rail(edge, self.rail_width(edge) + outward, window, cx);
    }

    /// The handle between a rail and the stage, dragged or moved by arrow keys to resize the rail.
    fn rail_resizer(&self, edge: Edge, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::keymap::{EdgeFarLeft, EdgeFarRight, EdgeLeft, EdgeRight};
        div()
            .id(match edge {
                Edge::Start => "resize-side-rail",
                Edge::End => "resize-tab-rail",
            })
            .flex_none()
            .w(px(RESIZER_WIDTH))
            .h_full()
            .cursor_col_resize()
            .key_context("RailResizer")
            .track_focus(&self.rails.resizers[edge as usize])
            .tooltip(crate::ui::tooltip("Drag or use arrow keys to resize"))
            .on_click(cx.listener(move |bench, _, window, cx| {
                bench.rails.resizers[edge as usize].focus(window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeLeft, window, cx| {
                bench.nudge_rail(edge, -16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeRight, window, cx| {
                bench.nudge_rail(edge, 16., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarLeft, window, cx| {
                bench.nudge_rail(edge, -40., window, cx)
            }))
            .on_action(cx.listener(move |bench, _: &EdgeFarRight, window, cx| {
                bench.nudge_rail(edge, 40., window, cx)
            }))
            .on_drag(DraggedRail(edge), |rail, _, _, cx| {
                cx.new(|_| DraggedRail(rail.0))
            })
    }

    /// Takes the rails as the person last left them, from the core's preferences.
    pub(crate) fn follow_rails(&mut self, preferences: &Preferences) {
        self.rails.follow(preferences);
        self.window_view = preferences.window_view;
        self.recent_commands = preferences
            .recent_command_keys
            .iter()
            .map(|key| SharedString::from(key.clone()))
            .collect();
    }

    /// Keeps the rails and the commands run lately in the core's preferences once changes
    /// settle, so a drag is written once, and the rails come back as they were. Nothing is
    /// kept before the preferences are first read, when the rails hold only the window's defaults.
    pub fn keep_rails(&mut self, cx: &mut Context<Self>) {
        if self.preferences.is_none() {
            return;
        }
        self.rails.keeping = Some(cx.spawn(async move |bench, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = bench.update(cx, |bench, _| {
                bench.rails.keeping = None;
                bench.write_rails();
            });
        }));
    }

    fn write_rails(&self) {
        let page = self.rails.page;
        let scope = if self.rails.all_projects {
            TabRailScope::All
        } else {
            TabRailScope::Project
        };
        let recent: Vec<&str> = self
            .recent_commands
            .iter()
            .map(|key| key.as_ref())
            .collect();
        let now = [
            ("sideRailOpen", json!(self.rails.side_open)),
            ("sideRailWidth", json!(self.rails.side_width)),
            ("tabRailOpen", json!(self.rails.tab_open)),
            ("tabRailWidth", json!(self.rails.tab_width)),
            ("tabTabRailPage", json!(page)),
            ("tabRailScope", json!(scope)),
            ("tabRailAllAgents", json!(self.rails.all_harnesses)),
            ("windowView", json!(self.window_view)),
            ("recentCommandKeys", json!(recent)),
        ];
        let kept = self
            .preferences
            .as_ref()
            .and_then(|kept| serde_json::to_value(kept).ok());
        for (key, value) in now {
            if kept.as_ref().and_then(|kept| kept.get(key)) != Some(&value) {
                crate::settings::set(self, key, value);
            }
        }
    }

    /// Shows the workbench in place of the goals, as anything opened in one of its tabs needs.
    pub fn show_workbench(&mut self, cx: &mut Context<Self>) {
        if self.window_view == WindowView::Goals {
            self.toggle_goals_view(cx);
        }
    }

    /// The agent whose chat the goals show, while they do.
    pub fn chat_in_goals(&self, cx: &App) -> Option<String> {
        let goals = self.goals.as_ref()?;
        let chat = goals.read(cx).chat_shown()?.to_owned();
        (self.window_view == WindowView::Goals).then_some(chat)
    }

    /// Switches the window between the workbench and the goals, which open on Home.
    pub fn toggle_goals_view(&mut self, cx: &mut Context<Self>) {
        self.window_view = match self.window_view {
            WindowView::Workbench => WindowView::Goals,
            WindowView::Goals => WindowView::Workbench,
        };
        if let (WindowView::Goals, Some(goals)) = (self.window_view, &self.goals) {
            goals.update(cx, |goals, cx| goals.home(cx));
        }
        self.keep_rails(cx);
        cx.notify();
    }

    /// Shows `goal` in the goals view.
    pub fn open_goal(&mut self, goal: &NoticeGoal, cx: &mut Context<Self>) {
        if self.window_view != WindowView::Goals {
            self.toggle_goals_view(cx);
        }
        self.goals_view(cx)
            .update(cx, |goals, cx| goals.open_goal(goal, cx));
    }

    /// Whether the window shows the goals rather than the workbench.
    pub fn showing_goals(&self) -> bool {
        self.window_view == WindowView::Goals
    }

    /// The goals view, made the first time it shows.
    fn goals_view(&mut self, cx: &mut Context<Self>) -> Entity<crate::goals::GoalsPane> {
        if let Some(goals) = &self.goals {
            return goals.clone();
        }
        let (asks, bench) = (self.asks.clone(), cx.entity().downgrade());
        let here = (self.project()).map(|project| project.path.to_string_lossy().into_owned());
        let goals = cx.new(|cx| crate::goals::GoalsPane::new(asks, bench, here, cx));
        self.goals_moved =
            Some(cx.subscribe(&goals, |_, _, _: &crate::goals::Moved, cx| cx.notify()));
        self.goals = Some(goals.clone());
        goals
    }

    /// Opens both rails when either is closed, else closes both.
    pub fn toggle_focus_mode(&mut self, cx: &mut Context<Self>) {
        let open = !(self.rails.side_open && self.rails.tab_open);
        self.rails.side_open = open;
        self.rails.tab_open = open;
        self.rails.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    /// Shows a closed rail over the stage while the pointer is at its edge or over it, and puts
    /// it away a moment after the pointer leaves.
    fn peek(&mut self, edge: Edge, inside: bool, cx: &mut Context<Self>) {
        if inside {
            self.rails.unpeek = None;
            if self.rails.peeking != Some(edge) {
                self.rails.peeking = Some(edge);
                cx.notify();
            }
            return;
        }
        self.rails.unpeek = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PEEK_CLOSE).await;
            let _ = this.update(cx, |bench, cx| {
                if bench.rails.peeking == Some(edge) {
                    bench.rails.peeking = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Takes up a saved chat again, in the project it ran in, and brings its tab forward.
    /// It goes by the chat's session rather than its place in the core's list, which forgets
    /// the oldest chats any device paged to.
    pub fn resume_chat(&self, recent: &RecentInfo, cx: &mut Context<Self>) {
        let Some(project) = self
            .projects()
            .iter()
            .find(|known| known.id == recent.project)
        else {
            return;
        };
        let session = recent
            .id
            .strip_prefix(&format!("{}:", recent.provider))
            .unwrap_or(&recent.id);
        let asked = json!({
            "provider": recent.provider,
            "sessionId": session,
            "project": project.path,
            "cwd": recent.cwd,
            "title": recent.title,
        });
        let target = self.target();
        let taken = target.request("resumeSession", asked);
        cx.spawn(async move |bench, cx| match taken.await {
            Ok(taken) => {
                let tab = json!({ "project": taken["project"], "kind": "agent", "target": taken["agentId"] });
                target.call("openTab", tab);
            }
            Err(error) => {
                let note = toast::error(format!("The chat did not open: {error}"));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
            }
        })
        .detach();
    }

    /// Brings forward the tab `by` places from the one in front, among those `same`
    /// accepts, going round the strip.
    fn step_tab(&self, by: isize, same: impl Fn(&Tab, &Tab) -> bool) {
        let (Some(strip), Some(front)) = (self.strip(), self.tab()) else {
            return;
        };
        let tabs: Vec<&Tab> = self
            .tabs(&strip)
            .iter()
            .filter(|tab| same(tab, front))
            .collect();
        let Some(at) = tabs.iter().position(|tab| tab.id == front.id) else {
            return;
        };
        let to = (at as isize + by).rem_euclid(tabs.len() as isize) as usize;
        self.activate_tab(&strip, &tabs[to].id);
    }

    /// Brings forward the tab at `place` in the strip, counting from one, or the last tab.
    fn go_to_tab(&self, place: Option<usize>) {
        let Some(strip) = self.strip() else {
            return;
        };
        let tabs = self.tabs(&strip);
        if let Some(tab) = place.map_or(tabs.last(), |place| tabs.get(place - 1)) {
            self.activate_tab(&strip, &tab.id);
        }
    }

    /// Brings forward the first terminal tab in the strip in front, else opens a terminal.
    fn focus_terminal(&self, cx: &mut App) {
        let terminal = self.strip().and_then(|strip| {
            let tab = self.tabs(&strip).iter().find(|tab| {
                tabs::first_view(&tab.pane).is_some_and(|(kind, _)| kind == PaneKind::Terminal)
            })?;
            Some((strip.clone(), tab.id.clone()))
        });
        match terminal {
            Some((strip, tab)) => self.activate_tab(&strip, &tab),
            None => self.new_terminal(cx),
        }
    }

    pub(crate) fn git_in_front(&self) -> bool {
        self.tab()
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| kind == PaneKind::Git)
    }

    /// Makes the pane of the tab in front, for the kinds made the first time they come to the
    /// front.
    fn make_front_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tell_reach(cx);
        let front = self
            .tab()
            .and_then(|tab| tabs::first_view(&tab.pane))
            .map(|(kind, _)| kind);
        if let Some(kind) = front {
            crate::pane_kind::make(kind, self, window, cx);
        }
    }

    /// Makes the Git pane when the project's Git tab comes to the front.
    pub(crate) fn git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        let Some(repo) = self.git_repo() else {
            return;
        };
        if project.path.to_string_lossy() != *self.watched_project.as_deref().unwrap_or_default() {
            return;
        }
        let target = self.target();
        if let Some(pane) = self.git.pane.as_ref().filter(|pane| {
            let pane = pane.read(cx);
            pane.repo == repo && pane.place() == target.place()
        }) {
            if std::mem::take(&mut self.git.behind) {
                let overview = self.reads.git_overview.as_ref();
                pane.update(cx, |pane, cx| {
                    if let Some(overview) = overview {
                        pane.overview(overview, cx);
                    }
                    pane.git_moved(None, None, cx);
                });
            }
            return;
        }
        self.git.behind = false;
        let bench = cx.entity().downgrade();
        let away = self.out_of_reach();
        let host = self.code_host_of(&repo);
        let pane = cx.new(|cx| GitPane::new(target, repo, bench, window, cx));
        pane.update(cx, |pane, cx| {
            pane.set_out_of_reach(away, cx);
            pane.set_code_host(host, cx);
        });
        if let Some(overview) = self.reads.git_overview.as_ref() {
            pane.update(cx, |pane, cx| pane.overview(overview, cx));
        }
        if !self.reads.catalog.is_empty() {
            let catalog = &self.reads.catalog;
            pane.update(cx, |pane, cx| pane.catalog(catalog, cx));
        }
        if let Some(drafts) = self.reads.git_draft.as_ref() {
            pane.update(cx, |pane, cx| pane.drafts(drafts, cx));
        }
        if let Some(sha) = self.git.wanted_commit.take() {
            pane.update(cx, |pane, cx| pane.show_commit(&sha, cx));
        }
        self.git.pane = Some(pane);
    }

    /// Makes the Search pane when the project's Search tab comes to the front.
    pub(crate) fn search_pane(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        let project = project.path.to_string_lossy().into_owned();
        let target = self.target();
        if self.search.as_ref().is_some_and(|pane| {
            let pane = pane.read(cx);
            pane.project == project && pane.place() == target.place()
        }) {
            return;
        }
        let bench = cx.weak_entity();
        self.search = Some(cx.new(|cx| SearchPane::new(target, project, bench, cx)));
    }

    /// Makes the board pane when the project's board tab comes to the front.
    pub(crate) fn board_pane(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        let folder = project.path.to_string_lossy().into_owned();
        let pane = match self.board.clone() {
            Some(pane) if pane.read(cx).folder == folder => pane,
            _ => {
                let (asks, bench) = (self.asks.clone(), cx.entity().downgrade());
                let pane =
                    cx.new(|cx| crate::board::BoardPane::new(asks, folder.clone(), bench, cx));
                self.board = Some(pane.clone());
                pane
            }
        };
        if let Some((_, card)) = self.board_card.take_if(|(at, _)| *at == folder) {
            pane.update(cx, |pane, cx| pane.reveal(card, cx));
        }
    }

    /// Brings the project's Search tab to the front with the keyboard in its find field,
    /// opening one when it has none, and searching for what is selected, as on the Mac.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        self.call_in_front(
            "openTab",
            json!({ "project": project.id, "kind": "search", "target": project.path.to_string_lossy() }),
        );
        let seed = self.selection(window, cx);
        self.search_pane(cx);
        if let Some(search) = self.search.clone() {
            search.update(cx, |search, cx| search.focus_find(seed, window, cx));
        }
    }

    /// The first line of the text selected where the keyboard is: a field or file, or a chat.
    fn selection(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let selected = match window.focused_input(cx) {
            Some(AnyInputState::Input(state)) => Some(state.read(cx).selected_text().to_string()),
            Some(AnyInputState::Textarea(state)) => {
                Some(state.read(cx).selected_text().to_string())
            }
            Some(AnyInputState::Editor(state)) => Some(state.read(cx).selected_text().to_string()),
            _ => self
                .chats
                .open
                .values()
                .find_map(|chat| chat.transcript.read(cx).focused_selection(window))
                .or_else(|| Some(gpui_kit::base::TextSelection::selected_text(window, cx))),
        }?;
        selected
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::to_owned)
    }

    /// Brings the project's Git tab to the front, opening one when it has none.
    pub fn open_git(&mut self, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Git, cx);
        if let Some(project) = self.project() {
            self.call_in_front(
                "openTab",
                json!({ "project": project.id, "kind": "git", "target": project.path.to_string_lossy() }),
            );
        }
    }

    /// Opens the Git tab of the project in front at commit `sha`.
    pub fn open_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        self.open_git(cx);
        match self.git.pane.clone() {
            Some(pane) => pane.update(cx, |pane, cx| pane.show_commit(sha, cx)),
            None => self.git.wanted_commit = Some(sha.to_owned()),
        }
    }

    /// Opens a terminal in the project in front, or among the terminals of no project while
    /// those are in front.
    pub fn new_terminal(&self, cx: &mut App) {
        match (self.strip(), self.project()) {
            (Some(Strip::Terminals), _) => self.new_loose_terminal(cx),
            (Some(Strip::Host(alias)), _) => self.open_ssh_terminal(&alias, cx),
            (_, Some(project)) => {
                self.take_keys_when_shown(TakesKeys::Terminal, cx);
                self.call_in_front("newTerminal", json!({ "project": project.id }))
            }
            _ => {}
        }
    }

    pub fn new_loose_terminal(&self, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Terminal, cx);
        self.call("newTerminal", Strip::Terminals.with(json!({})));
    }

    pub fn activate_project(&self, id: &str) {
        self.call_in_front("activateProject", json!({ "project": id }));
    }

    /// The strip in front: the terminals or SSH host shown instead of a project, else the
    /// project in front.
    pub fn strip(&self) -> Option<Strip> {
        let layout = &self.shown()?.layout;
        if layout.terminals_in_front {
            return Some(Strip::Terminals);
        }
        match layout
            .active_host
            .as_ref()
            .filter(|host| layout.hosts.contains_key(*host))
        {
            Some(host) => Some(Strip::Host(host.clone())),
            None => self
                .project()
                .map(|project| Strip::Project(project.id.clone())),
        }
    }

    fn layout_of(&self, strip: &Strip) -> Option<&ProjectLayout> {
        let layout = &self.shown()?.layout;
        match strip {
            Strip::Project(id) => layout.projects.get(id),
            Strip::Host(alias) => layout.hosts.get(alias),
            Strip::Terminals => Some(&layout.terminals),
        }
    }

    pub fn tabs(&self, strip: &Strip) -> &[Tab] {
        self.layout_of(strip)
            .map_or(&[], |layout| layout.tabs.as_slice())
    }

    /// The tab in front in the strip in front.
    pub fn tab(&self) -> Option<&Tab> {
        self.front_tab(&self.strip()?)
    }

    /// The tab in front in `strip`. While the one in front is closing, it is the one the core
    /// brings forward in its place, the next open tab after it, else the nearest before it.
    pub fn front_tab(&self, strip: &Strip) -> Option<&Tab> {
        let layout = self.layout_of(strip)?;
        let tabs = layout.tabs.as_slice();
        let open = |tab: &&Tab| !self.closing(&tab.id);
        let active = layout
            .active
            .as_deref()
            .and_then(|id| tabs.iter().position(|tab| tab.id == id));
        match active {
            Some(at) if open(&&tabs[at]) => Some(&tabs[at]),
            Some(at) => tabs[at..]
                .iter()
                .find(open)
                .or_else(|| tabs[..at].iter().rev().find(open)),
            None => tabs.iter().find(open),
        }
    }

    /// Whether `tab` was asked to close a moment ago and the layout has not caught up yet.
    fn closing(&self, tab: &str) -> bool {
        self.closing_tabs
            .borrow()
            .get(tab)
            .is_some_and(|asked| asked.elapsed() < CLOSE_SETTLES)
    }

    /// Forgets the tabs asked to close that the layout no longer lists, or that it has had
    /// time to answer for.
    fn settle_closing(&self) {
        let Some(layout) = self.shown().map(|view| &view.layout) else {
            return;
        };
        let listed = |id: &str| {
            std::iter::once(&layout.terminals)
                .chain(layout.projects.values())
                .chain(layout.hosts.values())
                .any(|strip| strip.tabs.iter().any(|tab| tab.id == id))
        };
        self.closing_tabs
            .borrow_mut()
            .retain(|id, asked| asked.elapsed() < CLOSE_SETTLES && listed(id));
    }

    pub fn activate_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.call_in_front("activateTab", strip.with(json!({ "tab": tab })));
        }
    }

    /// Moves a tab to `to` in its strip, counted as the strip is now.
    pub fn move_tab(&self, strip: &Strip, tab: &str, to: usize) {
        if !self.closing(tab) {
            self.call_in_front("moveTab", strip.with(json!({ "tab": tab, "to": to })));
        }
    }

    /// Keeps a tab opened to look at, so the next one opens beside it.
    pub fn keep_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.call_in_front("keepTab", strip.with(json!({ "tab": tab })));
        }
    }

    pub fn close_tabs(&self, strip: &Strip, tabs: &[String]) {
        let tabs: Vec<&String> = tabs.iter().filter(|tab| !self.closing(tab)).collect();
        if tabs.is_empty() {
            return;
        }
        let asked = std::time::Instant::now();
        self.closing_tabs
            .borrow_mut()
            .extend(tabs.iter().map(|tab| ((*tab).clone(), asked)));
        self.call_in_front("closeTabs", strip.with(json!({ "tabs": tabs })));
    }

    /// Names a tab; an empty name gives back the one its view gives it.
    pub fn rename_tab(&self, strip: &Strip, tab: &str, title: &str) {
        if !self.closing(tab) {
            self.call_in_front(
                "renameTab",
                strip.with(json!({ "tab": tab, "title": title })),
            );
        }
    }

    /// Opens a copy of a tab beside it, with new shells for its terminals.
    pub fn duplicate_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.call_in_front("duplicateTab", strip.with(json!({ "tab": tab })));
        }
    }

    /// Opens a terminal to an SSH host, in that host's strip.
    pub fn open_ssh_terminal(&self, alias: &str, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Terminal, cx);
        self.call("openSshTerminal", json!({ "host": alias }));
    }

    /// Brings the tab of the agent `id` to the front, in its project, opening one when none
    /// shows it, with the workbench in place of the goals.
    pub fn open_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        self.show_workbench(cx);
        self.bring_agent(id);
    }

    /// Brings the tab of the agent `id` to the front, in its project, opening one when none
    /// shows it.
    pub fn bring_agent(&mut self, id: &str) {
        let project_on = |view: &DeviceView| {
            view.workspace
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .map(|agent| agent.project.clone())
        };
        let tab = |project: String| json!({ "project": project, "kind": "agent", "target": id });
        let Some(project) = self.shown().and_then(project_on) else {
            // Opening a tab on a host's layout brings that host forward there.
            if let Some(project) = self.this_view().and_then(project_on) {
                return self.call("openTab", tab(project));
            }
            if let Some((host, project)) = self
                .feeds
                .hosts()
                .find_map(|(host, feed)| Some((host.clone(), project_on(feed.view.as_ref()?)?)))
            {
                self.call("chooseHost", json!({ "host": format!("host:{host}") }));
                let open = Ask::Call {
                    method: "openTab",
                    params: tab(project),
                };
                let _ = self.asks.send(Ask::On(host, Box::new(open)));
            }
            return;
        };
        let strip = Strip::Project(project.clone());
        let shown = (self.tabs(&strip).iter())
            .find(|tab| shows(tab, PaneKind::Agent).is_some_and(|shown| shown == id))
            .map(|tab| tab.id.clone());
        match shown {
            Some(shown) => {
                self.activate_project(&project);
                self.activate_tab(&strip, &shown);
            }
            None => self.call_in_front("openTab", tab(project)),
        }
    }

    /// The id of the agent the tab in front shows.
    pub fn front_tab_agent(&self) -> Option<&str> {
        shows(self.tab()?, PaneKind::Agent).map(String::as_str)
    }

    /// Tells this host's core which of its agents the person is looking at, so the ones read
    /// lose their unread mark.
    fn publish_on_screen(&mut self) {
        let agent_ids = match self.place() {
            Place::This => self.agent().map(|agent| agent.id).into_iter().collect(),
            Place::Account(_) => Vec::new(),
        };
        let shown = (agent_ids, self.window_active);
        if self.on_screen.as_ref() == Some(&shown) {
            return;
        }
        let _ = self.asks.send(Ask::OnScreen {
            agent_ids: shown.0.clone(),
            focused: shown.1,
        });
        self.on_screen = Some(shown);
    }

    /// The agent the tab in front shows.
    pub fn agent(&self) -> Option<AgentRow> {
        let project = self.project()?;
        let target = shows(self.tab()?, PaneKind::Agent)?;
        self.agents_of(&project.id)
            .into_iter()
            .find(|agent| &agent.id == target)
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
        self.this_view()?
            .sessions
            .iter()
            .find(|session| session.id.to_string() == id)
    }

    /// Terminals started outside every project, which the core numbers.
    pub fn loose_terminals(&self) -> Vec<&SessionInfo> {
        self.shown().map_or(Vec::new(), |view| {
            view.sessions
                .iter()
                .filter(|session| session.rail_name.is_some() && session.exit.is_none())
                .collect()
        })
    }

    /// The project's agents in the order the core ranks them for the person.
    /// Counts the stash as seen up to `at` here at once, ahead of the server, whose answer
    /// then agrees.
    pub fn seen_stash_at(&mut self, at: &str) {
        if let Some(stash) = self.reads.stash.as_mut() {
            stash.seen_at = Some(at.to_owned());
        }
    }

    /// The agent whose chat last took the keys.
    pub fn focused_agent(&self) -> Option<&str> {
        self.chats.focused.as_ref().map(|(agent, _)| agent.as_str())
    }

    pub fn agent_by_id(&self, id: &str) -> Option<AgentRow> {
        let view = self.shown()?;
        let agent = view.workspace.agents.iter().find(|agent| agent.id == id)?;
        Some(self.agent_row(view, agent))
    }

    pub fn agents_of(&self, project: &str) -> Vec<AgentRow> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        let ranked: Vec<&str> = view
            .inbox
            .agents
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect();
        let mut rows: Vec<AgentRow> = view
            .workspace
            .agents
            .iter()
            .filter(|agent| agent.project == project)
            .map(|agent| self.agent_row(view, agent))
            .collect();
        rows.sort_by_key(|row| {
            ranked
                .iter()
                .position(|id| *id == row.id)
                .unwrap_or(usize::MAX)
        });
        rows
    }

    /// One agent as the rails and tabs show it, from what the view says of it.
    fn agent_row(
        &self,
        view: &DeviceView,
        agent: &sikemux_wire::protocol::WorkspaceAgent,
    ) -> AgentRow {
        let shown = view
            .inbox
            .agents
            .iter()
            .find(|row| row.agent_id == agent.id);
        let status = shown.map_or(
            if agent.asleep {
                AgentStatus::Asleep
            } else {
                AgentStatus::Idle
            },
            |row| row.status,
        );
        AgentRow {
            active_at: shown.and_then(|row| row.active_at).unwrap_or(0),
            next_trigger: shown.and_then(|row| row.next_trigger),
            called_at: shown.and_then(|row| row.called_at),
            team: shown.and_then(|row| row.team),
            id: agent.id.clone(),
            provider: agent.provider.clone(),
            title: self.title_of(agent),
            name: agent.name.clone().filter(|_| self.titled(agent)),
            status,
            group: status.group(),
            busy: shown.is_some_and(|row| row.busy),
            background: shown.is_some_and(|row| row.background),
            subagents: shown.map_or(0, |row| row.subagents),
            tasks: shown.map_or(0, |row| row.tasks),
            project: agent.project.clone(),
            face: agent.face.clone(),
        }
    }

    /// The rows the agent rail lists: this project's, or every project's.
    pub fn listed_agents(&self) -> Vec<AgentRow> {
        let focus = self.rails.goal_focus.as_ref();
        let mut rows: Vec<AgentRow> = if self.rails.all_projects || focus.is_some() {
            self.projects()
                .iter()
                .flat_map(|project| self.agents_of(&project.id))
                .collect()
        } else {
            self.project()
                .map(|project| self.agents_of(&project.id))
                .unwrap_or_default()
        };
        if let Some(focus) = focus {
            rows.retain(|row| focus.agents.contains(&row.id));
        }
        let filter = self.filter_text();
        rows.retain(|row| row.title.to_lowercase().contains(filter));
        rows
    }

    /// Narrows the agent rail to a goal's agents and shows the workbench with the first in front.
    pub fn focus_goal(
        &mut self,
        title: String,
        agents: Vec<String>,
        project: &str,
        card: u64,
        cx: &mut Context<Self>,
    ) {
        let first = agents.first().cloned();
        self.rails.goal_focus = Some(crate::rail::GoalFocus {
            project: project.to_owned(),
            card,
            title,
            agents,
        });
        self.rails.page = TabRailPage::Agents;
        self.rails.tab_open = true;
        self.show_workbench(cx);
        if let Some(agent) = first {
            self.bring_agent(&agent);
        }
        cx.notify();
    }

    pub fn clear_goal_focus(&mut self, cx: &mut Context<Self>) {
        self.rails.goal_focus = None;
        cx.notify();
    }

    /// Keeps the rail's goal up with who works it now; a goal gone takes the narrowing with it.
    pub fn refocus_goal(&mut self, goals: &sikemux_view::goals::GoalsView, cx: &mut Context<Self>) {
        let Some(focus) = self.rails.goal_focus.as_mut() else {
            return;
        };
        let row =
            (goals.goals.iter()).find(|row| row.card == focus.card && row.project == focus.project);
        match row {
            Some(row) if row.agents == focus.agents && row.title == focus.title => return,
            Some(row) => {
                focus.agents = row.agents.clone();
                focus.title = row.title.clone();
            }
            None => self.rails.goal_focus = None,
        }
        cx.notify();
    }

    /// Whether the agent has a title of its own: the person named it, or it is neither empty,
    /// its kind nor its account's name. Until then its @name stands as its title.
    pub fn titled(&self, agent: &sikemux_wire::protocol::WorkspaceAgent) -> bool {
        let title = agent.title.trim();
        let account = (agent.profile_id.as_deref())
            .and_then(|id| self.accounts.profile(id))
            .map(|profile| profile.name.as_str());
        agent.renamed || !(title.is_empty() || title == agent.provider || Some(title) == account)
    }

    /// What the agent is called in its header, tab and rail row.
    pub fn title_of(&self, agent: &sikemux_wire::protocol::WorkspaceAgent) -> String {
        match &agent.name {
            Some(name) if !self.titled(agent) => format!("@{name}"),
            _ => agent.title.clone(),
        }
    }

    /// The project's saved chats no agent holds, which the agent list already shows.
    pub fn recent_of(&self, project: &str) -> Vec<&RecentInfo> {
        self.chats_of(project)
            .into_iter()
            .filter(|recent| recent.agent_id.is_none())
            .collect()
    }

    /// The project's pinned chats, held by an agent or not.
    pub fn pinned_of(&self, project: &str) -> Vec<&RecentInfo> {
        self.chats_of(project)
            .into_iter()
            .filter(|recent| recent.pinned)
            .collect()
    }

    /// The saved chat the agent holds, once the core lists it.
    pub fn chat_of(&self, agent: &str) -> Option<&RecentInfo> {
        self.shown()?
            .recent
            .iter()
            .find(|recent| recent.agent_id.as_deref() == Some(agent))
    }

    fn chats_of(&self, project: &str) -> Vec<&RecentInfo> {
        let provider = self.launcher().map(|launcher| launcher.provider.as_str());
        self.saved_chats(project)
            .into_iter()
            .filter(|recent| self.rails.all_harnesses || provider == Some(recent.provider.as_str()))
            .filter(|recent| recent.title.to_lowercase().contains(self.filter_text()))
            .collect()
    }

    pub fn sign_in(&mut self) {
        self.signing_in = true;
        let _ = self.asks.send(Ask::Answer {
            read: Read::SignIn,
            params: json!({}),
        });
    }

    /// Ends a sign-in the browser has not finished, which settles the one waiting on it.
    pub fn cancel_sign_in(&self) {
        self.call("accountSignInCancel", json!({}));
    }

    pub fn sign_out(&self) {
        self.call("accountSignOut", json!({}));
    }

    pub fn toggle_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.take().is_some() {
            self.look_for_older(false, crate::recent::PAGE, cx);
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("filter recent chats…"));
        let changes = cx.subscribe(&input, |bench, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if let Some(filter) = bench.filter.as_mut() {
                    filter.text = input.read(cx).value().to_lowercase();
                }
                bench.look_for_older(false, crate::recent::PAGE, cx);
                cx.notify();
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.filter = Some(crate::recent::Filter {
            input,
            text: String::new(),
            _changes: changes,
        });
    }

    /// What the rail's filter holds, in lower case; empty while it does not show.
    pub fn filter_text(&self) -> &str {
        self.filter
            .as_ref()
            .map_or("", |filter| filter.text.as_str())
    }
}

/// The trust questions already put to the person, and the account host in front whose trust
/// and agents' questions are read beside this host's.
#[derive(Default)]
pub(crate) struct Trust {
    /// By the host that asks and the question's id.
    asked: HashSet<String>,
}

impl Trust {
    /// This host's questions, then the account host's, each under the host that asks: the first
    /// not put to the person yet, now marked as put, while `free`. Forgets those no longer asked.
    fn next(
        &mut self,
        feeds: &crate::hosts::Feeds,
        free: bool,
    ) -> Option<(Option<String>, TrustQuestion)> {
        let front = feeds
            .front()
            .and_then(|host| Some((host, feeds.host(host)?)));
        let asks: Vec<(Option<String>, &TrustQuestion)> =
            (feeds.this.trust.iter())
                .map(|ask| (None, ask))
                .chain(front.into_iter().flat_map(|(host, feed)| {
                    feed.trust.iter().map(|ask| (Some(host.clone()), ask))
                }))
                .collect();
        let asked_as = |host: &Option<String>, ask: &TrustQuestion| {
            format!("{}:{}", host.as_deref().unwrap_or_default(), ask.id)
        };
        self.asked
            .retain(|asked| asks.iter().any(|(host, ask)| &asked_as(host, ask) == asked));
        if !free {
            return None;
        }
        let (host, ask) = asks
            .into_iter()
            .find(|(host, ask)| !self.asked.contains(&asked_as(host, ask)))?;
        self.asked.insert(asked_as(&host, ask));
        Some((host, ask.clone()))
    }
}

/// Whether two tabs show the same kind of pane, as ⌃Tab steps between them.
fn same_kind(tab: &Tab, front: &Tab) -> bool {
    let kind = |tab: &Tab| tabs::first_view(&tab.pane).map(|(kind, _)| kind);
    kind(tab) == kind(front)
}

/// What the tab's first view of `kind` shows, among its splits.
pub fn shows(tab: &Tab, kind: PaneKind) -> Option<&String> {
    crate::panes::views(&tab.pane)
        .find(|view| view.kind == kind)
        .map(|view| view.target)
}

/// One part of the window, drawn as a view of its own so that it draws again only when the
/// store changes everywhere or names this part.
pub struct Part {
    bench: WeakEntity<Workbench>,
    draw: fn(&mut Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
    column: bool,
    stores: Vec<Subscription>,
}

impl Render for Part {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        let drawn = self
            .bench
            .update(cx, |bench, cx| draw(bench, window, cx))
            .unwrap_or_else(|_| div().into_any_element());
        div()
            .size_full()
            .flex()
            .when(self.column, |part| part.flex_col())
            .child(drawn)
    }
}

/// The window's parts. A plain `cx.notify()` on the store draws them all again; output that
/// touches one part, such as a terminal's, draws only that one.
#[derive(Clone)]
pub struct Parts {
    pub top: Entity<Part>,
    pub rail: Entity<Part>,
    pub stage: Entity<Part>,
    pub tabs: Entity<Part>,
    pub finder: Entity<Part>,
    /// The welcome or the release notes over the whole window.
    pub over: Entity<Part>,
    /// The calendar of tokens burnt, under a chat's welcome.
    pub activity: Entity<crate::home::Activity>,
}

impl Parts {
    fn new(cx: &mut Context<Workbench>) -> Self {
        let bench = cx.entity();
        let mut part =
            |draw: fn(&mut Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
             column: bool| {
                cx.new(|cx| Part {
                    bench: bench.downgrade(),
                    draw,
                    column,
                    stores: vec![cx.observe(&bench, |_, _, cx| cx.notify())],
                })
            };
        Self {
            top: part(
                |bench, _, cx| tabs::top_bar(bench, cx).into_any_element(),
                true,
            ),
            rail: part(
                |bench, _, cx| rail::side_rail(bench, cx).into_any_element(),
                false,
            ),
            stage: part(
                |bench, window, cx| {
                    bench.compose(window, cx);
                    bench.hint(window, cx);
                    bench.make_front_pane(window, cx);
                    bench.mod_panes(cx);
                    crate::desk::prepare(bench, window, cx);
                    bench.settle_takes_keys();
                    // The frame being drawn is not done, so a view not drawn yet may still show in it.
                    if bench.keys(window, cx) == crate::keyboard::Keys::Loose {
                        cx.defer_in(window, |bench, window, cx| bench.settle_keys(window, cx));
                    }
                    let frame = bench.stage_frame.clone();
                    div()
                        .relative()
                        .size_full()
                        .flex()
                        .child(
                            canvas(|_, _, _| {}, move |bounds, _, _, _| frame.set(bounds))
                                .absolute()
                                .size_full(),
                        )
                        .child(home::stage(bench, window, cx))
                        .into_any_element()
                },
                false,
            ),
            tabs: part(|bench, _, cx| agents::tab_rail(bench, cx), false),
            finder: part(
                |bench, window, cx| {
                    crate::switcher::overlay(bench, cx)
                        .or_else(|| crate::palette::overlay(bench, window, cx))
                        .or_else(|| crate::finder::overlay(bench, window, cx))
                        .unwrap_or_else(|| div().into_any_element())
                },
                true,
            ),
            over: part(
                |bench, window, cx| {
                    crate::pairing::overlay(bench, window, cx)
                        .or_else(|| crate::command_popup::overlay(bench, cx))
                        .unwrap_or_else(|| crate::experience::draw(bench, window, cx))
                },
                true,
            ),
            activity: cx.new(|cx| crate::home::Activity::new(&bench, cx)),
        }
    }

    /// Draws the welcome's activity again, as when the pointer moves to another day.
    pub fn redraw_activity(&self, cx: &mut App) {
        self.activity.update(cx, |_, cx| cx.notify());
    }

    pub fn redraw(&self, part: &Entity<Part>, cx: &mut App) {
        part.update(cx, |_, cx| cx.notify());
    }

    /// Draws the stage again whenever `store` says it changed.
    pub fn stage_follows<T: 'static>(&self, store: &Entity<T>, cx: &mut App) {
        self.stage.update(cx, |part, cx| {
            part.stores.push(cx.observe(store, |_, _, cx| cx.notify()))
        });
    }

    /// Draws the stage again, and what is over the window, which shows some of the stage's
    /// state too, such as an install's terminal under the welcome.
    pub fn redraw_stage(&self, cx: &mut App) {
        self.redraw(&self.stage, cx);
        self.redraw(&self.over, cx);
    }
}

impl Workbench {
    /// A docked rail and its handle, `tuck` of the way under the stage. It keeps its width as
    /// it goes, so nothing in it wraps again; the stage takes the room it gives up.
    fn docked_rail(&self, edge: Edge, tuck: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let (part, width) = match edge {
            Edge::Start => (&self.parts.rail, self.rails.side_width),
            Edge::End => (&self.parts.tabs, self.rails.tab_width),
        };
        let rail = crate::settings::leaves(div().h_full().flex_none(), cx).child(
            part.clone()
                .cached(StyleRefinement::default().w(px(width)).h_full().flex_none()),
        );
        let drawer = div()
            .flex_none()
            .h_full()
            .w(px((width + RESIZER_WIDTH) * (1. - tuck)))
            .overflow_hidden()
            .flex()
            .opacity(1. - tuck);
        match edge {
            Edge::Start => drawer.child(rail).child(self.rail_resizer(edge, cx)),
            Edge::End => drawer
                .justify_end()
                .child(self.rail_resizer(edge, cx))
                .child(rail),
        }
    }

    /// A closed rail's edge of the window, which brings the rail out over the stage.
    fn rail_edge(
        &self,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (part, width) = match edge {
            Edge::Start => (&self.parts.rail, self.rails.side_width),
            Edge::End => (&self.parts.tabs, self.rails.tab_width),
        };
        let peeking = self.rails.peeking == Some(edge);
        let shown = motion::follow(
            match edge {
                Edge::Start => "peek-start-shown",
                Edge::End => "peek-end-shown",
            },
            if peeking { 1. } else { 0. },
            if peeking {
                motion::PEEK_IN
            } else {
                motion::PEEK_OUT
            },
            window,
            cx,
        );
        let away = (width + PEEK_GAP) * (1. - shown);
        let zone = div()
            .id(match edge {
                Edge::Start => "peek-start",
                Edge::End => "peek-end",
            })
            .absolute()
            .top(px(tabs::top_height(self) + 4.))
            .bottom(px(10.))
            .w(px(if shown > 0. { width + 20. } else { PEEK_REACH }))
            .px(SPACE_2H)
            .on_hover(cx.listener(move |bench, inside: &bool, _, cx| bench.peek(edge, *inside, cx)))
            .when(shown > 0., |zone| {
                zone.child(
                    div()
                        .h_full()
                        .relative()
                        .left(px(match edge {
                            Edge::Start => -away,
                            Edge::End => away,
                        }))
                        .opacity(shown)
                        .child(
                            crate::settings::leaves(div().h_full(), cx).child(
                                part.clone()
                                    .cached(StyleRefinement::default().w(px(width)).h_full()),
                            ),
                        ),
                )
            });
        match edge {
            Edge::Start => zone.left_0(),
            Edge::End => zone.right_0(),
        }
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::keymap::SHELL;
        let asked = self.asked(window, cx);
        #[cfg(feature = "drive")]
        if asked.is_some() {
            crate::drive::trace(format_args!("question drawn"));
        }
        let asking = self.asking_freely(window, cx);
        let this = cx.weak_entity();
        crate::see_through::follow(window, cx);
        let [side_tuck, tab_tuck] = [
            ("side-rail-tuck", self.rails.side_open),
            ("tab-rail-tuck", self.rails.tab_open),
        ]
        .map(|(id, open)| {
            let tucked = if open { 0. } else { 1. };
            let (ease, shortest) = (motion::RAIL_DRAWER, motion::RAIL_DRAWER_SHORTEST);
            motion::follow_share(id, tucked, ease, shortest, window, cx)
        });
        let goals = self.window_view == WindowView::Goals;
        if goals {
            self.goals_view(cx);
        }
        let t = Tokens::get(cx);
        window.set_ground(Some(crate::see_through::ground(t.surface_sunken, cx)));
        let parts = &self.parts;
        div()
            .id("shell")
            .key_context(SHELL)
            .track_focus(&self.keys.focus)
            .relative()
            .size_full()
            .capture_any_mouse_down(|_, _, _| keyboard_back())
            .on_drop(cx.listener(|bench, dropped: &ExternalPaths, window, cx| {
                bench.take_stray_drop(dropped.paths(), window, cx)
            }))
            .on_modifiers_changed(
                cx.listener(|bench, event, _, cx| bench.modifiers_changed(event, cx)),
            )
            .when_some(asking, |shell, agent| {
                crate::chat::question_keys(shell, &agent, false, &this)
            })
            .capture_key_down(cx.listener(|bench, event, _, cx| bench.switcher_key(event, cx)))
            .on_modifiers_changed(
                cx.listener(|bench, event, window, cx| bench.voice_keys(event, window, cx)),
            )
            .capture_key_down(cx.listener(|bench, _: &gpui_kit::KeyDownEvent, _, cx| {
                bench.voice.update(cx, |voice, cx| voice.abandon_hold(cx))
            }))
            .map(|shell| actions::listen_others(actions::listen(shell, cx), cx))
            .flex()
            .flex_col()
            .font_family(theme::UI)
            .text_size(crate::theme::ui_text(TEXT_SM))
            .line_height(crate::theme::ui_text(13. * 1.45))
            .text_color(t.text_secondary)
            .child(
                parts.top.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(tabs::top_height(self)))
                        .flex_none(),
                ),
            )
            .child(
                div()
                    .id("shell-row")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .pt(if goals { SPACE_2H } else { SPACE_1 })
                    .px(SPACE_2H)
                    .pb(SPACE_2H)
                    .on_drag_move(cx.listener(
                        |bench, event: &DragMoveEvent<DraggedRail>, window, cx| {
                            let edge = event.drag(cx).0;
                            let (x, bounds) = (event.event.position.x, event.bounds);
                            let width = match edge {
                                Edge::Start => x - bounds.left(),
                                Edge::End => bounds.right() - x,
                            };
                            bench.resize_rail(edge, f32::from(width) - 15., window, cx);
                        },
                    ))
                    .when(side_tuck < 1., |row| {
                        row.child(self.docked_rail(Edge::Start, side_tuck, cx))
                    })
                    .child(
                        parts
                            .stage
                            .clone()
                            .cached(StyleRefinement::default().flex_1().min_w_0().h_full()),
                    )
                    .when(tab_tuck < 1., |row| {
                        row.child(self.docked_rail(Edge::End, tab_tuck, cx))
                    }),
            )
            // Web pages cover all GPUI draws but deferred layers, so what floats goes here.
            .child(deferred(
                div()
                    .absolute()
                    .inset_0()
                    .when(!self.rails.side_open, |above| {
                        above.child(self.rail_edge(Edge::Start, window, cx))
                    })
                    .when(!self.rails.tab_open, |above| {
                        above.child(self.rail_edge(Edge::End, window, cx))
                    })
                    .children(asked)
                    .children(crate::add_task::view(self, cx))
                    .children(crate::add_provider::view(self, cx))
                    .children(crate::buddy_picker::view(self, cx))
                    .children(crate::share::view(self, cx))
                    .children(crate::boot::reconnecting(self, cx))
                    .children(crate::boot::stopped(self, cx))
                    .children(crate::whats_new::required(self, cx))
                    .children(crate::boot::opening(self, cx))
                    .children(crate::boot::stuck(self, cx))
                    .when(self.overlays.in_layer(Layer::Finder), |above| {
                        above.child(
                            parts
                                .finder
                                .clone()
                                .cached(StyleRefinement::default().absolute().inset_0()),
                        )
                    })
                    .when(self.over_shown(), |above| {
                        above.child(
                            parts
                                .over
                                .clone()
                                .cached(StyleRefinement::default().absolute().inset_0()),
                        )
                    }),
            ))
    }
}

/// Puts `text` at the end of the draft after a blank line, keeping what it holds, chips
/// and all, with the caret after it.
fn append(draft: &Entity<TextareaState>, text: &str, window: &mut Window, cx: &mut App) {
    draft.update(cx, |draft, cx| {
        let held = draft.value();
        let kept = held.trim_end().len();
        let joined = if kept == 0 {
            text.to_owned()
        } else {
            format!("\n\n{text}")
        };
        draft.set_selected_range(kept..held.len(), cx);
        draft.replace(joined, window, cx);
    });
}
