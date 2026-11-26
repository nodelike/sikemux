//! What a tab's page tells the engine as it runs.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use sikemux_loopback::{DONE, PORT};
use sikemux_process::sync::Locked;
use url::Url;

use crate::manager::{tab_may_load, BrowserManager};
use crate::strip::TabPage;
use crate::{
    burst, favicon, DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall,
    BLANK_URL,
};

/// How long the page of a sign-in that worked is left up to be seen before its tab closes.
const SIGNED_IN_SHOWN: Duration = Duration::from_secs(2);
const ACCOUNT_HOST: &str = "app.sikemux.com";

/// The page of a sign-in to a service that worked, once its script has moved it there.
fn is_signed_in(url: &str) -> bool {
    Url::parse(url)
        .is_ok_and(|url| is_this_computer(&url) && url.port() == Some(PORT) && url.path() == DONE)
}

/// A page Sikemux itself shows while signing in, served from this computer or the account's site.
fn is_sign_in_page(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        is_this_computer(&url) || (url.scheme() == "https" && url.host_str() == Some(ACCOUNT_HOST))
    })
}

fn is_this_computer(url: &Url) -> bool {
    url.scheme() == "http" && url.host_str().is_some_and(crate::address::is_loopback_host)
}

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

    /// Closes the tab `after` this long, if its page is then still one `at` accepts.
    fn close_if(&self, at: fn(&str) -> bool, after: Duration) {
        let (manager, agent) = (self.manager.clone(), self.agent_id.clone());
        let tab = self.tab_id.clone();
        crate::spawn(async move {
            tokio::time::sleep(after).await;
            let Some(manager) = manager.upgrade() else {
                return;
            };
            if manager
                .page_address(&agent, &tab)
                .is_some_and(|url| at(&url))
            {
                let _ = manager.close_tab(&agent, &tab);
            }
        });
    }
}

impl TabEvents for TabListener {
    fn may_load(&self, url: &str) -> bool {
        if url.trim_end_matches('/') == "sikemux:" {
            self.close_if(is_sign_in_page, Duration::ZERO);
            return false;
        }
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
        let (view, focus_lent) = manager
            .agents
            .locked()
            .get(&self.agent_id)
            .map(|agent| {
                let view = agent.views.get(&self.tab_id).cloned();
                (view, agent.focus_lent.contains(&self.tab_id))
            })
            .unwrap_or_default();
        if let Some(view) = view.clone().filter(|_| focus_lent) {
            crate::manager::lend_page_focus(view, true);
        }
        if !started {
            if let Some(agent) = manager.agents.locked().get_mut(&self.agent_id) {
                agent.cleared.take_if(|(tab, _)| *tab == self.tab_id);
            }
            manager.clear_foot(&self.agent_id);
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
        if is_signed_in(&url) {
            self.close_if(is_signed_in, SIGNED_IN_SHOWN);
        }
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
        let manager = self.manager.upgrade()?;
        let fixed = manager.viewport(&self.agent_id, &self.tab_id);
        let (_, tab) = manager
            .add_tab(&self.agent_id, &url, Some(opened_by), fixed)
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
        {
            let mut tabs = manager.tabs.locked();
            let facts = tabs.entry(self.tab_id.clone()).or_default();
            if dialog.is_none() {
                facts.form_write = None;
            }
            facts.dialog = dialog;
        }
        manager.dialog_changed.notify_waiters();
        manager.announce();
    }

    fn upload(&self) -> Option<Vec<PathBuf>> {
        self.manager.upgrade()?.take_upload(&self.tab_id)
    }

    fn document(&self, event: DocumentEvent) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let started = matches!(event, DocumentEvent::Started { .. });
        let failed = {
            let mut tabs = manager.tabs.locked();
            let facts = tabs.entry(self.tab_id.clone()).or_default();
            if started {
                facts.nav_seq = facts.nav_seq.wrapping_add(1);
            }
            facts.documents.note(event)
        };
        if started {
            manager.load_changed.notify_waiters();
        }
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
