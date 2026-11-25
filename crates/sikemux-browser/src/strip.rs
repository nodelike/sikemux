//! An agent's strip of tabs as plain bookkeeping: which pages it holds, which
//! is in front, and whose they are, tested without a window.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use url::Url;

use crate::manager::AGENT_TAB_LIMIT;
use crate::{BrowserSnapshot, BrowserTab};

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
    /// Tabs that finished loading a moment ago, keyed by mark like `acting`.
    pub settling: HashMap<String, u64>,
    /// Tabs whose page was let go to free memory. Their view is built again
    /// at the same address once they are needed.
    pub unloaded: HashSet<String>,
    /// Tabs built again that stay off screen until their page has loaded, so
    /// the pane keeps its last picture rather than flashing a blank page.
    pub revealing: HashMap<String, u64>,
    /// Since when each tab has been out of sight and idle.
    pub hidden_since: HashMap<String, Instant>,
    /// The person's tabs besides those they clicked or typed in: ones they opened. Tidying up
    /// leaves these open.
    pub kept: HashSet<String>,
    /// Tabs the agent worked in during its turn.
    pub driven: HashSet<String>,
    /// Tabs opened in the background, so a shut desk stays shut for them.
    pub background: HashSet<String>,
}

impl TabStrip {
    pub fn insert(&mut self, id: String, page: TabPage) {
        self.insert_tab(id, page, true, false);
    }

    pub fn insert_tab(&mut self, id: String, page: TabPage, activate: bool, background: bool) {
        self.order.push(id.clone());
        self.pages.insert(id.clone(), page);
        if background {
            self.background.insert(id.clone());
        }
        if activate || self.active.is_none() {
            self.active = Some(id);
        }
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
        self.settling.remove(id);
        self.unloaded.remove(id);
        self.revealing.remove(id);
        self.hidden_since.remove(id);
        self.kept.remove(id);
        self.driven.remove(id);
        self.background.remove(id);
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
        self.background.remove(id);
        self.active = Some(id.to_owned());
        true
    }

    pub fn mark_acting(&mut self, tab_id: Option<&str>, mark: u64) -> Option<String> {
        let tab = match tab_id {
            Some(id) if self.pages.contains_key(id) => id.to_owned(),
            Some(_) => return None,
            None => self.active.clone()?,
        };
        self.acting.insert(tab.clone(), mark);
        self.driven.insert(tab.clone());
        Some(tab)
    }

    /// Tabs whose pages are told they have focus: every tab the agent is driving this turn.
    pub fn page_focus(&self) -> HashSet<String> {
        self.driven
            .iter()
            .filter(|id| self.pages.contains_key(*id))
            .cloned()
            .collect()
    }

    pub fn release_acting(&mut self, tab: &str, mark: u64) -> bool {
        if self.acting.get(tab) != Some(&mark) {
            return false;
        }
        self.acting.remove(tab);
        true
    }

    pub fn settle(&mut self, tab: &str, mark: u64) {
        self.settling.insert(tab.to_owned(), mark);
    }

    pub fn release_settled(&mut self, tab: &str, mark: u64) -> bool {
        if self.settling.get(tab) != Some(&mark) {
            return false;
        }
        self.settling.remove(tab);
        true
    }

    pub fn release_revealing(&mut self, tab: &str, mark: u64) -> bool {
        if self.revealing.get(tab) != Some(&mark) {
            return false;
        }
        self.revealing.remove(tab);
        true
    }

    pub fn yours(&self, id: &str) -> bool {
        self.kept.contains(id) || crate::person::has_touched(id)
    }

    /// The tabs tidying up may close: the agent's own, other than the one in front or any
    /// actively running a tool call.
    pub(crate) fn agents_spare(&self) -> impl Iterator<Item = &String> {
        self.order.iter().filter(|id| {
            !self.yours(id) && self.active.as_ref() != Some(*id) && !self.acting.contains_key(*id)
        })
    }

    /// The agent's own tab on the same site as `url`, which it goes to rather than opening
    /// another.
    pub fn same_site(&self, url: &Url) -> Option<String> {
        let open_on_it = |id: &&String| {
            self.pages
                .get(*id)
                .is_some_and(|page| crate::address::same_site(&page.url, url.as_str()))
        };
        let theirs = |id: &&String| !crate::person::has_touched(id);
        self.active
            .iter()
            .chain(self.order.iter().rev())
            .find(|id| open_on_it(id) && theirs(id))
            .cloned()
    }

    /// The agent's tab that has gone unused longest, once it holds `AGENT_TAB_LIMIT`.
    pub fn over_limit(&self) -> Option<String> {
        if self.order.iter().filter(|id| !self.yours(id)).count() < AGENT_TAB_LIMIT {
            return None;
        }
        self.agents_spare()
            .filter_map(|id| Some((self.hidden_since.get(id)?, id)))
            .min()
            .map(|(_, id)| id)
            .or_else(|| self.agents_spare().next())
            .cloned()
    }

    /// Whether the tab must keep drawing frames while it is out of sight.
    pub fn awake(&self, id: &str) -> bool {
        self.pages.get(id).is_some_and(|page| page.loading)
            || self.acting.contains_key(id)
            || self.settling.contains_key(id)
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
                        local_file: None,
                        background: self.background.contains(id),
                    })
                })
                .collect(),
            active_tab_id: self.active.clone(),
        }
    }
}
