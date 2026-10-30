//! The browser's engine: what a tab is and does, shared by every app that shows
//! tabs and by the browser service, with no UI toolkit in it.

pub mod address;
pub mod burst;
mod discard;
pub mod documents;
pub mod downloads;
mod favicon;
mod handover;
pub mod history;
mod host;
#[cfg(target_os = "macos")]
pub mod input;
mod keys;
mod layout;
#[cfg(target_os = "linux")]
mod linux;
pub mod local_files;
#[cfg(target_os = "macos")]
pub mod macos;
mod manager;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod page_view;
mod person;
mod preview;
mod recording;
mod tab;
pub mod tools;
pub mod viewport;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use documents::DocumentEvent;
pub use handover::SavedStrip;
pub use host::{BrowserResult, Error, Host};
#[cfg(target_os = "linux")]
pub use linux::choose_page_rendering;
#[cfg(target_os = "linux")]
pub use linux::page::give_keyboard_back;
pub use manager::{BrowserManager, TabPage};
pub use tab::{Opener, ParentView, Tab, TabSpec};

pub(crate) use sikemux_base::runtime::spawn;

pub const BLANK_URL: &str = "about:blank";
/// Past this many dialogs in `DIALOG_WINDOW`, a page's dialogs are answered
/// with Cancel unseen, the way browsers offer to stop a page's dialogs.
pub(crate) const DIALOG_LIMIT: usize = 3;
pub(crate) const DIALOG_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
/// Parked pages sit outside the window, since a hidden view still takes file
/// drops over the spot it last covered. They are hidden too once they are idle.
pub const PARKED_ORIGIN: f64 = -100_000.0;
/// Injected into every document before its own scripts, so a page's calls are
/// already recorded by the time an agent asks about them.
pub const RECORDER_SCRIPT: &str = include_str!("recorder.js");
/// The agents' helpers, run in a world of their own beside the page.
pub const PAGE_SCRIPT: &str = include_str!("page.js");
pub const RECORDS_SCRIPT: &str = include_str!("records.js");

/// WebKit's own agent string names no browser at all, and sites answer that
/// with an "unsupported browser" page, so tabs — and the fetch that goes after
/// their icons — introduce themselves as Safari.
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";
pub const MOBILE_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1";

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
    /// The file on this computer the page shows, when it is one.
    pub local_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub tabs: Vec<BrowserTab>,
    pub active_tab_id: Option<String>,
}

/// Where the page area sits, in the main window's CSS pixels. The clips are
/// how much of either side lies outside the stage and must not be drawn. The
/// holes are app elements, like toasts, that must show through the page. The
/// dim is how dark a shade to lay over the page while an app panel floats on it.
/// The opacity follows the pane's own while it fades, which CSS cannot reach.
/// The bottom radius rounds the page's lower corners to the pane's edge.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub clip_left: f64,
    pub clip_right: f64,
    pub holes: Vec<BrowserHole>,
    #[serde(default)]
    pub dim: f64,
    #[serde(default = "opaque")]
    pub opacity: f64,
    #[serde(default)]
    pub bottom_radius: f64,
}

fn opaque() -> f64 {
    1.0
}

/// A rounded rectangle in the page's own coordinates.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHole {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
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

/// A file a page saved, announced when it starts and again when it ends. macOS never reports the saved path back, so the path
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

/// Why a tab's page cannot answer at all, so a call into it would only time out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabStall {
    /// WebKit gave up waiting on the page's process, which is hung or blocked
    /// on something like a system dialog.
    Unresponsive,
    /// The page's process ended, leaving the tab blank until it loads again.
    Crashed,
}

impl TabStall {
    pub fn message(self) -> &'static str {
        match self {
            TabStall::Unresponsive => {
                "the tab stopped responding; close it with browser_close_tab and open a new one"
            }
            TabStall::Crashed => {
                "the tab's page crashed; navigate or reload to load it again, or close the tab"
            }
        }
    }
}

/// An alert, confirm or prompt the page is blocked on until someone answers.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageDialog {
    pub kind: &'static str,
    pub message: String,
    pub default_text: Option<String>,
}

/// A page finished starting or loading, and where its history can go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLoad {
    pub started: bool,
    pub url: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// What a tab's page tells whoever holds the tab. Every call comes on the main
/// thread, while WebKit waits for the answer where there is one.
pub trait TabEvents: Send + Sync + 'static {
    /// Whether the page may go to `url`.
    fn may_load(&self, url: &str) -> bool;
    fn loaded(&self, load: PageLoad);
    fn titled(&self, title: String);
    /// The page changed its address without loading a new document.
    fn moved(&self, url: String, can_go_back: bool, can_go_forward: bool);
    /// The page opened a window at `url`. It becomes a new tab, made from `opener` where
    /// the engine gives one, and that tab is handed back; None blocks the window.
    fn new_window(&self, url: String, opener: Option<Opener>) -> Option<Tab>;
    /// The page closed its own window.
    fn closed(&self);
    /// Where a file the page saves goes, or None to refuse it. `suggested` is
    /// the name the page or the server gave it.
    fn download_started(&self, url: String, suggested: &Path) -> Option<PathBuf>;
    fn download_finished(&self, url: String, saved: bool);
    fn dialog(&self, dialog: Option<PageDialog>);
    /// Files an agent picked for the page's next file chooser.
    fn upload(&self) -> Option<Vec<PathBuf>>;
    fn document(&self, event: DocumentEvent);
    fn health(&self, stall: Option<TabStall>);
}
