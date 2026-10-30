//! An agent's desk, beside it in its tab: the pages its browser holds, one in front.

mod address;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, Hsla, InteractiveElement, IntoElement, Keystroke, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};
use gpui_native_view::{native_view, Frame, NativeSurface, Surface};
use serde_json::{json, Value};
use sikemux_browser::{
    BrowserBounds, BrowserDownload, BrowserManager, BrowserShortcut, BrowserSnapshot,
    DownloadState, Host, ParentView,
};
use sikemux_core::cli::protocol::{is_browser_method, is_sim_method, HarnessRequest};
use sikemux_core::harness::runs::RunStatus;
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::{CallId, Pane, PaneKind, WindowAnswer, WindowCall};
use tokio::sync::mpsc;

use crate::codehost::HostArea;
use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_2, RADIUS_3, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_6, TEXT_XS, TEXT_XS_H,
};
use crate::toast;
use crate::ui::{self, IconButton};
use crate::workbench::{shows, Strip, Workbench};

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "desk.toggle")]
pub struct ToggleDesk;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "browser.tabNew")]
pub struct NewPage;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "simulator.new")]
pub struct NewSimulator;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "browser.address")]
pub struct FocusAddress;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "browser.reload")]
pub struct ReloadPage;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "browser.back")]
pub struct PageBack;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "browser.forward")]
pub struct PageForward;

/// The browser engine and what the desks show of it.
pub struct Desks {
    engine: Option<Arc<BrowserManager>>,
    strips: HashMap<String, Arc<BrowserSnapshot>>,
    pages: HashMap<String, Page>,
    /// Each agent's tabs as last heard, so only a tab it newly opened brings its desk out.
    known: HashMap<String, HashSet<String>>,
    /// What each agent's desk shows: its pages, or its simulator.
    kinds: HashMap<String, DeskKind>,
    /// The agents in the view as last heard, so those that leave it are let go.
    present: HashSet<String>,
    page_files: Arc<PageFiles>,
    /// The files and task terminals on each agent's desk, beside its pages, in the order they came.
    items: HashMap<String, Vec<DeskItem>>,
    /// The item each desk shows in front of its pages, while it shows one.
    front: HashMap<String, DeskItem>,
    /// The file and the terminal each desk last showed, which its switch brings back.
    last_front: HashMap<(String, DeskKind), DeskItem>,
    /// Pages' site icons, decoded once from the inline image each page sends.
    favicons: std::cell::RefCell<HashMap<String, Arc<gpui_kit::Image>>>,
    /// The runs whose terminals have been put on a desk, so each is put there once.
    placed_runs: HashSet<String>,
    /// Whether the runs have been read once; runs going by then join their desks behind.
    runs_read: bool,
    /// The agent whose address ⌘L asked for, floated once its desk has drawn it.
    address_wanted: Option<String>,
    floating: Option<address::Floating>,
    /// Each desk's tab strip, and the tab it last scrolled into view.
    tab_scrolls: std::cell::RefCell<HashMap<String, (gpui_kit::ScrollHandle, Option<usize>)>>,
    /// What the engine tells the window, until the window listens.
    hearing: Option<mpsc::UnboundedReceiver<Heard>>,
    _theming: Subscription,
}

/// The window listening to its desks' engine.
pub struct DeskEars {
    _hearing: Option<gpui_kit::Task<()>>,
    _activation: Subscription,
}

/// Hears what the desks' engine tells the window, and registers the window with the core
/// each time it comes to the front.
pub fn listen(desks: &Entity<Desks>, window: &mut Window, cx: &mut Context<Workbench>) -> DeskEars {
    let hearing = desks.update(cx, |desks, _| desks.hearing.take());
    let task = hearing.map(|mut hearing| {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(heard) = hearing.recv().await {
                let heard = this.update_in(cx, |bench, window, cx| bench.hear(heard, window, cx));
                if heard.is_err() {
                    break;
                }
            }
        })
    });
    let activation = cx.observe_window_activation(window, |bench, window, _| {
        if window.is_window_active() {
            let _ = bench.asks.send(Ask::RegisterWindow);
        }
    });
    DeskEars {
        _hearing: task,
        _activation: activation,
    }
}

/// What a desk holds besides its pages: a file in an editor, the terminal of a task its
/// agent started, or what a link named, shown among its pages: a pull request, issue or run,
/// or a plugin's own item such as a Jira issue.
#[derive(Clone, PartialEq)]
pub enum DeskItem {
    File(PathBuf),
    Terminal {
        session: u64,
        title: String,
    },
    /// A pull request, issue or run, as the Git tab shows one.
    Hosted {
        url: String,
        shown: Entity<HostArea>,
    },
    /// What a plugin's own view shows of a link, such as a Jira issue.
    Plugin {
        url: String,
        shown: Entity<crate::plugin_pane::PluginPane>,
    },
}

impl DeskItem {
    fn kind(&self) -> DeskKind {
        match self {
            DeskItem::File(_) => DeskKind::Files,
            DeskItem::Terminal { .. } => DeskKind::Terminals,
            DeskItem::Hosted { .. } | DeskItem::Plugin { .. } => DeskKind::Pages,
        }
    }

    fn same(&self, other: &DeskItem) -> bool {
        match (self, other) {
            (DeskItem::File(one), DeskItem::File(other)) => one == other,
            (
                DeskItem::Terminal { session: one, .. },
                DeskItem::Terminal { session: other, .. },
            ) => one == other,
            (DeskItem::Hosted { url: one, .. }, DeskItem::Hosted { url: other, .. })
            | (DeskItem::Plugin { url: one, .. }, DeskItem::Plugin { url: other, .. }) => {
                one == other
            }
            _ => false,
        }
    }
}

/// What a desk shows, as its switch names it: its pages, its files, its task terminals, or its
/// simulator. Only the pages and the simulator are kept; files and terminals are shown by
/// bringing one to the front.
impl Desks {
    /// A page's site icon from the `data:` image it sends, decoded the first time it is drawn.
    fn favicon(&self, data: &str) -> Option<Arc<gpui_kit::Image>> {
        if let Some(icon) = self.favicons.borrow().get(data) {
            return Some(icon.clone());
        }
        let (mime, base64) = data.strip_prefix("data:")?.split_once(";base64,")?;
        let icon = crate::picture::decode(mime, base64)?;
        let mut favicons = self.favicons.borrow_mut();
        if favicons.len() >= MAX_FAVICONS {
            favicons.clear();
        }
        favicons.insert(data.to_owned(), icon.clone());
        Some(icon)
    }
}

/// How many site icons the desks keep decoded.
const MAX_FAVICONS: usize = 64;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DeskKind {
    #[default]
    Pages,
    Files,
    Terminals,
    Simulator,
}

/// One desk's page area and address bar.
struct Page {
    surface: NativeSurface,
    focus: FocusHandle,
    address: Entity<InputState>,
    _typing: Subscription,
}

/// What the engine tells the window.
enum Heard {
    Tabs,
    Downloaded(BrowserDownload),
    Shortcut(BrowserShortcut),
    Acting(String),
}

impl Desks {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (heard, hearing) = mpsc::unbounded_channel();
        sikemux_core::runtime::set(runtime().handle().clone());
        let home = sikemux_process::paths::config_dir();
        let page_files = Arc::new(PageFiles {
            folder: home
                .as_ref()
                .map(|home| home.join(sikemux_core::pages::FOLDER).join("tabs")),
            theme: Mutex::new(
                cx.try_global::<Tokens>()
                    .map(desk_theme)
                    .unwrap_or_default(),
            ),
            ground: Mutex::new(
                cx.try_global::<Tokens>()
                    .map(crate::theme::page_ground_rgb)
                    .unwrap_or_default(),
            ),
            engine: OnceLock::new(),
        });
        let engine = window_pointer(window).map(|view| {
            BrowserManager::new(Arc::new(DeskHost {
                view,
                heard,
                home,
                pages: page_files.clone(),
            }))
        });
        if let Some(engine) = &engine {
            let _ = page_files.engine.set(Arc::downgrade(engine));
        }
        let theming = cx.observe_global::<Tokens>({
            let page_files = page_files.clone();
            move |_, cx| {
                *page_files.theme.locked() = desk_theme(Tokens::get(cx));
                *page_files.ground.locked() = crate::theme::page_ground_rgb(Tokens::get(cx));
            }
        });
        let hearing = engine.is_some().then_some(hearing);
        Self {
            engine,
            strips: HashMap::new(),
            pages: HashMap::new(),
            known: HashMap::new(),
            kinds: HashMap::new(),
            present: HashSet::new(),
            page_files,
            items: HashMap::new(),
            front: HashMap::new(),
            last_front: HashMap::new(),
            favicons: Default::default(),
            placed_runs: HashSet::new(),
            runs_read: false,
            address_wanted: None,
            floating: None,
            tab_scrolls: Default::default(),
            hearing,
            _theming: theming,
        }
    }

    pub fn engine(&self) -> Option<&Arc<BrowserManager>> {
        self.engine.as_ref()
    }

    /// Agents whose pages or device came back from before an update's restart, let go on the
    /// first view if they are no longer in it.
    pub fn expect(&mut self, agents: impl IntoIterator<Item = String>) {
        self.present.extend(agents);
    }

    fn forget(&mut self, agent: &str) {
        self.strips.remove(agent);
        self.pages.remove(agent);
        self.known.remove(agent);
        self.kinds.remove(agent);
        self.items.remove(agent);
        self.front.remove(agent);
        self.last_front.retain(|(held, _), _| held != agent);
        self.tab_scrolls.borrow_mut().remove(agent);
    }

    fn show_kind(&mut self, agent: &str, kind: DeskKind, cx: &mut Context<Self>) {
        self.kinds.insert(agent.to_owned(), kind);
        cx.notify();
    }

    /// What the desk shows now: its simulator, else whatever is in front of its pages.
    fn kind_of(&self, agent: &str) -> DeskKind {
        if self.kinds.get(agent) == Some(&DeskKind::Simulator) {
            return DeskKind::Simulator;
        }
        self.front
            .get(agent)
            .map_or(DeskKind::Pages, DeskItem::kind)
    }

    /// Turns the desk to `kind`, its last tab of that kind in front; says whether the pages
    /// were asked for with none to show, so a blank one opens.
    fn pick_kind(&mut self, agent: &str, kind: DeskKind, cx: &mut Context<Self>) -> bool {
        match kind {
            DeskKind::Simulator => self.show_kind(agent, DeskKind::Simulator, cx),
            DeskKind::Pages => {
                self.show_kind(agent, DeskKind::Pages, cx);
                self.front.remove(agent);
                return (self.strips.get(agent)).is_none_or(|strip| strip.tabs.is_empty());
            }
            DeskKind::Files | DeskKind::Terminals => {
                let held = self.items.get(agent);
                let held_now = |item: &DeskItem| {
                    held.is_some_and(|items| items.iter().any(|it| it.same(item)))
                };
                let item = (self.last_front.get(&(agent.to_owned(), kind)))
                    .filter(|item| held_now(item))
                    .or_else(|| held?.iter().rev().find(|item| item.kind() == kind))
                    .cloned();
                if let Some(item) = item {
                    self.show_kind(agent, DeskKind::Pages, cx);
                    self.bring_front(agent, item, cx);
                }
            }
        }
        false
    }

    /// Shows `item` in front of the agent's pages, remembered as its kind's last.
    fn bring_front(&mut self, agent: &str, item: DeskItem, cx: &mut Context<Self>) {
        self.last_front
            .insert((agent.to_owned(), item.kind()), item.clone());
        self.front.insert(agent.to_owned(), item);
        cx.notify();
    }

    /// Puts `item` on the agent's desk once, in front of its pages when `front`.
    fn put(&mut self, agent: &str, item: DeskItem, front: bool, cx: &mut Context<Self>) {
        let items = self.items.entry(agent.to_owned()).or_default();
        if !items.iter().any(|held| held.same(&item)) {
            items.push(item.clone());
        }
        if front {
            self.bring_front(agent, item, cx);
            self.show_kind(agent, DeskKind::Pages, cx);
        }
        cx.notify();
    }

    /// Takes an item off the agent's desk; the last one left comes to the front, else its pages.
    fn take_off(&mut self, agent: &str, item: &DeskItem, cx: &mut Context<Self>) {
        let Some(items) = self.items.get_mut(agent) else {
            return;
        };
        items.retain(|held| !held.same(item));
        let next = items
            .iter()
            .rev()
            .find(|held| held.kind() == item.kind())
            .cloned();
        if (self.front.get(agent)).is_some_and(|front| front.same(item)) {
            match next {
                Some(next) => self.bring_front(agent, next, cx),
                None => {
                    self.front.remove(agent);
                }
            }
        }
        cx.notify();
    }

    fn holds(&self, agent: &str, item: &DeskItem) -> bool {
        (self.items.get(agent)).is_some_and(|items| items.iter().any(|held| held.same(item)))
    }

    /// Points the desks' files at `from`, or under it, to the same names under `to`.
    fn rename_files(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let held = (self.items.values_mut().flatten())
            .chain(self.front.values_mut())
            .chain(self.last_front.values_mut());
        for item in held {
            if let DeskItem::File(path) = item {
                if let Ok(rest) = path.strip_prefix(from) {
                    *path = to.join(rest);
                }
            }
        }
        cx.notify();
    }

    /// The files on every desk.
    fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.items.values().flatten().filter_map(|item| match item {
            DeskItem::File(path) => Some(path),
            DeskItem::Terminal { .. } | DeskItem::Hosted { .. } | DeskItem::Plugin { .. } => None,
        })
    }
}

/// The browser's and the simulator's work, off the window's thread.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("engines")
            .enable_all()
            .on_thread_start(crate::thread_priority::initiated)
            .build()
            .expect("the browser's runtime starts")
    })
}

pub fn spawn(work: impl std::future::Future<Output = ()> + Send + 'static) {
    runtime().spawn(work);
}

/// Runs `work` on the engines' threads and waits for it, from the window's.
pub async fn run<T: Send + 'static>(
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> Option<T> {
    runtime().spawn(work).await.ok()
}

#[cfg(target_os = "macos")]
fn window_pointer(window: &Window) -> Option<usize> {
    gpui_native_view::window_view(window)
        .map(|view| gpui_native_view::Retained::as_ptr(&view) as usize)
}

#[cfg(target_os = "linux")]
fn window_pointer(window: &Window) -> Option<usize> {
    gpui_native_view::window_x11(window).map(|window| window as usize)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn window_pointer(_: &Window) -> Option<usize> {
    None
}

struct DeskHost {
    /// GPUI's own view of the window on macOS, its X11 id on Linux; it outlives the engine.
    view: usize,
    heard: mpsc::UnboundedSender<Heard>,
    home: Option<PathBuf>,
    pages: Arc<PageFiles>,
}

impl Host for DeskHost {
    fn window(&self) -> Result<ParentView, String> {
        let ground = *self.pages.ground.locked();
        Ok(ParentView::beneath(self.view as *mut std::ffi::c_void).with_ground(ground))
    }

    fn state_dir(&self) -> Option<PathBuf> {
        Some(self.home.as_ref()?.join("browser"))
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        Some(self.home.as_ref()?.join("cache"))
    }

    fn videos_dir(&self) -> Option<PathBuf> {
        Some(sikemux_process::paths::home()?.join("Movies"))
    }

    fn page_url(&self, agent_id: &str, html: &str) -> Result<String, String> {
        self.pages.url(&draft_name(agent_id), html)
    }

    fn tabs_changed(&self) {
        let _ = self.heard.send(Heard::Tabs);
    }

    fn downloaded(&self, download: &BrowserDownload) {
        let _ = self.heard.send(Heard::Downloaded(download.clone()));
    }

    fn acting(&self, agent_id: &str) {
        let _ = self.heard.send(Heard::Acting(agent_id.to_owned()));
    }

    fn shortcut(&self, shortcut: BrowserShortcut) {
        let _ = self.heard.send(Heard::Shortcut(shortcut));
    }
}

/// Where the pages agents show on their desks are kept, drawn in the window's theme.
struct PageFiles {
    folder: Option<PathBuf>,
    theme: Mutex<String>,
    /// What a tab's page shows until it draws, on Linux.
    ground: Mutex<[u8; 3]>,
    engine: OnceLock<Weak<BrowserManager>>,
}

impl PageFiles {
    /// An address for `html` in a desk tab, kept as `name` so showing it again replaces the copy.
    fn url(&self, name: &str, html: &str) -> Result<String, String> {
        let folder = self
            .folder
            .as_ref()
            .ok_or("Sikemux has no folder to keep pages in")?;
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        let path = folder.join(format!("{name}.html"));
        let themed = sikemux_core::pages::themed(html, &self.theme.locked(), None);
        std::fs::write(&path, themed).map_err(|error| error.to_string())?;
        self.engine()?.local_file_url(&path)
    }

    fn engine(&self) -> Result<Arc<BrowserManager>, String> {
        self.engine
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "the browser has stopped".into())
    }
}

/// One draft per agent, which each `page_preview` replaces.
fn draft_name(agent: &str) -> String {
    format!("preview-{}", agent.replace(':', "-"))
}

/// A tab has nothing behind it, so its pages draw the window's surface themselves.
fn desk_theme(t: &Tokens) -> String {
    crate::theme::page_rule(t, Some(crate::theme::page_ground(t)))
}

/// Where a desk's page shows: the engine lays the agent's tab in front over the frame.
struct DeskPage {
    engine: Arc<BrowserManager>,
    agent: String,
}

impl Surface for DeskPage {
    fn place(&self, frame: Option<Frame>) {
        let bounds = frame.map(|Frame { bounds, visible }| BrowserBounds {
            x: f64::from(bounds.left()),
            y: f64::from(bounds.top()),
            width: f64::from(bounds.size.width),
            height: f64::from(bounds.size.height),
            clip_left: f64::from(visible.left() - bounds.left()),
            clip_right: f64::from(bounds.right() - visible.right()),
            holes: Vec::new(),
            dim: 0.,
            opacity: 1.,
            bottom_radius: f64::from(f32::from(RADIUS_3) - 1.),
        });
        if let Err(error) = self.engine.set_bounds(&self.agent, bounds) {
            eprintln!("sikemux: the desk's page could not be placed: {error}");
        }
    }

    #[cfg(target_os = "macos")]
    fn responder(&self) -> Option<gpui_native_view::Retained<gpui_native_view::NSView>> {
        self.engine.active_view(&self.agent).ok()?.1.view_here()
    }
}

impl Workbench {
    fn hear(&mut self, heard: Heard, window: &mut Window, cx: &mut Context<Self>) {
        match heard {
            Heard::Tabs => self.read_strips(cx),
            Heard::Downloaded(download) => {
                #[cfg(feature = "drive")]
                crate::drive::trace(format_args!(
                    "download {:?} {}",
                    download.state, download.path
                ));
                self.notify(downloaded(&download), cx)
            }
            Heard::Shortcut(shortcut) => {
                let shift = if shortcut.shift { "Shift+" } else { "" };
                let keys = crate::keymap::keystroke(&format!("Meta+{shift}{}", shortcut.code));
                if let Some(keystroke) = keys
                    .and_then(|keys| Keystroke::parse_with_os(&keys, cx.platform_info().os).ok())
                {
                    let page = self.desks.read(cx).pages.get(&shortcut.agent_id);
                    if let Some(focus) = page.map(|page| page.focus.clone()) {
                        focus.focus(window, cx);
                    }
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(keystroke, cx);
                    });
                }
            }
            Heard::Acting(agent) => {
                self.read_strips(cx);
                self.show_pages(&agent, cx);
                if !self.desk_in_sight(&agent) {
                    if let Some(engine) = self.desks.read(cx).engine.clone() {
                        let _ = engine.set_bounds(&agent, None);
                    }
                }
            }
        }
    }

    /// Closes the pages and lets go of the device of each agent that left the view, and lets go
    /// of the device of each agent that went to sleep, so another agent can take it up.
    pub(crate) fn forget_departed(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.view else {
            return;
        };
        let agents = &view.workspace.agents;
        let present: HashSet<String> = agents.iter().map(|agent| agent.id.clone()).collect();
        let asleep: Vec<String> = agents
            .iter()
            .filter(|agent| agent.asleep && self.sims.read(cx).attached_by(&agent.id))
            .map(|agent| agent.id.clone())
            .collect();
        let departed = self.desks.update(cx, |desks, cx| {
            let departed: Vec<String> = desks.present.difference(&present).cloned().collect();
            desks.present = present;
            for agent in &departed {
                if let Some(engine) = &desks.engine {
                    let _ = engine.close_agent(agent);
                }
                desks.forget(agent);
            }
            if !departed.is_empty() {
                cx.notify();
            }
            departed
        });
        for agent in departed.iter().chain(&asleep) {
            self.sims.update(cx, |sims, _| sims.let_go_device(agent));
        }
    }

    /// Reads every agent's tabs again, bringing out the desk of an agent that opened one and
    /// shutting the desk of one whose last tab closed.
    fn read_strips(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let agents: Vec<String> = self
            .view
            .as_ref()
            .map(|view| {
                view.workspace
                    .agents
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for agent in agents {
            let Ok(strip) = engine.snapshot(&agent) else {
                continue;
            };
            let now: HashSet<String> = strip.tabs.iter().map(|tab| tab.id.clone()).collect();
            let before = self.desks.update(cx, |desks, _| {
                desks.known.insert(agent.clone(), now.clone())
            });
            let opened =
                (before.as_ref()).is_some_and(|before| now.iter().any(|tab| !before.contains(tab)));
            let emptied = now.is_empty() && before.is_some_and(|before| !before.is_empty());
            if opened && !self.desk_open_anywhere(&agent) {
                self.reveal_desk(&agent);
            }
            self.desks.update(cx, |desks, cx| {
                desks.strips.insert(agent.clone(), Arc::new(strip));
                cx.notify();
            });
            if emptied {
                self.shut_empty_desk(&agent, cx);
            }
        }
    }

    /// Shuts the agent's desk once nothing is left on it, so no empty desk stays open.
    fn shut_empty_desk(&mut self, agent: &str, cx: &mut Context<Self>) {
        let empty = self.desk_kind(agent, cx) != DeskKind::Simulator
            && self
                .desks
                .read(cx)
                .items
                .get(agent)
                .is_none_or(Vec::is_empty)
            && (self.desks.read(cx).strips.get(agent)).is_none_or(|strip| strip.tabs.is_empty());
        if !empty {
            return;
        }
        if let Some((strip, tab, desk)) = self.desk_place(agent) {
            if !self.panes.desk_shutting(&desk) {
                self.shut_desk(strip, tab, desk, cx);
            }
        }
    }

    /// The project tab holding the agent's desk, its strip and the desk's view there.
    fn desk_place(&self, agent: &str) -> Option<(Strip, String, String)> {
        let layout = &self.view.as_ref()?.layout;
        layout.projects.iter().find_map(|(project, strip)| {
            strip.tabs.iter().find_map(|tab| {
                view_of(&tab.pane, PaneKind::Desk, agent)
                    .map(|view| (Strip::Project(project.clone()), tab.id.clone(), view))
            })
        })
    }

    /// The project tab holding the agent, its strip and its view there, wherever it is.
    fn agent_place(&self, agent: &str) -> Option<(Strip, String, String)> {
        let layout = &self.view.as_ref()?.layout;
        layout.projects.iter().find_map(|(project, strip)| {
            strip.tabs.iter().find_map(|tab| {
                view_of(&tab.pane, PaneKind::Agent, agent)
                    .map(|view| (Strip::Project(project.clone()), tab.id.clone(), view))
            })
        })
    }

    /// Whether the agent's desk is in the tab in front, or about to be, as `show_pages` brings
    /// it out beside the agent there. The engine holds a new tab back until it knows.
    fn desk_in_sight(&self, agent: &str) -> bool {
        self.tab().is_some_and(|tab| {
            view_of(&tab.pane, PaneKind::Desk, agent).is_some()
                || (!self.desk_open_anywhere(agent)
                    && view_of(&tab.pane, PaneKind::Agent, agent).is_some())
        })
    }

    fn desk_open_anywhere(&self, agent: &str) -> bool {
        self.view.as_ref().is_some_and(|view| {
            view.layout.projects.values().any(|strip| {
                strip
                    .tabs
                    .iter()
                    .any(|tab| view_of(&tab.pane, PaneKind::Desk, agent).is_some())
            })
        })
    }

    fn reveal_desk(&self, agent: &str) {
        if let Some((strip, tab, view)) = self.agent_place(agent) {
            self.split_pane(&strip, &tab, &view, PaneKind::Desk, agent, true);
        }
    }

    /// Turns the agent's desk to its simulator, bringing the desk out if it is closed.
    pub(crate) fn show_simulator(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.desks.update(cx, |desks, cx| {
            desks.show_kind(agent, DeskKind::Simulator, cx)
        });
        if !self.desk_open_anywhere(agent) {
            self.reveal_desk(agent);
        }
    }

    /// What the desk shows now: its simulator, else whatever is in front of its pages.
    pub(crate) fn desk_kind(&self, agent: &str, cx: &App) -> DeskKind {
        self.desks.read(cx).kind_of(agent)
    }

    /// What the desk's switch does, as the Mac's: a kind's last tab comes to the front, the
    /// pages open a blank one when there is none, and the simulator shows its device.
    fn pick_kind(&mut self, agent: &str, kind: DeskKind, cx: &mut Context<Self>) {
        if self
            .desks
            .update(cx, |desks, cx| desks.pick_kind(agent, kind, cx))
        {
            self.new_page(agent, cx);
        }
    }

    /// Opens `path` on the agent's desk in front of its pages, its cursor at `line` and `column`
    /// counted from one, bringing the desk out if it is closed.
    pub(crate) fn open_file_on_desk(
        &mut self,
        agent: &str,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(line) = line {
            let at = crate::editor::OpenAt {
                path: path.clone(),
                line: line.saturating_sub(1),
                column: column.map(|column| column.saturating_sub(1)),
            };
            self.editors.update(cx, |editors, _| editors.reveal(at));
        }
        self.put_on_desk(agent, DeskItem::File(path), true, cx);
    }

    /// Puts `item` on the agent's desk once, in front when `front`, bringing the desk out then.
    fn put_on_desk(&mut self, agent: &str, item: DeskItem, front: bool, cx: &mut Context<Self>) {
        self.desks
            .update(cx, |desks, cx| desks.put(agent, item, front, cx));
        if front && !self.desk_open_anywhere(agent) {
            self.reveal_desk(agent);
        }
    }

    /// Takes an item off the agent's desk, asking first when that loses a file's unsaved edits.
    fn close_desk_item(&mut self, agent: &str, item: DeskItem, cx: &mut Context<Self>) {
        let closing = match &item {
            DeskItem::File(path) => vec![crate::editor::FileKey::new(self.place(), path)],
            DeskItem::Terminal { .. } | DeskItem::Hosted { .. } | DeskItem::Plugin { .. } => {
                Vec::new()
            }
        };
        let agent = agent.to_owned();
        crate::editor::close_unsaved(
            self,
            closing,
            move |bench, cx| {
                bench
                    .desks
                    .update(cx, |desks, cx| desks.take_off(&agent, &item, cx));
                bench.shut_empty_desk(&agent, cx);
            },
            cx,
        );
    }

    /// Brings to the front the desk terminal showing `session`, and says which agent's it is.
    pub(crate) fn show_desk_terminal(
        &mut self,
        session: u64,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let (agent, item) = self.desks.read(cx).items.iter().find_map(|(agent, items)| {
            items
                .iter()
                .find(|item| matches!(item, DeskItem::Terminal { session: held, .. } if *held == session))
                .map(|item| (agent.clone(), item.clone()))
        })?;
        self.put_on_desk(&agent, item, true, cx);
        Some(agent)
    }

    /// Puts the terminal of each task an agent starts on that agent's desk, as its run opens one,
    /// in place of the terminal of the task's earlier run, and takes it off once the run is stopped.
    /// Runs going when the window first reads them join their desks behind.
    pub(crate) fn place_agent_runs(&mut self, cx: &mut Context<Self>) {
        struct AgentRun {
            execution: String,
            agent: String,
            session: u64,
            task: String,
            title: String,
            stopped: bool,
        }
        let runs: Vec<AgentRun> = (self.reads.runs.iter())
            .filter_map(|listed| {
                let run = &listed.run;
                Some(AgentRun {
                    execution: run.execution_id.clone(),
                    agent: listed.agent_id.clone()?,
                    session: run.pty_id?,
                    task: listed.terminal_key.clone(),
                    title: run.label.clone().unwrap_or_else(|| run.task_id.clone()),
                    stopped: matches!(run.status, RunStatus::Stopping | RunStatus::Stopped),
                })
            })
            .collect();
        let terminal = |session| DeskItem::Terminal {
            session,
            title: String::new(),
        };
        let front = self
            .desks
            .update(cx, |desks, _| std::mem::replace(&mut desks.runs_read, true));
        let mut emptied = HashSet::new();
        for run in &runs {
            if run.stopped {
                if self
                    .desks
                    .read(cx)
                    .holds(&run.agent, &terminal(run.session))
                {
                    let item = terminal(run.session);
                    self.desks
                        .update(cx, |desks, cx| desks.take_off(&run.agent, &item, cx));
                    emptied.insert(run.agent.clone());
                }
                continue;
            }
            let placed = self.desks.update(cx, |desks, _| {
                desks.placed_runs.insert(run.execution.clone())
            });
            if !placed || self.agent_place(&run.agent).is_none() {
                continue;
            }
            let earlier = runs.iter().filter(|other| {
                other.session != run.session && other.agent == run.agent && other.task == run.task
            });
            for other in earlier {
                let item = terminal(other.session);
                self.desks
                    .update(cx, |desks, cx| desks.take_off(&run.agent, &item, cx));
            }
            let item = DeskItem::Terminal {
                session: run.session,
                title: run.title.clone(),
            };
            self.put_on_desk(&run.agent, item, front, cx);
        }
        for agent in emptied {
            self.shut_empty_desk(&agent, cx);
        }
    }

    /// Points the desks' files at `from`, or under it, to the same names under `to`.
    pub(crate) fn rename_desk_files(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.desks
            .update(cx, |desks, cx| desks.rename_files(from, to, cx));
    }

    /// The files on every desk, which their editors keep open while no tab shows them.
    pub(crate) fn desk_files<'a>(&self, cx: &'a App) -> impl Iterator<Item = &'a PathBuf> {
        self.desks.read(cx).files()
    }

    /// Turns the agent's desk to its pages, bringing the desk out if it is closed, so the
    /// person sees what the agent does there.
    fn show_pages(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.desks
            .update(cx, |desks, cx| desks.show_kind(agent, DeskKind::Pages, cx));
        if !self.desk_open_anywhere(agent) {
            self.reveal_desk(agent);
        }
    }

    /// The desk view of `agent` in `tab`, by id.
    pub fn desk_of(&self, tab: &str, agent: &str) -> Option<String> {
        let strip = self.strip()?;
        let tab = self.tabs(&strip).iter().find(|held| held.id == tab)?;
        view_of(&tab.pane, PaneKind::Desk, agent)
    }

    /// Opens the agent's desk beside its view, or closes it when it is open.
    pub fn toggle_desk(&mut self, tab: &str, view: &str, agent: &str, cx: &mut Context<Self>) {
        let Some(strip) = self.strip() else {
            return;
        };
        match self.desk_of(tab, agent) {
            Some(desk) if self.panes.desk_shutting(&desk) => {
                self.panes.slide_desk(&desk, true);
                self.parts.redraw_stage(cx);
            }
            Some(desk) => self.shut_desk(strip, tab.to_owned(), desk, cx),
            None => {
                self.split_pane(&strip, tab, view, PaneKind::Desk, agent, false);
                self.open_first_page(agent, cx);
            }
        }
    }

    /// Slides the desk back to the window's edge, then closes it unless it was opened again.
    fn shut_desk(&mut self, strip: Strip, tab: String, desk: String, cx: &mut Context<Self>) {
        if cx.reduce_motion() {
            return self.close_pane(&strip, &tab, &desk);
        }
        let lasts = self.panes.slide_desk(&desk, false);
        self.parts.redraw_stage(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lasts).await;
            let _ = this.update(cx, |bench, _| {
                if bench.panes.desk_shutting(&desk) {
                    bench.close_pane(&strip, &tab, &desk);
                }
            });
        })
        .detach();
    }

    /// The agent in the pane the person last used, else the tab's first agent, with its view.
    fn front_agent(&self) -> Option<(String, String, String)> {
        let tab = self.tab()?;
        let focused = self
            .panes
            .focused
            .as_deref()
            .and_then(|focused| crate::panes::find_view(&tab.pane, focused));
        let agent = match focused {
            Some(view) if matches!(view.kind, PaneKind::Agent | PaneKind::Desk) => {
                view.target.clone()
            }
            _ => shows(tab, PaneKind::Agent)?.clone(),
        };
        let view = view_of(&tab.pane, PaneKind::Agent, &agent)?;
        Some((tab.id.clone(), view, agent))
    }

    /// The agent in front, else the project's agent that worked last, brought to the front;
    /// with neither, the person hears that a desk belongs to an agent.
    fn nearest_agent(&self, cx: &mut Context<Self>) -> Option<(String, String, String)> {
        if let Some(front) = self.front_agent() {
            return Some(front);
        }
        let project = self.project().map(|project| project.id.clone());
        let placed = project
            .map(|project| self.agents_of(&project))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|agent| {
                Some((
                    agent.active_at,
                    agent.id.clone(),
                    self.agent_place(&agent.id)?,
                ))
            })
            .max_by_key(|(active_at, ..)| *active_at);
        let Some((_, agent, (strip, tab, view))) = placed else {
            self.notify(
                toast::info("Start an agent first — the desk and its browser belong to one"),
                cx,
            );
            return None;
        };
        self.activate_tab(&strip, &tab);
        Some((tab, view, agent))
    }

    /// The agent whose desk holds the keys.
    fn focused_desk(&self, window: &Window, cx: &App) -> Option<String> {
        let floating = (self.desks.read(cx).floating.as_ref())
            .filter(|floating| floating.input.read(cx).focus_handle(cx).is_focused(window))
            .map(|floating| floating.agent.clone());
        floating.or_else(|| {
            self.desks.read(cx).pages.iter().find_map(|(agent, page)| {
                let address = page.address.read(cx).focus_handle(cx);
                (page.focus.is_focused(window) || address.is_focused(window)).then(|| agent.clone())
            })
        })
    }

    pub fn toggle_front_desk(&mut self, cx: &mut Context<Self>) {
        if let Some((tab, view, agent)) = self.nearest_agent(cx) {
            self.toggle_desk(&tab, &view, &agent, cx);
        }
    }

    /// The nearest agent's desk, turned to its simulator.
    pub fn front_simulator(&mut self, cx: &mut Context<Self>) {
        if let Some((_, _, agent)) = self.nearest_agent(cx) {
            self.show_simulator(&agent, cx);
        }
    }

    /// A new page on the nearest agent's desk, which opens if it is closed.
    pub fn new_front_page(&mut self, cx: &mut Context<Self>) {
        let Some((tab, view, agent)) = self.nearest_agent(cx) else {
            return;
        };
        self.desks
            .update(cx, |desks, cx| desks.show_kind(&agent, DeskKind::Pages, cx));
        if let (None, Some(strip)) = (self.desk_of(&tab, &agent), self.strip()) {
            self.split_pane(&strip, &tab, &view, PaneKind::Desk, &agent, false);
        }
        self.new_page(&agent, cx);
    }

    /// The front agent, while its desk is open on its pages.
    fn front_pages(&self, cx: &App) -> Option<String> {
        let (tab, _, agent) = self.front_agent()?;
        self.desk_of(&tab, &agent)?;
        let kind = self
            .desks
            .read(cx)
            .kinds
            .get(&agent)
            .copied()
            .unwrap_or_default();
        (kind == DeskKind::Pages).then_some(agent)
    }

    /// The file the front agent's desk shows, while it is open on its pages.
    pub(crate) fn desk_file(&self, cx: &App) -> Option<PathBuf> {
        match self.desks.read(cx).front.get(&self.front_pages(cx)?)? {
            DeskItem::File(path) => Some(path.clone()),
            _ => None,
        }
    }

    /// Closes what the desk holding the keys shows, its page or its simulator, and says whether
    /// it had one to close; elsewhere, ⌘W closes the tab as it would anywhere else.
    pub fn close_shown_page(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let file_desk = self.front_agent().and_then(|(tab, _, agent)| {
            let desk = self.desk_of(&tab, &agent)?;
            let keys_in_file = matches!(self.desks.read(cx).front.get(&agent),
                Some(DeskItem::File(path)) if self.editors.read(cx).has_keys(path, window, cx));
            (keys_in_file || self.panes.focused.as_deref() == Some(desk.as_str())).then_some(agent)
        });
        if let Some(agent) = file_desk {
            if let Some(item) = self.desks.read(cx).front.get(&agent).cloned() {
                self.close_desk_item(&agent, item, cx);
                return true;
            }
        }
        let Some(agent) = self.focused_desk(window, cx) else {
            return false;
        };
        match self
            .desks
            .read(cx)
            .kinds
            .get(&agent)
            .copied()
            .unwrap_or_default()
        {
            DeskKind::Simulator => {
                self.desks
                    .update(cx, |desks, cx| desks.show_kind(&agent, DeskKind::Pages, cx));
                true
            }
            _ => {
                if let Some(linked @ (DeskItem::Hosted { .. } | DeskItem::Plugin { .. })) =
                    self.desks.read(cx).front.get(&agent).cloned()
                {
                    self.close_desk_item(&agent, linked, cx);
                    return true;
                }
                let shown = self
                    .desks
                    .read(cx)
                    .strips
                    .get(&agent)
                    .and_then(|strip| strip.active_tab_id.clone());
                match shown {
                    Some(page) => {
                        self.with_engine(cx, |engine| engine.close_tab(&agent, &page));
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// Floats the nearest desk's address over its page, all of it selected to type over,
    /// opening the desk on its pages, with a blank one when it has none.
    pub fn focus_front_address(&mut self, cx: &mut Context<Self>) {
        let Some((tab, view, agent)) = self.nearest_agent(cx) else {
            return;
        };
        self.desks
            .update(cx, |desks, cx| desks.show_kind(&agent, DeskKind::Pages, cx));
        if let (None, Some(strip)) = (self.desk_of(&tab, &agent), self.strip()) {
            self.split_pane(&strip, &tab, &view, PaneKind::Desk, &agent, false);
        }
        self.open_first_page(&agent, cx);
        self.desks.update(cx, |desks, cx| {
            desks.address_wanted = Some(agent);
            cx.notify();
        });
    }

    /// Steps through the pages of the desk holding the keys, and says whether one does.
    pub fn step_desk_page(&self, by: i32, window: &Window, cx: &App) -> bool {
        let Some(agent) = self.focused_desk(window, cx).filter(|agent| {
            self.desks
                .read(cx)
                .kinds
                .get(agent)
                .copied()
                .unwrap_or_default()
                == DeskKind::Pages
        }) else {
            return false;
        };
        let Some(strip) = self
            .desks
            .read(cx)
            .strips
            .get(&agent)
            .filter(|strip| !strip.tabs.is_empty())
        else {
            return false;
        };
        let at = strip
            .tabs
            .iter()
            .position(|tab| Some(&tab.id) == strip.active_tab_id.as_ref())
            .unwrap_or(0) as i32;
        let next = &strip.tabs[(at + by).rem_euclid(strip.tabs.len() as i32) as usize].id;
        self.with_engine(cx, |engine| engine.switch_tab(&agent, next));
        true
    }

    pub fn reload_front_page(&self, cx: &App) {
        if let Some(agent) = self.front_pages(cx) {
            self.with_engine(cx, |engine| engine.reload(&agent));
        }
    }

    /// Steps the front desk's page back or forward through its history.
    pub fn step_front_page(&self, by: i32, cx: &App) {
        if let Some(agent) = self.front_pages(cx) {
            self.with_engine(cx, |engine| engine.history(&agent, by));
        }
    }

    /// A desk opened on an agent with no pages starts on a blank one, ready for an address.
    fn open_first_page(&self, agent: &str, cx: &App) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        if self
            .desks
            .read(cx)
            .strips
            .get(agent)
            .is_some_and(|strip| !strip.tabs.is_empty())
        {
            return;
        }
        let agent = agent.to_owned();
        runtime().spawn(async move {
            if let Err(error) = engine.open_tab(&agent, None).await {
                eprintln!("sikemux: the desk could not open a page: {error}");
            }
        });
    }

    /// Opens `url` in a new page on `agent`'s desk, bringing the agent and its desk to the
    /// front. Answers false when this window has no browser, so the caller can open it elsewhere.
    pub fn open_on_desk(&mut self, agent: &str, url: &str, cx: &App) -> bool {
        if self.desks.read(cx).engine.is_none() {
            return false;
        }
        self.open_agent(agent);
        self.show_on_desk(agent, url, cx)
    }

    /// Opens a link on `agent`'s desk: the pull request it names, as the Git tab shows one,
    /// else its web page. Answers false when this window has no browser, so the caller can
    /// open it elsewhere.
    pub fn open_link_on_desk(&mut self, agent: &str, url: &str, cx: &mut Context<Self>) -> bool {
        if self.desks.read(cx).engine.is_none() {
            return false;
        }
        let held = (self.desks.read(cx).items.get(agent).into_iter().flatten())
            .find(|item| match item {
                DeskItem::Hosted { url: held, .. } | DeskItem::Plugin { url: held, .. } => {
                    held == url
                }
                _ => false,
            })
            .cloned();
        if let Some(held) = held {
            self.open_agent(agent);
            self.put_on_desk(agent, held, true, cx);
            return true;
        }
        let target = self.chat_target(agent);
        let hosted = target.request("codeHostLink", json!({ "url": url }));
        let plugin = target.request("pluginLink", json!({ "url": url }));
        let (agent, url) = (agent.to_owned(), url.to_owned());
        cx.spawn(async move |bench, cx| {
            let hosted = hosted.await.ok().and_then(|found| {
                let host = serde_json::from_value(found["codeHost"].clone()).ok()?;
                Some((host, found["item"].clone()))
            });
            let plugin = match hosted {
                Some(_) => None,
                None => plugin.await.ok().and_then(|found| {
                    Some((found["plugin"].as_str()?.to_owned(), found["at"].clone()))
                }),
            };
            bench
                .update(cx, |bench, cx| {
                    match (hosted, plugin) {
                        (Some((host, item)), _) => {
                            bench.open_hosted_on_desk(&agent, &url, target, host, &item, cx)
                        }
                        (None, Some((plugin, at))) => {
                            bench.open_plugin_on_desk(&agent, &url, target, &plugin, &at, cx)
                        }
                        (None, None) => {
                            bench.open_on_desk(&agent, &url, cx);
                        }
                    }
                    bench.parts.redraw_stage(cx);
                })
                .ok();
        })
        .detach();
        true
    }

    /// Puts the pull request, issue or run `item` names of `host`'s repository on `agent`'s
    /// desk, in front.
    fn open_hosted_on_desk(
        &mut self,
        agent: &str,
        url: &str,
        target: crate::core::Target,
        host: sikemux_wire::protocol::ProjectCodeHost,
        item: &Value,
        cx: &mut Context<Self>,
    ) {
        let project = self.agent_by_id(agent).and_then(|row| {
            self.projects()
                .iter()
                .find(|project| project.id == row.project)
                .cloned()
        });
        let bench = cx.entity().downgrade();
        let (agent_id, link) = (agent.to_owned(), url.to_owned());
        let shown = cx.new(|cx| {
            HostArea::on_desk(
                target,
                bench,
                agent_id,
                link,
                host,
                item,
                project.as_ref(),
                cx,
            )
        });
        self.open_agent(agent);
        self.put_on_desk(
            agent,
            DeskItem::Hosted {
                url: url.to_owned(),
                shown,
            },
            true,
            cx,
        );
    }

    /// Puts what `plugin`'s own view shows of `at` on `agent`'s desk, in front, or the link's
    /// web page when its view cannot show it alone.
    fn open_plugin_on_desk(
        &mut self,
        agent: &str,
        url: &str,
        target: crate::core::Target,
        plugin: &str,
        at: &Value,
        cx: &mut Context<Self>,
    ) {
        let surface = crate::plugin_pane::surface(plugin).unwrap_or_default();
        let place = format!("{plugin}:{surface}");
        let slot = format!("desk-{agent}-{url}");
        let bench = cx.entity().downgrade();
        let shown = cx.new(|_| crate::plugin_pane::PluginPane::new(&place, slot, target, bench));
        if !shown.update(cx, |shown, cx| shown.show_alone(at, cx)) {
            self.open_on_desk(agent, url, cx);
            return;
        }
        self.open_agent(agent);
        self.put_on_desk(
            agent,
            DeskItem::Plugin {
                url: url.to_owned(),
                shown,
            },
            true,
            cx,
        );
    }

    /// The plugin views the desks hold, which hear as the plugin panes in tabs do that the
    /// core read them again.
    pub(crate) fn desk_plugin_panes(
        &self,
        cx: &App,
    ) -> Vec<Entity<crate::plugin_pane::PluginPane>> {
        (self.desks.read(cx).items.values().flatten())
            .filter_map(|item| match item {
                DeskItem::Plugin { shown, .. } => Some(shown.clone()),
                _ => None,
            })
            .collect()
    }

    /// Swaps the desk's pull request, issue or run `shown` for the web page of its link, which
    /// the host could not show here.
    pub(crate) fn desk_link_fell_back(
        &mut self,
        agent: &str,
        shown: gpui_kit::EntityId,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let held = (self.desks.read(cx).items.get(agent).into_iter().flatten())
            .find(|item| matches!(item, DeskItem::Hosted { shown: held, .. } if held.entity_id() == shown))
            .cloned();
        let Some(held) = held else {
            return;
        };
        self.desks
            .update(cx, |desks, cx| desks.take_off(agent, &held, cx));
        self.show_on_desk(agent, url, cx);
    }

    /// Hands a watched read to the desks' pull requests, each taking only its own.
    pub(crate) fn read_desk_pulls(&self, key: &str, value: &Value, cx: &mut Context<Self>) {
        let shown: Vec<Entity<HostArea>> = (self.desks.read(cx).items.values().flatten())
            .filter_map(|item| match item {
                DeskItem::Hosted { shown, .. } => Some(shown.clone()),
                _ => None,
            })
            .collect();
        for shown in shown {
            shown.update(cx, |shown, cx| shown.read(key, value, cx));
        }
    }

    /// Opens `url` in a new page on `agent`'s desk, leaving the tab in front where it is.
    pub fn show_on_desk(&mut self, agent: &str, url: &str, cx: &App) -> bool {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return false;
        };
        if !self.desk_open_anywhere(agent) {
            self.reveal_desk(agent);
        }
        let (agent, url) = (agent.to_owned(), url.to_owned());
        runtime().spawn(async move {
            if let Err(error) = engine.open_tab(&agent, Some(&url)).await {
                eprintln!("sikemux: the desk could not open {url}: {error}");
            }
        });
        true
    }

    /// Opens a blank page on the agent's desk with its address floating, ready to type.
    fn new_page(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let agent = agent.to_owned();
        self.desks.update(cx, |desks, cx| {
            desks.address_wanted = Some(agent.clone());
            cx.notify();
        });
        runtime().spawn(async move {
            if let Err(error) = engine.open_tab(&agent, None).await {
                eprintln!("sikemux: the desk could not open a page: {error}");
            }
        });
    }

    /// The address of the page in front on the agent's desk.
    fn front_url<'a>(&self, agent: &str, cx: &'a App) -> Option<&'a str> {
        let strip = self.desks.read(cx).strips.get(agent)?;
        (strip.tabs.iter())
            .find(|tab| Some(&tab.id) == strip.active_tab_id.as_ref())
            .map(|tab| tab.url.as_str())
    }

    /// Opens what was typed in the agent's address, or reloads the page when it is unchanged.
    fn go_typed(&self, agent: &str, input: &str, cx: &App) {
        let Some(front) = self.front_url(agent, cx) else {
            return self.open_typed(agent, input, cx);
        };
        match input.is_empty() || shown_address(front) == input {
            true => self.with_engine(cx, |engine| engine.reload(agent)),
            false => self.with_engine(cx, |engine| engine.navigate_typed(agent, input)),
        }
    }

    /// Opens what was typed in a new page, for a desk whose pages are all closed.
    fn open_typed(&self, agent: &str, input: &str, cx: &App) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let (agent, input) = (agent.to_owned(), input.to_owned());
        runtime().spawn(async move {
            if let Err(error) = engine.open_typed(&agent, &input).await {
                eprintln!("sikemux: the desk could not open {input}: {error}");
            }
        });
    }

    fn with_engine(
        &self,
        cx: &App,
        act: impl FnOnce(&BrowserManager) -> sikemux_browser::BrowserResult<()>,
    ) {
        if let Some(engine) = &self.desks.read(cx).engine {
            if let Err(error) = act(engine) {
                eprintln!("sikemux: the desk's browser refused: {error}");
            }
        }
    }
}

pub fn install(cx: &mut App) {
    address::install(cx);
}

/// Makes the page areas and address bars of the desks the tab in front shows.
pub fn prepare(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let agents: Vec<String> = bench.tab().map_or_else(Vec::new, |tab| {
        crate::panes::views(&tab.pane)
            .filter(|view| view.kind == PaneKind::Desk)
            .map(|view| view.target.clone())
            .collect()
    });
    let simulated: Vec<&String> = agents
        .iter()
        .filter(|agent| bench.desks.read(cx).kinds.get(*agent) == Some(&DeskKind::Simulator))
        .collect();
    bench.sims.update(cx, |sims, cx| {
        sims.stop_screens_hidden(&agents);
        for agent in simulated {
            match sims.live(agent) {
                true => sims.start_screen(agent, cx),
                false => sims.pick_device_for(agent, cx),
            }
        }
    });
    let Some(engine) = bench.desks.read(cx).engine.clone() else {
        return;
    };
    for agent in agents {
        if bench.desks.read(cx).pages.contains_key(&agent) {
            continue;
        }
        if !bench.desks.read(cx).strips.contains_key(&agent) {
            if let Ok(strip) = engine.snapshot(&agent) {
                let tabs = strip.tabs.iter().map(|tab| tab.id.clone()).collect();
                bench.desks.update(cx, |desks, _| {
                    desks.known.insert(agent.clone(), tabs);
                    desks.strips.insert(agent.clone(), Arc::new(strip));
                });
            }
        }
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter address"));
        let typed = agent.clone();
        let typing = cx.subscribe_in(
            &address,
            window,
            move |bench, address, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let input = address.read(cx).value().trim().to_string();
                    bench.go_typed(&typed, &input, cx);
                    let page = bench.desks.read(cx).pages.get(&typed);
                    if let Some(focus) = page.map(|page| page.focus.clone()) {
                        focus.focus(window, cx);
                    }
                }
            },
        );
        let page = Page {
            surface: NativeSurface::new(DeskPage {
                engine: engine.clone(),
                agent: agent.clone(),
            }),
            focus: cx.focus_handle(),
            address,
            _typing: typing,
        };
        bench
            .desks
            .update(cx, |desks, _| desks.pages.insert(agent, page));
    }
    if let Some(agent) = bench.desks.read(cx).address_wanted.clone() {
        if bench.desks.read(cx).pages.contains_key(&agent) {
            bench
                .desks
                .update(cx, |desks, _| desks.address_wanted = None);
            bench.float_address(agent, window, cx);
        }
    }
    let desks = bench.desks.read(cx);
    let shown: Vec<(Entity<InputState>, String)> = desks
        .pages
        .iter()
        .filter_map(|(agent, page)| {
            let url = desks.strips.get(agent).and_then(|strip| {
                strip
                    .tabs
                    .iter()
                    .find(|tab| Some(&tab.id) == strip.active_tab_id.as_ref())
                    .map(|tab| shown_address(&tab.url))
            })?;
            let typing = page.address.read(cx).focus_handle(cx).is_focused(window);
            (!typing && page.address.read(cx).value() != url.as_str())
                .then(|| (page.address.clone(), url))
        })
        .collect();
    for (address, url) in shown {
        address.update(cx, |address, cx| address.set_value(url, window, cx));
    }
}

#[cfg(feature = "drive")]
impl Workbench {
    /// What each desk holds and where the keys are, for the test bundle's driver to check.
    pub fn drive_state(&self, window: &Window, cx: &App) -> Value {
        let desks = &self.desks.read(cx);
        let agents: std::collections::BTreeSet<&String> = (desks.strips.keys())
            .chain(desks.items.keys())
            .chain(desks.kinds.keys())
            .chain(desks.pages.keys())
            .collect();
        let item = |item: &DeskItem| match item {
            DeskItem::File(path) => json!({ "file": path }),
            DeskItem::Terminal { session, title } => {
                json!({ "terminal": title, "session": session })
            }
            DeskItem::Hosted { url, .. } => json!({ "pull": url }),
            DeskItem::Plugin { url, .. } => json!({ "plugin": url }),
        };
        let desk = |agent: &String| {
            let page = desks.pages.get(agent);
            let address = page.map(|page| page.address.read(cx));
            let floating = desks
                .floating
                .as_ref()
                .filter(|floating| floating.agent == *agent);
            let floating_focused = floating.is_some_and(|floating| {
                floating.input.read(cx).focus_handle(cx).is_focused(window)
            });
            let pages = desks.strips.get(agent).map(|strip| {
                (strip.tabs.iter())
                    .map(|tab| {
                        json!({
                            "title": tab.title,
                            "url": tab.url,
                            "active": tab.active,
                            "loading": tab.loading,
                            "back": tab.can_go_back,
                            "forward": tab.can_go_forward,
                        })
                    })
                    .collect::<Vec<_>>()
            });
            json!({
                "open": self.desk_open_anywhere(agent),
                "kind": match desks.kinds.get(agent).copied().unwrap_or_default() {
                    DeskKind::Simulator => "simulator",
                    _ => "pages",
                },
                "pages": pages,
                "items": desks.items.get(agent).map(|items| items.iter().map(item).collect::<Vec<_>>()),
                "front": desks.front.get(agent).map(item),
                "pageFocused": page.is_some_and(|page| page.focus.is_focused(window)),
                "pageBounds": page.and_then(|page| page.surface.placed()).map(|frame| {
                    let bounds = frame.visible;
                    [bounds.left(), bounds.top(), bounds.size.width, bounds.size.height].map(f32::from)
                }),
                "addressFocused": floating_focused || address.is_some_and(|address| address.focus_handle(cx).is_focused(window)),
                "addressFloating": floating.is_some(),
                "address": address.map(|address| address.value().to_string()),
            })
        };
        let files = (self.editors.read(cx).open_files(cx).into_iter())
            .map(|(path, line, dirty)| json!({ "path": path, "line": line, "dirty": dirty }))
            .collect::<Vec<_>>();
        let size = window.viewport_size();
        json!({
            "window": [f32::from(size.width), f32::from(size.height)],
            "contexts": window.context_stack().iter().map(|context| format!("{context:?}")).collect::<Vec<_>>(),
            "desks": agents.into_iter().map(|agent| (agent.clone(), desk(agent))).collect::<serde_json::Map<_, _>>(),
            "files": files,
            "spots": crate::drive::spots(),
            "rail": match self.rails.page {
                crate::workbench::RailPage::Agents => "agents",
                crate::workbench::RailPage::Terminals => "terminals",
                crate::workbench::RailPage::Files => "files",
            },
            "railBounds": self.rails.frame.get(),
            "tree": self.files.drive_state(),
            "git": self.git.pane.as_ref().map(|git| git.read(cx).drive_state()),
            "sky": crate::ui::grain::drive_state(window, cx),
        })
    }
}

/// What a page's download says: it started, failed, or finished with a way to keep the file.
fn downloaded(download: &BrowserDownload) -> Notification {
    let held = PathBuf::from(&download.path);
    let name = held.file_name().map_or_else(
        || download.url.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    match download.state {
        DownloadState::Started => toast::info(format!("Downloading {name}")),
        DownloadState::Failed => toast::error(format!("Download of {name} failed")).autohide(false),
        DownloadState::Finished => {
            toast::success(format!("Downloaded {name}")).action(move |_, _, cx| {
                let (note, held) = (cx.entity(), held.clone());
                Button::new("keep-download")
                    .label("Keep in Downloads")
                    .children(ui::spot(|| "keep-download".into()))
                    .on_click(move |_, window, cx| {
                        match keep_download(&held) {
                            Ok(kept) => cx.reveal_path(&kept),
                            Err(error) => window.push_notification(
                                toast::error(format!("The download was not kept: {error}")),
                                cx,
                            ),
                        }
                        note.update(cx, |note, cx| note.dismiss(window, cx));
                    })
            })
        }
    }
}

/// Moves a file a page saved from the browser's own folder into Downloads, beside what is
/// there. A link keeps the file as macOS marked it on download and never replaces another.
fn keep_download(held: &Path) -> Result<PathBuf, String> {
    let downloads = sikemux_process::paths::home()
        .ok_or("this account has no home folder")?
        .join("Downloads");
    let name = held
        .file_name()
        .ok_or("the download has no name")?
        .to_string_lossy()
        .into_owned();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    for n in 1..=100 {
        let kept = match n {
            1 => downloads.join(&name),
            n => downloads.join(format!("{stem} ({n}){extension}")),
        };
        match std::fs::hard_link(held, &kept) {
            Ok(()) => {
                let _ = std::fs::remove_file(held);
                return Ok(kept);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!("Downloads already has 100 files named like {name}"))
}

fn shown_address(url: &str) -> String {
    if url == sikemux_browser::BLANK_URL {
        String::new()
    } else {
        url.to_owned()
    }
}

/// WebKit paints a blank page white, so the desk leaves it off and shows its own ground.
fn idle_blank(tab: &sikemux_browser::BrowserTab) -> bool {
    tab.url == sikemux_browser::BLANK_URL && !tab.loading
}

fn view_of(pane: &Pane, kind: PaneKind, target: &str) -> Option<String> {
    crate::panes::views(pane)
        .find(|view| view.kind == kind && view.target == target)
        .map(|view| view.id.clone())
}

/// A desk's head, its strip of tabs beside the kind switch, as tall as the Mac's.
const HEAD: f32 = 34.;
/// The gap a pane keeps inside its cell, which the desk's edge keeps round the switch.
const GAP: f32 = 5.;
/// The kind switch's box: four 24pt kinds 2pt apart, 4pt inside its edge, a gap short of the head.
const SWITCH_W: f32 = 4. * 24. + 3. * 2. + 2. * 4.;
const SWITCH_H: f32 = HEAD - GAP;
/// A desk tab's width, and the narrowest it gets before the strip scrolls, as the Mac's.
const PILL_W: f32 = 180.;
const PILL_MIN: f32 = 120.;
/// The hover group of a desk tab, whose × shows while the pointer is on it.
const DESK_PILL: &str = "desk-pill";

/// The desk of `agent`.
pub fn pane(
    bench: &Workbench,
    agent: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let kind = bench.desk_kind(agent, cx);
    if kind == DeskKind::Simulator {
        return simulator_desk(bench, agent, cx);
    }
    let desks = bench.desks.read(cx);
    let strip = desks.strips.get(agent).cloned();
    let front_item = desks.front.get(agent).cloned();
    let held: Vec<DeskItem> = desks.items.get(agent).cloned().unwrap_or_default();
    let strip = strip.as_deref();
    let front_item = front_item.as_ref();
    let front = strip.and_then(|strip| {
        strip
            .tabs
            .iter()
            .find(|tab| Some(&tab.id) == strip.active_tab_id.as_ref())
    });
    let mut head = div()
        .w_full()
        .min_w_0()
        .h(px(HEAD))
        .flex_none()
        .flex()
        .items_center()
        .pl(px(SWITCH_W + GAP - 1.))
        .pr(SPACE_1H);
    let scroll = bench
        .desks
        .read(cx)
        .tab_scrolls
        .borrow_mut()
        .entry(agent.to_owned())
        .or_default()
        .0
        .clone();
    let mut pills = div()
        .id("desk-tabs")
        .flex_initial()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .overflow_x_scroll()
        .track_scroll(&scroll);
    let root = bench.agent_by_id(agent).and_then(|row| {
        bench
            .projects()
            .iter()
            .find(|project| project.id == row.project)
            .map(|project| project.path.clone())
    });
    let acting = bench
        .agent_by_id(agent)
        .map(|row| (crate::theme::brand(&row.provider), row));
    ui::forget_spots(|| format!("desk-page-{agent}-"));
    ui::forget_spots(|| format!("desk-item-{agent}-"));
    ui::forget_spots(|| format!("desk-close-{agent}-"));
    let mut front_pill = None;
    let mut pill_count = 0;
    let pages = match kind {
        DeskKind::Pages => strip.map(|strip| strip.tabs.as_slice()).unwrap_or_default(),
        _ => &[],
    };
    for page in pages {
        let label = if !page.title.is_empty() {
            page.title.clone()
        } else if page.url == sikemux_browser::BLANK_URL {
            "New tab".to_owned()
        } else {
            page.url.clone()
        };
        let shown = page.active && front_item.is_none();
        let (switch_agent, switch_tab) = (agent.to_owned(), page.id.clone());
        let (close_agent, close_tab) = (agent.to_owned(), page.id.clone());
        if shown {
            front_pill = Some(pill_count);
        }
        pill_count += 1;
        let favicon = page
            .favicon
            .as_deref()
            .and_then(|icon| bench.desks.read(cx).favicon(icon));
        let mark = match favicon {
            _ if page.loading => Spinner::new()
                .with_size(px(13.))
                .color(Tokens::get(cx).text_tertiary)
                .into_any_element(),
            Some(icon) => gpui_kit::img(icon)
                .size(px(13.))
                .flex_none()
                .into_any_element(),
            None => ui::icon(Icon::Globe, 13.).flex_none().into_any_element(),
        };
        let at_work = acting
            .as_ref()
            .filter(|_| page.acting)
            .map(|(tint, row)| (*tint, row.mark(16., cx)));
        pills = pills.child(
            pill(
                SharedString::from(format!("desk-tab-{}", page.id)),
                mark,
                label,
                shown,
                at_work,
                ui::spot(|| format!("desk-close-{agent}-{}", page.title)),
                cx.listener(move |bench, _, _, cx| {
                    cx.stop_propagation();
                    bench.with_engine(cx, |engine| engine.close_tab(&close_agent, &close_tab))
                }),
                cx,
            )
            .children(ui::spot(|| format!("desk-page-{agent}-{}", page.title)))
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.desks.update(cx, |desks, cx| {
                    desks.front.remove(&switch_agent);
                    cx.notify();
                });
                bench.with_engine(cx, |engine| engine.switch_tab(&switch_agent, &switch_tab));
            }))
            .context_menu(ui::menu(tab_menu(
                cx.weak_entity(),
                agent,
                DeskTab::Page {
                    id: page.id.clone(),
                    url: page.url.clone(),
                },
                None,
            ))),
        );
    }
    for (index, item) in held
        .iter()
        .enumerate()
        .filter(|(_, item)| item.kind() == kind)
    {
        let shown = front_item.is_some_and(|front| front.same(item));
        let (mark, label) = match item {
            DeskItem::File(path) => {
                let name = path.file_name().map_or_else(
                    || path.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                (crate::files::file_glyph(&name, cx), name)
            }
            DeskItem::Terminal { title, .. } => (
                ui::icon(Icon::Terminal, 13.)
                    .text_color(t.live)
                    .flex_none()
                    .into_any_element(),
                title.clone(),
            ),
            DeskItem::Hosted { shown, .. } => shown.read(cx).desk_face(cx),
            DeskItem::Plugin { shown, .. } => shown.read(cx).desk_face(cx),
        };
        let (pick_agent, pick_item) = (agent.to_owned(), item.clone());
        let (close_agent, close_item) = (agent.to_owned(), item.clone());
        if shown {
            front_pill = Some(pill_count);
        }
        pill_count += 1;
        pills = pills.child(
            pill(
                SharedString::from(format!("desk-item-{agent}-{index}")),
                mark,
                label.clone(),
                shown,
                None,
                ui::spot(|| format!("desk-close-{agent}-{label}")),
                cx.listener(move |bench, _, _, cx| {
                    cx.stop_propagation();
                    bench.close_desk_item(&close_agent, close_item.clone(), cx);
                    bench.parts.redraw_stage(cx);
                    bench.parts.redraw(&bench.parts.tabs, cx);
                }),
                cx,
            )
            .children(ui::spot(|| format!("desk-item-{agent}-{label}")))
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.desks.update(cx, |desks, cx| {
                    desks.bring_front(&pick_agent, pick_item.clone(), cx)
                });
                bench.parts.redraw(&bench.parts.tabs, cx);
            }))
            .context_menu(ui::menu(tab_menu(
                cx.weak_entity(),
                agent,
                DeskTab::Item(item.clone()),
                root.clone(),
            ))),
        );
    }
    let new_agent = agent.to_owned();
    if let Some((_, shown)) = bench.desks.read(cx).tab_scrolls.borrow_mut().get_mut(agent) {
        if *shown != front_pill {
            *shown = front_pill;
            if let Some(front) = front_pill {
                scroll.scroll_to_item(front);
            }
        }
    }
    head = head
        .child(
            div()
                .relative()
                .flex_initial()
                .min_w_0()
                .h_full()
                .flex()
                .pl(SPACE_1H)
                .child(pills)
                .child(crate::track::sideways_scroller())
                .children(ui::spot(|| format!("desk-tabs-{agent}")).map(|spot| {
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(6.))
                        .right_0()
                        .child(spot)
                })),
        )
        .when(kind == DeskKind::Pages, |head| {
            head.child(
                div().flex_none().pl(SPACE_1).child(
                    IconButton::new("desk-new-page", Icon::Plus, "New browser tab")
                        .glyph(13.)
                        .width(24.)
                        .height(24.)
                        .on_click(
                            cx.listener(move |bench, _, _, cx| bench.new_page(&new_agent, cx)),
                        ),
                ),
            )
        });
    let no_pages = strip.is_none_or(|strip| strip.tabs.is_empty());
    let page = bench.desks.read(cx).pages.get(agent);
    let (back, forward, reload) = (agent.to_owned(), agent.to_owned(), agent.to_owned());
    let bar = div()
        .h(px(38.))
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_0)
        .px(SPACE_1H)
        .py(SPACE_1)
        .border_b_1()
        .border_color(t.border)
        .child(
            IconButton::new("desk-back", Icon::Chevron, "Back")
                .glyph(11.)
                .turned(std::f32::consts::PI)
                .disabled(!front.is_some_and(|tab| tab.can_go_back))
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.with_engine(cx, |engine| engine.history(&back, -1))
                })),
        )
        .child(
            IconButton::new("desk-forward", Icon::Chevron, "Forward")
                .glyph(11.)
                .disabled(!front.is_some_and(|tab| tab.can_go_forward))
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.with_engine(cx, |engine| engine.history(&forward, 1))
                })),
        )
        .child(
            IconButton::new("desk-reload", Icon::Refresh, "Reload")
                .glyph(12.)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.with_engine(cx, |engine| engine.reload(&reload))
                })),
        )
        .children(page.map(|page| {
            let floating = (bench.desks.read(cx).floating.as_ref())
                .is_some_and(|floating| floating.agent == agent);
            let focused = page.address.read(cx).focus_handle(cx).is_focused(window);
            div()
                .flex_1()
                .min_w_0()
                .h(px(26.))
                .ml(SPACE_1)
                .flex()
                .items_center()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(if focused {
                    t.border_selected
                } else {
                    gpui_kit::transparent_black()
                })
                .bg(if focused {
                    t.surface_sunken.opacity(0.8)
                } else {
                    t.text_primary.opacity(0.05)
                })
                .when(!focused, |field| {
                    field.hover(move |field| field.bg(t.text_primary.opacity(0.08)))
                })
                .when(!floating, |field| {
                    field.child(Input::new(&page.address).appearance(false))
                })
        }));
    if let Some(item) = front_item {
        let shown = match item {
            DeskItem::File(path) => {
                crate::editor::pane(bench, &cx.entity(), &path.to_string_lossy(), window, cx)
            }
            DeskItem::Terminal { session, .. } => crate::terminal::pane(
                bench,
                &cx.entity(),
                &crate::hosts::Place::This,
                &session.to_string(),
                cx,
            ),
            DeskItem::Hosted { shown, .. } => {
                div().size_full().child(shown.clone()).into_any_element()
            }
            DeskItem::Plugin { shown, .. } => {
                div().size_full().child(shown.clone()).into_any_element()
            }
        };
        return frame(
            bench,
            agent,
            kind,
            div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .child(head)
                .child(div().flex_1().min_h_0().child(shown)),
            cx,
        );
    }
    let body = div()
        .flex_1()
        .min_h_0()
        .mx(px(1.))
        .mb(px(1.))
        .relative()
        .key_context("BrowserPage")
        .children(page.filter(|_| !no_pages).map(|page| {
            if front.is_some_and(idle_blank) {
                div()
                    .track_focus(&page.focus)
                    .size_full()
                    .into_any_element()
            } else {
                native_view(
                    SharedString::from(format!("desk-page-{agent}")),
                    &page.surface,
                )
                .track_focus(&page.focus)
                .size_full()
                .into_any_element()
            }
        }))
        .when(no_pages, |body| {
            body.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(t.text_tertiary)
                    .child(ui::text("No pages yet", TEXT_XS_H, FontWeight::NORMAL)),
            )
        })
        .children(address::overlay(bench, agent, cx));
    frame(
        bench,
        agent,
        kind,
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(head)
            .when(!no_pages, |content| content.child(bar))
            .child(body),
        cx,
    )
}

/// A desk's own frame, as the Mac draws it: a solid ground with no grain, the kind switch in
/// its top corner, and an edge that steps round the switch a pane's gap clear of it.
fn frame(
    bench: &Workbench,
    agent: &str,
    kind: DeskKind,
    content: impl IntoElement,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (edge, ground) = (t.border, crate::see_through::ground(t.surface, cx));
    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .rounded(RADIUS_3)
        .child(
            gpui_kit::canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    if let Ok(path) = stepped_edge(bounds, gpui_kit::PathBuilder::fill()).build() {
                        window.paint_path(path, ground);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(content)
        .child(
            gpui_kit::canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    let stroke = gpui_kit::PathBuilder::stroke(px(1.));
                    if let Ok(path) = stepped_edge(bounds, stroke).build() {
                        window.paint_path(path, edge);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(SWITCH_W))
                .h(px(SWITCH_H))
                .p(SPACE_1)
                .rounded(RADIUS_3)
                .border_1()
                .border_color(t.border)
                .bg(ground)
                .child(kinds(bench, agent, kind, cx)),
        )
        .into_any_element()
}

/// The desk's edge: a rounded box whose top-left corner steps in round the kind switch, as the
/// Mac's cutCornerOutline draws it, on the half pixel so the line is sharp.
fn stepped_edge(
    bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    mut path: gpui_kit::PathBuilder,
) -> gpui_kit::PathBuilder {
    let (radius, inner) = (f32::from(RADIUS_3), f32::from(RADIUS_3) + GAP);
    let at = |x: f32, y: f32| gpui_kit::point(bounds.origin.x + px(x), bounds.origin.y + px(y));
    let (left, top) = (0.5, 0.5);
    let (right, bottom) = (
        f32::from(bounds.size.width) - 0.5,
        f32::from(bounds.size.height) - 0.5,
    );
    let (step_x, step_y) = (SWITCH_W + GAP + 0.5, SWITCH_H + GAP + 0.5);
    let arc = |path: &mut gpui_kit::PathBuilder, r: f32, sweep: bool, x: f32, y: f32| {
        path.arc_to(
            gpui_kit::point(px(r), px(r)),
            px(0.),
            false,
            sweep,
            at(x, y),
        );
    };
    path.move_to(at(left, step_y + radius));
    arc(&mut path, radius, true, left + radius, step_y);
    path.line_to(at(step_x - inner, step_y));
    arc(&mut path, inner, false, step_x, step_y - inner);
    path.line_to(at(step_x, top + radius));
    arc(&mut path, radius, true, step_x + radius, top);
    path.line_to(at(right - radius, top));
    arc(&mut path, radius, true, right, top + radius);
    path.line_to(at(right, bottom - radius));
    arc(&mut path, radius, true, right - radius, bottom);
    path.line_to(at(left + radius, bottom));
    arc(&mut path, radius, true, left, bottom - radius);
    path.close();
    path
}

/// A tab of a desk's strip, as the Mac draws one: its mark and name on a faint fill, lifted
/// while shown, ringed in the agent's colour while it works there, its × showing on hover.
#[allow(clippy::too_many_arguments)]
fn pill(
    id: SharedString,
    mark: AnyElement,
    label: String,
    shown: bool,
    at_work: Option<(Hsla, AnyElement)>,
    close_spot: Option<AnyElement>,
    close: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    let close_id = SharedString::from(format!("{id}-close"));
    let rest = t.text_primary.opacity(0.05);
    let tinted = at_work.as_ref().map(|(tint, _)| *tint);
    div()
        .id(id)
        .group(DESK_PILL)
        .flex_initial()
        .w(px(PILL_W))
        .min_w(px(PILL_MIN))
        .relative()
        .h(px(24.))
        .pl(SPACE_2)
        .pr(SPACE_2)
        .group_hover(DESK_PILL, |pill| pill.pr(SPACE_6))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(RADIUS_2)
        .bg(match (shown, tinted) {
            (true, _) => t.surface_chosen,
            (false, Some(tint)) => tint.opacity(0.14),
            (false, None) => rest,
        })
        .when_some(tinted, |pill, tint| {
            pill.border_1().border_color(tint.opacity(0.7))
        })
        .when(!shown, |pill| {
            pill.hover(move |pill| pill.bg(t.surface_hover))
        })
        .text_color(if shown || tinted.is_some() {
            t.text_primary
        } else {
            t.text_secondary
        })
        .child(mark)
        .child(
            ui::text(label, TEXT_XS, FontWeight::NORMAL)
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        )
        .children(at_work.map(|(_, mark)| ui::gives_way(DESK_PILL, mark)))
        .child(
            ui::hover_actions(DESK_PILL)
                .right(px(2.))
                .top_0()
                .bottom_0()
                .child(
                    div()
                        .relative()
                        .child(ui::close_button(close_id, "Close tab", close, cx))
                        .children(close_spot),
                ),
        )
}

/// The desk's switch between its pages and its simulator.
/// A tab of a desk's strip, as its menu needs it.
enum DeskTab {
    Page { id: String, url: String },
    Item(DeskItem),
}

/// What right-clicking a desk's tab offers, as the Mac's desk tab menu does: a file's paths
/// and Reveal in Finder, a page's link, then Close.
fn tab_menu(
    bench: WeakEntity<Workbench>,
    agent: &str,
    tab: DeskTab,
    root: Option<PathBuf>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let agent = agent.to_owned();
    move |popup, _, _| {
        let copy = |text: String, said: &'static str| {
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                window.push_notification(toast::success(said), cx);
            }
        };
        let close = {
            let (bench, agent) = (bench.clone(), agent.clone());
            let tab = match &tab {
                DeskTab::Page { id, .. } => Err(id.clone()),
                DeskTab::Item(item) => Ok(item.clone()),
            };
            PopupMenuItem::new("Close")
                .icon(Icon::Close)
                .on_click(move |_, _, cx| {
                    let _ = bench.update(cx, |bench, cx| {
                        match &tab {
                            Err(page) => {
                                bench.with_engine(cx, |engine| engine.close_tab(&agent, page))
                            }
                            Ok(item) => bench.close_desk_item(&agent, item.clone(), cx),
                        }
                        bench.parts.redraw_stage(cx);
                    });
                })
        };
        match &tab {
            DeskTab::Item(DeskItem::File(path)) => {
                let relative = root
                    .as_deref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .filter(|inside| !inside.as_os_str().is_empty())
                    .or_else(|| path.file_name().map(Path::new))
                    .map_or_else(String::new, |shown| shown.to_string_lossy().into_owned());
                let revealed = path.clone();
                popup
                    .item(
                        PopupMenuItem::new("Copy Path")
                            .icon(Icon::Copy)
                            .on_click(copy(path.to_string_lossy().into_owned(), "Copied path")),
                    )
                    .item(
                        PopupMenuItem::new("Copy Relative Path")
                            .icon(Icon::Copy)
                            .on_click(copy(relative, "Copied relative path")),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(ui::REVEAL)
                            .icon(Icon::Folder)
                            .on_click(move |_, _, cx| cx.reveal_path(&revealed)),
                    )
                    .separator()
                    .item(close)
            }
            DeskTab::Page { url, .. }
            | DeskTab::Item(DeskItem::Hosted { url, .. })
            | DeskTab::Item(DeskItem::Plugin { url, .. }) => popup
                .item(
                    PopupMenuItem::new("Copy Link")
                        .icon(Icon::Copy)
                        .disabled(url.is_empty() || url == sikemux_browser::BLANK_URL)
                        .on_click(copy(url.clone(), "Copied link")),
                )
                .separator()
                .item(close),
            DeskTab::Item(DeskItem::Terminal { .. }) => popup.item(close),
        }
    }
}

/// The desk's switch between what it holds, as the Mac's: its pages, its files, its task
/// terminals and its simulator. A kind with nothing in it is greyed, but for the pages and the
/// simulator, which open one; one the agent is at work in, and not shown, takes its colour.
fn kinds(
    bench: &Workbench,
    agent: &str,
    shown: DeskKind,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let tint = bench
        .agent_by_id(agent)
        .map(|row| crate::theme::brand(&row.provider));
    let pages = bench
        .desks
        .read(cx)
        .strips
        .get(agent)
        .map_or(&[][..], |strip| strip.tabs.as_slice());
    let items = bench
        .desks
        .read(cx)
        .items
        .get(agent)
        .map_or(&[][..], Vec::as_slice);
    let held = |kind: DeskKind| items.iter().filter(|item| item.kind() == kind).count();
    let mut row = div().flex().items_center().gap(SPACE_0);
    for (id, icon, label, kind, count, busy) in [
        (
            "desk-pages",
            Icon::Globe,
            "Browser",
            DeskKind::Pages,
            pages.len() + held(DeskKind::Pages),
            pages.iter().any(|page| page.acting),
        ),
        (
            "desk-files",
            Icon::Editor,
            "Files",
            DeskKind::Files,
            held(DeskKind::Files),
            false,
        ),
        (
            "desk-terminals",
            Icon::Terminal,
            "Terminals",
            DeskKind::Terminals,
            held(DeskKind::Terminals),
            false,
        ),
        (
            "desk-simulator",
            Icon::Phone,
            "iOS Simulator",
            DeskKind::Simulator,
            0,
            false,
        ),
    ]
    .into_iter()
    .filter(|(_, _, _, kind, _, _)| cfg!(target_os = "macos") || *kind != DeskKind::Simulator)
    {
        let named = match count {
            0 => label.to_owned(),
            count => format!("{label} · {count}"),
        };
        let empty = count == 0 && matches!(kind, DeskKind::Files | DeskKind::Terminals);
        let spot = format!("desk-kind-{agent}-{id}");
        let agent = agent.to_owned();
        row = row.child(
            div().relative().children(ui::spot(|| spot)).child(
                IconButton::new(id, icon, named)
                    .glyph(13.)
                    .width(24.)
                    .height(21.)
                    .pressed(shown == kind)
                    .disabled(empty)
                    .when_some(tint.filter(|_| busy && shown != kind), |button, tint| {
                        button.tint(tint)
                    })
                    .on_click(
                        cx.listener(move |bench, _, _, cx| bench.pick_kind(&agent, kind, cx)),
                    ),
            ),
        );
    }
    row
}

fn simulator_desk(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let head = div()
        .h(px(HEAD))
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .pl(px(SWITCH_W + GAP - 1. + 6.))
        .pr(SPACE_1H)
        .children(crate::sim::pane::picker(bench, agent, cx))
        .when(bench.sims.read(cx).acting(agent), |head| {
            head.child(ui::icon(Icon::Pointer, 11.).text_color(t.accent))
        })
        .child(div().flex_1())
        .child(crate::sim::pane::tools(bench, agent, cx));
    frame(
        bench,
        agent,
        DeskKind::Simulator,
        div().size_full().flex().flex_col().child(head).child(
            div()
                .flex_1()
                .min_h_0()
                .child(crate::sim::pane::body(bench, agent, cx)),
        ),
        cx,
    )
}

impl Workbench {
    /// Answers a call the core hands this window: an agent's browser tools run on the
    /// engine, on a thread of their own, since most answers wait on a page.
    pub fn answer_window_call(
        &mut self,
        call_id: CallId,
        call: WindowCall,
        cx: &mut Context<Self>,
    ) {
        let asks = self.asks.clone();
        let answer: crate::opening::Answer = Box::new(move |answer: Result<Value, String>| {
            let _ = asks.send(Ask::AnswerWindow(call_id, WindowAnswer::from(answer)));
        });
        let request = match call {
            WindowCall::Open { request } => {
                return self.open_from_cli(call_id, request, answer, cx)
            }
            WindowCall::OpenPlugin { plugin, at } => {
                self.bring_window_forward(cx);
                self.open_plugin_at(&plugin, at, cx);
                return answer(Ok(Value::Null));
            }
            WindowCall::Harness { request } => request,
        };
        let request: HarnessRequest = match serde_json::from_value(request) {
            Ok(request) => request,
            Err(error) => {
                return answer(Err(format!(
                    "Sikemux could not read the tool call: {error}"
                )))
            }
        };
        match request.method.as_str() {
            "app.console" => return answer(crate::console::read(&request.params)),
            "page.show" => return self.show_page(request, answer, cx),
            "ui.open" => return self.ui_open(request, answer, cx),
            _ => {}
        }
        let run: Box<dyn FnOnce() -> Result<Value, String> + Send> =
            if is_sim_method(&request.method) {
                let engine = Arc::clone(self.sims.read(cx).engine());
                Box::new(move || sikemux_simulator::tools::execute(&engine, &request))
            } else if let Some(engine) = self
                .desks
                .read(cx)
                .engine
                .clone()
                .filter(|_| is_browser_method(&request.method))
            {
                Box::new(move || sikemux_browser::tools::execute(&engine, &request))
            } else {
                return answer(Err(format!(
                    "this window cannot answer {} yet",
                    request.method
                )));
            };
        let spawned = std::thread::Builder::new()
            .name("sikemux-window-call".into())
            .spawn(move || answer(run()));
        if let Err(error) = spawned {
            eprintln!("sikemux: a tool call could not start: {error}");
        }
    }

    /// A page an agent kept, which an agent in a terminal sees on its desk.
    fn show_page(
        &self,
        request: HarnessRequest,
        answer: impl FnOnce(Result<Value, String>) + Send + 'static,
        cx: &App,
    ) {
        let Some(agent) = request.agent_id else {
            return answer(Ok(json!({})));
        };
        let id = request.params["id"].as_str().unwrap_or_default().to_owned();
        if !sikemux_core::pages::is_page_id(&id) {
            return answer(Err("that is not a page the core keeps".into()));
        }
        let page = (request.params["desk"] == true)
            .then(|| crate::core::request(&self.asks, "page", json!({ "id": id })));
        let files = self.desks.read(cx).page_files.clone();
        spawn(async move {
            let Some(page) = page else {
                return answer(Ok(json!({})));
            };
            let shown = async {
                let page = page.await?;
                let html = page["html"].as_str().ok_or("that page is gone")?;
                let url = files.url(&id, html)?;
                files
                    .engine()?
                    .open_tab(&agent, Some(&url))
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(json!({}))
            };
            answer(shown.await);
        });
    }
}
