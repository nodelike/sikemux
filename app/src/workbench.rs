use crate::keyboard::Keyboard;
use crate::keyboard::TakesKeys;
use crate::rail::DraggedRail;
use crate::rail::Edge;
use crate::trust::Trust;
use sikemux_core::server::call;
use sikemux_core::server::workspace::SessionChoice;
use sikemux_view::changed::Topic;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;

use gpui_kit::component::input::{AnyInputState, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, deferred, div, px, AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context,
    DragMoveEvent, Entity, ExternalPaths, FocusHandle, Image, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StyleRefinement, Styled, Subscription, Task,
    WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::layout::{PaneKind, ProjectLayout, Tab};
use sikemux_view::prefs::{Preferences, TabRailPage, WindowView};
use sikemux_wire::protocol::{
    LauncherInfo, NoticeGoal, ProjectInfo, RemoteStatus, SessionId, SessionInfo,
};
use tokio::sync::mpsc;

use crate::battery::Battery;
use crate::chat::START;
use crate::core::{Ask, Link, Target};
use crate::files::FileTree;
use crate::hosts::Place;
use crate::motion;
use crate::overlays::{Layer, Overlays};
use crate::pickers::finder::ProjectFiles;
use crate::reads::{Read, Reads};
use crate::search::SearchPane;
use crate::theme::{self, Tokens, SPACE_1, SPACE_2H, TEXT_SM};
use crate::toast;
use crate::{agents, home, rail, tabs, terminal};
use serde::Serialize;
use sikemux_core::server::agent_accounts::AccountUsage;
use sikemux_core::server::agent_view::{AgentViewKind, AgentViewParams};
use sikemux_core::server::call::Call;
use sikemux_core::server::layout::{ActivateProjectParams, HostParams, TabParams, TabsParams};
use sikemux_core::server::spare::{NewTerminal, SshParams};
use sikemux_core::server::workspace::{AgentPatch, UpdateAgentParams};

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
/// The decoded pictures a long chat full of screenshots keeps loaded: about a dozen at full size.
const PICTURE_BUDGET: usize = 256 * 1024 * 1024;

/// The longest a space's name may be, as on the Mac.
pub(crate) const MAX_SPACE_NAME: usize = 40;

pub(crate) mod actions;
mod feed;

/// The one store: what the core last said, and what only this window decides.
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
    pub pairing: crate::pairing::RemotePairing,
    /// Whether this window has looked for what kept running while Sikemux was closed.
    pub kept_running_told: bool,
    /// Each waiting `sikemux open --wait`, with the tabs it opened that are still open.
    pub(crate) open_waits: Vec<(sikemux_wire::protocol::CallId, HashMap<String, bool>)>,
    /// Each open project settings tab's state, by the project's folder.
    pub project_settings: HashMap<String, crate::project_settings::ProjectSettings>,
    pub shown: crate::screens::experience::Shown,
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
    pub run_cards: crate::tasks::test_card::RunCards,
    /// The space whose projects the rail shows, else every project.
    pub space: Option<String>,
    pub git: crate::git::GitTab,
    pub chats: crate::chat::Chats,
    pub run_clock: Entity<crate::tasks::chip::RunClock>,
    pub files: FileTree,
    /// The file tree's, which its arrow keys and F2 move and act in.
    pub files_focus: FocusHandle,
    /// The agent the project picker moves to the project picked, while it does.
    pub moving: Option<String>,
    /// The command deck's keys, the last used first.
    pub recent_commands: Vec<SharedString>,
    /// When a refused file listing is asked again.
    pub(crate) files_retry: crate::retry::Retry,
    pub project_files: ProjectFiles,
    /// The kind of agent the rail was switched to, such as "claude"; its account is the one the
    /// person chose for that kind.
    pub launcher_choice: Option<String>,
    /// The plan limits are to be read once the core lists its agents, as the app starts.
    usage_wanted: bool,
    pub pinch: Option<crate::text_size::Pinch>,
    pub calendar: crate::activity_view::Calendar,
    /// The rail's filter of recent chats, while it shows.
    pub filter: Option<crate::pickers::recent::Filter>,
    /// The front project's saved chats paged or searched to beyond the recent ones.
    pub older: Option<crate::pickers::recent::OlderChats>,
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
    /// This strip, as the core's tab calls name it.
    pub(crate) fn at(&self) -> At {
        match self {
            Strip::Project(project) => At::project(project),
            Strip::Host(alias) => At::host(alias),
            Strip::Terminals => At::terminals(),
        }
    }
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

/// How often the Limits panel reads the plan limits again while it shows, as the Mac app does.
const LIMITS_EVERY: Duration = Duration::from_secs(5 * 60);

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
            pairing: crate::pairing::RemotePairing::new(cx),
            shown: Default::default(),
            picture: crate::picture::AccountPicture::default(),
            signing_in: false,
            remote: None,
            accounts: Default::default(),
            badge: 0,
            reorder: None,
            place: None,
            last_place: None,
            rails: crate::rail::Rails::new(cx),
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
            git: crate::git::GitTab::default(),
            run_clock: cx.new(|_| crate::tasks::chip::RunClock::default()),
            files: FileTree::default(),
            files_focus: cx.focus_handle(),
            moving: None,
            recent_commands: Vec::new(),
            files_retry: Default::default(),
            project_files: ProjectFiles::default(),
            launcher_choice: None,
            usage_wanted: false,
            pinch: None,
            calendar: crate::activity_view::Calendar::default(),
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

    /// Files dropped where nothing takes them go to the terminal holding the keys, else to the
    /// chat in front.
    fn take_stray_drop(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = crate::terminal::focused(self) {
            return crate::terminal::take_dropped(self, id, paths);
        }
        let into = match self.showing_goals() {
            true => Some(self.chat_in_goals(cx).unwrap_or_else(|| START.to_owned())),
            false => self.chats.focused.clone().map(|(agent, _)| agent),
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

    /// Brings forward the project's tab showing `target`, opening one when none does.
    pub fn show(&self, kind: PaneKind, target: &str, title: Option<&str>) {
        if let Some(project) = self.project() {
            let tab = OpenTab::new(At::project(&project.id), kind, target);
            self.open_in_front(OpenTab {
                title: title.map(str::to_owned),
                ..tab
            });
        }
    }

    pub fn open_in_front(&self, tab: OpenTab) {
        self.target().call_typed::<call::OpenTab>(tab);
    }

    pub fn open_here(&self, tab: OpenTab) {
        Target::here(&self.asks).call_typed::<call::OpenTab>(tab);
    }

    /// Makes what the tab in front needs before it is drawn, which takes a window: its composer
    /// and the hint in it, its pane, its mods' panes and its desk, then who waits for the keys.
    fn settle_front(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compose(window, cx);
        self.hint(window, cx);
        self.make_front_pane(window, cx);
        self.mod_panes(cx);
        crate::desk::prepare(self, window, cx);
        self.settle_takes_keys();
    }

    /// Whether the person chose the rails' Compact density.
    pub fn rail_compact(&self) -> bool {
        self.preferences().rail_density == sikemux_view::prefs::RailDensity::Compact
    }

    pub fn window_handle(&self) -> AnyWindowHandle {
        self.window
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

    pub fn notify(&self, note: Notification, cx: &mut Context<Self>) {
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| window.push_notification(note, cx));
        });
    }

    /// Waits for `asked`, then does `then` with what it answered, or tells the person why it
    /// did not, after `what`, until they dismiss it.
    pub fn report<T: 'static>(
        &self,
        asked: impl std::future::Future<Output = Result<T, String>> + 'static,
        what: &'static str,
        then: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |bench, cx| {
            let answer = asked.await;
            let _ = bench.update(cx, |bench, cx| match answer {
                Ok(answer) => then(bench, answer, cx),
                Err(error) => {
                    bench.notify(toast::error(format!("{what}: {error}")).autohide(false), cx)
                }
            });
        })
        .detach();
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

    pub fn call_typed<C: Call>(&self, params: C::Params)
    where
        C::Params: Serialize,
    {
        Target::here(&self.asks).call_typed::<C>(params);
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
            self.open_in_front(
                OpenTab::new(
                    At::project(&project.id),
                    PaneKind::Files,
                    path.to_string_lossy(),
                )
                .preview(preview),
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

    /// Starts a chat in the project in front, or in the one picked when no project is in front,
    /// or on an SSH host's strip opens its Install Sikemux pane.
    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.strip() {
            Some(Strip::Host(alias)) => self.open_install_pane(&alias),
            Some(Strip::Project(project)) => self.start_chat_in(&project, window, cx),
            _ => self.pick_project(crate::pickers::picker::ThenPicked::StartChat, window, cx),
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
            self.target()
                .call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                    retitling.agent,
                    AgentPatch {
                        title: Some(title),
                        renamed: Some(true),
                        ..AgentPatch::default()
                    },
                ));
        }
        self.parts.redraw_stage(cx);
        self.parts.redraw(&self.parts.rail, cx);
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
        let to = if terminal {
            AgentViewKind::Terminal
        } else {
            AgentViewKind::Chat
        };
        self.chat_target(agent)
            .call_typed::<call::SwitchAgentView>(AgentViewParams {
                agent_id: agent.to_owned(),
                to,
            });
    }

    /// Shows the workbench in place of the goals, as anything opened in one of its tabs needs.
    pub fn show_workbench(&mut self, cx: &mut Context<Self>) {
        if self.showing_goals() {
            self.toggle_goals_view(cx);
        }
    }

    /// The agent whose chat the goals show, while they do.
    pub fn chat_in_goals(&self, cx: &App) -> Option<String> {
        let goals = self.goals.as_ref()?;
        let chat = goals.read(cx).chat_shown()?.to_owned();
        self.showing_goals().then_some(chat)
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

    pub fn open_goal(&mut self, goal: &NoticeGoal, cx: &mut Context<Self>) {
        if !self.showing_goals() {
            self.toggle_goals_view(cx);
        }
        self.goals_view(cx)
            .update(cx, |goals, cx| goals.open_goal(goal, cx));
    }

    pub fn showing_goals(&self) -> bool {
        self.window_view == WindowView::Goals
    }

    /// The goals' pane, while the window shows the goals rather than the workbench.
    pub fn goals_in_front(&self) -> Option<&Entity<crate::goals::GoalsPane>> {
        self.goals.as_ref().filter(|_| self.showing_goals())
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
        let asked = SessionChoice {
            provider: recent.provider.clone(),
            session_id: session.to_owned(),
            project: project.path.clone(),
            cwd: Some(recent.cwd.clone()),
            title: Some(recent.title.clone()),
        };
        let target = self.target();
        let taken = target.request_typed::<call::ResumeSession>(asked);
        cx.spawn(async move |bench, cx| match taken.await {
            Ok(taken) => {
                target.call_typed::<call::OpenTab>(OpenTab::new(
                    At::project(&taken.project),
                    PaneKind::Agent,
                    taken.agent_id,
                ));
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
        self.open_in_front(OpenTab::new(
            At::project(&project.id),
            PaneKind::Search,
            project.path.to_string_lossy(),
        ));
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

    /// Opens a terminal in the project in front, or among the terminals of no project while
    /// those are in front.
    pub fn new_terminal(&self, cx: &mut App) {
        match (self.strip(), self.project()) {
            (Some(Strip::Terminals), _) => self.new_loose_terminal(cx),
            (Some(Strip::Host(alias)), _) => self.open_ssh_terminal(&alias, cx),
            (_, Some(project)) => {
                self.take_keys_when_shown(TakesKeys::Terminal, cx);
                self.target().call_typed::<call::NewTerminal>(NewTerminal {
                    project: project.id.clone(),
                    terminals: false,
                })
            }
            _ => {}
        }
    }

    pub fn new_loose_terminal(&self, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Terminal, cx);
        self.call_typed::<call::NewTerminal>(NewTerminal::loose());
    }

    pub fn activate_project(&self, id: &str) {
        self.target()
            .call_typed::<call::ActivateProject>(ActivateProjectParams {
                project: id.to_owned(),
            });
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
            self.target()
                .call_typed::<call::ActivateTab>(TabParams::new(strip.at(), tab));
        }
    }

    /// Moves a tab to `to` in its strip, counted as the strip is now.
    pub fn move_tab(&self, strip: &Strip, tab: &str, to: usize) {
        if !self.closing(tab) {
            self.target().call_typed::<call::MoveTab>(TabParams {
                to: Some(to),
                ..TabParams::new(strip.at(), tab)
            });
        }
    }

    /// Keeps a tab opened to look at, so the next one opens beside it.
    pub fn keep_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.target()
                .call_typed::<call::KeepTab>(TabParams::new(strip.at(), tab));
        }
    }

    pub fn close_tabs(&self, strip: &Strip, tabs: &[String]) {
        let tabs: Vec<String> = (tabs.iter())
            .filter(|tab| !self.closing(tab))
            .cloned()
            .collect();
        if tabs.is_empty() {
            return;
        }
        let asked = std::time::Instant::now();
        self.closing_tabs
            .borrow_mut()
            .extend(tabs.iter().map(|tab| (tab.clone(), asked)));
        self.target().call_typed::<call::CloseTabs>(TabsParams {
            at: strip.at(),
            tabs,
        });
    }

    /// Names a tab; an empty name gives back the one its view gives it.
    pub fn rename_tab(&self, strip: &Strip, tab: &str, title: &str) {
        if !self.closing(tab) {
            self.target().call_typed::<call::RenameTab>(TabParams {
                title: Some(title.to_owned()),
                ..TabParams::new(strip.at(), tab)
            });
        }
    }

    /// Opens a copy of a tab beside it, with new shells for its terminals.
    pub fn duplicate_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.target()
                .call_typed::<call::DuplicateTab>(TabParams::new(strip.at(), tab));
        }
    }

    pub fn open_ssh_terminal(&self, alias: &str, cx: &mut App) {
        self.take_keys_when_shown(TakesKeys::Terminal, cx);
        self.call_typed::<call::OpenSshTerminal>(SshParams {
            host: alias.to_owned(),
        });
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
        let tab = |project: String| OpenTab::new(At::project(&project), PaneKind::Agent, id);
        let Some(project) = self.shown().and_then(project_on) else {
            // Opening a tab on a host's layout brings that host forward there.
            if let Some(project) = self.this_view().and_then(project_on) {
                return self.open_here(tab(project));
            }
            if let Some((host, project)) = self
                .feeds
                .hosts()
                .find_map(|(host, feed)| Some((host.clone(), project_on(feed.view.as_ref()?)?)))
            {
                self.call_typed::<call::ChooseHost>(HostParams {
                    host: format!("host:{host}"),
                });
                Target::new(&self.asks, Place::Account(host))
                    .call_typed::<call::OpenTab>(tab(project));
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
            None => self.open_in_front(tab(project)),
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
            self.look_for_older(false, crate::pickers::recent::PAGE, cx);
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("filter recent chats…"));
        let changes = cx.subscribe(&input, |bench, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if let Some(filter) = bench.filter.as_mut() {
                    filter.text = input.read(cx).value().to_lowercase();
                }
                bench.look_for_older(false, crate::pickers::recent::PAGE, cx);
                cx.notify();
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.filter = Some(crate::pickers::recent::Filter {
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
                    bench.settle_front(window, cx);
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
                    crate::pickers::switcher::overlay(bench, cx)
                        .or_else(|| crate::palette::overlay(bench, window, cx))
                        .or_else(|| crate::pickers::finder::overlay(bench, window, cx))
                        .unwrap_or_else(|| div().into_any_element())
                },
                true,
            ),
            over: part(
                |bench, window, cx| {
                    crate::pairing::overlay(bench, window, cx)
                        .or_else(|| crate::command_popup::overlay(bench, cx))
                        .unwrap_or_else(|| crate::screens::experience::draw(bench, window, cx))
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

impl Workbench {}

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
        let goals = self.showing_goals();
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
                    .children(crate::tasks::add_task::view(self, cx))
                    .children(crate::add_provider::view(self, cx))
                    .children(crate::buddy_picker::view(self, cx))
                    .children(crate::share::view(self, cx))
                    .children(crate::boot::reconnecting(self, cx))
                    .children(crate::boot::stopped(self, cx))
                    .children(crate::updates::required(self, cx))
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
