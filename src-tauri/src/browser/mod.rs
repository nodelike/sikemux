//! The browser engine as the Tauri app hosts it: the window its tabs go in, the
//! folders it keeps things in, the events the React pane listens for, and the
//! commands the pane calls. The engine itself is `sikemux-browser`.

use std::path::PathBuf;
use std::sync::Arc;

use sikemux_browser::{
    BrowserBounds, BrowserDownload, BrowserShortcut, BrowserSnapshot, ParentView,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};

#[cfg(target_os = "macos")]
pub(crate) use sikemux_browser::macos;
pub use sikemux_browser::{history, tools, SavedStrip};

/// The engine's tab manager, as the app keeps it in its state.
pub type BrowserManager = Arc<sikemux_browser::BrowserManager>;

pub const BROWSER_TABS_EVENT: &str = "browser-tabs-changed";
pub const BROWSER_SHORTCUT_EVENT: &str = "browser-shortcut";
pub const BROWSER_DOWNLOAD_EVENT: &str = "browser-download";
pub const BROWSER_ACTING_EVENT: &str = "browser-agent-acting";

/// The engine for this app's main window.
pub fn engine(app: &AppHandle) -> BrowserManager {
    sikemux_browser::BrowserManager::new(Arc::new(AppHost { app: app.clone() }))
}

struct AppHost {
    app: AppHandle,
}

impl sikemux_browser::Host for AppHost {
    fn window(&self) -> Result<ParentView, String> {
        let window = self
            .app
            .get_window("main")
            .ok_or("main window is not open")?;
        #[cfg(target_os = "macos")]
        {
            let view = window.ns_view().map_err(|error| error.to_string())?;
            Ok(ParentView::on_top(view))
        }
        #[cfg(target_os = "linux")]
        {
            use gtk::glib::translate::ToGlibPtr;
            let container = window.default_vbox().map_err(|error| error.to_string())?;
            let pointer: *mut gtk::ffi::GtkBox = container.to_glib_none().0;
            Ok(ParentView::on_top(pointer.cast()))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = window;
            Err("browser tabs need macOS or Linux".into())
        }
    }

    fn state_dir(&self) -> Option<PathBuf> {
        browser_state_dir(&self.app).ok()
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        self.app.path().app_cache_dir().ok()
    }

    fn videos_dir(&self) -> Option<PathBuf> {
        self.app.path().video_dir().ok()
    }

    fn page_url(&self, agent_id: &str, html: &str) -> Result<String, String> {
        crate::pages::preview_url(&self.app, agent_id, html)
    }

    fn tabs_changed(&self) {
        let _ = self.app.emit(BROWSER_TABS_EVENT, ());
    }

    fn downloaded(&self, download: &BrowserDownload) {
        let _ = self.app.emit(BROWSER_DOWNLOAD_EVENT, download);
    }

    fn acting(&self, agent_id: &str) {
        let _ = self.app.emit(BROWSER_ACTING_EVENT, agent_id);
    }

    fn shortcut(&self, shortcut: BrowserShortcut) {
        let _ = self.app.emit(BROWSER_SHORTCUT_EVENT, shortcut);
    }
}

impl From<sikemux_browser::Error> for AppError {
    fn from(error: sikemux_browser::Error) -> Self {
        match error {
            sikemux_browser::Error::BadArg(message) => AppError::BadArg(message),
            sikemux_browser::Error::Window(message) => AppError::Window(message),
            sikemux_browser::Error::Other(message) => AppError::Other(message),
        }
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

#[tauri::command]
pub async fn browser_snapshot(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<BrowserSnapshot> {
    Ok(manager.snapshot(&agent_id)?)
}

#[tauri::command]
pub async fn browser_new_tab(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: Option<String>,
) -> AppResult<String> {
    Ok(manager.open_tab(&agent_id, url.as_deref()).await?)
}

#[tauri::command]
pub async fn browser_close_agent(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    Ok(manager.close_agent(&agent_id)?)
}

#[tauri::command]
pub async fn browser_switch_tab(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    tab_id: String,
) -> AppResult<()> {
    Ok(manager.switch_tab(&agent_id, &tab_id)?)
}

#[tauri::command]
pub async fn browser_close_tab(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    tab_id: String,
) -> AppResult<()> {
    Ok(manager.close_tab(&agent_id, &tab_id)?)
}

#[tauri::command]
pub async fn browser_navigate(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: String,
) -> AppResult<()> {
    Ok(manager.navigate_typed(&agent_id, &url)?)
}

#[tauri::command]
pub async fn browser_suggest(
    manager: State<'_, BrowserManager>,
    query: String,
) -> AppResult<history::AddressSuggestions> {
    Ok(manager.suggest(&query))
}

#[tauri::command]
pub async fn browser_back(manager: State<'_, BrowserManager>, agent_id: String) -> AppResult<()> {
    Ok(manager.history(&agent_id, -1)?)
}

#[tauri::command]
pub async fn browser_forward(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    Ok(manager.history(&agent_id, 1)?)
}

#[tauri::command]
pub async fn browser_reload(manager: State<'_, BrowserManager>, agent_id: String) -> AppResult<()> {
    Ok(manager.reload(&agent_id)?)
}

#[tauri::command]
pub async fn browser_set_bounds(
    manager: State<'_, BrowserManager>,
    agent_id: String,
    bounds: Option<BrowserBounds>,
) -> AppResult<()> {
    Ok(manager.set_bounds(&agent_id, bounds)?)
}

/// A picture of the agent's page as it stands, which the app slides in the
/// page's place while the stage moves: a native view cannot keep step with it.
#[tauri::command]
pub async fn browser_page_still(
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(
        manager.page_still(&agent_id).await?,
    ))
}
