//! What the engine needs from the app that hosts it: the window tabs go in,
//! where to keep things, and someone to tell when tabs change.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{BrowserDownload, BrowserShortcut, ParentView};

pub trait Host: Send + Sync + 'static {
    /// The view of the window tabs are added to.
    fn window(&self) -> Result<ParentView, String>;
    /// Where the browser keeps its history and the strips saved over a restart.
    fn state_dir(&self) -> Option<PathBuf>;
    /// The app's cache, where files pages save wait until the person keeps them.
    fn cache_dir(&self) -> Option<PathBuf>;
    /// Where a recording goes when the agent names no path.
    fn videos_dir(&self) -> Option<PathBuf>;
    /// An address a tab can load the page an agent wrote as `html` from.
    fn page_url(&self, agent_id: &str, html: &str) -> Result<String, String>;
    /// An agent's strip changed: its tabs, their pages or which one is shown.
    fn tabs_changed(&self);
    fn downloaded(&self, download: &BrowserDownload);
    /// An agent's tools took the wheel, so its browser can come on screen.
    fn acting(&self, agent_id: &str);
    /// An app chord was pressed while a page had the keyboard.
    fn shortcut(&self, shortcut: BrowserShortcut);
}

/// The host, or nowhere at all for a manager made without one.
#[derive(Clone)]
pub(crate) struct HostRef(pub(crate) Arc<dyn Host>);

impl Default for HostRef {
    fn default() -> Self {
        HostRef(Arc::new(NoHost))
    }
}

struct NoHost;

impl Host for NoHost {
    fn window(&self) -> Result<ParentView, String> {
        Err("no window can take a browser tab".into())
    }
    fn state_dir(&self) -> Option<PathBuf> {
        None
    }
    fn cache_dir(&self) -> Option<PathBuf> {
        None
    }
    fn videos_dir(&self) -> Option<PathBuf> {
        None
    }
    fn page_url(&self, _agent_id: &str, _html: &str) -> Result<String, String> {
        Err("pages cannot be shown here".into())
    }
    fn tabs_changed(&self) {}
    fn downloaded(&self, _download: &BrowserDownload) {}
    fn acting(&self, _agent_id: &str) {}
    fn shortcut(&self, _shortcut: BrowserShortcut) {}
}

/// Why a browser call failed, in the same kinds the app reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadArg(&'static str),
    Window(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadArg(message) => write!(f, "invalid argument: {message}"),
            Error::Window(message) => write!(f, "window: {message}"),
            Error::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type BrowserResult<T> = Result<T, Error>;
