use crate::keyboard::Keyboard;
use crate::rail::DraggedRail;
use crate::rail::Edge;
use crate::trust::Trust;
use sikemux_core::server::call;
use sikemux_view::layout::At;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, deferred, div, px, AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context,
    DragMoveEvent, Entity, ExternalPaths, FocusHandle, Image, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StyleRefinement, Styled, Subscription, Task,
    WeakEntity, Window,
};
use sikemux_view::device::DeviceView;
use sikemux_view::prefs::{Preferences, WindowView};
use sikemux_view::workspace::{LauncherInfo, ProjectInfo};
use sikemux_wire::protocol::RemoteStatus;
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
use sikemux_core::server::call::Call;
use sikemux_core::server::layout::ActivateProjectParams;

/// A click in the app's own window takes the keyboard back from a page holding it. On
/// Linux a page is a window of another connection, which X would otherwise leave it with.
#[cfg(target_os = "linux")]
fn keyboard_back() {
    sikemux_browser::give_keyboard_back();
}

#[cfg(not(target_os = "linux"))]
fn keyboard_back() {}

/// The decoded pictures a long chat full of screenshots keeps loaded: about a dozen at full size.
const PICTURE_BUDGET: usize = 256 * 1024 * 1024;

/// The longest a space's name may be, as on the Mac.
pub(crate) const MAX_SPACE_NAME: usize = 40;

pub mod actions;
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
    /// What the per-project and worktree watches were last made for.
    pub(crate) watched: feed::Watched,
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
    pub(crate) closing_tabs: std::cell::RefCell<HashMap<String, std::time::Instant>>,
    /// The card beside the agent row the pointer rests on.
    pub agent_card: crate::agents::card::Card,
    /// Where the stage was last drawn, for what rises over the stage alone.
    pub stage_frame: Rc<Cell<Bounds<Pixels>>>,
    /// The space whose projects the rail shows, else every project.
    pub space: Option<String>,
    pub git: crate::git::GitTab,
    pub chats: crate::chat::Chats,
    pub tasks: crate::tasks::Tasks,
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
    pub board: crate::board::BoardTab,
    pub goals: crate::goals::GoalsTab,
    /// What the window shows, as the person last left it.
    pub window_view: WindowView,
    /// The mod panes of the strip in front, by view.
    pub mod_panes: HashMap<String, Entity<crate::mod_pane::ModPane>>,
    /// Settings, while they take the stage's place.
    pub settings: Entity<crate::settings::Settings>,
    pub feeds: crate::hosts::Feeds,
    /// The stage's tabs while they move between one another.
    pub(crate) track: crate::track::Slide<String>,
    pub(crate) trust: Trust,
    /// The skills agents saved in their own folders that the person was told of, by path.
    pub(crate) skills_offered: HashSet<PathBuf>,
    /// The window the store draws, for what is said in it from outside a draw.
    pub(crate) window: AnyWindowHandle,
    pub parts: Parts,
    pub panes: crate::panes::Panes,
    pub desks: Entity<crate::desk::Desks>,
    pub(crate) desk_news: crate::desk::DeskNews,
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
            here.watch_plain(read);
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
            watched: feed::Watched::default(),
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
            stage_frame: Rc::default(),
            space: None,
            feeds: Default::default(),
            track: crate::track::Slide::default(),
            trust: Trust::default(),
            skills_offered: HashSet::new(),
            chats: crate::chat::Chats::default(),
            git: crate::git::GitTab::default(),
            tasks: crate::tasks::Tasks::new(cx),
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
            board: Default::default(),
            goals: Default::default(),
            window_view: WindowView::default(),
            mod_panes: HashMap::new(),
            settings: cx.new(|_| crate::settings::Settings::default()),
            window: window.window_handle(),
            parts: Parts::new(cx),
            panes: crate::panes::Panes::default(),
            desk_news: crate::desk::DeskNews::new(&desks, window, cx),
            desks,
            _sim_news: cx.subscribe(&sims, Self::hear_sims),
            sims,
            _older: None,
        };
        bench.parts.stage_follows(&bench.sims, cx);
        bench.parts.stage_follows(&bench.desks, cx);
        bench.desk_news.follow(&bench.desks, &bench.sims, cx);
        bench.parts.stage_follows(&bench.editors, cx);
        bench.parts.stage_follows(&bench.settings, cx);
        bench.keys.focus.focus(window, cx);
        bench.wait_patiently(cx);
        crate::handover::follow(&mut bench, cx);
        bench
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

    /// Opens settings on `page`, as a link to one of them does.
    pub fn open_settings(
        &mut self,
        page: crate::settings::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::open(self, page, window, cx);
    }

    pub fn activate_project(&self, id: &str) {
        self.target()
            .call_typed::<call::ActivateProject>(ActivateProjectParams {
                project: id.to_owned(),
            });
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
                    .children(crate::overlays::dialogs(self, cx))
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
