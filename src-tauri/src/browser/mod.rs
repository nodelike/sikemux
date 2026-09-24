//! Each agent owns a strip of browser tabs. A tab is a native child webview of
//! the main window (WKWebView on macOS), so pages get real input, real pixels,
//! and the system cookie jar. The React pane only draws the chrome around it
//! and tells this module where the page area is.
//!
//! Once a window has a child webview, Tauri stops treating it as a "webview
//! window": `get_webview_window("main")` returns `None` and commands taking a
//! `WebviewWindow` fail. The app reaches the main window with `get_window`.

pub mod agents;
mod favicon;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod recording;
pub mod tools;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Rect, Size, State, Url,
    Webview, WebviewUrl,
};

use crate::error::{AppError, AppResult};

pub const BROWSER_TABS_EVENT: &str = "browser-tabs-changed";
pub const BROWSER_SHORTCUT_EVENT: &str = "browser-shortcut";
pub const BROWSER_DOWNLOAD_EVENT: &str = "browser-download";
pub const BROWSER_ACTING_EVENT: &str = "browser-agent-acting";
pub const BLANK_URL: &str = "about:blank";
/// Injected into every document before its own scripts, so a page's calls are
/// already recorded by the time an agent asks about them.
const RECORDER_SCRIPT: &str = include_str!("recorder.js");
const PAGE_DIALOGS_SCRIPT: &str = include_str!("page-dialogs.js");
const MAX_URL_LEN: usize = 8192;
const PARKED_BOUNDS: BrowserBounds = BrowserBounds {
    x: 0.0,
    y: 0.0,
    width: 1200.0,
    height: 800.0,
};

const ACTING_LINGER: Duration = Duration::from_secs(3);

/// WebKit's own agent string names no browser at all, and sites answer that
/// with an "unsupported browser" page, so tabs — and the fetch that goes after
/// their icons — introduce themselves as Safari.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The site's own icon, inline, since the window can only draw `data:`.
    pub favicon: Option<String>,
    /// The agent's browser tools are working in this tab right now.
    pub acting: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub tabs: Vec<BrowserTab>,
    pub active_tab_id: Option<String>,
}

/// Where the page area sits, in the main window's CSS pixels.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A command chord pressed while the page had keyboard focus. The app's own
/// keymap decides what it means.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserShortcut {
    pub agent_id: String,
    pub tab_id: String,
    pub key: String,
    pub code: String,
    pub shift: bool,
    pub alt: bool,
}

/// A file a page handed to the download folder, announced when it starts and
/// again when it ends. macOS never reports the saved path back, so the path
/// chosen at the start is the one carried through.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownload {
    pub agent_id: String,
    pub tab_id: String,
    pub url: String,
    pub path: String,
    pub state: DownloadState,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Started,
    Finished,
    Failed,
}

/// An alert, confirm or prompt the page is blocked on until someone answers.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageDialog {
    pub kind: &'static str,
    pub message: String,
    pub default_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabPage {
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub favicon: Option<String>,
}

/// Which tab is shown and in what order the strip lists them. Kept apart from
/// the webviews so the bookkeeping can be tested without a window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabStrip {
    pub order: Vec<String>,
    pub active: Option<String>,
    pub pages: HashMap<String, TabPage>,
    /// Tabs the agent is working in, each with the mark that put it there, so
    /// only the latest mark may take it away again.
    pub acting: HashMap<String, u64>,
}

impl TabStrip {
    pub fn insert(&mut self, id: String, page: TabPage) {
        self.order.push(id.clone());
        self.pages.insert(id.clone(), page);
        self.active = Some(id);
    }

    /// Closing the shown tab hands the spot to its right neighbour, or the
    /// left one at the end of the strip.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.order.iter().position(|entry| entry == id) else {
            return false;
        };
        self.order.remove(index);
        self.pages.remove(id);
        self.acting.remove(id);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .order
                .get(index)
                .or_else(|| self.order.get(index.wrapping_sub(1)))
                .cloned();
        }
        true
    }

    pub fn activate(&mut self, id: &str) -> bool {
        if !self.pages.contains_key(id) {
            return false;
        }
        self.active = Some(id.to_owned());
        true
    }

    pub fn mark_acting(&mut self, mark: u64) -> Option<String> {
        let tab = self.active.clone()?;
        self.acting.insert(tab.clone(), mark);
        Some(tab)
    }

    pub fn release_acting(&mut self, tab: &str, mark: u64) -> bool {
        if self.acting.get(tab) != Some(&mark) {
            return false;
        }
        self.acting.remove(tab);
        true
    }

    pub fn snapshot(&self) -> BrowserSnapshot {
        BrowserSnapshot {
            tabs: self
                .order
                .iter()
                .filter_map(|id| {
                    let page = self.pages.get(id)?;
                    Some(BrowserTab {
                        id: id.clone(),
                        title: page.title.clone(),
                        url: page.url.clone(),
                        active: self.active.as_deref() == Some(id),
                        loading: page.loading,
                        can_go_back: page.can_go_back,
                        can_go_forward: page.can_go_forward,
                        favicon: page.favicon.clone(),
                        acting: self.acting.contains_key(id),
                    })
                })
                .collect(),
            active_tab_id: self.active.clone(),
        }
    }
}

#[derive(Default)]
struct AgentBrowser {
    strip: TabStrip,
    views: HashMap<String, Webview>,
    bounds: Option<BrowserBounds>,
}

#[derive(Default)]
pub struct BrowserManager {
    agents: Mutex<HashMap<String, AgentBrowser>>,
    next_tab: AtomicU64,
    next_acting_mark: AtomicU64,
    shortcuts_installed: AtomicBool,
    downloads: Mutex<HashMap<(String, String), PathBuf>>,
    icons: Mutex<favicon::IconCache>,
    dialogs: Mutex<HashMap<String, PageDialog>>,
    uploads: Mutex<HashMap<String, Vec<PathBuf>>>,
    #[cfg(target_os = "macos")]
    recordings: Mutex<HashMap<String, recording::Session>>,
}

impl BrowserManager {
    pub fn snapshot(&self, agent_id: &str) -> AppResult<BrowserSnapshot> {
        validate_agent_id(agent_id)?;
        Ok(self
            .lock()
            .get(agent_id)
            .map(|agent| agent.strip.snapshot())
            .unwrap_or(BrowserSnapshot {
                tabs: Vec::new(),
                active_tab_id: None,
            }))
    }

    pub async fn open_tab(
        &self,
        app: &AppHandle,
        agent_id: &str,
        url: Option<&str>,
    ) -> AppResult<String> {
        validate_agent_id(agent_id)?;
        let url = normalize_url(url.unwrap_or_default());
        validate_url(&url)?;
        let parsed = Url::parse(&url).map_err(|_| AppError::BadArg("invalid browser url"))?;
        let window = app
            .get_window("main")
            .ok_or_else(|| AppError::Window("main window is not open".into()))?;
        let tab_id = format!(
            "browser-{}-{}",
            label_safe(agent_id),
            self.next_tab.fetch_add(1, Ordering::AcqRel)
        );
        // A parked tab keeps this frame, so a page the agent drives before the
        // pane shows it still lays out like a desktop window, not a 1px slit.
        let bounds = self
            .lock()
            .get(agent_id)
            .and_then(|agent| agent.bounds)
            .unwrap_or(PARKED_BOUNDS);

        let builder = self.tab_builder(app, agent_id, &tab_id, parsed);
        let webview = window
            .add_child(
                builder,
                LogicalPosition::new(bounds.x, bounds.y),
                LogicalSize::new(bounds.width, bounds.height),
            )
            .map_err(window_error)?;
        let _ = webview.hide();
        #[cfg(target_os = "macos")]
        {
            let (app_handle, agent, tab) = (app.clone(), agent_id.to_owned(), tab_id.clone());
            let _ = webview.with_webview(move |platform| {
                let (moved_agent, moved_tab) = (agent.clone(), tab.clone());
                let (dialog_app, dialog_tab) = (app_handle.clone(), tab.clone());
                let (upload_app, upload_tab) = (app_handle.clone(), tab.clone());
                macos::adopt(
                    platform.inner(),
                    agent,
                    tab,
                    move |url, back, forward| {
                        let manager = app_handle.state::<BrowserManager>();
                        manager.note_page(&app_handle, &moved_agent, &moved_tab, |page| {
                            page.url = url;
                            page.can_go_back = back;
                            page.can_go_forward = forward;
                        });
                    },
                    move |dialog| {
                        let manager = dialog_app.state::<BrowserManager>();
                        let mut dialogs = manager.dialogs_lock();
                        match dialog {
                            Some(dialog) => dialogs.insert(dialog_tab.clone(), dialog),
                            None => dialogs.remove(&dialog_tab),
                        };
                    },
                    move || {
                        let manager = upload_app.state::<BrowserManager>();
                        manager.take_upload(&upload_tab)
                    },
                );
            });
        }
        self.install_shortcuts(app);
        {
            let mut agents = self.lock();
            let agent = agents.entry(agent_id.to_owned()).or_default();
            agent.strip.insert(
                tab_id.clone(),
                TabPage {
                    url: url.clone(),
                    loading: url != BLANK_URL,
                    ..TabPage::default()
                },
            );
            agent.views.insert(tab_id.clone(), webview);
        }
        self.relayout(agent_id);
        self.announce(app);
        Ok(tab_id)
    }

    fn tab_builder(
        &self,
        app: &AppHandle,
        agent_id: &str,
        tab_id: &str,
        url: Url,
    ) -> WebviewBuilder<tauri::Wry> {
        let builder = WebviewBuilder::new(tab_id, WebviewUrl::External(url))
            .accept_first_mouse(true)
            .focused(false)
            .zoom_hotkeys_enabled(true)
            .initialization_script(RECORDER_SCRIPT)
            .initialization_script(PAGE_DIALOGS_SCRIPT);
        #[cfg(target_os = "macos")]
        let builder = builder.user_agent(USER_AGENT);

        let (load_app, load_agent, load_tab) =
            (app.clone(), agent_id.to_owned(), tab_id.to_owned());
        let (title_app, title_agent, title_tab) =
            (app.clone(), agent_id.to_owned(), tab_id.to_owned());
        let (popup_app, popup_agent) = (app.clone(), agent_id.to_owned());
        let (download_app, download_agent, download_tab) =
            (app.clone(), agent_id.to_owned(), tab_id.to_owned());
        builder
            .on_download(move |_, event| {
                let manager = download_app.state::<BrowserManager>();
                manager.note_download(&download_app, &download_agent, &download_tab, event);
                true
            })
            .on_page_load(move |webview, payload| {
                let loading = payload.event() == PageLoadEvent::Started;
                let url = payload.url().to_string();
                let history = history_state(&webview);
                let manager = load_app.state::<BrowserManager>();
                manager.note_page(&load_app, &load_agent, &load_tab, |page| {
                    if !favicon::same_site(&page.url, &url) {
                        page.favicon = None;
                    }
                    page.loading = loading;
                    page.url = url;
                    if let Some((back, forward)) = history {
                        page.can_go_back = back;
                        page.can_go_forward = forward;
                    }
                });
                if !loading {
                    let (app, agent, tab) =
                        (load_app.clone(), load_agent.clone(), load_tab.clone());
                    tauri::async_runtime::spawn(favicon::refresh(app, agent, tab, webview));
                }
            })
            .on_document_title_changed(move |_, title| {
                let manager = title_app.state::<BrowserManager>();
                manager.note_page(&title_app, &title_agent, &title_tab, |page| {
                    page.title = title;
                });
            })
            .on_new_window(move |url, _| {
                let (app, agent) = (popup_app.clone(), popup_agent.clone());
                tauri::async_runtime::spawn(async move {
                    let manager = app.state::<BrowserManager>();
                    let _ = manager.open_tab(&app, &agent, Some(url.as_str())).await;
                });
                NewWindowResponse::Deny
            })
    }

    fn note_download(
        &self,
        app: &AppHandle,
        agent_id: &str,
        tab_id: &str,
        event: DownloadEvent<'_>,
    ) {
        let key = |url: &Url| (tab_id.to_owned(), url.to_string());
        let (url, path, state) = match event {
            DownloadEvent::Requested { url, destination } => {
                let folder = app
                    .path()
                    .download_dir()
                    .unwrap_or_else(|_| std::env::temp_dir());
                let path = unique_download_path(&folder, &download_file_name(&url, destination));
                *destination = path.clone();
                self.downloads_lock().insert(key(&url), path.clone());
                (url, path, DownloadState::Started)
            }
            DownloadEvent::Finished { url, path, success } => {
                let chosen = self.downloads_lock().remove(&key(&url));
                let path = path.or(chosen).unwrap_or_default();
                (
                    url,
                    path,
                    if success {
                        DownloadState::Finished
                    } else {
                        DownloadState::Failed
                    },
                )
            }
            _ => return,
        };
        let _ = app.emit(
            BROWSER_DOWNLOAD_EVENT,
            BrowserDownload {
                agent_id: agent_id.to_owned(),
                tab_id: tab_id.to_owned(),
                url: url.to_string(),
                path: path.to_string_lossy().into_owned(),
                state,
            },
        );
    }

    fn dialogs_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PageDialog>> {
        self.dialogs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn dialog(&self, tab_id: &str) -> Option<PageDialog> {
        self.dialogs_lock().get(tab_id).cloned()
    }

    fn uploads_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<PathBuf>>> {
        self.uploads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Files for the tab's next file chooser, answered in place of the person.
    pub fn offer_upload(&self, tab_id: &str, paths: Vec<PathBuf>) {
        self.uploads_lock().insert(tab_id.to_owned(), paths);
    }

    #[cfg(target_os = "macos")]
    fn recordings_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, recording::Session>> {
        self.recordings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn upload_pending(&self, tab_id: &str) -> bool {
        self.uploads_lock().contains_key(tab_id)
    }

    /// `None` once the chooser took the files or the offer was withdrawn.
    pub fn take_upload(&self, tab_id: &str) -> Option<Vec<PathBuf>> {
        self.uploads_lock().remove(tab_id)
    }

    fn downloads_lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), PathBuf>> {
        self.downloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn note_page(
        &self,
        app: &AppHandle,
        agent_id: &str,
        tab_id: &str,
        update: impl FnOnce(&mut TabPage),
    ) {
        let changed = {
            let mut agents = self.lock();
            match agents
                .get_mut(agent_id)
                .and_then(|agent| agent.strip.pages.get_mut(tab_id))
            {
                Some(page) => {
                    let before = page.clone();
                    update(page);
                    *page != before
                }
                None => false,
            }
        };
        if changed {
            self.announce(app);
        }
    }

    pub fn close_tab(&self, app: &AppHandle, agent_id: &str, tab_id: &str) -> AppResult<()> {
        validate_agent_id(agent_id)?;
        let view = {
            let mut agents = self.lock();
            let agent = agents
                .get_mut(agent_id)
                .ok_or(AppError::BadArg("unknown browser tab"))?;
            if !agent.strip.remove(tab_id) {
                return Err(AppError::BadArg("unknown browser tab"));
            }
            let view = agent.views.remove(tab_id);
            if agent.strip.order.is_empty() {
                agents.remove(agent_id);
            }
            view
        };
        if let Some(view) = view {
            drop_view(view);
        }
        self.relayout(agent_id);
        self.announce(app);
        Ok(())
    }

    pub fn close_agent(&self, app: &AppHandle, agent_id: &str) -> AppResult<()> {
        validate_agent_id(agent_id)?;
        #[cfg(target_os = "macos")]
        self.recordings_lock().remove(agent_id);
        let views = self
            .lock()
            .remove(agent_id)
            .map(|agent| agent.views.into_values().collect::<Vec<_>>())
            .unwrap_or_default();
        for view in views {
            drop_view(view);
        }
        self.announce(app);
        Ok(())
    }

    pub fn switch_tab(&self, app: &AppHandle, agent_id: &str, tab_id: &str) -> AppResult<()> {
        validate_agent_id(agent_id)?;
        let switched = self
            .lock()
            .get_mut(agent_id)
            .is_some_and(|agent| agent.strip.activate(tab_id));
        if !switched {
            return Err(AppError::BadArg("unknown browser tab"));
        }
        self.relayout(agent_id);
        self.announce(app);
        Ok(())
    }

    /// `None` parks every tab of the agent off screen: the pane is hidden,
    /// showing its blank page, or an app overlay needs to paint over it.
    pub fn set_bounds(&self, agent_id: &str, bounds: Option<BrowserBounds>) -> AppResult<()> {
        validate_agent_id(agent_id)?;
        if let Some(bounds) = bounds {
            validate_bounds(&bounds)?;
        }
        {
            let mut agents = self.lock();
            match agents.get_mut(agent_id) {
                Some(agent) => agent.bounds = bounds,
                None if bounds.is_some() => {
                    agents.entry(agent_id.to_owned()).or_default().bounds = bounds;
                }
                None => return Ok(()),
            }
        }
        self.relayout(agent_id);
        Ok(())
    }

    pub fn navigate(&self, app: &AppHandle, agent_id: &str, url: &str) -> AppResult<()> {
        let url = normalize_url(url);
        validate_url(&url)?;
        let parsed = Url::parse(&url).map_err(|_| AppError::BadArg("invalid browser url"))?;
        let (tab_id, view) = self.active_view(agent_id)?;
        view.navigate(parsed).map_err(window_error)?;
        self.note_page(app, agent_id, &tab_id, |page| {
            page.url = url;
            page.loading = true;
        });
        Ok(())
    }

    pub fn reload(&self, agent_id: &str) -> AppResult<()> {
        let (_, view) = self.active_view(agent_id)?;
        view.reload().map_err(window_error)
    }

    pub fn history(&self, agent_id: &str, delta: i32) -> AppResult<()> {
        let (_, view) = self.active_view(agent_id)?;
        #[cfg(target_os = "macos")]
        {
            view.with_webview(move |platform| macos::history(platform.inner(), delta))
                .map_err(window_error)
        }
        #[cfg(not(target_os = "macos"))]
        {
            view.eval(if delta < 0 {
                "history.back()"
            } else {
                "history.forward()"
            })
            .map_err(window_error)
        }
    }

    pub fn active_view(&self, agent_id: &str) -> AppResult<(String, Webview)> {
        validate_agent_id(agent_id)?;
        self.lock()
            .get(agent_id)
            .and_then(|agent| {
                let id = agent.strip.active.clone()?;
                let view = agent.views.get(&id)?.clone();
                Some((id, view))
            })
            .ok_or(AppError::BadArg("this agent has no open browser tab"))
    }

    /// Show the active tab inside the pane's page area and park the rest.
    fn relayout(&self, agent_id: &str) {
        let plan: Vec<(Webview, Option<BrowserBounds>)> = {
            let agents = self.lock();
            let Some(agent) = agents.get(agent_id) else {
                return;
            };
            agent
                .views
                .iter()
                .map(|(id, view)| {
                    let shown = agent.strip.active.as_deref() == Some(id.as_str());
                    (view.clone(), agent.bounds.filter(|_| shown))
                })
                .collect()
        };
        for (view, bounds) in plan {
            match bounds {
                Some(bounds) => {
                    let _ = view.set_bounds(Rect {
                        position: Position::Logical(LogicalPosition::new(bounds.x, bounds.y)),
                        size: Size::Logical(LogicalSize::new(bounds.width, bounds.height)),
                    });
                    let _ = view.show();
                }
                None => {
                    let _ = view.hide();
                }
            }
        }
    }

    /// Tells the app which agent took the wheel, so its browser can come on
    /// screen even before it has a tab.
    pub fn announce_acting(&self, app: &AppHandle, agent_id: &str) {
        let _ = app.emit(BROWSER_ACTING_EVENT, agent_id);
    }

    /// Marks the tab the agent's tools are on, so the strip can show it working there.
    pub fn mark_acting(&self, app: &AppHandle, agent_id: &str) -> Option<(String, u64)> {
        let mark = self.next_acting_mark.fetch_add(1, Ordering::AcqRel);
        let tab = self.lock().get_mut(agent_id)?.strip.mark_acting(mark)?;
        self.announce(app);
        Some((tab, mark))
    }

    /// The highlight lingers a moment after the last action, so a run of quick
    /// tool calls reads as one stretch of work rather than a flicker.
    pub fn release_acting(&self, app: &AppHandle, agent_id: &str, marks: Vec<(String, u64)>) {
        if marks.is_empty() {
            return;
        }
        let app = app.clone();
        let agent_id = agent_id.to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(ACTING_LINGER).await;
            let manager = app.state::<BrowserManager>();
            let mut released = false;
            if let Some(agent) = manager.lock().get_mut(&agent_id) {
                for (tab, mark) in &marks {
                    released |= agent.strip.release_acting(tab, *mark);
                }
            }
            if released {
                manager.announce(&app);
            }
        });
    }

    fn announce(&self, app: &AppHandle) {
        let _ = app.emit(BROWSER_TABS_EVENT, ());
    }

    fn install_shortcuts(&self, app: &AppHandle) {
        if self.shortcuts_installed.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let app = app.clone();
            let _ = app
                .clone()
                .run_on_main_thread(move || macos::install_shortcuts(app));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app;
        }
    }

    pub fn drain(&self) {
        let views: Vec<Webview> = self
            .lock()
            .drain()
            .flat_map(|(_, agent)| agent.views.into_values())
            .collect();
        for view in views {
            drop_view(view);
        }
    }

    pub fn mcp_launch(&self, app: &AppHandle) -> AppResult<BrowserMcpLaunch> {
        if let Some(command) = std::env::var_os("SIKEMUX_TOOLS_MCP_EXECUTABLE") {
            return Ok(BrowserMcpLaunch {
                command: std::path::PathBuf::from(command)
                    .to_string_lossy()
                    .into_owned(),
                args: Vec::new(),
            });
        }
        let executable_name = if cfg!(windows) {
            "sikemux-tools-mcp.exe"
        } else {
            "sikemux-tools-mcp"
        };
        if let Ok(current) = std::env::current_exe() {
            if let Some(parent) = current.parent() {
                let bundled = parent.join(executable_name);
                if bundled.is_file() {
                    return Ok(BrowserMcpLaunch {
                        command: bundled.to_string_lossy().into_owned(),
                        args: Vec::new(),
                    });
                }
            }
        }
        if let Ok(resource_dir) = app.path().resource_dir() {
            let bundled = resource_dir.join(executable_name);
            if bundled.is_file() {
                return Ok(BrowserMcpLaunch {
                    command: bundled.to_string_lossy().into_owned(),
                    args: Vec::new(),
                });
            }
        }
        Err(AppError::Other(
            "browser MCP sidecar is missing; build it with node scripts/build-cli-sidecar.mjs"
                .into(),
        ))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, AgentBrowser>> {
        self.agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Debug)]
pub struct BrowserMcpLaunch {
    pub command: String,
    pub args: Vec<String>,
}

fn drop_view(view: Webview) {
    #[cfg(target_os = "macos")]
    {
        let label = view.label().to_owned();
        let _ = view.with_webview(move |_| macos::forget(&label));
    }
    let _ = view.close();
}

/// Back/forward availability straight from the page's own history. `None`
/// where the platform gives no answer, so the last known state stands.
fn history_state(webview: &Webview) -> Option<(bool, bool)> {
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        webview
            .with_webview(move |platform| {
                let _ = sender.send(macos::history_state(platform.inner()));
            })
            .ok()?;
        receiver.try_recv().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = webview;
        None
    }
}

/// Where Sikemux keeps the files an agent host needs to find the browser: MCP
/// configs and the private home copies.
pub(crate) fn browser_state_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("browser data directory unavailable: {error}")))?
        .join("browser"))
}

fn window_error(error: tauri::Error) -> AppError {
    AppError::Window(error.to_string())
}

pub(crate) fn validate_agent_id(agent_id: &str) -> AppResult<()> {
    if agent_id.is_empty()
        || agent_id.len() > 128
        || !agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
    {
        return Err(AppError::BadArg("invalid browser agent id"));
    }
    Ok(())
}

fn validate_url(url: &str) -> AppResult<()> {
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return Err(AppError::BadArg("browser url is empty or too long"));
    }
    let scheme = url
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        scheme.as_str(),
        "javascript" | "data" | "tauri" | "asset" | "ipc"
    ) {
        return Err(AppError::BadArg("browser url scheme is not allowed"));
    }
    Ok(())
}

fn validate_bounds(bounds: &BrowserBounds) -> AppResult<()> {
    let finite = [bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite() && value.abs() < 1.0e6);
    if !finite || bounds.width < 1.0 || bounds.height < 1.0 {
        return Err(AppError::BadArg("invalid browser bounds"));
    }
    Ok(())
}

/// The name the page suggested, else the last path segment of the URL, with
/// anything that could leave the download folder stripped out.
fn download_file_name(url: &Url, suggested: &Path) -> String {
    let candidate = suggested
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_owned))
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_default();
    let cleaned: String = candidate
        .chars()
        .map(|char| {
            if matches!(char, '/' | '\\' | ':') {
                '_'
            } else {
                char
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned.chars().take(200).collect()
    }
}

/// `name`, `name (2)`, `name (3)`... whichever does not exist yet, keeping
/// the extension at the end.
fn unique_download_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .expect("some numbered name is free")
}

fn label_safe(agent_id: &str) -> String {
    agent_id
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char
            } else {
                '-'
            }
        })
        .collect()
}

pub fn normalize_url(input: &str) -> String {
    let value = input.trim();
    if value.is_empty() {
        return BLANK_URL.into();
    }
    if value == BLANK_URL || value.contains("://") {
        return value.to_owned();
    }
    if value.starts_with("localhost") || value.starts_with("127.0.0.1") || value.contains('.') {
        return format!(
            "http{}://{value}",
            if value.starts_with("localhost") || value.starts_with("127.0.0.1") {
                ""
            } else {
                "s"
            }
        );
    }
    let query = url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    format!("https://www.google.com/search?q={query}")
}

#[tauri::command]
pub async fn browser_snapshot(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<BrowserSnapshot> {
    manager.snapshot(&agent_id)
}

#[tauri::command]
pub async fn browser_new_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: Option<String>,
) -> AppResult<String> {
    manager.open_tab(&app, &agent_id, url.as_deref()).await
}

#[tauri::command]
pub async fn browser_close_agent(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    manager.close_agent(&app, &agent_id)
}

#[tauri::command]
pub async fn browser_switch_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    tab_id: String,
) -> AppResult<()> {
    manager.switch_tab(&app, &agent_id, &tab_id)
}

#[tauri::command]
pub async fn browser_close_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    tab_id: String,
) -> AppResult<()> {
    manager.close_tab(&app, &agent_id, &tab_id)
}

#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: String,
) -> AppResult<()> {
    manager.navigate(&app, &agent_id, &url)
}

#[tauri::command]
pub async fn browser_back(manager: State<'_, BrowserManager>, agent_id: String) -> AppResult<()> {
    manager.history(&agent_id, -1)
}

#[tauri::command]
pub async fn browser_forward(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    manager.history(&agent_id, 1)
}

#[tauri::command]
pub async fn browser_reload(manager: State<'_, BrowserManager>, agent_id: String) -> AppResult<()> {
    manager.reload(&agent_id)
}

#[tauri::command]
pub async fn browser_set_bounds(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    bounds: Option<BrowserBounds>,
) -> AppResult<()> {
    manager.set_bounds(&agent_id, bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str) -> TabPage {
        TabPage {
            url: url.into(),
            ..TabPage::default()
        }
    }

    #[test]
    fn the_tab_the_agent_works_in_is_marked_until_its_latest_mark_is_released() {
        let mut strip = TabStrip::default();
        strip.insert("a".into(), page("https://a.test"));

        assert_eq!(strip.mark_acting(1).as_deref(), Some("a"));
        assert_eq!(strip.mark_acting(2).as_deref(), Some("a"));
        assert!(!strip.release_acting("a", 1));
        assert!(strip.snapshot().tabs[0].acting);

        assert!(strip.release_acting("a", 2));
        assert!(!strip.snapshot().tabs[0].acting);
    }

    #[test]
    fn nothing_is_marked_without_a_tab_and_a_closed_tab_drops_its_mark() {
        let mut strip = TabStrip::default();
        assert_eq!(strip.mark_acting(1), None);

        strip.insert("a".into(), page("https://a.test"));
        strip.mark_acting(2);
        strip.remove("a");

        assert!(strip.acting.is_empty());
    }

    #[test]
    fn a_new_tab_takes_the_active_spot() {
        let mut strip = TabStrip::default();
        strip.insert("a".into(), page("https://a"));
        strip.insert("b".into(), page("https://b"));
        assert_eq!(strip.active.as_deref(), Some("b"));
        assert_eq!(strip.order, vec!["a", "b"]);
    }

    #[test]
    fn closing_the_shown_tab_moves_right_then_left() {
        let mut strip = TabStrip::default();
        for id in ["a", "b", "c"] {
            strip.insert(id.into(), page(id));
        }
        assert!(strip.activate("b"));
        assert!(strip.remove("b"));
        assert_eq!(strip.active.as_deref(), Some("c"));
        assert!(strip.remove("c"));
        assert_eq!(strip.active.as_deref(), Some("a"));
        assert!(strip.remove("a"));
        assert_eq!(strip.active, None);
        assert!(!strip.remove("a"));
    }

    #[test]
    fn closing_another_tab_keeps_the_shown_one() {
        let mut strip = TabStrip::default();
        for id in ["a", "b", "c"] {
            strip.insert(id.into(), page(id));
        }
        assert!(strip.activate("a"));
        assert!(strip.remove("c"));
        assert_eq!(strip.active.as_deref(), Some("a"));
        assert!(!strip.activate("zzz"));
    }

    #[test]
    fn snapshot_lists_tabs_in_strip_order_with_the_active_flag() {
        let mut strip = TabStrip::default();
        strip.insert("a".into(), page("https://a"));
        strip.insert("b".into(), page("https://b"));
        strip.activate("a");
        let snapshot = strip.snapshot();
        assert_eq!(snapshot.active_tab_id.as_deref(), Some("a"));
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|tab| (tab.id.as_str(), tab.active))
                .collect::<Vec<_>>(),
            vec![("a", true), ("b", false)]
        );
    }

    #[test]
    fn normalizes_addresses_and_searches() {
        assert_eq!(normalize_url("  "), BLANK_URL);
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(normalize_url("http://a.b"), "http://a.b");
        assert_eq!(
            normalize_url("rust lifetimes"),
            "https://www.google.com/search?q=rust+lifetimes"
        );
    }

    #[test]
    fn script_bearing_and_app_schemes_never_load_in_a_tab() {
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("DATA:text/html,hi").is_err());
        assert!(validate_url("tauri://localhost").is_err());
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url(&"x".repeat(MAX_URL_LEN + 1)).is_err());
    }

    #[test]
    fn download_names_come_from_the_page_then_the_url_and_stay_in_the_folder() {
        let url = Url::parse("https://a.test/files/report.pdf?x=1").unwrap();
        assert_eq!(
            download_file_name(&url, Path::new("Quarterly.pdf")),
            "Quarterly.pdf"
        );
        assert_eq!(download_file_name(&url, Path::new("")), "report.pdf");
        assert_eq!(
            download_file_name(&url, Path::new("../../etc/passwd")),
            "passwd"
        );
        assert_eq!(download_file_name(&url, Path::new(".hidden")), "hidden");
        let bare = Url::parse("https://a.test/").unwrap();
        assert_eq!(download_file_name(&bare, Path::new("")), "download");
    }

    #[test]
    fn a_taken_download_name_gets_a_number_before_its_extension() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "a.pdf"),
            folder.path().join("a.pdf")
        );
        std::fs::write(folder.path().join("a.pdf"), b"x").unwrap();
        std::fs::write(folder.path().join("a (2).pdf"), b"x").unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "a.pdf"),
            folder.path().join("a (3).pdf")
        );
        std::fs::write(folder.path().join("notes"), b"x").unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "notes"),
            folder.path().join("notes (2)")
        );
    }

    #[test]
    fn browser_agent_ids_and_bounds_are_bounded() {
        assert!(validate_agent_id("agent-1:ok_x").is_ok());
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("../x").is_err());
        assert_eq!(label_safe("agent:1/x"), "agent-1-x");
        let good = BrowserBounds {
            x: 10.0,
            y: 20.0,
            width: 300.0,
            height: 200.0,
        };
        assert!(validate_bounds(&good).is_ok());
        assert!(validate_bounds(&BrowserBounds { width: 0.0, ..good }).is_err());
        assert!(validate_bounds(&BrowserBounds {
            x: f64::NAN,
            ..good
        })
        .is_err());
    }
}
