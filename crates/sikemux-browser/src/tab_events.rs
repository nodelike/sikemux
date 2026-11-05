//! What a tab's page tells the engine as it runs.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::time::Instant;

use sikemux_process::sync::Locked;
use url::Url;

use crate::manager::{tab_may_load, BrowserManager};
use crate::strip::TabPage;
use crate::{
    burst, favicon, DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall,
    BLANK_URL,
};

/// The tab whose page opened another as its window, and what the engine makes that one from.
pub(crate) struct OpenedBy {
    pub(crate) tab_id: String,
    pub(crate) opener: Option<Opener>,
}

/// What one tab's page tells the manager.
pub(crate) struct TabListener {
    pub(crate) manager: Weak<BrowserManager>,
    pub(crate) agent_id: String,
    pub(crate) tab_id: String,
    /// The tab whose page opened this one as its window, which alone may close it.
    pub(crate) opened_from: Option<String>,
    pub(crate) popups: Mutex<burst::Burst>,
    pub(crate) downloads: Mutex<burst::Burst>,
}

impl TabListener {
    pub(crate) fn note(&self, update: impl FnOnce(&mut TabPage)) {
        if let Some(manager) = self.manager.upgrade() {
            manager.note_page(&self.agent_id, &self.tab_id, update);
        }
    }
}

impl TabEvents for TabListener {
    fn may_load(&self, url: &str) -> bool {
        Url::parse(url).is_ok_and(|url| tab_may_load(&url))
    }

    fn loaded(&self, load: PageLoad) {
        let PageLoad {
            started,
            url,
            can_go_back,
            can_go_forward,
        } = load;
        self.note(|page| {
            if !favicon::same_site(&page.url, &url) {
                page.favicon = None;
            }
            page.loading = started;
            page.url = url;
            page.can_go_back = can_go_back;
            page.can_go_forward = can_go_forward;
        });
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        if !started {
            let view = manager
                .agents
                .locked()
                .get(&self.agent_id)
                .and_then(|agent| agent.views.get(&self.tab_id).cloned());
            if let Some(view) = view {
                let (agent, tab) = (self.agent_id.clone(), self.tab_id.clone());
                crate::spawn(async move { favicon::refresh(&manager, agent, tab, view).await });
            }
        }
    }

    fn titled(&self, title: String) {
        self.note(|page| page.title = title);
    }

    fn moved(&self, url: String, can_go_back: bool, can_go_forward: bool) {
        self.note(|page| {
            page.url = url;
            page.can_go_back = can_go_back;
            page.can_go_forward = can_go_forward;
        });
    }

    fn new_window(&self, url: String, opener: Option<Opener>) -> Option<Tab> {
        if !self.popups.locked().admit(Instant::now()) {
            return None;
        }
        let url = if url.is_empty() {
            BLANK_URL.to_owned()
        } else {
            url
        };
        let opened_by = OpenedBy {
            tab_id: self.tab_id.clone(),
            opener,
        };
        let (_, tab) = self
            .manager
            .upgrade()?
            .add_tab(&self.agent_id, &url, Some(opened_by))
            .ok()?;
        Some(tab)
    }

    fn closed(&self) {
        let (Some(manager), Some(opened_from)) = (self.manager.upgrade(), self.opened_from.clone())
        else {
            return;
        };
        let (agent, tab) = (self.agent_id.clone(), self.tab_id.clone());
        crate::spawn(async move { manager.close_window(&agent, &tab, &opened_from) });
    }

    fn download_started(&self, url: String, suggested: &Path) -> Option<PathBuf> {
        if !self.downloads.locked().admit(Instant::now()) {
            return None;
        }
        self.manager
            .upgrade()?
            .download_started(&self.agent_id, &self.tab_id, &url, suggested)
    }

    fn download_finished(&self, url: String, saved: bool) {
        if let Some(manager) = self.manager.upgrade() {
            manager.download_finished(&self.agent_id, &self.tab_id, &url, saved);
        }
    }

    fn dialog(&self, dialog: Option<PageDialog>) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        manager
            .tabs
            .locked()
            .entry(self.tab_id.clone())
            .or_default()
            .dialog = dialog;
    }

    fn upload(&self) -> Option<Vec<PathBuf>> {
        self.manager.upgrade()?.take_upload(&self.tab_id)
    }

    fn document(&self, event: DocumentEvent) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let failed = manager
            .tabs
            .locked()
            .entry(self.tab_id.clone())
            .or_default()
            .documents
            .note(event);
        if failed {
            self.note(|page| page.loading = false);
        }
    }

    fn health(&self, stall: Option<TabStall>) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        manager
            .tabs
            .locked()
            .entry(self.tab_id.clone())
            .or_default()
            .stall = stall;
    }
}
