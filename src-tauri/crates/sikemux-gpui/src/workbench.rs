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
use sikemux_wire::protocol::{
    AgentFace, AgentGroup, AgentStatus, DeviceView, LauncherInfo, MidTurnSend, Pane, PaneKind,
    Preferences, ProjectCodeHost, ProjectInfo, ProjectLayout, RecentInfo, RemoteStatus, SessionId,
    SessionInfo, SpaceInfo, Tab, TabRailPage, TabRailScope,
};
use tokio::sync::mpsc;

use crate::battery::Battery;
use crate::chat::{Chat, Composing, Question, Sent};
use crate::codehost::{Area, HostArea, OpenArea};
use crate::core::{utc_offset_minutes, Ask, ChatAsk, ChatNews, Link, Target, Update, Watch};
use crate::desk::{
    DeskAlone, FocusAddress, NewPage, NewSimulator, PageBack, PageForward, ReloadPage, ToggleDesk,
};
use crate::editor::OpenAt;
use crate::files::FileTree;
use crate::finder::{Finder, OpenFile, ProjectFiles};
use crate::git::{FocusGit, GitPane};
use crate::hosts::Place;
use crate::motion;
use crate::search::{FocusSearch, FocusSearchWindow, SearchPane};
use crate::terminal::NewTerminal;
use crate::theme::{self, Tokens};
use crate::toast;
use crate::{agents, home, rail, tabs, terminal};

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

pub struct Workbench {
    pub version: String,
    pub view: Option<DeviceView>,
    /// The latest answer to each watched call, by key.
    reads: HashMap<&'static str, Value>,
    preferences: Preferences,
    pub lost: Option<String>,
    /// The core stopped on purpose, so it waits for the person to start it again.
    pub stopped: bool,
    /// The core holding the socket stopped answering, by its process id, and whether the
    /// person asked to restart it.
    pub stuck: Option<(u32, bool)>,
    /// Opening has taken long enough to show the mark.
    pub boot_shown: bool,
    /// Opening has taken long enough to say what the window waits for.
    pub slow_start: bool,
    /// Dictation, heard into the chat or terminal holding the keyboard.
    pub voice: Entity<crate::voice::Dictation>,
    _voice_heard: [Subscription; 4],
    /// Catches the keys pressed while nothing holds the keyboard, and the shortcuts Settings
    /// answers first.
    _before_shortcuts: Subscription,
    /// Give the view in front the keyboard whenever nothing else holds it: once a view in
    /// focus stops being drawn, the window comes forward or the workbench itself is given it.
    _keys_settle: [Subscription; 3],
    /// What the person just opened, which takes the keyboard once it shows.
    takes_keys: Cell<Option<PendingKeys>>,
    window_active: bool,
    /// Whether any of the window shows: not minimised, hidden or wholly behind another window.
    window_shown: bool,
    /// The agents last told to the core as on screen, and whether the window was in front.
    on_screen: Option<(Vec<String>, bool)>,
    /// The project the per-project watches were last made for, by path.
    pub(crate) watched_project: Option<String>,
    /// The host whose core those watches went to.
    watched_place: crate::hosts::Place,
    pub battery: Option<Battery>,
    pub agent_setup: crate::agent_setup::Setup,
    /// The welcome or the release notes, while one is over the window.
    pub experience: Option<crate::experience::Experience>,
    /// A custom command running in a terminal over the window.
    pub command_popup: Option<crate::command_popup::Popup>,
    /// The question over the window while a phone waits to connect.
    pub pairing: crate::pairing::Pairing,
    /// Whether this window has looked for what kept running while Sikemux was closed.
    pub kept_running_told: bool,
    /// Each waiting `sikemux open --wait`, with the tabs it opened that are still open.
    pub(crate) open_waits: Vec<(sikemux_wire::protocol::CallId, HashMap<String, bool>)>,
    /// Each open project settings tab's state, by the project's folder.
    pub project_settings: HashMap<String, crate::project_settings::ProjectSettings>,
    pub shown: crate::experience::Shown,
    /// The repository and branch whose latest CI run the top bar shows.
    pub watched_ci: Option<CiRepo>,
    pub ci_card: Entity<crate::ci_card::CiCard>,
    /// The commit whose runs from every workflow, tags' included, the glyph sums up.
    watched_head: Option<String>,
    /// The account's picture, and the hash the core named the one asked for by.
    picture: Option<Arc<Image>>,
    picture_hash: Option<String>,
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
    pub all_projects: bool,
    /// The leads whose teams the agent rail folds away.
    pub folded_teams: HashSet<String>,
    /// The agent rows the rail drew last, for telling the rows that have just arrived.
    pub(crate) rail_seen: std::cell::RefCell<crate::agents::RailSeen>,
    /// The strip's tabs as drawn last, for telling those that have just opened and closed.
    pub(crate) strip_seen:
        std::cell::RefCell<crate::motion::ListMemory<sikemux_wire::protocol::Tab>>,
    /// Where the strip's tabs were drawn, for the chosen surface to glide between them.
    pub(crate) strip_glide: std::cell::RefCell<crate::motion::GlideTrack>,
    /// How far the strip's tabs are scrolled, and the tab last scrolled into view.
    pub(crate) strip_scroll: gpui_kit::ScrollHandle,
    /// The tab rail's list of agents and chats, whose foot brings older chats into view.
    pub(crate) rail_scroll: gpui_kit::ScrollHandle,
    pub(crate) strip_scrolled_to: std::cell::RefCell<Option<String>>,
    /// Tabs asked to close that the layout still lists, and when, so a second click or ⌘W
    /// before the core's answer does not ask for a tab it has already closed.
    closing_tabs: std::cell::RefCell<HashMap<String, std::time::Instant>>,
    /// Where the rail's agent rows were drawn, for the chosen surface to glide between them.
    pub(crate) rail_glide: std::cell::RefCell<crate::motion::GlideTrack>,
    /// The card beside the agent row the pointer rests on.
    pub agent_card: crate::agent_card::Card,
    /// The task each project's Run button starts, when the person chose one, by project path.
    pub picked_tasks: HashMap<String, String>,
    /// Where the tab rail's frame was last drawn, left, top, width and height, for the driver.
    pub rail_frame: Rc<Cell<[f32; 4]>>,
    /// Where the stage was last drawn, for what rises over the stage alone.
    pub stage_frame: Rc<Cell<Bounds<Pixels>>>,
    /// The run whose failed tests the person closed the card of, by execution.
    pub tests_closed: Option<String>,
    /// The run whose "Why?" card is open, by execution.
    pub failure_shown: Option<String>,
    /// The space whose projects the rail shows, else every project.
    pub space: Option<String>,
    /// Which rails stay open; a closed one comes out over the stage while the pointer is at its edge.
    pub side_rail_open: bool,
    pub tab_rail_open: bool,
    side_rail_width: f32,
    tab_rail_width: f32,
    /// The handles beside the rails, which arrow keys move once focused.
    resizers: [FocusHandle; 2],
    pub peeking: Option<Edge>,
    unpeek: Option<Task<()>>,
    /// The repository changed while the Git pane was out of sight, so it reads it and its
    /// review again once it shows rather than on every change.
    git_behind: bool,
    /// Lets go of the chats out of sight once they have been for a while, counted from when
    /// the chat now in front came there.
    resting_chats: Option<(Option<String>, Task<()>)>,
    pub run_clock: Entity<crate::tabs::RunClock>,
    /// Which of its pages the tab rail shows.
    pub rail_page: RailPage,
    pub files: FileTree,
    /// The file tree's, which its arrow keys and F2 move and act in.
    pub files_focus: FocusHandle,
    pub finder: Option<Finder>,
    pub palette: Option<crate::palette::Palette>,
    /// The agent the project picker moves to the project picked, while it does.
    pub moving: Option<String>,
    /// Edit Spaces, while it is open under the space switch.
    pub space_editor: Option<Entity<crate::space_editor::SpaceEditor>>,
    pub switcher: Option<crate::switcher::Switcher>,
    /// The command deck's keys, the last used first.
    pub recent_commands: Vec<SharedString>,
    /// The rails' state waiting to be kept in the core's preferences, once a drag settles.
    keeping_rails: Option<Task<()>>,
    /// When a refused file listing is asked again.
    files_retry: crate::retry::Retry,
    /// The Git pane takes the keyboard once it shows, as when ⌘⌥3 opens it.
    pub project_files: ProjectFiles,
    /// The kind of agent the rail was switched to, such as "claude"; its account is the one the
    /// person chose for that kind.
    pub launcher_choice: Option<String>,
    /// The plan limits are to be read once the core lists its agents, as the app starts.
    usage_wanted: bool,
    /// The rail lists the recent chats of every agent, not one's.
    pub all_harnesses: bool,
    pub pinch: Option<crate::pinch::Pinch>,
    /// Which of the calendar's metrics it shows, an index into `home::METRICS`.
    pub metric: usize,
    /// The calendar day the pointer is over, counted from the Unix epoch.
    pub metric_day: Option<i64>,
    pub filter: Option<Entity<InputState>>,
    pub filter_text: String,
    /// The front project's saved chats paged or searched to beyond the recent ones.
    pub older: Option<crate::recent::OlderChats>,
    pub asks: mpsc::UnboundedSender<Ask>,
    /// The chats of the agents with a tab open, by agent id.
    pub chats: HashMap<String, Chat>,
    /// The chat in front and whether its session was ready when its draft last took the keys.
    focused_chat: Option<(String, bool)>,
    /// Text handed to agents whose composer is not made yet, put in once it is.
    deliveries: HashMap<String, String>,
    /// What agents asked the person, and the assumptions they went on: this host's, then the
    /// chosen account host's.
    pub questions: Vec<Question>,
    /// The chosen account host's questions, apart, so this host's reading again keeps them.
    host_questions: Vec<Question>,
    pub terminals: terminal::Terminals,
    pub editors: crate::editor::Editors,
    /// The Git pane of the project whose Git tab was last in front.
    pub git: Option<Entity<GitPane>>,
    /// What the Git tab shows: the local workbench, or one of the code host's areas.
    pub area: Area,
    /// The code host's areas of the project whose Git tab last showed one.
    pub host: Option<Entity<HostArea>>,
    /// The repository picked from the list inside a project folder, by the folder.
    git_picked: Option<(String, String)>,
    /// The list of repositories inside the project folder shows in the Git tab.
    git_listing: bool,
    /// A pull request or issue to open once the code host's area shows.
    opening: Option<(Area, u64, String)>,
    /// The Search pane of the project whose Search tab was last in front.
    pub search: Option<Entity<SearchPane>>,
    pub board: Option<Entity<crate::board::BoardPane>>,
    /// The plugin panes of the strip in front, by view.
    pub plugin_panes: HashMap<String, Entity<crate::plugin_pane::PluginPane>>,
    /// Each plugin's item in the top bar, by plugin.
    pub plugin_bars: HashMap<String, Entity<crate::plugin_pane::PluginPane>>,
    /// The place each plugin's pane opens at next, by plugin, until its pane is made.
    plugin_opening: HashMap<String, String>,
    pub top_bar_hovered: bool,
    /// Settings, while they take the stage's place.
    pub settings: Option<crate::settings::Settings>,
    /// The agent whose title is being edited in place in its header.
    pub retitling: Option<Retitling>,
    /// The question the shell is putting to the person, over the window.
    asking: Option<Asking>,
    /// The account's other hosts this app reaches, by key: each one's view, as its core sends it.
    pub host_views: HashMap<String, DeviceView>,
    /// The account's other hosts this app is connected to now, by key.
    pub hosts_reached: HashSet<String>,
    /// Whether each host asks the person before a new device joins, by key, none for this one.
    pub approvals: HashMap<Option<String>, bool>,
    /// Why this app reaches none of the account's other hosts, when it cannot.
    pub hosts_unavailable: Option<String>,
    /// The stage's tabs while they move between one another.
    pub(crate) track: crate::track::Slide<String>,
    /// The tab rail's pages while they move between one another.
    pub(crate) rail_slide: crate::track::Slide<RailPage>,
    /// The task the person is writing in the Add a task dialog, while it is open.
    pub adding_task: Option<crate::add_task::AddTask>,
    pub adding_provider: Option<crate::add_provider::AddProvider>,
    pub buddy_picker: Option<crate::buddy_picker::BuddyPicker>,
    /// A task its checks held back, waiting on the person.
    pub run_check: Option<crate::run_check::RunCheck>,
    /// The runs the person started here, whose terminal opens once the run has one.
    pub runs_to_show: HashSet<String>,
    pub sharing: Option<crate::share::Sharing>,
    /// The agent in front whose worktree its header reads, by id and folder, and where it is.
    pub watched_worktree: Option<(String, String, crate::hosts::Place)>,
    /// The core's trust questions already put to the person, by id.
    trust_asked: HashSet<String>,
    /// The skills agents saved in their own folders that the person was told of, by path.
    pub(crate) skills_offered: HashSet<String>,
    /// The account host whose trust questions `host-trust` holds, asked beside this host's.
    trust_host: Option<String>,
    /// Holds the keyboard while nothing in the window does, so the window's shortcuts reach it.
    pub focus: FocusHandle,
    /// The window the store draws, for what is said in it from outside a draw.
    pub(crate) window: AnyWindowHandle,
    pub parts: Parts,
    pub panes: crate::panes::Panes,
    pub desks: crate::desk::Desks,
    pub sims: crate::sim_pane::Simulators,
    _filtering: Option<Subscription>,
    pub(crate) _older: Option<Task<()>>,
}

#[derive(Clone, PartialEq)]
pub struct CiRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub account: Option<String>,
    pub branch: String,
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

type Confirmed = Rc<dyn Fn(&mut Workbench, Choice, &mut Window, &mut Context<Workbench>)>;

/// How the person answered a confirm: its main button, its second one, or Cancel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Yes,
    Other,
    No,
}
type Typed = Rc<dyn Fn(&mut Workbench, String, &mut Window, &mut Context<Workbench>)>;

/// A question the shell puts to the person over the window, and what its answer does.
enum Asking {
    Confirm {
        title: SharedString,
        body: SharedString,
        yes: SharedString,
        other: Option<SharedString>,
        destructive: bool,
        then: Confirmed,
    },
    Prompt {
        title: SharedString,
        initial: String,
        field: Option<Entity<InputState>>,
        then: Typed,
    },
    Review(Entity<crate::agent_review::AgentReview>),
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RailPage {
    Agents,
    Terminals,
    Files,
}

impl RailPage {
    /// The pages in the order they sit, left to right.
    pub const ALL: [RailPage; 3] = [RailPage::Agents, RailPage::Terminals, RailPage::Files];
}

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
    /// When it last worked, in Unix milliseconds; 0 when the core does not know.
    pub active_at: u64,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    pub next_trigger: Option<u64>,
    /// When it was last named with `@` or messaged by another agent, in Unix milliseconds.
    pub called_at: Option<u64>,
    /// Its place on its lead's team, which the rows after a lead belong to.
    pub team: Option<TeamLink>,
}

/// Where an agent sits on its team: the lead heads it, the last worker ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamLink {
    Head,
    Member,
    Last,
}

impl Workbench {
    pub fn new(link: Link, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Link { mut updates, asks } = link;
        let _ = asks.send(Ask::Remote(crate::core::RemoteAsk::Status));
        crate::keymap::report_use(asks.clone(), cx);
        cx.defer_in(window, |bench, _, cx| crate::accounts::read(bench, cx));
        cx.set_decoded_image_budget(PICTURE_BUDGET);
        // The window's close button hides the app as ⌘H does, so the Dock, a link or
        // `sikemux open` brings the window back as it was.
        #[cfg(target_os = "macos")]
        window.on_window_should_close(cx, |_, cx| {
            cx.hide();
            false
        });
        crate::faces::install(cx);
        crate::file_icons::install(cx);
        crate::syntax::install(cx);
        crate::finder::install(cx);
        crate::palette::install(cx);
        crate::desk::install(cx);
        crate::welcome::install(cx);
        crate::keymap::install(cx);
        crate::chat::install(cx);
        crate::terminal::install(cx);
        crate::files::install(cx);
        crate::command_popup::install(cx);
        crate::pairing::install(cx);
        crate::editor::install(cx);
        crate::search::install(cx);
        crate::settings::install(cx);
        crate::project_switcher::install(cx);
        crate::model_picker::install(cx);
        crate::hang_watch::start(window, cx);
        crate::ui::grain::animate(window, cx);
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
        let _ = asks.send(Ask::Watch(
            "theme",
            Watch::new("theme", json!({}), &["preferences"]),
        ));
        let _ = asks.send(Ask::Watch(
            "plugins",
            Watch::new("plugins", json!({}), &["plugins"]),
        ));
        let _ = asks.send(Ask::Watch(
            "hosts",
            Watch::new("hosts", json!({}), &["hosts"]),
        ));
        let _ = asks.send(Ask::Watch(
            "update",
            Watch::new("updateStatus", json!({}), &["update"]),
        ));
        let _ = asks.send(Ask::Watch(
            "trust",
            Watch::new("trustAsks", json!({}), &["trust"]),
        ));
        let _ = asks.send(Ask::Watch(
            "questions",
            Watch::new("questions", json!({}), &["questions"]),
        ));
        let _ = asks.send(Ask::Watch(
            "shares",
            Watch::new("shares", json!({}), &["shares"]),
        ));
        let _ = asks.send(Ask::Watch(
            "stash",
            Watch::new("stash", json!({}), &["stash", "account"]),
        ));
        let _ = asks.send(Ask::Watch(
            "catalog",
            Watch::new("agentCatalog", json!({}), &[]),
        ));
        let _ = asks.send(Ask::Watch(
            "usage",
            Watch::new("agentUsages", json!({}), &["usage"]),
        ));
        let _ = asks.send(Ask::Watch(
            "skills-added",
            Watch::new("skillsAdded", json!({}), &["skills"]),
        ));
        let _ = asks.send(Ask::Watch(
            "preferences",
            Watch::new("preferences", json!({}), &["preferences"]),
        ));
        let _ = asks.send(Ask::Watch(
            "whatsNew",
            Watch::new("whatsNew", json!({}), &["preferences"]),
        ));
        let _ = asks.send(Ask::Follow("git"));
        let _ = asks.send(Ask::Follow("files"));
        let _ = asks.send(Ask::Follow("lsp"));
        let _ = asks.send(Ask::Follow("boards"));
        let _ = asks.send(Ask::Follow("agentSignIn"));
        let _ = asks.send(Ask::Follow("agentStatus"));
        let _ = asks.send(Ask::Follow("agentAccounts"));
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
        let focus = cx.focus_handle();
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
            cx.observe_window_visibility(window, |bench, _, window, cx| {
                bench.window_shown = window.is_visible();
                bench.watch_ci();
                crate::terminal::keep_blinking(bench, cx)
            }),
            cx.observe(&voice, |bench, _, cx| {
                if bench.settings.is_some() {
                    bench.parts.redraw(&bench.parts.stage, cx);
                }
            }),
        ];
        let mut bench = Self {
            version: String::new(),
            view: None,
            reads: HashMap::new(),
            preferences: Preferences::default(),
            lost: None,
            stopped: false,
            stuck: None,
            boot_shown: false,
            slow_start: false,
            voice,
            _voice_heard: voice_heard,
            _before_shortcuts: {
                let this = cx.weak_entity();
                cx.intercept_keystrokes(move |event, window, cx| {
                    let _ = this.update(cx, |bench, cx| {
                        if !bench.catch_loose_keys(event, window, cx) {
                            crate::settings::before_shortcut(bench, &event.keystroke, window, cx)
                        }
                    });
                })
            },
            _keys_settle: [
                cx.on_focus_lost(window, |bench, window, cx| bench.settle_keys(window, cx)),
                cx.observe_window_activation(window, |bench, window, cx| {
                    if window.is_window_active() {
                        bench.settle_keys(window, cx)
                    }
                }),
                cx.on_focus(&focus, window, |bench, window, cx| {
                    bench.settle_keys(window, cx)
                }),
            ],
            takes_keys: Cell::new(None),
            window_active: window.is_window_active(),
            window_shown: window.is_visible(),
            on_screen: None,
            watched_project: None,
            watched_place: crate::hosts::Place::This,
            watched_ci: None,
            watched_head: None,
            ci_card: {
                let bench = cx.weak_entity();
                cx.new(|_| crate::ci_card::CiCard::new(bench))
            },
            battery: crate::battery::read(),
            agent_setup: Default::default(),
            experience: None,
            command_popup: None,
            project_settings: HashMap::new(),
            kept_running_told: false,
            open_waits: Vec::new(),
            pairing: crate::pairing::Pairing::new(cx),
            shown: Default::default(),
            picture: None,
            picture_hash: None,
            signing_in: false,
            remote: None,
            accounts: Default::default(),
            badge: 0,
            reorder: None,
            place: None,
            last_place: None,
            all_projects: false,
            folded_teams: HashSet::new(),
            rail_seen: Default::default(),
            strip_seen: Default::default(),
            closing_tabs: Default::default(),
            strip_glide: Default::default(),
            strip_scroll: Default::default(),
            rail_scroll: Default::default(),
            strip_scrolled_to: Default::default(),
            rail_glide: Default::default(),
            agent_card: crate::agent_card::Card::default(),
            picked_tasks: HashMap::new(),
            rail_frame: Rc::default(),
            stage_frame: Rc::default(),
            tests_closed: None,
            failure_shown: None,
            space: None,
            side_rail_open: true,
            tab_rail_open: true,
            side_rail_width: rail::SIDE_RAIL.initial,
            tab_rail_width: agents::TAB_RAIL.initial,
            resizers: [cx.focus_handle(), cx.focus_handle()],
            retitling: None,
            asking: None,
            host_views: HashMap::new(),
            hosts_reached: HashSet::new(),
            approvals: HashMap::new(),
            hosts_unavailable: None,
            track: crate::track::Slide::default(),
            rail_slide: crate::track::Slide::default(),
            adding_task: None,
            adding_provider: None,
            buddy_picker: None,
            run_check: None,
            runs_to_show: HashSet::new(),
            sharing: None,
            watched_worktree: None,
            trust_asked: HashSet::new(),
            skills_offered: HashSet::new(),
            trust_host: None,
            peeking: None,
            unpeek: None,
            resting_chats: None,
            git_behind: false,
            run_clock: cx.new(|_| crate::tabs::RunClock::default()),
            rail_page: RailPage::Agents,
            files: FileTree::default(),
            files_focus: cx.focus_handle(),
            finder: None,
            palette: None,
            moving: None,
            space_editor: None,
            switcher: None,
            recent_commands: Vec::new(),
            keeping_rails: None,
            files_retry: Default::default(),
            project_files: ProjectFiles::default(),
            launcher_choice: None,
            usage_wanted: false,
            all_harnesses: false,
            pinch: None,
            metric: 0,
            metric_day: None,
            filter: None,
            filter_text: String::new(),
            older: None,
            asks,
            chats: HashMap::new(),
            focused_chat: None,
            deliveries: HashMap::new(),
            questions: Vec::new(),
            host_questions: Vec::new(),
            terminals: terminal::Terminals::default(),
            editors: crate::editor::Editors::default(),
            git: None,
            area: Area::Local,
            host: None,
            opening: None,
            git_picked: None,
            git_listing: false,
            search: None,
            board: None,
            plugin_panes: HashMap::new(),
            plugin_bars: HashMap::new(),
            plugin_opening: HashMap::new(),
            top_bar_hovered: false,
            settings: None,
            focus,
            window: window.window_handle(),
            parts: Parts::new(cx),
            panes: crate::panes::Panes::default(),
            desks: crate::desk::Desks::new(window, cx),
            sims: crate::sim_pane::Simulators::new(cx),
            _filtering: None,
            _older: None,
        };
        bench.focus.focus(window, cx);
        bench.wait_patiently(cx);
        crate::handover::follow(&mut bench, cx);
        bench
    }

    /// Whether the Limits panel is on screen: the tab rail open on its agents page, for one
    /// agent.
    fn limits_shown(&self) -> bool {
        self.tab_rail_open
            && self.rail_page == RailPage::Agents
            && !self.all_harnesses
            && self.launcher().is_some()
    }

    /// Reads the plan limits again: each lookup reaches the agent's service on the person's
    /// sign-in, so it runs as the app starts, every few minutes while the Limits panel shows,
    /// and when the person asks.
    pub fn read_usage(&self) {
        self.call("readAgentUsages", json!({}));
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        #[cfg(feature = "drive")]
        if let Update::Read(key, _) = &update {
            crate::drive::trace(format_args!("main read {key}"));
        }
        if self.holds(&update) {
            return;
        }
        match update {
            Update::Connected(version) => {
                // The app opens on this host, as the Mac's does, whichever host was last in front.
                if self.version.is_empty() {
                    self.call("chooseHost", json!({ "host": "this" }));
                    self.usage_wanted = true;
                }
                self.version = version;
                self.lost = None;
                self.stopped = false;
                self.stuck = None;
                self.on_screen = None;
                for (agent, chat) in self.chats.iter().filter(|(_, chat)| !chat.resting) {
                    self.ask_chat(ChatAsk::Attach {
                        agent: agent.clone(),
                        since: chat.mark.clone(),
                    });
                }
                self.terminals.reattach(&self.asks);
                self.files.relist_shown(&self.target());
                crate::editor::reconnected(self, cx);
            }
            Update::From(_, update) if matches!(*update, Update::Chat(..)) => {
                self.apply(*update, cx)
            }
            Update::HostView(host, view) => {
                let in_front = self.place() == crate::hosts::Place::Account(host.clone());
                self.host_views.insert(host, *view);
                self.settle_closing();
                if in_front {
                    self.watch_project();
                    self.watch_worktree();
                    self.close_chats(cx);
                    self.open_chat(cx);
                    self.rest_chats_later(cx);
                    self.look_for_older(false, crate::recent::PAGE, cx);
                    self.front_moved(cx);
                    self.files.reveal(self.front_file(), &self.target());
                    crate::settings::front_moved(self, cx);
                }
                self.publish_on_screen();
            }
            Update::HostReach(host, reached) => {
                if reached {
                    self.hosts_reached.insert(host.clone());
                    crate::editor::host_reached(self, &Place::Account(host.clone()));
                    self.read_approval(Some(host.clone()), cx);
                } else {
                    self.hosts_reached.remove(&host);
                }
                self.parts.redraw(&self.parts.stage, cx);
            }
            Update::HostsUnavailable(reason) => self.hosts_unavailable = Some(reason),
            Update::Read("hosts", value) => {
                let linked = |id: &String| {
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|host| host["id"] == id.as_str())
                };
                self.host_views.retain(|id, _| linked(id));
                self.hosts_reached.retain(linked);
                self.approvals
                    .retain(|id, _| id.as_ref().is_none_or(linked));
                if !self.approvals.contains_key(&None) {
                    self.read_approval(None, cx);
                }
                self.reads.insert("hosts", value);
            }
            Update::View(view) => {
                crate::ui::moving_face::set_on(view.face_style.motion, cx);
                self.view = Some(*view);
                self.forget_departed();
                self.settle_closing();
                // The core finds its agents a moment after it starts, so the plan limits are
                // read once it lists them.
                if self.usage_wanted && !self.launchers().is_empty() {
                    self.usage_wanted = false;
                    self.read_usage();
                }
                let sessions = &self
                    .view
                    .as_ref()
                    .map_or(&[][..], |view| view.sessions.as_slice());
                let hosts = &self.host_views;
                self.terminals.keep(
                    |place, id| match place {
                        Place::This => sessions.iter().any(|session| session.id == id),
                        Place::Account(host) => hosts.get(host).is_none_or(|view| {
                            view.sessions.iter().any(|session| session.id == id)
                        }),
                    },
                    &self.asks,
                );
                crate::editor::keep(self);
                crate::agent_setup::settle(self, cx);
                self.settle_open_waits();
                crate::notifications::badge(self);
                crate::notifications::kept_running(self, cx);
                self.note_place();
                self.watch_project();
                self.watch_worktree();
                self.watch_picture();
                self.watch_ci();
                self.close_chats(cx);
                self.open_chat(cx);
                self.rest_chats_later(cx);
                self.look_for_older(false, crate::recent::PAGE, cx);
                self.front_moved(cx);
                self.files.reveal(self.front_file(), &self.target());
                crate::settings::front_moved(self, cx);
                self.publish_on_screen();
            }
            Update::Chat(agent, news) => {
                let Some(chat) = self.chats.get_mut(&agent) else {
                    return;
                };
                match news {
                    ChatNews::Attached(attachment) => {
                        if let Some(ask) = chat.attached(&agent, attachment, cx) {
                            self.ask_chat(ask);
                        }
                    }
                    ChatNews::Changes { seq, changes } => {
                        let gap = chat.changed(seq, changes, cx);
                        let (since, wanted) = (chat.mark.clone(), chat.wanted());
                        if gap {
                            self.ask_chat(ChatAsk::Attach {
                                agent: agent.clone(),
                                since,
                            });
                        }
                        for tool in wanted {
                            self.ask_chat(ChatAsk::Detail {
                                agent: agent.clone(),
                                tool,
                            });
                        }
                    }
                    ChatNews::Page(page) => chat.page(page, cx),
                    ChatNews::Later(page) => {
                        if let Some(ask) = chat.later_page(&agent, page, cx) {
                            self.ask_chat(ask);
                        }
                    }
                    ChatNews::Around(page) => chat.around_page(page, cx),
                    ChatNews::Latest(page) => chat.latest_page(page, cx),
                    ChatNews::Found { asked, found } => self.found(&agent, asked, found, cx),
                    ChatNews::Detail { tool, detail } => chat.detail(tool, detail, cx),
                    ChatNews::Replied { request, error } => chat.replied(&request, error),
                    ChatNews::TaskStopped { task, error } => chat.task_stopped(&task, error),
                    ChatNews::Refused { error, text } => {
                        if let Some(composing) = chat.composer.as_mut() {
                            composing.refusal = Some(error.into());
                            let held = composing
                                .sent
                                .take()
                                .filter(|sent| sent.content.text().trim() == text);
                            composing.returned = Some(held.unwrap_or_else(|| Sent {
                                content: text.into(),
                                attachments: Vec::new(),
                                context: Vec::new(),
                                tracked_context: HashMap::new(),
                            }));
                        }
                        return self.parts.redraw(&self.parts.stage, cx);
                    }
                }
                let restage = self
                    .chats
                    .get_mut(&agent)
                    .is_some_and(Chat::take_head_moved);
                if restage && self.agent().is_some_and(|front| front.id == agent) {
                    self.parts.redraw(&self.parts.stage, cx);
                }
                return;
            }
            Update::Read("skills-added", value) => return self.offer_skills(&value, cx),
            Update::Read("plugins", value) => {
                self.reads.insert("plugins", value);
                self.watch_ci();
            }
            Update::Read("ci", value) => {
                self.reads.insert("ci", value);
                self.watch_head_runs();
            }
            Update::Read("update", value) => {
                let ready = |value: &Value| {
                    value["state"] == "ready"
                        && value["available"]["version"] != env!("CARGO_PKG_VERSION")
                };
                let already = self.read("update").is_some_and(ready);
                let now = ready(&value);
                self.reads.insert("update", value);
                if self.over_shown() {
                    self.parts.redraw(&self.parts.over, cx);
                }
                if now && !already {
                    // The core put the update in place of this app; opening the app again starts it.
                    // An AppImage runs from a mount that goes with this process, so it starts again
                    // from its file.
                    if let Some(appimage) = std::env::var_os("APPIMAGE") {
                        cx.set_restart_path(appimage.into());
                    }
                    if self
                        .read("update")
                        .is_some_and(|update| update["forced"] == true)
                    {
                        crate::core::force_next_core_update();
                    }
                    // Restarting runs the restart hooks, which update this workbench.
                    cx.defer(|cx| cx.restart());
                }
            }
            Update::Read(key @ ("questions" | "host-questions"), value) => {
                match serde_json::from_value::<Vec<Question>>(value) {
                    Ok(mut questions) if key == "host-questions" => {
                        for question in &mut questions {
                            question.host = self.trust_host.clone();
                        }
                        self.host_questions = questions;
                    }
                    Ok(questions) => {
                        self.questions = questions;
                        self.questions.retain(|question| question.host.is_none());
                    }
                    Err(error) => eprintln!("sikemux: the core's questions do not read: {error}"),
                }
                self.questions.retain(|question| question.host.is_none());
                self.questions.extend(self.host_questions.iter().cloned());
            }
            Update::Read("theme", value) => {
                if let Ok(palette) = serde_json::from_value(value) {
                    theme::apply(&palette, cx);
                    crate::syntax::restyle(cx);
                    cx.refresh_windows();
                }
            }
            Update::Read("picture", value) => self.picture = decode_picture(&value),
            Update::Read("sign-in", value) => match value["url"].as_str() {
                Some(url) => {
                    cx.open_url(url);
                    let _ = self.asks.send(Ask::Answer {
                        key: "sign-in-finished",
                        method: "accountSignInFinish",
                        params: json!({}),
                    });
                }
                None => self.signing_in = false,
            },
            Update::Read("sign-in-finished", _) => {
                self.signing_in = false;
                crate::pairing::offer_account_phones(self, cx);
            }
            Update::Failed(error) => {
                return self.notify(toast::error(error).autohide(false), cx);
            }
            Update::Refused(key, error) => {
                eprintln!("sikemux: the core refused {key}: {error}");
                if key.starts_with("sign-in") {
                    self.signing_in = false;
                }
                if key == "files.list" {
                    self.files_retry.refused(
                        cx,
                        |bench| &mut bench.files_retry,
                        |bench, _| {
                            if bench.out_of_reach().is_none() {
                                bench.files.relist_shown(&bench.target());
                            }
                        },
                    );
                }
                if key == crate::settings::ACTIVITY {
                    self.reads.insert(key, Value::Null);
                    self.parts.redraw(&self.parts.stage, cx);
                    self.parts.activity.update(cx, |_, cx| cx.notify());
                }
            }
            Update::Read("files.list", value) => {
                self.files_retry.answered();
                self.files.listed(&value, &self.target());
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Update::Read("git-project-repo", value) => {
                let was = self.git_repo();
                self.reads.insert("git-project-repo", value);
                return self.follow_git_repo(was, cx);
            }
            Update::Read("git-overview", value) => {
                self.files.mark(&value);
                match (&self.git, self.git_in_front()) {
                    (Some(pane), true) => pane.update(cx, |pane, cx| pane.overview(&value, cx)),
                    (Some(_), false) => self.git_behind = true,
                    (None, _) => {}
                }
                self.reads.insert("git-overview", value);
                self.parts.redraw(&self.parts.top, cx);
                return self.parts.redraw(&self.parts.tabs, cx);
            }
            Update::Read("projectFiles", value) => {
                if self.project_files.answered(&value) {
                    self.rank(cx);
                    self.files_listed(cx);
                }
                return;
            }
            Update::Read("preferences", value) => {
                if let Ok(preferences) = serde_json::from_value::<Preferences>(value.clone()) {
                    crate::keymap::bind(&preferences.keybinding_overrides, cx);
                    if self.keeping_rails.is_none() {
                        self.follow_rails(&preferences);
                    }
                    self.sims.offer_to_agents(preferences.ios_simulator);
                    let on = preferences.voice_dictation;
                    self.voice.update(cx, |voice, cx| voice.follow(on, cx));
                    let scale = preferences.chat_text_scale as f32;
                    for chat in self.chats.values_mut() {
                        chat.rescale(scale, cx);
                    }
                    crate::ui::grain::set(&preferences, cx);
                    crate::theme::set_text_scale(preferences.ui_text_scale as f32, cx);
                    crate::see_through::set(&preferences, cx);
                    crate::experience::preferences_read(self, &preferences, cx);
                    self.preferences = preferences;
                }
                self.reads.insert("preferences", value);
                self.terminals.redraw_all(cx);
                crate::notifications::introduce(self, cx);
                crate::notifications::kept_running(self, cx);
            }
            Update::Read("git-draft", value) => {
                if let Some(pane) = &self.git {
                    pane.update(cx, |pane, cx| pane.drafts(&value, cx));
                }
                self.reads.insert("git-draft", value);
            }
            Update::Read("stash", mut value) => {
                let seen = |read: &Value| {
                    read["seenAt"]
                        .as_str()
                        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                };
                let kept = self.reads.get("stash").and_then(|read| {
                    let at = seen(read)?;
                    (seen(&value).is_none_or(|told| told < at)).then(|| read["seenAt"].clone())
                });
                if let Some(kept) = kept {
                    value["seenAt"] = kept;
                }
                self.reads.insert("stash", value);
                let unseen = !crate::stash::Stash::of(self).new_items().is_empty();
                if unseen && self.stash_in_front() {
                    self.see_stash();
                }
            }
            Update::Read(key @ ("drafts" | "host-drafts"), value) => {
                self.drafts(&value, key == "host-drafts", cx);
                self.reads.insert(key, value);
            }
            Update::Read("catalog", value) => {
                if let Some(pane) = &self.git {
                    pane.update(cx, |pane, cx| pane.catalog(&value, cx));
                }
                self.reads.insert("catalog", value);
            }
            Update::Read(key @ ("trust" | "host-trust"), value) => {
                self.reads.insert(key, value);
                self.ask_trust(cx);
            }
            Update::Read("project-config", value) => {
                crate::keymap::bind_project_actions(&crate::commands::action_keys(&value), cx);
                self.reads.insert("project-config", value);
            }
            Update::Read(key @ ("runs" | "ports"), value) => {
                self.reads.insert(key, value);
                if key == "runs" {
                    self.place_agent_runs();
                    self.show_started_runs();
                }
                self.time_run(cx);
            }
            Update::Read("whatsNew", value) => crate::experience::whats_new_read(self, &value, cx),
            Update::Read(key, value) if crate::codehost::on_desks(key) => {
                self.read_desk_pulls(key, &value, cx);
            }
            Update::Read(key, value) if crate::codehost::watches(key) => {
                if let Some(host) = &self.host {
                    host.update(cx, |host, cx| host.read(key, &value, cx));
                }
            }
            Update::Read(key, value) => {
                self.reads.insert(key, value);
                if matches!(key, "pickerFolders" | "sshHosts") {
                    self.picker_answered(cx);
                }
            }
            Update::Terminal(session, screenful) => {
                let key = self.terminals.key(&Place::This, session);
                self.terminals.apply(key, screenful, cx);
                return self.terminals.redraw(key, cx);
            }
            Update::From(host, update) if matches!(*update, Update::Terminal(..)) => {
                if let Update::Terminal(session, screenful) = *update {
                    let key = self.terminals.key(&Place::Account(host), session);
                    self.terminals.apply(key, screenful, cx);
                    return self.terminals.redraw(key, cx);
                }
            }
            Update::From(host, update) if matches!(&*update, Update::Changed { topic, .. } if topic == "pluginView") => {
                if let Update::Changed { paths, .. } = *update {
                    self.plugin_views_changed(&Place::Account(host), paths.as_deref(), cx);
                }
            }
            Update::From(host, update) if matches!(&*update, Update::Changed { topic, .. } if topic == "files" || topic == "git") => {
                if let Update::Changed {
                    topic,
                    project,
                    paths,
                } = *update
                {
                    let place = Place::Account(host);
                    if topic == "files" {
                        crate::editor::changed(
                            self,
                            &place,
                            paths.as_deref().unwrap_or_default(),
                            cx,
                        );
                    }
                    self.project_changed(&place, &topic, project, paths, cx);
                }
            }
            Update::From(..) => {}
            Update::Lost(reason) => self.lost = Some(reason),
            Update::Stopped => {
                self.lost = None;
                self.stopped = true;
            }
            Update::Stuck(pid) => {
                self.lost = None;
                self.stuck = Some((pid, false));
            }
            Update::ShutDown => return cx.quit(),
            Update::Notice { title, body, agent } => {
                return crate::notifications::notice(self, title, body, agent, cx);
            }
            Update::FocusAgent(agent) => {
                cx.activate(true);
                return self.open_agent(&agent);
            }
            Update::AccountDevice { key, name } => return self.new_account_device(key, name, cx),
            Update::Network(change) => {
                return self
                    .voice
                    .update(cx, |voice, cx| voice.network_changed(change, cx));
            }
            Update::Remote(status) => {
                let was_required = self.remote.as_ref().map(|remote| &remote.update_required);
                if was_required != Some(&status.update_required) {
                    cx.notify();
                }
                self.remote = Some(*status);
                crate::settings::remote_moved(self, cx);
                crate::pairing::remote_moved(self, cx);
                return self.parts.redraw(&self.parts.stage, cx);
            }
            Update::WindowCall(call_id, call) => self.answer_window_call(call_id, call, cx),
            Update::Changed {
                topic,
                project,
                paths,
            } => {
                if topic == "files" {
                    crate::editor::changed(
                        self,
                        &Place::This,
                        paths.as_deref().unwrap_or_default(),
                        cx,
                    );
                }
                self.project_changed(&Place::This, &topic, project.clone(), paths.clone(), cx);
                if topic == "boards" {
                    if let Some(board) = &self.board {
                        board.update(cx, |board, cx| board.changed(cx));
                    }
                }
                if topic == "pluginView" {
                    self.plugin_views_changed(&Place::This, paths.as_deref(), cx);
                }
                if topic == "lsp" {
                    crate::editor::diagnostics_changed(
                        self,
                        paths.as_deref().unwrap_or_default(),
                        cx,
                    );
                }
                if topic == "agentSignIn" {
                    crate::accounts::pages_moved(self, cx);
                }
                if topic == "agentAccounts" {
                    crate::accounts::book_moved(self, cx);
                }
                if topic == "agentStatus" {
                    crate::accounts::statuses_moved(self, cx);
                }
                if topic == "git" {
                    crate::editor::git_changed(self, project.as_deref(), paths.as_deref(), cx);
                }
                return;
            }
        }
        cx.notify();
    }

    pub fn ask_chat(&self, ask: ChatAsk) {
        self.chat_target(ask.agent()).ask(Ask::Chat(ask));
    }

    /// Where `agent`'s chat asks: the core of the host the agent runs on.
    pub fn chat_target(&self, agent: &str) -> Target {
        self.chats.get(agent).map_or_else(
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
        if let Some(chat) = self.chats.get_mut(&agent) {
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
        self.chats.insert(agent.clone(), chat);
        self.ask_chat(ChatAsk::Attach { agent, since: None });
    }

    /// Lets go of the chats out of sight once they have stayed so for `CHAT_RESTS_AFTER`.
    fn rest_chats_later(&mut self, cx: &mut Context<Self>) {
        let front = self.agent().map(|agent| agent.id);
        let anyone = self
            .chats
            .iter()
            .any(|(agent, chat)| !chat.resting && Some(agent) != front.as_ref());
        if !anyone {
            self.resting_chats = None;
            return;
        }
        if self
            .resting_chats
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
        self.resting_chats = Some((front, task));
    }

    fn rest_chats(&mut self) {
        self.resting_chats = None;
        let front = self.agent().map(|agent| agent.id);
        let resting: Vec<String> = self
            .chats
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

    /// Makes the composer of the chat in front, which needs the window.
    fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let Some(chat) = self
            .chats
            .get_mut(&agent)
            .filter(|chat| chat.composer.is_none())
        else {
            return;
        };
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(true)
        });
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Type the answer")
        });
        let sending = {
            let agent = agent.clone();
            let was_empty = Cell::new(true);
            cx.subscribe_in(
                &draft,
                window,
                move |bench, draft, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary,
                    } => bench.send(&agent, *secondary, window, cx),
                    InputEvent::Change => {
                        let empty = draft.read(cx).value().is_empty();
                        if was_empty.replace(empty) != empty {
                            if !empty {
                                bench.began_writing(&agent);
                            }
                            cx.notify();
                        }
                        if let Some(composing) = bench
                            .chats
                            .get_mut(&agent)
                            .and_then(|chat| chat.composer.as_mut())
                        {
                            composing.drop_untagged(window, cx);
                        }
                        bench.offer(&agent, cx);
                        bench.typed_draft(&agent, cx);
                    }
                    _ => {}
                },
            )
        };
        let keeping = {
            let agent = agent.clone();
            cx.subscribe_in(
                &secret,
                window,
                move |bench, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        bench.keep_secret(&agent, window, cx);
                    }
                },
            )
        };
        let mic = {
            let (voice, toggle) = (
                self.voice.clone(),
                crate::chat::dictate(&agent, cx.weak_entity()),
            );
            let agent = agent.clone();
            cx.new(|cx| crate::voice::MicButton::new(voice, agent, toggle, cx))
        };
        let bench = cx.weak_entity();
        let view = cx.new(|_| crate::chat::ComposerView {
            bench,
            agent: None,
            covered: false,
        });
        chat.composer = Some(Composing {
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
            preparing: None,
            send_when_ready: false,
            install: None,
            sent: None,
            returned: None,
            synced: json!({ "text": "" }),
            read_rev: 0,
            arrived: None,
            _keeping: Task::ready(()),
            _heard: [sending, keeping],
        });
        let key = crate::chat::draft::read_key(crate::chat::draft::is_elsewhere(chat));
        if let (Some(composing), Some(drafts)) = (chat.composer.as_mut(), self.reads.get(key)) {
            composing.start_from(drafts, &agent, window, cx);
        }
        self.watch_drafts();
        if let Some(text) = self.deliveries.remove(&agent) {
            self.deliver(&agent, text, window, cx);
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
        let project = self.view.as_ref().and_then(|view| {
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
        if let Some(session) = self.agent_terminal(agent) {
            let key = self.terminals.key(&Place::This, session);
            crate::terminal::hand_to(self, key, text, cx);
            if let Some(focus) = self.terminals.focus(key) {
                window.focus(focus, cx);
            }
            return;
        }
        let Some(draft) = self
            .chats
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.draft.clone()))
        else {
            self.deliveries
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
        let Some(chat) = self.chats.get(agent) else {
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
        let Some(chat) = self.chats.get_mut(&agent) else {
            return;
        };
        let mut returned = false;
        if let Some(composing) = chat.composer.as_mut() {
            let empty = composing.draft.read(cx).value().trim().is_empty();
            if let Some(held) = composing.returned.take().filter(|_| empty) {
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
        let Some(chat) = self.chats.get_mut(&agent) else {
            return;
        };
        let hint = SharedString::from(chat.hint(&agent, &self.questions, steer_first));
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
        let Some(chat) = self.chats.get(agent) else {
            return;
        };
        let shown = (agent.to_owned(), chat.ready());
        if self.focused_chat.as_ref() == Some(&shown) {
            return;
        }
        let Some(draft) = chat
            .composer
            .as_ref()
            .map(|composing| composing.draft.clone())
        else {
            return;
        };
        self.focused_chat = Some(shown);
        let held = window.focused(cx);
        let free = held.as_ref().is_none_or(|held| {
            *held == self.focus
                || self.chats.values().any(|chat| {
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
        let preparing = self
            .chats
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
            .is_some_and(|composing| composing.preparing.is_some());
        if preparing || self.hold_for_install(agent, cx) {
            return;
        }
        if self.starts_in_worktree(agent, cx) {
            let text = self
                .chats
                .get(agent)
                .and_then(|chat| Some(chat.composer.as_ref()?.draft.read(cx).value().to_string()))
                .unwrap_or_default();
            return self.begin_worktree(agent, text, cx);
        }
        let Some(composing) = self
            .chats
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
                            .or_else(|| chat.steer_waiting(agent))
                    },
                    cx,
                );
                cx.notify();
            }
            return;
        }
        let unknown = self
            .chats
            .get(agent)
            .and_then(|chat| chat.unknown_command(&text))
            .map(|name| format!("/{name} is not available in this session"));
        if let Some(refused) = unknown {
            if let Some(composing) = self
                .chats
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
                    composing.sent = Some(sent);
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
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.secret.clone()))
        else {
            return;
        };
        let text = field.read(cx).value().to_string();
        let sending = self.chats.get(agent).is_some_and(Chat::sending_answers);
        if text.is_empty() || sending {
            return;
        }
        self.answer(agent, |chat, open| chat.keep_secret(text, open), cx);
        field.update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }

    /// Changes the chat of `agent` with its open questions to hand, and asks the core what that asks.
    pub fn answer(
        &mut self,
        agent: &str,
        change: impl FnOnce(&mut Chat, &[&Question]) -> Option<Ask>,
        cx: &mut Context<Self>,
    ) {
        let open: Vec<&Question> = self
            .questions
            .iter()
            .filter(|question| question.asks(agent))
            .collect();
        if let Some(ask) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| change(chat, &open))
        {
            self.ask_answering(agent, ask, cx);
        }
    }

    /// Lets go of the questions `keep` turns down, of this host and of the host in front.
    pub fn keep_questions(&mut self, keep: impl Fn(&Question) -> bool) {
        self.questions.retain(&keep);
        self.host_questions.retain(keep);
    }

    /// Whether the agent asks before it acts, kept for its next session and changed in the one running.
    pub fn switch_mode(&self, agent: &str, mode: &str) {
        self.chat_target(agent).call(
            "updateAgent",
            json!({ "agentId": agent, "patch": { "permissionMode": mode } }),
        );
        self.ask_chat(ChatAsk::Mode {
            agent: agent.to_owned(),
            mode: mode.to_owned(),
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
            .keys()
            .filter(|agent| !shown.contains(agent))
            .cloned()
            .collect();
        let closed = !gone.is_empty();
        for agent in gone {
            if let Some(composing) = self.chats.remove(&agent).and_then(|chat| chat.composer) {
                self.release_keys(composing.draft.read(cx).focus_handle(cx), cx);
            }
            self.focused_chat.take_if(|(shown, _)| *shown == agent);
            self.ask_chat(ChatAsk::Detach { agent });
        }
        if closed {
            self.watch_drafts();
        }
    }

    /// Asks for the rows after the last the chat holds, when those were dropped.
    pub fn ask_later(&mut self, agent: &str) {
        if let Some(ask) = self.chats.get_mut(agent).and_then(|chat| chat.later(agent)) {
            self.ask_chat(ask);
        }
    }

    /// Brings `agent`'s chat to its latest rows.
    pub fn show_latest(&mut self, agent: &str, cx: &mut Context<Self>) {
        if let Some(ask) = self
            .chats
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
            .get_mut(agent)
            .and_then(|chat| chat.earlier(agent))
        {
            self.ask_chat(ask);
        }
    }

    /// Reads the file tree, the finder's list and search again when `topic` moved on
    /// `place`, while the project in front is on that host.
    fn project_changed(
        &mut self,
        place: &Place,
        topic: &str,
        project: Option<String>,
        paths: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.place() != *place {
            return;
        }
        let root = self
            .files
            .root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned());
        if topic == "git" {
            match (&self.git, self.git_in_front()) {
                (Some(pane), true) => pane.update(cx, |pane, cx| {
                    pane.git_moved(project.as_deref(), paths.as_deref(), cx)
                }),
                (Some(_), false) => self.git_behind = true,
                (None, _) => {}
            }
        }
        if topic == "git" && (project.is_none() || project == root) {
            if self.files.changed(paths.as_deref()) {
                self.parts.redraw(&self.parts.tabs, cx);
            }
            if self.finder.is_some() {
                self.project_files.read(root.as_deref(), &self.target());
            }
        }
        if topic == "files" {
            if let Some(search) = &self.search {
                search.update(cx, |search, cx| search.changed(cx));
            }
        }
    }

    /// The core the panes of the project in front ask: this host's, or the chosen account
    /// host's.
    pub fn target(&self) -> Target {
        Target::new(&self.asks, self.place())
    }

    /// The name of the account host the panes ask while it is out of reach, else none.
    fn out_of_reach(&self) -> Option<SharedString> {
        let crate::hosts::Place::Account(key) = self.place() else {
            return None;
        };
        if self.hosts_reached.contains(&key) {
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

    /// Tells the Git pane and the code host's areas whether their host is in reach.
    fn tell_reach(&self, cx: &mut Context<Self>) {
        let away = self.out_of_reach();
        if let Some(pane) = &self.git {
            let host = self.code_host_of(&pane.read(cx).repo);
            let signed_in = self.host_signed_in(cx);
            pane.update(cx, |pane, cx| {
                pane.set_out_of_reach(away.clone(), cx);
                pane.set_code_host(host, signed_in, cx);
            });
        }
        if let Some(area) = &self.host {
            area.update(cx, |area, cx| area.set_out_of_reach(away, cx));
        }
    }

    fn host_signed_in(&self, cx: &App) -> bool {
        self.host
            .as_ref()
            .is_some_and(|area| area.read(cx).signed_in())
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
        if self.git_listing {
            return None;
        }
        if let Some((at, repo)) = &self.git_picked {
            if at == folder {
                return Some(repo.clone());
            }
        }
        let found = self.read("git-project-repo")?;
        match found["repo"].as_str() {
            Some(repo) => Some(repo.to_owned()),
            None if found["repos"].is_array() => None,
            None => Some(folder.clone()),
        }
    }

    /// The project folder the Git views follow.
    pub(crate) fn git_folder(&self) -> Option<&str> {
        self.watched_project.as_deref()
    }

    /// The repositories directly inside the project folder, while it is not one itself.
    pub(crate) fn git_repos(&self) -> Option<&Vec<Value>> {
        self.read("git-project-repo")?["repos"].as_array()
    }

    /// Opens `repo` in the project's Git views until the project changes.
    pub(crate) fn pick_git_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git_picked = self.watched_project.clone().map(|folder| (folder, repo));
        self.git_listing = false;
        self.follow_git_repo(was, cx);
    }

    /// Shows the repositories inside the project folder again.
    pub(crate) fn list_git_repos(&mut self, cx: &mut Context<Self>) {
        let was = self.git_repo();
        self.git_listing = true;
        self.follow_git_repo(was, cx);
    }

    /// Points the git overview at the repository shown, when it moved from `was`.
    fn follow_git_repo(&mut self, was: Option<String>, cx: &mut Context<Self>) {
        let repo = self.git_repo();
        if let Some(repo) = repo
            .as_ref()
            .filter(|repo| Some(*repo) != was.as_ref() || self.read("git-overview").is_none())
        {
            let target = self.target();
            target.watch(
                "git-overview",
                Watch::new("git.overview", json!({ "repo": repo }), &["git"]),
            );
            let draft = format!("git:{repo}");
            target.watch(
                "git-draft",
                Watch::new("drafts", json!({ "keys": [draft] }), &["drafts"]),
            );
        }
        self.parts.redraw(&self.parts.stage, cx);
    }

    /// Points the watches that follow the project in front at it.
    fn watch_project(&mut self) {
        let path = self
            .project()
            .map(|project| project.path.to_string_lossy().into_owned());
        let place = self.place();
        if path == self.watched_project && place == self.watched_place {
            return;
        }
        if place != self.watched_place {
            for key in ["activity", "tasks", "runs", "ports"] {
                self.reads.remove(key);
            }
        }
        let target = Target::new(&self.asks, place.clone());
        let activity = json!({ "utcOffsetMinutes": utc_offset_minutes(), "project": path });
        target.watch(
            "activity",
            Watch::new("activitySummary", activity, &["activity"]),
        );
        match &place {
            crate::hosts::Place::Account(host) if self.trust_host.as_ref() != Some(host) => {
                self.reads.remove("host-trust");
                self.trust_host = Some(host.clone());
                target.watch("host-trust", Watch::new("trustAsks", json!({}), &["trust"]));
                self.host_questions.clear();
                target.watch(
                    "host-questions",
                    Watch::new("questions", json!({}), &["questions"]),
                );
            }
            crate::hosts::Place::This if self.trust_host.is_some() => {
                for key in ["host-trust", "host-questions"] {
                    target.unwatch(key);
                    self.reads.remove(key);
                }
                self.trust_host = None;
                self.host_questions.clear();
                self.questions.retain(|question| question.host.is_none());
            }
            _ => {}
        }
        self.reads.remove("git-project-repo");
        self.git_listing = false;
        match &path {
            Some(path) => {
                target.watch(
                    "tasks",
                    Watch::new("tasks", json!({ "project": path }), &["tasks", "runs"]),
                );
                target.watch("runs", Watch::new("taskRuns", json!({}), &["runs"]));
                target.watch(
                    "project-config",
                    Watch::new("projectConfig", json!({ "project": path }), &["tasks"]),
                );
                target.watch(
                    "ports",
                    Watch::new("ports", json!({ "project": path }), &["ports"]),
                );
                target.watch(
                    "git-project-repo",
                    Watch::new(
                        "git.projectRepo",
                        json!({ "project": path }),
                        &["git", "preferences"],
                    ),
                );
            }
            None => {
                for key in [
                    "tasks",
                    "runs",
                    "project-config",
                    "ports",
                    "git-project-repo",
                ] {
                    target.unwatch(key);
                    self.reads.remove(key);
                }
            }
        }
        self.reads.remove("git-draft");
        self.files.show(path.clone().map(PathBuf::from), &target);
        self.watched_project = path;
        self.watched_place = place;
        self.watch_drafts();
    }

    /// Keeps the core answering the drafts on show: the commit message of the project in
    /// front, and every open chat's.
    fn watch_drafts(&self) {
        let keys = |elsewhere: bool| -> Vec<String> {
            self.chats
                .iter()
                .filter(|(_, chat)| {
                    chat.composer.is_some() && crate::chat::draft::is_elsewhere(chat) == elsewhere
                })
                .map(|(agent, _)| crate::chat::draft_key(agent))
                .collect()
        };
        let watch = |keys: Vec<String>| Watch::new("drafts", json!({ "keys": keys }), &["drafts"]);
        let _ = self.asks.send(Ask::Watch("drafts", watch(keys(false))));
        let there = keys(true);
        let place = self.place();
        if there.is_empty() || place == crate::hosts::Place::This {
            Target::here(&self.asks).unwatch("host-drafts");
        } else {
            Target::new(&self.asks, place).watch("host-drafts", watch(there));
        }
    }

    /// Keeps the core checking the CI of the repository in front while its host is signed in
    /// and the window shows; a hidden window has it check none.
    fn watch_ci(&mut self) {
        let shown = self.ci_repo().filter(|_| self.window_shown);
        if shown == self.watched_ci {
            return;
        }
        let repos: Vec<Value> = shown
            .iter()
            .map(|ci| json!({ "provider": format!("sikemux.{}", ci.host), "owner": ci.owner, "name": ci.name, "account": ci.account, "host": null }))
            .collect();
        let _ = self.asks.send(Ask::Watch(
            "ci-shown",
            Watch::new("showCi", json!({ "repos": repos }), &[]),
        ));
        if let Some(ci) = &shown {
            let runs = json!({ "owner": ci.owner, "name": ci.name, "account": ci.account, "branch": ci.branch, "perPage": 1 });
            let call = json!({ "plugin": format!("sikemux.{}", ci.host), "method": "runs", "params": runs });
            let _ = self
                .asks
                .send(Ask::Watch("ci", Watch::new("plugin", call, &["ci"])));
        }
        self.watched_ci = shown;
    }

    fn ci_repo(&self) -> Option<CiRepo> {
        let project = self.project()?;
        let host = project
            .code_host
            .as_ref()
            .filter(|host| host.host == "github")?;
        let plugin = format!("sikemux.{}", host.host);
        let signed_in = self
            .read("plugins")?
            .as_array()?
            .iter()
            .any(|entry| entry["id"] == plugin.as_str() && entry["signedIn"] == true);
        let (owner, name) = host.repo.split_once('/')?;
        if !signed_in {
            return None;
        }
        Some(CiRepo {
            host: host.host.clone(),
            owner: owner.to_owned(),
            name: name.to_owned(),
            account: project
                .code_hosts
                .get(&host.host)
                .and_then(|choice| choice.account.clone()),
            branch: project.git.as_ref()?.branch.clone(),
        })
    }

    /// Watches every run of the commit the branch's newest run is for, which also finds those
    /// a tag on it started.
    fn watch_head_runs(&mut self) {
        let head = self
            .read("ci")
            .and_then(|ci| ci["value"]["runs"][0]["sha"].as_str());
        let Some(ci) = self.watched_ci.as_ref().filter(|_| head.is_some()) else {
            return;
        };
        if head == self.watched_head.as_deref() {
            return;
        }
        let runs = json!({ "owner": ci.owner, "name": ci.name, "account": ci.account, "headSha": head, "perPage": 20 });
        let call =
            json!({ "plugin": format!("sikemux.{}", ci.host), "method": "runs", "params": runs });
        let _ = self
            .asks
            .send(Ask::Watch("ci-head", Watch::new("plugin", call, &["ci"])));
        self.watched_head = head.map(str::to_owned);
    }

    /// The branch's newest commit's checks, while the core is checking it: the latest run of
    /// each workflow a push or pull request started, as the code host counts a commit's checks.
    /// Runs started by hand or on a schedule only happen to sit on the commit, so they are left out.
    pub fn head_runs(&self) -> Vec<sikemux_wire::codehost::Run> {
        let (Some(_), Some(head)) = (&self.watched_ci, &self.watched_head) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        self.read("ci-head")
            .and_then(|runs| runs["value"]["runs"].as_array())
            .into_iter()
            .flatten()
            .filter(|run| run["sha"] == head.as_str())
            .filter_map(|run| {
                serde_json::from_value::<sikemux_wire::codehost::Run>(run.clone()).ok()
            })
            .filter(|run| {
                matches!(
                    run.event.as_str(),
                    "push" | "pull_request" | "pull_request_target" | "merge_group"
                )
            })
            .filter(|run| seen.insert((run.workflow_id.clone(), run.branch.clone())))
            .collect()
    }

    /// Asks for the account's picture when it is one this window has not got.
    fn watch_picture(&mut self) {
        let hash = self
            .view
            .as_ref()
            .and_then(|view| view.account.as_ref()?.picture_hash.clone());
        if hash == self.picture_hash {
            return;
        }
        self.picture = None;
        if let Some(hash) = &hash {
            let _ = self.asks.send(Ask::Watch(
                "picture",
                Watch::new("accountPicture", json!({ "hash": hash }), &[]),
            ));
        }
        self.picture_hash = hash;
    }

    /// Files dropped where nothing takes them go to the terminal holding the keys, else to the
    /// chat in front.
    fn take_stray_drop(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = crate::terminal::focused(self) {
            return crate::terminal::take_dropped(self, id, paths);
        }
        if let Some((agent, _)) = self.focused_chat.clone() {
            self.attach(&agent, paths.to_vec(), window, cx);
        }
    }

    /// Whether anything is over the whole window: a phone waiting to pair, a command's terminal,
    /// or the welcome, the release notes or runtime diagnostics.
    fn over_shown(&self) -> bool {
        self.experience.is_some() || self.command_popup.is_some() || self.someone_waits()
    }

    /// Whether a push from a watch carries what the window already holds, so nothing in it
    /// moved and nothing need draw again. An answer to a request always counts, as what asked
    /// waits for it.
    fn holds(&self, update: &Update) -> bool {
        match update {
            Update::Read(
                key @ ("hosts" | "plugins" | "catalog" | "preferences" | "tasks" | "usage" | "ci"
                | "ci-head" | "shares" | "stash" | "update" | "runs" | "ports" | "trust"
                | "drafts"),
                value,
            ) => self.reads.get(key) == Some(value),
            Update::HostView(host, view) => self.host_views.get(host) == Some(&**view),
            _ => false,
        }
    }

    /// The clock and every "ago" move each minute, which draws the whole window again. What
    /// counts seconds, such as a running tool call or the Run chip, counts them itself.
    fn until_next_minute() -> Duration {
        let now = chrono::Local::now();
        let into_second = Duration::from_nanos(u64::from(now.timestamp_subsec_nanos()));
        Duration::from_secs(60 - u64::from(now.second())) - into_second
    }

    /// The run of the project in front's tasks the person started that is going, and the last.
    pub fn person_runs(&self) -> (Option<&Value>, Option<&Value>) {
        let Some(path) = self.watched_project.as_deref() else {
            return (None, None);
        };
        let mine: Vec<&Value> = self
            .read("runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|run| run["project"] == path && run["agentId"].is_null())
            .collect();
        let active = |run: &Value| {
            matches!(
                run["status"].as_str(),
                Some("awaiting-trust" | "starting" | "running" | "stopping")
            )
        };
        (
            mine.iter().rev().find(|run| active(run)).copied(),
            mine.last().copied(),
        )
    }

    /// Counts the seconds on the Run chip while the person's task runs with no port to show.
    fn time_run(&mut self, cx: &mut Context<Self>) {
        let listening = |run: &Value| {
            self.read("ports")
                .and_then(Value::as_array)
                .is_some_and(|ports| {
                    ports
                        .iter()
                        .any(|port| port["owner"]["executionId"] == run["executionId"])
                })
        };
        let run = self.person_runs().0;
        let counting = run.is_some_and(|run| !listening(run));
        let started_at = run.and_then(|run| run["startedAt"].as_i64());
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

    /// Opens the Add a task dialog for the project whose tasks the Run menu lists.
    pub fn add_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.watched_project.clone() {
            self.adding_task = Some(crate::add_task::AddTask::new(project, window, cx));
            cx.notify();
        }
    }

    /// Opens the task dialog on the person's own `task` of `project`, to change it.
    pub fn edit_task(
        &mut self,
        project: &str,
        task: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adding_task = Some(crate::add_task::AddTask::edit(
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
        self.read("preferences")
            .is_some_and(|preferences| preferences["railDensity"] == "compact")
    }

    pub fn window_handle(&self) -> AnyWindowHandle {
        self.window
    }

    pub fn watched_project(&self) -> Option<&str> {
        self.watched_project.as_deref()
    }

    pub fn stop_run(&self, execution: &str) {
        self.call_in_front("taskStop", json!({ "executionId": execution }));
    }

    pub fn picture(&self) -> Option<Arc<Image>> {
        self.picture.clone()
    }

    pub fn preferences(&self) -> Preferences {
        self.preferences.clone()
    }

    pub fn read(&self, key: &str) -> Option<&Value> {
        self.reads.get(key)
    }

    /// Forgets what was read under `key`, so nothing shows the answer of what it watched before.
    pub(crate) fn forget_read(&mut self, key: &str) {
        self.reads.remove(key);
    }

    /// Gives the workbench the keyboard if `held`, which is going away, has it.
    fn release_keys(&self, held: FocusHandle, cx: &mut Context<Self>) {
        let (window, focus) = (self.window, self.focus.clone());
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                if held.is_focused(window) {
                    window.focus(&focus, cx)
                }
            });
        });
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
        let _ = self.asks.send(Ask::Call { method, params });
    }

    /// The view of the host in front: another host's own, while one on the account is chosen,
    /// else this host's.
    pub fn shown(&self) -> Option<&DeviceView> {
        match self.view.as_ref()?.layout.chosen_host.as_ref() {
            Some(key) => self.host_views.get(key),
            None => self.view.as_ref(),
        }
    }

    /// One of the core's methods on the host in front, for what the person did there.
    pub fn call_in_front(&self, method: &'static str, params: Value) {
        let call = Ask::Call { method, params };
        let _ = self.asks.send(match self.place() {
            crate::hosts::Place::Account(host) => Ask::On(host, Box::new(call)),
            crate::hosts::Place::This => call,
        });
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
        self.read("catalog")
            .and_then(Value::as_array)
            .and_then(|kinds| kinds.iter().find(|kind| kind["type"] == provider))
            .is_none_or(|kind| !kind["chat"].is_null())
    }

    /// What a new chat started from the rail starts with.
    pub fn launcher(&self) -> Option<&LauncherInfo> {
        let launchers = self.launchers();
        let chosen = |id: &str| launchers.iter().find(|launcher| launcher.id == id);
        let of_kind = |kind: &str| self.launcher_of_kind(kind);
        let last_kind = self.preferences.last_agent_type.as_deref();
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

    /// The agent's name in the core's catalog, such as "Claude".
    pub fn agent_label(&self, provider: &str) -> Option<&str> {
        self.read("catalog")?
            .as_array()?
            .iter()
            .find(|kind| kind["type"] == provider)?["label"]
            .as_str()
    }

    /// The launcher's account's plan usage as the core last read it, or why it has none.
    pub fn usage_of(&self, launcher: &str) -> Option<&Value> {
        self.read("usage")?
            .as_array()?
            .iter()
            .find(|account| account["launcher"] == launcher)
    }

    /// The most of any window of its plan the launcher's account has used.
    pub fn usage_peak(&self, launcher: &str) -> Option<f64> {
        let windows = self.usage_of(launcher)?["usage"]["windows"].as_array()?;
        windows
            .iter()
            .filter_map(|window| window["usedPercent"].as_f64())
            .map(|used| used.clamp(0., 100.))
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
            Edge::Start => &mut self.side_rail_open,
            Edge::End => &mut self.tab_rail_open,
        };
        *open = !*open;
        self.peeking = None;
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
        let Some(title) = self.view.as_ref().and_then(|view| {
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
        self.retitling = Some(Retitling {
            agent: agent.to_owned(),
            in_rail,
            input,
            title,
            _events: events,
        });
        self.parts.redraw(&self.parts.stage, cx);
        self.parts.redraw(&self.parts.rail, cx);
    }

    pub fn finish_retitle(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(retitling) = self.retitling.take() else {
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
        self.parts.redraw(&self.parts.stage, cx);
        self.parts.redraw(&self.parts.rail, cx);
    }

    /// Asks the person whether something goes ahead; `then` hears yes or no.
    pub fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(&mut Workbench, bool, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.confirm_or(
            title,
            body,
            yes,
            None,
            destructive,
            move |bench, choice, window, cx| then(bench, choice == Choice::Yes, window, cx),
            cx,
        );
    }

    /// Asks as [`Self::confirm`] does, with a second way to go ahead named `other`.
    #[allow(clippy::too_many_arguments)]
    pub fn confirm_or(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        other: Option<SharedString>,
        destructive: bool,
        then: impl Fn(&mut Workbench, Choice, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            other,
            destructive,
            then: Rc::new(then),
        });
        cx.notify();
    }

    /// Asks the person for a line of text, starting from `initial`; `then` hears what they
    /// typed, and nothing is done when they give up.
    pub fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        initial: impl Into<String>,
        then: impl Fn(&mut Workbench, String, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Prompt {
            title: title.into(),
            initial: initial.into(),
            field: None,
            then: Rc::new(then),
        });
        cx.notify();
    }

    pub(crate) fn show_review(
        &mut self,
        review: Entity<crate::agent_review::AgentReview>,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Review(review));
        cx.notify();
    }

    pub(crate) fn close_review(&mut self, cx: &mut Context<Self>) {
        if matches!(self.asking, Some(Asking::Review(_))) {
            self.take_asking(cx);
            cx.notify();
        }
    }

    /// Takes the question being asked away, however it was answered; a trust question that
    /// came in behind it is asked next.
    fn take_asking(&mut self, cx: &mut Context<Self>) -> Option<Asking> {
        let bench = cx.entity();
        cx.defer(move |cx| bench.update(cx, |bench, cx| bench.ask_trust(cx)));
        self.asking.take()
    }

    /// The question being asked, as a dialog, making a prompt's field the first time.
    fn asked(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bench = cx.entity();
        let answered = move |choice: Choice| {
            let bench = bench.clone();
            move |window: &mut Window, cx: &mut App| {
                bench.update(cx, |bench, cx| {
                    if let Some(Asking::Confirm { then, .. }) = bench.take_asking(cx) {
                        then(bench, choice, window, cx);
                    }
                    cx.notify();
                })
            }
        };
        Some(match self.asking.as_mut()? {
            Asking::Review(review) => {
                let t = Tokens::get(cx).clone();
                let leaving = cx.entity();
                div()
                    .id("agent-review-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::black().opacity(0.4))
                    .occlude()
                    .on_click(move |_, _, cx| {
                        leaving.update(cx, |bench, cx| bench.close_review(cx));
                    })
                    .child(
                        div()
                            .id("agent-review")
                            .occlude()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .bg(t.surface_overlay)
                            .border_1()
                            .border_color(t.border)
                            .rounded(crate::theme::RADIUS_3)
                            .shadow(t.shadow_float())
                            .child(review.clone()),
                    )
                    .into_any_element()
            }
            Asking::Confirm {
                title,
                body,
                yes,
                other,
                destructive,
                ..
            } => crate::ui::Confirm::new(title.clone(), body.clone(), yes.clone())
                .marked()
                .destructive(*destructive)
                .on_yes(answered(Choice::Yes))
                .on_no(answered(Choice::No))
                .when_some(other.clone(), |confirm, other| {
                    confirm.other(other, answered(Choice::Other))
                })
                .into_any_element(),
            Asking::Prompt {
                title,
                initial,
                field,
                ..
            } => {
                let field = field
                    .get_or_insert_with(|| {
                        let initial = initial.clone();
                        cx.new(|cx| {
                            let mut state = InputState::new(window, cx);
                            state.set_value(initial, window, cx);
                            state.focus(window, cx);
                            state.select_all(window, cx);
                            state
                        })
                    })
                    .clone();
                let (typing, giving_up) = (cx.entity(), cx.entity());
                crate::ui::Prompt::new(title.clone(), &field)
                    .marked()
                    .on_yes(move |typed, window, cx| {
                        typing.update(cx, |bench, cx| {
                            if let Some(Asking::Prompt { then, .. }) = bench.take_asking(cx) {
                                then(bench, typed.trim().to_owned(), window, cx);
                            }
                            cx.notify();
                        })
                    })
                    .on_no(move |_, cx| {
                        giving_up.update(cx, |bench, cx| {
                            bench.take_asking(cx);
                            cx.notify();
                        })
                    })
                    .into_any_element()
            }
        })
    }

    /// Puts the core's next trust question to the person, each once, while nothing else is asked.
    fn ask_trust(&mut self, cx: &mut Context<Self>) {
        let listed = |key: &str| {
            self.read(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        // This host's questions, then the chosen account host's, each under the host that asks.
        let asks: Vec<(Option<String>, Value)> = listed("trust")
            .into_iter()
            .map(|ask| (None, ask))
            .chain(
                listed("host-trust")
                    .into_iter()
                    .map(|ask| (self.trust_host.clone(), ask)),
            )
            .collect();
        let asked_as = |host: &Option<String>, ask: &Value| {
            format!(
                "{}:{}",
                host.as_deref().unwrap_or_default(),
                ask["id"].as_str().unwrap_or_default()
            )
        };
        self.trust_asked
            .retain(|asked| asks.iter().any(|(host, ask)| &asked_as(host, ask) == asked));
        if self.asking.is_some() {
            return;
        }
        let Some((host, ask)) = asks.into_iter().find(|(host, ask)| {
            ask["id"].is_string() && !self.trust_asked.contains(&asked_as(host, ask))
        }) else {
            return;
        };
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let id = text(&ask["id"]);
        self.trust_asked.insert(asked_as(&host, &ask));
        let on = host.as_ref().and_then(|host| {
            self.host_choices()
                .into_iter()
                .find(|option| option.choice == crate::hosts::HostChoice::Account(host.clone()))
                .map(|option| option.label)
        });
        let commands: Vec<String> = ask["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|command| format!("{}: {}", text(&command["label"]), text(&command["command"])))
            .collect();
        let body: Vec<String> = [text(&ask["body"]), commands.join("\n")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        #[cfg(feature = "drive")]
        crate::drive::trace(format_args!("trust question set"));
        let title = match on {
            Some(name) => format!("{} on {name}", text(&ask["title"])),
            None => text(&ask["title"]),
        };
        self.confirm(
            title,
            body.join("\n\n"),
            text(&ask["confirmLabel"]),
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

    /// The terminal an agent runs in, while it runs in one rather than as a chat.
    pub fn agent_terminal(&self, agent: &str) -> Option<SessionId> {
        self.kept_agent(agent)?.pty_id
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

    fn kept_agent(&self, agent: &str) -> Option<&sikemux_wire::protocol::WorkspaceAgent> {
        self.view
            .as_ref()?
            .workspace
            .agents
            .iter()
            .find(|kept| kept.id == agent)
    }

    /// Moves an agent between its chat and its own CLI in a terminal, carrying what was
    /// typed and not sent.
    pub fn switch_view(&self, agent: &str, terminal: bool) {
        let to = if terminal { "terminal" } else { "chat" };
        self.call("switchAgentView", json!({ "agentId": agent, "to": to }));
    }

    pub fn rail_width(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Start => self.side_rail_width,
            Edge::End => self.tab_rail_width,
        }
    }

    /// Sets a rail's width, within what that rail allows, and draws it at once.
    fn resize_rail(&mut self, edge: Edge, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        let (kept, range) = match edge {
            Edge::Start => (&mut self.side_rail_width, rail::SIDE_RAIL),
            Edge::End => (&mut self.tab_rail_width, agents::TAB_RAIL),
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
            .track_focus(&self.resizers[edge as usize])
            .tooltip(crate::ui::tooltip("Drag or use arrow keys to resize"))
            .on_click(cx.listener(move |bench, _, window, cx| {
                bench.resizers[edge as usize].focus(window, cx)
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
    fn follow_rails(&mut self, preferences: &Preferences) {
        self.side_rail_open = preferences.side_rail_open;
        self.tab_rail_open = preferences.tab_rail_open;
        let width =
            |width: f64, range: RailWidth| (width as f32).clamp(range.min, range.max).round();
        self.side_rail_width = width(preferences.side_rail_width, rail::SIDE_RAIL);
        self.tab_rail_width = width(preferences.tab_rail_width, agents::TAB_RAIL);
        self.rail_page = match preferences.tab_rail_page {
            TabRailPage::Agents => RailPage::Agents,
            TabRailPage::Terminals => RailPage::Terminals,
            TabRailPage::Files => RailPage::Files,
        };
        self.all_projects = preferences.tab_rail_scope == TabRailScope::All;
        self.all_harnesses = preferences.tab_rail_all_agents;
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
        if self.read("preferences").is_none() {
            return;
        }
        self.keeping_rails = Some(cx.spawn(async move |bench, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = bench.update(cx, |bench, _| {
                bench.keeping_rails = None;
                bench.write_rails();
            });
        }));
    }

    fn write_rails(&self) {
        let page = match self.rail_page {
            RailPage::Agents => TabRailPage::Agents,
            RailPage::Terminals => TabRailPage::Terminals,
            RailPage::Files => TabRailPage::Files,
        };
        let scope = if self.all_projects {
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
            ("sideRailOpen", json!(self.side_rail_open)),
            ("sideRailWidth", json!(self.side_rail_width)),
            ("tabRailOpen", json!(self.tab_rail_open)),
            ("tabRailWidth", json!(self.tab_rail_width)),
            ("tabRailPage", json!(page)),
            ("tabRailScope", json!(scope)),
            ("tabRailAllAgents", json!(self.all_harnesses)),
            ("recentCommandKeys", json!(recent)),
        ];
        let kept = self.read("preferences");
        for (key, value) in now {
            if kept.and_then(|kept| kept.get(key)) != Some(&value) {
                crate::settings::set(self, key, value);
            }
        }
    }

    /// Opens both rails when either is closed, else closes both.
    pub fn toggle_focus_mode(&mut self, cx: &mut Context<Self>) {
        let open = !(self.side_rail_open && self.tab_rail_open);
        self.side_rail_open = open;
        self.tab_rail_open = open;
        self.peeking = None;
        self.keep_rails(cx);
        cx.notify();
    }

    /// Shows a closed rail over the stage while the pointer is at its edge or over it, and puts
    /// it away a moment after the pointer leaves.
    fn peek(&mut self, edge: Edge, inside: bool, cx: &mut Context<Self>) {
        if inside {
            self.unpeek = None;
            if self.peeking != Some(edge) {
                self.peeking = Some(edge);
                cx.notify();
            }
            return;
        }
        self.unpeek = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PEEK_CLOSE).await;
            let _ = this.update(cx, |bench, cx| {
                if bench.peeking == Some(edge) {
                    bench.peeking = None;
                    cx.notify();
                }
            });
        }));
    }

    /// The space the rail shows, while it still exists.
    pub fn space(&self) -> Option<&SpaceInfo> {
        let spaces = &self.shown()?.workspace.spaces;
        spaces
            .iter()
            .find(|space| Some(&space.id) == self.space.as_ref())
    }

    /// Asks for a name and makes a space with it, as the Mac does, then shows it, or moves the
    /// project `into` it when one asked.
    pub fn new_space(&mut self, into: Option<String>, cx: &mut Context<Self>) {
        self.prompt(
            "Create space",
            "",
            move |bench, name, _, cx| {
                let name: String = name.trim().chars().take(MAX_SPACE_NAME).collect();
                if name.is_empty() {
                    return;
                }
                let made = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos());
                let id = format!("space-{made:x}");
                let mut spaces = bench
                    .view
                    .as_ref()
                    .map_or_else(Vec::new, |view| view.workspace.spaces.clone());
                spaces.push(SpaceInfo {
                    id: id.clone(),
                    name,
                    icon: "folder".into(),
                });
                bench.call("setSpaces", json!({ "spaces": spaces }));
                match &into {
                    Some(project) => bench.call(
                        "updateProject",
                        json!({ "id": project, "patch": { "space": id } }),
                    ),
                    None => bench.show_space(Some(id), cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    /// Sets this host's spaces to what `edit` makes of them.
    pub(crate) fn edit_spaces(&self, edit: impl FnOnce(&mut Vec<SpaceInfo>)) {
        let mut spaces = self
            .view
            .as_ref()
            .map_or_else(Vec::new, |view| view.workspace.spaces.clone());
        edit(&mut spaces);
        self.call("setSpaces", json!({ "spaces": spaces }));
    }

    /// Asks for the space's new name, as the Mac does.
    pub fn rename_space(&mut self, id: String, cx: &mut Context<Self>) {
        let current = self
            .view
            .as_ref()
            .and_then(|view| view.workspace.spaces.iter().find(|space| space.id == id))
            .map(|space| space.name.clone())
            .unwrap_or_default();
        self.prompt(
            "Rename space",
            current,
            move |bench, name, _, _| bench.name_space(&id, &name),
            cx,
        );
        cx.notify();
    }

    /// Marks the space with `icon`, or with none when it is empty.
    pub fn set_space_icon(&self, id: &str, icon: &str) {
        self.edit_spaces(|spaces| {
            if let Some(space) = spaces.iter_mut().find(|space| space.id == id) {
                space.icon = icon.to_owned();
            }
        });
    }

    /// Deletes the space once the person agrees; its projects stay open and show under All.
    pub fn delete_space(&mut self, id: String, cx: &mut Context<Self>) {
        let name = self
            .view
            .as_ref()
            .and_then(|view| view.workspace.spaces.iter().find(|space| space.id == id))
            .map(|space| space.name.clone())
            .unwrap_or_default();
        self.confirm(
            format!("Delete “{name}”?"),
            "Its projects stay open and show under All.",
            "Delete",
            true,
            move |bench, yes, _, cx| {
                if !yes {
                    return;
                }
                bench.edit_spaces(|spaces| spaces.retain(|space| space.id != id));
                let held: Vec<String> = bench
                    .view
                    .as_ref()
                    .map(|view| {
                        view.workspace
                            .projects
                            .iter()
                            .filter(|project| project.space.as_ref() == Some(&id))
                            .map(|project| project.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for project in held {
                    bench.call(
                        "updateProject",
                        json!({ "id": project, "patch": { "space": null } }),
                    );
                }
                if bench.space.as_ref() == Some(&id) {
                    bench.show_space(None, cx);
                }
            },
            cx,
        );
    }

    /// Shows one space's projects, or every project, and leaves a project the space hides
    /// for the first one it shows.
    pub fn show_space(&mut self, space: Option<String>, cx: &mut Context<Self>) {
        self.space = space;
        let shown = |project: &ProjectInfo| {
            self.space.is_none() || project.space.as_ref() == self.space.as_ref()
        };
        if !self.project().is_some_and(shown) {
            if let Some(first) = self.projects().iter().find(|project| shown(project)) {
                self.activate_project(&first.id);
            }
        }
        cx.notify();
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

    /// Gives the keyboard to `wants` once it shows. Only what the person opens asks for this,
    /// never what an agent opens beside them.
    /// The stage draws again at once, so a view already in front takes the keys too.
    pub fn take_keys_when_shown(&self, wants: TakesKeys, cx: &mut App) {
        let from = self.tab().map(|tab| tab.id.clone());
        self.takes_keys.set(Some(PendingKeys { wants, from }));
        self.parts.redraw(&self.parts.stage, cx);
    }

    /// Whether `shown`, now drawn, is what waits for the keyboard. It stops waiting once claimed.
    pub fn claims_keys(&self, shown: TakesKeys) -> bool {
        match self.takes_keys.take() {
            Some(pending) if pending.wants == shown => true,
            other => {
                self.takes_keys.set(other);
                false
            }
        }
    }

    /// What holds the keyboard, as the last frame drawn tells.
    fn keys(&self, window: &Window, cx: &App) -> Keys {
        let Some(held) = window.focused(cx) else {
            return Keys::Loose;
        };
        if held == self.focus {
            return Keys::Loose;
        }
        let stack = window.context_stack();
        match stack.last() {
            None => Keys::Gone,
            Some(own) if stack.len() == 1 || own.contains(crate::keymap::SHELL) => Keys::Loose,
            Some(_) => Keys::Held,
        }
    }

    /// Gives the keyboard to the view in front, its terminal or its chat's draft, when nothing
    /// with keys of its own holds it: nothing, a view no longer drawn, the workbench itself or a
    /// button of its chrome. The workbench keeps it over Settings, a palette or a question, and
    /// while a view waits to take it.
    pub(crate) fn settle_keys(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keys(window, cx) == Keys::Held {
            return;
        }
        let held = window.focused(cx);
        let kept = self.settings.is_some()
            || self.experience.is_some()
            || self.palette.is_some()
            || self.finder.is_some()
            || self.switcher.is_some()
            || self.front_asks().is_some()
            || self.waits_for_keys();
        if kept {
            // What holds them may be a field not drawn yet, which takes them once it is.
            if held.is_none() {
                self.focus.focus(window, cx);
            }
            return;
        }
        if let Some(view) = self.focused_view() {
            self.focus_view(&view, window, cx);
        }
        if window.focused(cx) == held {
            self.focus.focus(window, cx);
        }
    }

    /// Presses a key again on the workbench when nothing in it held the keyboard, such as a view
    /// no longer drawn, so the window's shortcuts work wherever the focus was.
    fn catch_loose_keys(
        &mut self,
        event: &gpui_kit::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Below the window's own root, anything in focus has a context of its own.
        if event.context_stack.len() > 1 {
            return false;
        }
        self.focus.focus(window, cx);
        if !self.focus.is_focused(window) {
            return false;
        }
        cx.stop_propagation();
        let keystroke = event.keystroke.clone();
        window.defer(cx, move |window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        });
        true
    }

    fn waits_for_keys(&self) -> bool {
        let pending = self.takes_keys.take();
        let waits = pending.is_some();
        self.takes_keys.set(pending);
        waits
    }

    /// Stops waiting to give the keyboard away once another kind of tab comes to the front,
    /// so what failed to open never takes the keys later.
    fn settle_takes_keys(&self) {
        let Some(pending) = self.takes_keys.take() else {
            return;
        };
        let front = self.tab();
        let moved = front.map(|tab| &tab.id) != pending.from.as_ref();
        let shows = front
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| pending.wants.shows_in(kind));
        if !moved || shows {
            self.takes_keys.set(Some(pending));
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

    fn git_in_front(&self) -> bool {
        self.tab()
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| kind == PaneKind::Git)
    }

    /// Makes the Git pane when the project's Git tab comes to the front.
    fn git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tell_reach(cx);
        let front = self.tab().and_then(|tab| tabs::first_view(&tab.pane));
        let (Some((PaneKind::Git, _)), Some(project)) = (front, self.project()) else {
            return;
        };
        let Some(repo) = self.git_repo() else {
            return;
        };
        if project.path.to_string_lossy() != *self.watched_project.as_deref().unwrap_or_default() {
            return;
        }
        let target = self.target();
        if let Some(pane) = self.git.as_ref().filter(|pane| {
            let pane = pane.read(cx);
            pane.repo == repo && pane.place() == target.place()
        }) {
            if std::mem::take(&mut self.git_behind) {
                let overview = self.reads.get("git-overview");
                pane.update(cx, |pane, cx| {
                    if let Some(overview) = overview {
                        pane.overview(overview, cx);
                    }
                    pane.git_moved(None, None, cx);
                });
            }
            return;
        }
        self.git_behind = false;
        let bench = cx.entity().downgrade();
        let away = self.out_of_reach();
        let (host, signed_in) = (self.code_host_of(&repo), self.host_signed_in(cx));
        let pane = cx.new(|cx| GitPane::new(target, repo, bench, window, cx));
        pane.update(cx, |pane, cx| {
            pane.set_out_of_reach(away, cx);
            pane.set_code_host(host, signed_in, cx);
        });
        if let Some(overview) = self.reads.get("git-overview") {
            pane.update(cx, |pane, cx| pane.overview(overview, cx));
        }
        if let Some(catalog) = self.reads.get("catalog") {
            pane.update(cx, |pane, cx| pane.catalog(catalog, cx));
        }
        if let Some(drafts) = self.reads.get("git-draft") {
            pane.update(cx, |pane, cx| pane.drafts(drafts, cx));
        }
        self.git = Some(pane);
    }

    /// Makes the Search pane when the project's Search tab comes to the front.
    fn search_pane(&mut self, cx: &mut Context<Self>) {
        let front = self.tab().and_then(|tab| tabs::first_view(&tab.pane));
        let (Some((PaneKind::Search, _)), Some(project)) = (front, self.project()) else {
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

    /// Keeps a pane for each plugin view of the strip in front, so a plugin's tab keeps what
    /// was picked in it while another tab is in front, and lets go of the rest.
    fn plugin_panes(&mut self, cx: &mut Context<Self>) {
        fn plugin_views(pane: &Pane, into: &mut Vec<(String, String)>) {
            match pane {
                Pane::View {
                    id,
                    kind: PaneKind::Plugin,
                    target,
                } => into.push((id.clone(), target.clone())),
                Pane::View { .. } => {}
                Pane::Split { panes, .. } => panes.iter().for_each(|pane| plugin_views(pane, into)),
            }
        }
        let mut shown = Vec::new();
        if let Some(strip) = self.strip() {
            for tab in self.tabs(&strip) {
                plugin_views(&tab.pane, &mut shown);
            }
        }
        let mut in_front = Vec::new();
        if let Some(tab) = self.tab() {
            plugin_views(&tab.pane, &mut in_front);
        }
        self.plugin_panes.retain(|id, pane| {
            let kept = shown.iter().any(|(kept, _)| kept == id);
            match kept && in_front.iter().any(|(front, _)| front == id) {
                true => pane.update(cx, |pane, cx| pane.show(cx)),
                false => pane.update(cx, |pane, _| pane.hide()),
            }
            kept
        });
        for (id, target) in shown {
            if self.plugin_panes.contains_key(&id) {
                continue;
            }
            let (core, bench) = (self.target(), cx.entity().downgrade());
            let pane =
                cx.new(|_| crate::plugin_pane::PluginPane::new(&target, id.clone(), core, bench));
            let plugin = crate::plugin_pane::parts(&target).0.to_owned();
            if let Some(at) = self.plugin_opening.remove(&plugin) {
                pane.update(cx, |pane, cx| pane.open_at(&at, cx));
            }
            self.plugin_panes.insert(id, pane);
        }
        self.plugin_bars(cx);
    }

    /// Makes the top bar's item of each enabled plugin that draws one, here or on the host chosen.
    fn plugin_bars(&mut self, cx: &mut Context<Self>) {
        let enabled: Vec<String> = self
            .read("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|plugin| plugin["enabled"].as_bool() == Some(true))
            .filter_map(|plugin| plugin["id"].as_str().map(str::to_owned))
            .filter(|id| crate::plugin_pane::has_top_bar(id))
            .collect();
        let core = self.target();
        self.plugin_bars
            .retain(|id, bar| enabled.contains(id) && bar.read(cx).host.place() == core.place());
        for id in enabled {
            if self.plugin_bars.contains_key(&id) {
                continue;
            }
            let target = format!("{id}:{}", crate::plugin_pane::TOP_BAR);
            let (core, bench) = (self.target(), cx.entity().downgrade());
            let bar = cx.new(|_| {
                crate::plugin_pane::PluginPane::new(&target, "topbar".into(), core, bench)
            });
            self.plugin_bars.insert(id, bar);
        }
    }

    /// Opens `plugin`'s pane at the place `at` names and brings its tab to the front: the pane this
    /// strip already holds, else a new tab.
    pub fn open_plugin_at(&mut self, plugin: &str, at: String, cx: &mut Context<Self>) {
        let (Some(surface), Some(strip)) = (crate::plugin_pane::surface(plugin), self.strip())
        else {
            return;
        };
        let open = self
            .plugin_panes
            .values()
            .find(|pane| pane.read(cx).host.plugin == plugin)
            .cloned();
        match open {
            Some(pane) => pane.update(cx, |pane, cx| pane.open_at(&at, cx)),
            None => {
                self.plugin_opening.insert(plugin.to_owned(), at);
            }
        }
        let target = format!("{plugin}:{surface}");
        let params = json!({ "kind": PaneKind::Plugin, "target": target });
        self.call_in_front("openTab", strip.with(params));
    }

    /// Opens a plugin's palette, its rows read from the plugin's view.
    pub fn open_plugin_palette(
        &mut self,
        palette: &'static crate::plugin_pane::PluginPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = crate::palette::Spec::new(
            palette.id,
            palette.placeholder,
            Vec::new(),
            move |row, bench, _, cx| bench.open_plugin_at(palette.plugin, row.key.to_string(), cx),
        )
        .empty("reading…");
        let spec = match palette.typed {
            Some(typed) => spec.typed(typed),
            None => spec,
        };
        self.toggle_palette(spec, window, cx);
        self.read_plugin_palette(palette, cx);
    }

    fn read_plugin_palette(
        &mut self,
        palette: &'static crate::plugin_pane::PluginPalette,
        cx: &mut Context<Self>,
    ) {
        let settings = self
            .preferences()
            .plugin_settings
            .get(palette.plugin)
            .cloned()
            .unwrap_or(Value::Null);
        let params = palette
            .params
            .map_or_else(|| json!({}), |params| params(&settings));
        let asked = self.target().request(
            "pluginView",
            json!({
                "plugin": palette.plugin,
                "method": palette.view,
                "params": params,
                "slot": format!("{}/palette", palette.plugin),
            }),
        );
        cx.spawn(async move |this, cx| {
            let read = match asked.await {
                Ok(answer) => match answer.get("failed") {
                    Some(failed) => Err(crate::plugin_pane::Failure::of(failed)),
                    None => Ok((palette.rows)(&answer["value"])),
                },
                Err(error) => Err(crate::plugin_pane::Failure::core(error)),
            };
            let _ = this.update(cx, |bench, cx| {
                if let Some(open) = bench
                    .palette
                    .as_mut()
                    .filter(|open| open.id() == palette.id)
                {
                    match read {
                        Ok(rows) => open.refill(rows, cx),
                        Err(failure) => open.failed(
                            failure,
                            move |bench, _, cx| {
                                if let Some(open) = bench.palette.as_mut() {
                                    open.refill(Vec::new(), cx);
                                }
                                bench.parts.redraw(&bench.parts.finder, cx);
                                bench.read_plugin_palette(palette, cx);
                            },
                            cx,
                        ),
                    }
                    bench.parts.redraw(&bench.parts.finder, cx);
                }
            });
        })
        .detach();
    }

    /// Tells the plugin panes on `place` that some of their plugins' views changed.
    fn plugin_views_changed(
        &self,
        place: &Place,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        let desks = self.desk_plugin_panes();
        for pane in (self.plugin_panes.values())
            .chain(self.plugin_bars.values())
            .chain(&desks)
        {
            pane.update(cx, |pane, cx| {
                if pane.host.place() == place {
                    pane.changed(paths, cx);
                }
            });
        }
    }

    /// Makes the board pane when the project's board tab comes to the front.
    fn board_pane(&mut self, cx: &mut Context<Self>) {
        let front = self.tab().and_then(|tab| tabs::first_view(&tab.pane));
        let (Some((PaneKind::Board, _)), Some(project)) = (front, self.project()) else {
            return;
        };
        let folder = project.path.to_string_lossy().into_owned();
        if self
            .board
            .as_ref()
            .is_some_and(|pane| pane.read(cx).folder == folder)
        {
            return;
        }
        let (asks, bench) = (self.asks.clone(), cx.entity().downgrade());
        self.board = Some(cx.new(|cx| crate::board::BoardPane::new(asks, folder, bench, cx)));
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
        let path = project.path.to_string_lossy().into_owned();
        let seed = self.selection(window, cx);
        let target = self.target();
        let search = match self.search.as_ref().filter(|search| {
            let search = search.read(cx);
            search.project == path && search.place() == target.place()
        }) {
            Some(search) => search.clone(),
            None => {
                let bench = cx.weak_entity();
                let search = cx.new(|cx| SearchPane::new(target, path, bench, cx));
                self.search = Some(search.clone());
                search
            }
        };
        search.update(cx, |search, cx| search.focus_find(seed, window, cx));
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
                .values()
                .find_map(|chat| chat.transcript.read(cx).focused_selection(window))
                .or_else(|| Some(gpui_kit::base::TextSelection::selected_text(window, cx))),
        }?;
        selected
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::to_owned)
    }

    /// Makes the code host's areas when the Git tab in front shows one, and keeps them
    /// on the project's repository and branch.
    fn host_area(&mut self, cx: &mut Context<Self>) {
        let front = self.tab().and_then(|tab| tabs::first_view(&tab.pane));
        let (Some((PaneKind::Git, _)), Some(project)) = (front, self.project()) else {
            if let Some(shown) = &self.host {
                shown.update(cx, |shown, _| shown.hide());
            }
            return;
        };
        let Some(host) = project.code_host.clone() else {
            if self.area != Area::Local {
                self.area = Area::Local;
                self.parts.redraw(&self.parts.rail, cx);
            }
            return;
        };
        let (folder, branch) = (
            project.path.to_string_lossy().into_owned(),
            crate::codehost::branch_of(project),
        );
        let area = self.area;
        let account = project
            .code_hosts
            .get(&host.host)
            .and_then(|choice| choice.account.clone());
        let opening = self.opening.take();
        match &self.host {
            Some(shown)
                if shown.read(cx).project == folder && shown.read(cx).place() == &self.place() =>
            {
                shown.update(cx, |shown, cx| {
                    shown.project_moved(&host, account, branch, cx);
                    shown.show(area, cx);
                    if let Some((area, number, repo)) = opening {
                        shown.open_item(area, number, repo, cx);
                    }
                });
            }
            _ => {
                if let Some(old) = &self.host {
                    old.update(cx, |old, _| old.hide());
                }
                let (target, bench) = (self.target(), cx.entity().downgrade());
                let away = self.out_of_reach();
                let shown = cx.new(|_| HostArea::new(target, bench, folder, host, account, branch));
                shown.update(cx, |shown, cx| {
                    shown.set_out_of_reach(away, cx);
                    shown.show(area, cx);
                    if let Some((area, number, repo)) = opening {
                        shown.open_item(area, number, repo, cx);
                    }
                });
                self.host = Some(shown);
            }
        }
    }

    /// Brings the project's Git tab to the front showing `area`.
    pub(crate) fn open_area(&mut self, area: Area, cx: &mut Context<Self>) {
        self.area = area;
        self.open_git(cx);
        cx.notify();
    }

    /// Brings the project's Git tab to the front with pull request or issue `number` of the
    /// host's repository `repo` open.
    pub fn open_host_item(
        &mut self,
        area: Area,
        number: u64,
        repo: String,
        cx: &mut Context<Self>,
    ) {
        self.opening = Some((area, number, repo));
        self.open_area(area, cx);
    }

    /// Opens what a web link in a page names in the app: a pull request or issue of one of
    /// the projects' repositories in that project's Git tab, anything else in the browser.
    pub(crate) fn follow_web_link(&mut self, href: &str, cx: &mut Context<Self>) {
        let asked = self
            .target()
            .request("codeHostLink", json!({ "url": href }));
        let href = href.to_owned();
        cx.spawn(async move |bench, cx| {
            let found = asked.await.ok();
            bench
                .update(cx, |bench, cx| {
                    let opened = found.is_some_and(|found| bench.open_linked_item(&found, cx));
                    if !opened {
                        cx.open_url(&href);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Brings forward the project whose repository `found` names, its Git tab showing the pull
    /// request or issue it names, and says whether one of the projects holds it.
    fn open_linked_item(&mut self, found: &Value, cx: &mut Context<Self>) -> bool {
        let Ok(host) = serde_json::from_value::<ProjectCodeHost>(found["codeHost"].clone()) else {
            return false;
        };
        let item = &found["item"];
        let area = match item["kind"].as_str() {
            Some("pull") => Area::Pulls,
            Some("issue") => Area::Issues,
            _ => return false,
        };
        let Some(number) = item["number"].as_u64() else {
            return false;
        };
        let holds = |project: &&ProjectInfo| {
            project
                .code_host
                .as_ref()
                .is_some_and(|held| held.host == host.host && held.repo == host.repo)
        };
        let Some(project) = self.projects().iter().find(holds).cloned() else {
            return false;
        };
        self.opening = Some((area, number, host.repo.clone()));
        self.area = area;
        self.activate_project(&project.id);
        self.call_in_front(
            "openTab",
            json!({ "project": project.id, "kind": "git", "target": project.path.to_string_lossy() }),
        );
        cx.notify();
        true
    }

    /// Brings the project's Git tab to the front, opening one when it has none.
    fn open_git(&mut self, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Git, cx);
        if let Some(project) = self.project() {
            self.call_in_front(
                "openTab",
                json!({ "project": project.id, "kind": "git", "target": project.path.to_string_lossy() }),
            );
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

    pub fn close_tab(&self, strip: &Strip, tab: &str) {
        self.close_tabs(strip, &[tab.to_owned()]);
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

    /// Brings an agent's tab to the front.
    /// Brings the tab of the agent `id` to the front, in its project.
    pub fn open_agent(&self, id: &str) {
        let project_on = |view: &DeviceView| {
            view.workspace
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .map(|agent| agent.project.clone())
        };
        let Some(project) = self.shown().and_then(project_on) else {
            let tab =
                |project: String| json!({ "project": project, "kind": "agent", "target": id });
            // Opening a tab on a host's layout brings that host forward there.
            if let Some(project) = self.view.as_ref().and_then(project_on) {
                return self.call("openTab", tab(project));
            }
            if let Some((host, project)) = self
                .host_views
                .iter()
                .find_map(|(host, view)| Some((host.clone(), project_on(view)?)))
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
        if let Some(tab) = self
            .tabs(&strip)
            .iter()
            .find(|tab| shows(tab, PaneKind::Agent).is_some_and(|shown| shown == id))
        {
            self.activate_project(&project);
            self.activate_tab(&strip, &tab.id);
        }
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
        self.view
            .as_ref()?
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
    /// The agent `id` as the rails draw it, in whichever project it is.
    /// Counts the stash as seen up to `at` here at once, ahead of the server, whose answer
    /// then agrees.
    pub fn seen_stash_at(&mut self, at: &str) {
        if let Some(stash) = self.reads.get_mut("stash") {
            stash["seenAt"] = Value::from(at);
        }
    }

    /// The agent whose chat last took the keys.
    pub fn focused_agent(&self) -> Option<&str> {
        self.focused_chat.as_ref().map(|(agent, _)| agent.as_str())
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
        let ranked: Vec<&str> = view.inbox["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|agent| agent["agentId"].as_str())
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
        let chat = view.chats.iter().find(|chat| chat.agent_id == agent.id);
        let session = view
            .sessions
            .iter()
            .find(|session| session.agent_id.as_deref() == Some(agent.id.as_str()));
        let shown = view.inbox["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["agentId"] == agent.id.as_str());
        AgentRow {
            active_at: shown.and_then(|row| row["activeAt"].as_u64()).unwrap_or(0),
            next_trigger: shown.and_then(|row| row["nextTrigger"].as_u64()),
            called_at: shown.and_then(|row| row["calledAt"].as_u64()),
            team: shown.and_then(|row| match row["team"].as_str()? {
                "head" => Some(TeamLink::Head),
                "member" => Some(TeamLink::Member),
                "last" => Some(TeamLink::Last),
                _ => None,
            }),
            id: agent.id.clone(),
            provider: agent.provider.clone(),
            title: self.title_of(agent),
            name: agent.name.clone().filter(|_| self.titled(agent)),
            status: chat
                .map(|chat| chat.status)
                .or_else(|| session.and_then(|session| session.status))
                .unwrap_or(if agent.asleep {
                    AgentStatus::Asleep
                } else {
                    AgentStatus::Idle
                }),
            subagents: chat.map_or(0, |chat| chat.subagents),
            tasks: chat.map_or(0, |chat| chat.tasks),
            project: agent.project.clone(),
            face: agent.face.clone(),
            group: chat
                .map(|chat| chat.group)
                .or_else(|| session.and_then(|session| session.group))
                .unwrap_or_default(),
        }
    }

    /// The rows the agent rail lists: this project's, or every project's.
    pub fn listed_agents(&self) -> Vec<AgentRow> {
        let mut rows = if self.all_projects {
            self.projects()
                .iter()
                .flat_map(|project| self.agents_of(&project.id))
                .collect()
        } else {
            self.project()
                .map(|project| self.agents_of(&project.id))
                .unwrap_or_default()
        };
        rows.retain(|row| row.title.to_lowercase().contains(&self.filter_text));
        rows
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
            .filter(|recent| self.all_harnesses || provider == Some(recent.provider.as_str()))
            .filter(|recent| recent.title.to_lowercase().contains(&self.filter_text))
            .collect()
    }

    pub fn sign_in(&mut self) {
        self.signing_in = true;
        let _ = self.asks.send(Ask::Answer {
            key: "sign-in",
            method: "accountSignIn",
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
            self._filtering = None;
            self.filter_text.clear();
            self.look_for_older(false, crate::recent::PAGE, cx);
            return;
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("filter recent chats…"));
        self._filtering = Some(
            cx.subscribe(&filter, |bench, filter, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    bench.filter_text = filter.read(cx).value().to_lowercase();
                    bench.look_for_older(false, crate::recent::PAGE, cx);
                    cx.notify();
                }
            }),
        );
        filter.update(cx, |state, cx| state.focus(window, cx));
        self.filter = Some(filter);
    }
}

/// The picture `accountPicture` answers with: its type, and its bytes in base64.
fn decode_picture(value: &Value) -> Option<Arc<Image>> {
    crate::picture::decode(value["mime"].as_str()?, value["data"].as_str()?)
}

/// Whether two tabs show the same kind of pane, as ⌃Tab steps between them.
fn same_kind(tab: &Tab, front: &Tab) -> bool {
    let kind = |tab: &Tab| tabs::first_view(&tab.pane).map(|(kind, _)| kind);
    kind(tab) == kind(front)
}

/// What the tab's first view of `kind` shows, among its splits.
pub fn shows(tab: &Tab, kind: PaneKind) -> Option<&String> {
    shown_in(&tab.pane, kind)
}

fn shown_in(pane: &Pane, kind: PaneKind) -> Option<&String> {
    match pane {
        Pane::View {
            kind: shown,
            target,
            ..
        } if *shown == kind => Some(target),
        Pane::View { .. } => None,
        Pane::Split { panes, .. } => panes.iter().find_map(|pane| shown_in(pane, kind)),
    }
}

/// One part of the window, drawn as a view of its own so that it draws again only when the
/// store changes everywhere or names this part.
pub struct Part {
    bench: WeakEntity<Workbench>,
    draw: fn(&mut Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
    column: bool,
    _store: Subscription,
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

/// A view that takes the keyboard once it shows, after the person opened it.
#[derive(Clone, PartialEq)]
pub enum TakesKeys {
    Terminal,
    Git,
    File(PathBuf),
    Board,
}

impl TakesKeys {
    fn shows_in(&self, kind: PaneKind) -> bool {
        match self {
            Self::Terminal => kind == PaneKind::Terminal,
            Self::Git => kind == PaneKind::Git,
            Self::File(_) => kind == PaneKind::Files,
            Self::Board => kind == PaneKind::Board,
        }
    }
}

#[derive(PartialEq)]
enum Keys {
    /// Nothing with keys of its own holds the keyboard.
    Loose,
    /// A view the last frame did not draw holds it: one gone, or one not drawn yet.
    Gone,
    Held,
}

/// A view waiting to take the keyboard, and the tab in front when it was asked for.
struct PendingKeys {
    wants: TakesKeys,
    from: Option<String>,
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
                    _store: cx.observe(&bench, |_, _, cx| cx.notify()),
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
                    bench.git_pane(window, cx);
                    bench.host_area(cx);
                    bench.search_pane(cx);
                    bench.board_pane(cx);
                    bench.plugin_panes(cx);
                    crate::desk::prepare(bench, window, cx);
                    bench.settle_takes_keys();
                    // The frame being drawn is not done, so a view not drawn yet may still show in it.
                    if bench.keys(window, cx) == Keys::Loose {
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
        // What is over the window can show the stage's state too, such as an install's terminal.
        if part == &self.stage {
            self.over.update(cx, |_, cx| cx.notify());
        }
    }
}

impl Workbench {
    /// A docked rail and its handle, `tuck` of the way under the stage. It keeps its width as
    /// it goes, so nothing in it wraps again; the stage takes the room it gives up.
    fn docked_rail(&self, edge: Edge, tuck: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let (part, width) = match edge {
            Edge::Start => (&self.parts.rail, self.side_rail_width),
            Edge::End => (&self.parts.tabs, self.tab_rail_width),
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
            Edge::Start => (&self.parts.rail, self.side_rail_width),
            Edge::End => (&self.parts.tabs, self.tab_rail_width),
        };
        let peeking = self.peeking == Some(edge);
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
            .px(px(10.))
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
        use crate::keymap::*;
        use crate::panes::Towards;
        use sikemux_wire::protocol::SplitDir;
        let asked = self.asked(window, cx);
        #[cfg(feature = "drive")]
        if asked.is_some() {
            crate::drive::trace(format_args!("question drawn"));
        }
        let asking = self.asking_freely(window);
        let this = cx.weak_entity();
        crate::see_through::follow(window, cx);
        let [side_tuck, tab_tuck] = [
            ("side-rail-tuck", self.side_rail_open),
            ("tab-rail-tuck", self.tab_rail_open),
        ]
        .map(|(id, open)| {
            let tucked = if open { 0. } else { 1. };
            let (ease, shortest) = (motion::RAIL_DRAWER, motion::RAIL_DRAWER_SHORTEST);
            motion::follow_share(id, tucked, ease, shortest, window, cx)
        });
        let t = Tokens::get(cx);
        window.set_ground(Some(crate::see_through::ground(t.surface_sunken, cx)));
        let parts = &self.parts;
        div()
            .id("shell")
            .key_context(SHELL)
            .track_focus(&self.focus)
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
            .on_action(
                cx.listener(|bench, _: &crate::places::ClosePlace, _, cx| bench.close_place(cx)),
            )
            .on_action(
                cx.listener(|bench, _: &crate::places::LastPlace, _, cx| {
                    bench.go_to_last_place(cx)
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::places::NextGroup, _, cx| bench.next_group(cx)),
            )
            .on_action(cx.listener(|bench, _: &ToggleYolo, _, _| bench.toggle_yolo_in_front()))
            .on_action(
                cx.listener(|bench, _: &crate::places::FocusTerminals, _, _| {
                    bench.call("showTerminals", json!({}))
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::places::FocusFiles, _, cx| bench.focus_files(cx)),
            )
            .on_action(
                cx.listener(|bench, _: &crate::places::FocusAgents, window, cx| {
                    bench.focus_agents(window, cx)
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::places::FocusBoard, _, cx| bench.focus_board(cx)),
            )
            .on_action(
                cx.listener(|bench, _: &crate::switcher::NextPlace, window, cx| {
                    bench.step_place(1, window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::switcher::PreviousPlace, window, cx| {
                    bench.step_place(-1, window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::commands::OpenCommands, window, cx| {
                    bench.toggle_commands(window, cx);
                }),
            )
            .on_action(
                cx.listener(
                    |bench, _: &crate::agent_palette::ChooseAgent, window, cx| match bench
                        .chosen_host()
                    {
                        crate::hosts::HostChoice::Ssh(alias) => bench.open_install_pane(&alias),
                        _ => bench.toggle_agent_palette(window, cx),
                    },
                ),
            )
            .on_action(
                cx.listener(|bench, _: &crate::picker::OpenProject, window, cx| {
                    bench.toggle_picker(crate::picker::Picking::Projects, window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::picker::OpenHost, window, cx| {
                    bench.toggle_picker(crate::picker::Picking::Hosts, window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::picker::OpenPlace, window, cx| {
                    bench.toggle_picker(crate::picker::Picking::Both, window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::new_tab::NewTab, window, cx| {
                    bench.toggle_new_tab(window, cx);
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::theme_palette::SwitchTheme, window, cx| {
                    bench.toggle_theme_palette(window, cx);
                }),
            )
            .on_modifiers_changed(
                cx.listener(|bench, event, window, cx| bench.voice_keys(event, window, cx)),
            )
            .capture_key_down(cx.listener(|bench, _: &gpui_kit::KeyDownEvent, _, cx| {
                bench.voice.update(cx, |voice, cx| voice.abandon_hold(cx))
            }))
            .on_action(cx.listener(|bench, _: &OpenFile, window, cx| {
                bench.toggle_finder(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|bench, _: &NewTerminal, _, cx| bench.new_terminal(cx)))
            .on_action(cx.listener(|bench, action: &RunProjectAction, _, cx| {
                bench.run_command(json!({ "action": action.id }), action.label.clone(), cx)
            }))
            .on_action(
                cx.listener(|bench, _: &TextSizeIncrease, _, _| {
                    bench.resize_front_text(Some(true))
                }),
            )
            .on_action(
                cx.listener(|bench, _: &TextSizeDecrease, _, _| {
                    bench.resize_front_text(Some(false))
                }),
            )
            .on_action(cx.listener(|bench, _: &TextSizeReset, _, _| bench.resize_front_text(None)))
            .on_action(cx.listener(|bench, _: &FocusGit, _, cx| bench.open_git(cx)))
            .on_action(cx.listener(|bench, open: &OpenArea, _, cx| bench.open_area(open.area, cx)))
            .on_action(
                cx.listener(|bench, _: &FocusSearch, window, cx| bench.open_search(window, cx)),
            )
            .on_action(
                cx.listener(|bench, _: &FocusSearchWindow, window, cx| {
                    bench.open_search(window, cx)
                }),
            )
            .on_action(
                cx.listener(|bench, _: &crate::settings::ToggleSettings, window, cx| {
                    crate::settings::toggle(bench, window, cx)
                }),
            )
            .on_action(cx.listener(|bench, _: &NewChat, window, cx| bench.new_chat(window, cx)))
            .on_action(cx.listener(|bench, _: &FocusTerminal, _, cx| bench.focus_terminal(cx)))
            .on_action(cx.listener(|bench, _: &ToggleDesk, _, cx| bench.toggle_front_desk(cx)))
            .on_action(cx.listener(|bench, _: &DeskAlone, _, cx| bench.toggle_desk_alone(cx)))
            .on_action(cx.listener(|bench, _: &NewPage, _, cx| bench.new_front_page(cx)))
            .on_action(cx.listener(|bench, _: &FocusAddress, _, cx| bench.focus_front_address(cx)))
            .on_action(cx.listener(|bench, _: &ReloadPage, _, _| bench.reload_front_page()))
            .on_action(cx.listener(|bench, _: &PageBack, _, _| bench.step_front_page(-1)))
            .on_action(cx.listener(|bench, _: &PageForward, _, _| bench.step_front_page(1)))
            .on_action(cx.listener(|bench, _: &NewSimulator, _, cx| {
                bench.front_simulator(cx);
                bench.parts.redraw(&bench.parts.stage, cx);
            }))
            .on_action(cx.listener(|bench, _: &PreviousTab, _, _| bench.step_tab(-1, |_, _| true)))
            .on_action(cx.listener(|bench, _: &NextTab, _, _| bench.step_tab(1, |_, _| true)))
            .on_action(cx.listener(|bench, _: &PreviousOfKind, window, cx| {
                if !bench.step_desk_page(-1, window, cx) {
                    bench.step_tab(-1, same_kind);
                }
            }))
            .on_action(cx.listener(|bench, _: &NextOfKind, window, cx| {
                if !bench.step_desk_page(1, window, cx) {
                    bench.step_tab(1, same_kind);
                }
            }))
            .on_action(cx.listener(|bench, _: &GoToTab1, _, _| bench.go_to_tab(Some(1))))
            .on_action(cx.listener(|bench, _: &GoToTab2, _, _| bench.go_to_tab(Some(2))))
            .on_action(cx.listener(|bench, _: &GoToTab3, _, _| bench.go_to_tab(Some(3))))
            .on_action(cx.listener(|bench, _: &GoToTab4, _, _| bench.go_to_tab(Some(4))))
            .on_action(cx.listener(|bench, _: &GoToTab5, _, _| bench.go_to_tab(Some(5))))
            .on_action(cx.listener(|bench, _: &GoToTab6, _, _| bench.go_to_tab(Some(6))))
            .on_action(cx.listener(|bench, _: &GoToTab7, _, _| bench.go_to_tab(Some(7))))
            .on_action(cx.listener(|bench, _: &GoToTab8, _, _| bench.go_to_tab(Some(8))))
            .on_action(cx.listener(|bench, _: &GoToLastTab, _, _| bench.go_to_tab(None)))
            .on_action(cx.listener(|bench, _: &CloseTab, window, cx| bench.close_front(window, cx)))
            .on_action(cx.listener(|bench, _: &FocusMode, _, cx| bench.toggle_focus_mode(cx)))
            .on_action(
                cx.listener(|bench, _: &SplitRow, _, cx| bench.split_front(SplitDir::Row, cx)),
            )
            .on_action(
                cx.listener(|bench, _: &SplitColumn, _, cx| {
                    bench.split_front(SplitDir::Column, cx)
                }),
            )
            .on_action(
                cx.listener(|bench, _: &SplitStack, _, cx| bench.split_front(SplitDir::Stack, cx)),
            )
            .on_action(cx.listener(|bench, _: &FocusLeft, window, cx| {
                bench.focus_towards(Towards::Left, window, cx)
            }))
            .on_action(cx.listener(|bench, _: &FocusRight, window, cx| {
                bench.focus_towards(Towards::Right, window, cx)
            }))
            .on_action(cx.listener(|bench, _: &FocusUp, window, cx| {
                bench.focus_towards(Towards::Up, window, cx)
            }))
            .on_action(cx.listener(|bench, _: &FocusDown, window, cx| {
                bench.focus_towards(Towards::Down, window, cx)
            }))
            .on_action(
                cx.listener(|bench, _: &ResizeLeft, _, _| bench.resize_towards(Towards::Left)),
            )
            .on_action(
                cx.listener(|bench, _: &ResizeRight, _, _| bench.resize_towards(Towards::Right)),
            )
            .on_action(cx.listener(|bench, _: &ResizeUp, _, _| bench.resize_towards(Towards::Up)))
            .on_action(
                cx.listener(|bench, _: &ResizeDown, _, _| bench.resize_towards(Towards::Down)),
            )
            .on_action(cx.listener(|bench, _: &ZoomPane, _, cx| bench.toggle_zoom(cx)))
            .on_action(cx.listener(|bench, _: &QuitAndStopEverything, _, _| {
                let _ = bench.asks.send(Ask::Shutdown);
            }))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(
                cx.listener(|bench, open: &OpenAt, _, cx| {
                    crate::editor::go(bench, open.clone(), cx)
                }),
            )
            .flex()
            .flex_col()
            .font_family(theme::UI)
            .text_size(crate::theme::ui_text(13.))
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
                    .pt(px(4.))
                    .px(px(10.))
                    .pb(px(10.))
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
                    .when(!self.side_rail_open, |above| {
                        above.child(self.rail_edge(Edge::Start, window, cx))
                    })
                    .when(!self.tab_rail_open, |above| {
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
                    .when(
                        self.finder.is_some() || self.palette.is_some() || self.switcher.is_some(),
                        |above| {
                            above.child(
                                parts
                                    .finder
                                    .clone()
                                    .cached(StyleRefinement::default().absolute().inset_0()),
                            )
                        },
                    )
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
