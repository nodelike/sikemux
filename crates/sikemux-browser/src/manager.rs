//! Each agent owns a strip of browser tabs, shown in its pane and driven by its
//! browser tools. A tab is a page the engine adds to the host's window, so
//! pages get real input, real pixels, and the system cookie jar. Whoever hosts
//! the engine draws the chrome around it and says where the page area is.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;
use url::Url;

use crate::host::{BrowserResult, Error, Host, HostRef};
#[cfg(target_os = "linux")]
use crate::linux::page;
#[cfg(target_os = "macos")]
use crate::macos;
#[cfg(target_os = "macos")]
use crate::macos as page;
use crate::recording;
use crate::{
    address, burst, documents, downloads, favicon, history, local_files, viewport, BrowserBounds,
    BrowserDownload, BrowserSnapshot, BrowserTab, DocumentEvent, DownloadState, Opener, PageDialog,
    PageLoad, Tab, TabEvents, TabSpec, TabStall, BLANK_URL, PARKED_ORIGIN, USER_AGENT,
};

const MAX_URL_LEN: usize = 8192;
/// A page opening more tabs than this in `POPUP_WINDOW` is in a loop.
const POPUP_LIMIT: usize = 4;
const POPUP_WINDOW: Duration = Duration::from_secs(10);
/// A page saving more files than this in `DOWNLOAD_WINDOW` is filling the disk.
const DOWNLOAD_LIMIT: usize = 8;
const DOWNLOAD_WINDOW: Duration = Duration::from_secs(30);
/// Files the person never kept are cleared from the holding folder after this long.
const HELD_DOWNLOAD_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);
/// What a parked page lays out at before its pane has ever shown one.
const PARKED_SIZE: (f64, f64) = (1200.0, 800.0);
/// The longest a new tab waits for the app to bring its agent's desk on screen.
const DESK_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// How many of its own tabs an agent keeps before opening one closes its longest unused.
pub(crate) const AGENT_TAB_LIMIT: usize = 6;
/// How many closed tabs each desk remembers to open again.
const CLOSED_MEMORY: usize = 20;

const ACTING_LINGER: Duration = Duration::from_secs(3);
/// A hidden page runs no animation frames, and React reveals streamed content
/// on one, so a parked tab stays shown this long after it finishes loading.
const LOAD_SETTLE: Duration = Duration::from_secs(2);
/// The longest a reloaded tab waits off screen for its page before showing
/// whatever it has so far.
const REVEAL_LIMIT: Duration = Duration::from_secs(3);

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
    /// The person's tabs besides those they clicked or typed in: ones they opened, and the
    /// one in front when the agent's turn ended. Tidying up leaves these open.
    pub kept: HashSet<String>,
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
        self.settling.remove(id);
        self.unloaded.remove(id);
        self.revealing.remove(id);
        self.hidden_since.remove(id);
        self.kept.remove(id);
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

    /// The tabs tidying up may close: the agent's own, other than the one in front.
    fn agents_spare(&self) -> impl Iterator<Item = &String> {
        self.order
            .iter()
            .filter(|id| !self.yours(id) && self.active.as_ref() != Some(*id))
    }

    /// The agent's own tab on the same site as `url`, which it goes to rather than opening
    /// another.
    pub fn same_site(&self, url: &Url) -> Option<String> {
        let site = url.origin();
        let open_on_it = |id: &&String| {
            self.pages
                .get(*id)
                .and_then(|page| Url::parse(&page.url).ok())
                .is_some_and(|open| open.origin() == site)
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
                    })
                })
                .collect(),
            active_tab_id: self.active.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct AgentBrowser {
    pub(crate) strip: TabStrip,
    pub(crate) views: HashMap<String, Tab>,
    pub(crate) bounds: Option<BrowserBounds>,
    /// The size of the page area the last time it showed a page.
    pub(crate) seen: Option<(f64, f64)>,
    /// The app was told the agent took the wheel and has not yet said where its desk shows,
    /// or that it will not.
    pub(crate) desk_coming: bool,
    pub(crate) viewports: HashMap<String, viewport::Viewport>,
    /// What each tab's view was last given, so a swipe moving one page every
    /// frame does not also re-place every tab parked behind it.
    pub(crate) applied: HashMap<String, (viewport::Layout, bool)>,
    pub(crate) downloads: downloads::DownloadLog,
}

impl AgentBrowser {
    /// Unloaded tabs that must have their page back: the one the pane shows,
    /// and any an agent is working in.
    pub(crate) fn needed_back(&self) -> Vec<String> {
        self.strip
            .unloaded
            .iter()
            .filter(|id| {
                (self.bounds.is_some() && self.strip.active.as_deref() == Some(id.as_str()))
                    || self.strip.acting.contains_key(id.as_str())
            })
            .cloned()
            .collect()
    }

    pub(crate) fn layout_of(&self, tab_id: &str) -> viewport::Layout {
        let shown = self.strip.active.as_deref() == Some(tab_id)
            && !self.strip.revealing.contains_key(tab_id);
        viewport::layout(
            self.bounds.as_ref().filter(|_| shown),
            self.viewports.get(tab_id).copied(),
            self.seen.unwrap_or(PARKED_SIZE),
        )
    }
}

#[derive(Default)]
pub struct BrowserManager {
    pub(crate) agents: Mutex<HashMap<String, AgentBrowser>>,
    /// Rung, under `agents`, whenever an agent's download starts, finishes or fails.
    downloads_moved: Condvar,
    pub(crate) next_tab: AtomicU64,
    pub(crate) next_mark: AtomicU64,
    pub(crate) shortcuts_installed: AtomicBool,
    pub(crate) downloads: Mutex<HashMap<(String, String), PathBuf>>,
    pub(crate) icons: Mutex<favicon::IconCache>,
    pub(crate) history: history::History,
    pub(crate) dialogs: Mutex<HashMap<String, PageDialog>>,
    /// Files offered to a tab's file choosers, and whether one took them yet.
    pub(crate) uploads: Mutex<HashMap<String, (Vec<PathBuf>, bool)>>,
    pub(crate) documents: Mutex<HashMap<String, documents::DocumentLog>>,
    pub(crate) stalls: Mutex<HashMap<String, TabStall>>,
    pub(crate) local_files: local_files::LocalFiles,
    pub(crate) recordings: Mutex<HashMap<String, recording::Session>>,
    /// The addresses of each agent's closed tabs, latest last.
    pub(crate) closed: Mutex<HashMap<String, Vec<String>>>,
    pub(crate) relieving: AtomicBool,
    /// Rung whenever the app says where an agent's desk shows, or that it does not.
    pub(crate) desk_answers: tokio::sync::Notify,
    pub(crate) host: HostRef,
    pub(crate) me: Weak<BrowserManager>,
}

impl BrowserManager {
    /// The engine for `host`'s window. It starts watching for memory running short.
    pub fn new(host: Arc<dyn Host>) -> Arc<Self> {
        let manager = Arc::new_cyclic(|me| BrowserManager {
            host: HostRef(host),
            me: me.clone(),
            ..BrowserManager::default()
        });
        crate::discard::watch_pressure(Arc::downgrade(&manager));
        manager
    }

    pub(crate) fn host(&self) -> &dyn Host {
        self.host.0.as_ref()
    }

    /// The manager itself, for work that outlives the call that starts it.
    pub(crate) fn this(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    /// A loopback address a tab can load `path` from.
    pub fn local_file_url(&self, path: &Path) -> Result<String, String> {
        if self.local_files.full() {
            self.release_unused_files();
        }
        self.local_files.url_for(path)
    }

    /// Stops showing local files no open tab is on.
    fn release_unused_files(&self) {
        let urls: Vec<String> = self
            .agents
            .locked()
            .values()
            .flat_map(|agent| agent.strip.pages.values().map(|page| page.url.clone()))
            .collect();
        self.local_files.keep_only(&urls);
    }

    /// The page `input` opens: a web address, a search, or a local file served to the tab.
    pub(crate) fn page_url(&self, input: &str) -> BrowserResult<String> {
        match address::resolve(input) {
            address::Address::Local(path) => self.local_file_url(&path).map_err(Error::Other),
            address::Address::Url(url) | address::Address::Search(url) => Ok(url),
        }
    }

    pub fn snapshot(&self, agent_id: &str) -> BrowserResult<BrowserSnapshot> {
        validate_agent_id(agent_id)?;
        let mut snapshot = self
            .agents
            .locked()
            .get(agent_id)
            .map(|agent| agent.strip.snapshot())
            .unwrap_or(BrowserSnapshot {
                tabs: Vec::new(),
                active_tab_id: None,
            });
        for tab in &mut snapshot.tabs {
            tab.local_file = self.local_files.path_of(&tab.url);
        }
        Ok(snapshot)
    }

    pub async fn open_tab(&self, agent_id: &str, url: Option<&str>) -> BrowserResult<String> {
        validate_agent_id(agent_id)?;
        let url = self.page_url(url.unwrap_or_default())?;
        let (tab_id, _) = self.add_tab(agent_id, &url, None)?;
        self.until_desk_answers(agent_id).await;
        Ok(tab_id)
    }

    /// Adds a tab at `url` to the end of the agent's strip and shows it.
    fn add_tab(
        &self,
        agent_id: &str,
        url: &str,
        opened_by: Option<OpenedBy>,
    ) -> BrowserResult<(String, Tab)> {
        validate_url(url)?;
        let parsed = Url::parse(url).map_err(|_| Error::BadArg("invalid browser url"))?;
        let tab_id = format!(
            "browser-{}-{}",
            label_safe(agent_id),
            self.next_tab.fetch_add(1, Ordering::AcqRel)
        );
        let webview = self.create_view(agent_id, &tab_id, parsed, USER_AGENT, opened_by)?;
        {
            let mut agents = self.agents.locked();
            let agent = agents.entry(agent_id.to_owned()).or_default();
            agent.strip.insert(
                tab_id.clone(),
                TabPage {
                    url: url.to_owned(),
                    loading: url != BLANK_URL,
                    ..TabPage::default()
                },
            );
            agent.views.insert(tab_id.clone(), webview.clone());
        }
        self.relayout(agent_id);
        self.announce();
        Ok((tab_id, webview))
    }

    /// While the app brings the agent's desk on screen, a new tab waits a moment to be laid
    /// out in it, so what the agent reads first is the page at the pane's size.
    async fn until_desk_answers(&self, agent_id: &str) {
        let deadline = tokio::time::Instant::now() + DESK_WAIT;
        loop {
            let answered = self.desk_answers.notified();
            tokio::pin!(answered);
            answered.as_mut().enable();
            let coming = self
                .agents
                .locked()
                .get(agent_id)
                .is_some_and(|agent| agent.desk_coming);
            if !coming || tokio::time::timeout_at(deadline, answered).await.is_err() {
                return;
            }
        }
    }

    /// Adds the tab's page to the main window, parked off screen.
    pub(crate) fn create_view(
        &self,
        agent_id: &str,
        tab_id: &str,
        url: Url,
        user_agent: &str,
        opened_by: Option<OpenedBy>,
    ) -> BrowserResult<Tab> {
        let parent = self.host().window().map_err(Error::Window)?;
        let size = self
            .agents
            .locked()
            .get(agent_id)
            .and_then(|agent| agent.seen)
            .unwrap_or(PARKED_SIZE);
        let events = Arc::new(TabListener {
            manager: self.me.clone(),
            agent_id: agent_id.to_owned(),
            tab_id: tab_id.to_owned(),
            opened_from: opened_by.as_ref().map(|by| by.tab_id.clone()),
            popups: Mutex::new(burst::Burst::new(POPUP_LIMIT, POPUP_WINDOW)),
            downloads: Mutex::new(burst::Burst::new(DOWNLOAD_LIMIT, DOWNLOAD_WINDOW)),
        });
        let tab = Tab::open(
            parent,
            TabSpec {
                agent_id,
                tab_id,
                url: url.as_str(),
                user_agent,
                size,
                opener: opened_by.and_then(|by| by.opener),
            },
            events,
        )
        .map_err(Error::Window)?;
        self.install_shortcuts();
        Ok(tab)
    }

    /// Builds an unloaded tab's view again at the address it was on. The
    /// back and forward history went with the old page.
    pub(crate) fn restore(&self, agent_id: &str, tab_id: &str) -> Option<Tab> {
        let (url, user_agent) = {
            let mut agents = self.agents.locked();
            let agent = agents.get_mut(agent_id)?;
            if !agent.strip.unloaded.remove(tab_id) {
                return agent.views.get(tab_id).cloned();
            }
            let url = agent.strip.pages.get(tab_id)?.url.clone();
            let user_agent = agent
                .viewports
                .get(tab_id)
                .and_then(|fixed| fixed.user_agent())
                .unwrap_or(USER_AGENT);
            (url, user_agent)
        };
        let parsed = Url::parse(&url)
            .ok()
            .filter(tab_may_load)
            .unwrap_or_else(|| Url::parse(BLANK_URL).expect("about:blank parses"));
        let blank = parsed.as_str() == BLANK_URL;
        let view = match self.create_view(agent_id, tab_id, parsed, user_agent, None) {
            Ok(view) => view,
            Err(error) => {
                eprintln!("Sikemux could not reload browser tab {tab_id}: {error}");
                if let Some(agent) = self.agents.locked().get_mut(agent_id) {
                    agent.strip.unloaded.insert(tab_id.to_owned());
                }
                return None;
            }
        };
        let mark = self.next_mark.fetch_add(1, Ordering::AcqRel);
        let kept = {
            let mut agents = self.agents.locked();
            match agents
                .get_mut(agent_id)
                .filter(|agent| agent.strip.pages.contains_key(tab_id))
            {
                Some(agent) => {
                    if let Some(page) = agent.strip.pages.get_mut(tab_id) {
                        page.loading = !blank;
                    }
                    if !blank {
                        agent.strip.revealing.insert(tab_id.to_owned(), mark);
                    }
                    agent.views.insert(tab_id.to_owned(), view.clone());
                    true
                }
                None => false,
            }
        };
        if !kept {
            view.close();
            return None;
        }
        self.announce();
        let Some(manager) = self.me.upgrade() else {
            return Some(view);
        };
        let (agent_id, tab_id) = (agent_id.to_owned(), tab_id.to_owned());
        crate::spawn(async move {
            tokio::time::sleep(REVEAL_LIMIT).await;
            let released = manager
                .agents
                .locked()
                .get_mut(&agent_id)
                .is_some_and(|agent| agent.strip.release_revealing(&tab_id, mark));
            if released {
                manager.relayout(&agent_id);
            }
        });
        Some(view)
    }

    /// Whether the agent's shown tab has no live page yet: unloaded, or
    /// still loading again off screen.
    pub fn reloading(&self, agent_id: &str) -> bool {
        self.agents.locked().get(agent_id).is_some_and(|agent| {
            agent.strip.active.as_ref().is_some_and(|id| {
                agent.strip.unloaded.contains(id) || agent.strip.revealing.contains_key(id)
            })
        })
    }

    /// Where a file a page in `tab_id` saves waits, or None to refuse it.
    pub(crate) fn download_started(
        &self,
        agent_id: &str,
        tab_id: &str,
        url: &str,
        suggested: &Path,
    ) -> Option<PathBuf> {
        let parsed = Url::parse(url).ok()?;
        let folder = self.held_downloads_dir()?;
        let path = unique_download_path(&folder, &download_file_name(&parsed, suggested));
        self.downloads
            .locked()
            .insert((tab_id.to_owned(), url.to_owned()), path.clone());
        self.note_download(agent_id, tab_id, url, &path, DownloadState::Started);
        Some(path)
    }

    pub(crate) fn download_finished(&self, agent_id: &str, tab_id: &str, url: &str, saved: bool) {
        let path = self
            .downloads
            .locked()
            .remove(&(tab_id.to_owned(), url.to_owned()))
            .unwrap_or_default();
        #[cfg(target_os = "macos")]
        if saved {
            quarantine(&path);
        }
        let state = if saved {
            DownloadState::Finished
        } else {
            DownloadState::Failed
        };
        self.note_download(agent_id, tab_id, url, &path, state);
    }

    pub(crate) fn note_download(
        &self,
        agent_id: &str,
        tab_id: &str,
        url: &str,
        path: &Path,
        state: DownloadState,
    ) {
        let bytes = (state == DownloadState::Finished)
            .then(|| std::fs::metadata(path).ok().map(|meta| meta.len()))
            .flatten();
        let download = BrowserDownload {
            agent_id: agent_id.to_owned(),
            tab_id: tab_id.to_owned(),
            url: url.to_owned(),
            path: path.to_string_lossy().into_owned(),
            state,
        };
        if let Some(agent) = self.agents.locked().get_mut(agent_id) {
            agent.downloads.note(&download, bytes);
        }
        self.downloads_moved.notify_all();
        self.host().downloaded(&download);
    }

    /// The number the agent's next download will get, to find later which
    /// downloads an action started.
    pub fn download_mark(&self, agent_id: &str) -> u64 {
        self.agents
            .locked()
            .get(agent_id)
            .map_or(0, |agent| agent.downloads.mark())
    }

    pub fn downloads_since(&self, agent_id: &str, mark: u64) -> Vec<downloads::SavedFile> {
        self.agents
            .locked()
            .get(agent_id)
            .map(|agent| agent.downloads.since(mark))
            .unwrap_or_default()
    }

    /// [`Self::downloads_since`] once none of them is still going, or once `limit` is up.
    pub fn settled_downloads_since(
        &self,
        agent_id: &str,
        mark: u64,
        limit: Duration,
    ) -> Vec<downloads::SavedFile> {
        let deadline = Instant::now() + limit;
        let mut agents = self.agents.locked();
        loop {
            let files = agents
                .get(agent_id)
                .map(|agent| agent.downloads.since(mark))
                .unwrap_or_default();
            let left = deadline.saturating_duration_since(Instant::now());
            let going = files
                .iter()
                .any(|file| file.state == DownloadState::Started);
            if !going || left.is_zero() {
                return files;
            }
            agents = match self.downloads_moved.wait_timeout(agents, left) {
                Ok((agents, _)) => agents,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    pub fn recent_downloads(&self, agent_id: &str) -> Vec<downloads::SavedFile> {
        self.downloads_since(agent_id, 0)
    }

    pub fn dialog(&self, tab_id: &str) -> Option<PageDialog> {
        self.dialogs.locked().get(tab_id).cloned()
    }

    /// Files for the tab's file choosers, answered in place of the person
    /// until [`Self::withdraw_upload`]. Every chooser one click opens gets
    /// them, since a page that opens its own on top of the input's would
    /// otherwise have the input's files dropped for an empty answer.
    pub fn offer_upload(&self, tab_id: &str, paths: Vec<PathBuf>) {
        self.uploads
            .locked()
            .insert(tab_id.to_owned(), (paths, false));
    }

    /// Whether no chooser has taken the offered files yet.
    pub fn upload_pending(&self, tab_id: &str) -> bool {
        self.uploads
            .locked()
            .get(tab_id)
            .is_some_and(|(_, taken)| !taken)
    }

    /// `None` when nothing is offered.
    pub fn take_upload(&self, tab_id: &str) -> Option<Vec<PathBuf>> {
        let mut uploads = self.uploads.locked();
        let (paths, taken) = uploads.get_mut(tab_id)?;
        *taken = true;
        Some(paths.clone())
    }

    pub fn withdraw_upload(&self, tab_id: &str) {
        self.uploads.locked().remove(tab_id);
    }

    /// The tab's top-level loads, oldest first; with `since_current`, only
    /// the one that produced the page on screen and any tried after it.
    pub fn documents(&self, tab_id: &str, since_current: bool) -> Vec<documents::DocumentLoad> {
        self.documents
            .locked()
            .get(tab_id)
            .map(|log| log.loads(since_current))
            .unwrap_or_default()
    }

    /// Set once WebKit has seen the tab's page stop answering or crash, and
    /// cleared when it answers again or starts a new load. A hung page is only
    /// noticed a few seconds after input reaches it; `input::probe_responsiveness`
    /// sends some.
    pub fn stalled(&self, tab_id: &str) -> Option<TabStall> {
        self.stalls.locked().get(tab_id).copied()
    }

    pub fn note_page(&self, agent_id: &str, tab_id: &str, update: impl FnOnce(&mut TabPage)) {
        let (changed, loading, shown) = {
            let mut agents = self.agents.locked();
            match agents
                .get_mut(agent_id)
                .and_then(|agent| agent.strip.pages.get_mut(tab_id))
            {
                Some(page) => {
                    let before = page.clone();
                    update(page);
                    let loading = (before.loading != page.loading).then_some(page.loading);
                    let visited = !page.loading && (loading.is_some() || before.url != page.url);
                    let shown = (*page != before && !page.loading).then(|| (page.clone(), visited));
                    (*page != before, loading, shown)
                }
                None => (false, None, None),
            }
        };
        if changed {
            self.announce();
        }
        if let Some((page, visited)) = shown {
            self.history.note(
                self.host().state_dir().as_deref(),
                &page.url,
                &page.title,
                page.favicon.as_deref(),
                visited,
            );
        }
        match loading {
            Some(true) => self.relayout(agent_id),
            Some(false) => self.settle(agent_id, tab_id),
            None => {}
        }
    }

    pub(crate) fn settle(&self, agent_id: &str, tab_id: &str) {
        let mark = self.next_mark.fetch_add(1, Ordering::AcqRel);
        match self.agents.locked().get_mut(agent_id) {
            Some(agent) => {
                agent.strip.settle(tab_id, mark);
                agent.strip.revealing.remove(tab_id);
            }
            None => return,
        }
        self.relayout(agent_id);
        let Some(manager) = self.me.upgrade() else {
            return;
        };
        let (agent_id, tab_id) = (agent_id.to_owned(), tab_id.to_owned());
        crate::spawn(async move {
            tokio::time::sleep(LOAD_SETTLE).await;
            let released = manager
                .agents
                .locked()
                .get_mut(&agent_id)
                .is_some_and(|agent| agent.strip.release_settled(&tab_id, mark));
            if released {
                manager.relayout(&agent_id);
            }
        });
    }

    /// A page closed the window it was opened as. The tab that opened it comes back
    /// in its place if it was the one shown.
    fn close_window(&self, agent_id: &str, tab_id: &str, opened_from: &str) {
        if let Some(agent) = self.agents.locked().get_mut(agent_id) {
            if agent.strip.active.as_deref() == Some(tab_id) {
                agent.strip.activate(opened_from);
            }
        }
        let _ = self.close_tab(agent_id, tab_id);
    }

    pub fn close_tab(&self, agent_id: &str, tab_id: &str) -> BrowserResult<()> {
        validate_agent_id(agent_id)?;
        let view = {
            let mut agents = self.agents.locked();
            let agent = agents
                .get_mut(agent_id)
                .ok_or(Error::BadArg("unknown browser tab"))?;
            let url = agent.strip.pages.get(tab_id).map(|page| page.url.clone());
            if !agent.strip.remove(tab_id) {
                return Err(Error::BadArg("unknown browser tab"));
            }
            crate::person::untouched(tab_id);
            if let Some(url) = url.filter(|url| url != BLANK_URL) {
                let mut closed = self.closed.locked();
                let closed = closed.entry(agent_id.to_owned()).or_default();
                closed.push(url);
                if closed.len() > CLOSED_MEMORY {
                    closed.remove(0);
                }
            }
            let view = agent.views.remove(tab_id);
            agent.viewports.remove(tab_id);
            self.documents.locked().remove(tab_id);
            self.stalls.locked().remove(tab_id);
            if agent.strip.order.is_empty() {
                agents.remove(agent_id);
            }
            view
        };
        if let Some(view) = view {
            view.close();
        }
        self.release_unused_files();
        self.relayout(agent_id);
        self.announce();
        Ok(())
    }

    pub fn close_agent(&self, agent_id: &str) -> BrowserResult<()> {
        validate_agent_id(agent_id)?;
        self.recordings.locked().remove(agent_id);
        self.closed.locked().remove(agent_id);
        let views = self
            .agents
            .locked()
            .remove(agent_id)
            .map(|agent| agent.views.into_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let views: Vec<Tab> = {
            let mut documents = self.documents.locked();
            views
                .into_iter()
                .map(|(tab_id, view)| {
                    crate::person::untouched(&tab_id);
                    documents.remove(&tab_id);
                    self.stalls.locked().remove(&tab_id);
                    view
                })
                .collect()
        };
        if !views.is_empty() {
            eprintln!(
                "Sikemux closed {} browser tab(s) with agent {agent_id}",
                views.len()
            );
        }
        for view in views {
            view.close();
        }
        self.release_unused_files();
        self.announce();
        Ok(())
    }

    /// Tidies up after the agent's turn: the tab in front becomes the person's, and the
    /// agent's other tabs close.
    pub fn end_turn(&self, agent_id: &str) {
        let spare: Vec<String> = {
            let mut agents = self.agents.locked();
            let Some(agent) = agents.get_mut(agent_id) else {
                return;
            };
            if let Some(front) = agent.strip.active.clone() {
                agent.strip.kept.insert(front);
            }
            agent.strip.agents_spare().cloned().collect()
        };
        for tab_id in spare {
            let _ = self.close_tab(agent_id, &tab_id);
        }
    }

    /// Makes room for the agent's next tab, and says which address it closed for it.
    pub fn make_room(&self, agent_id: &str) -> Option<String> {
        let (tab_id, url) = {
            let agents = self.agents.locked();
            let strip = &agents.get(agent_id)?.strip;
            let tab_id = strip.over_limit()?;
            let url = strip.pages.get(&tab_id)?.url.clone();
            (tab_id, url)
        };
        self.close_tab(agent_id, &tab_id).ok().map(|()| url)
    }

    /// The agent's own tab on the same site as `url`, if it has one.
    pub fn same_site_tab(&self, agent_id: &str, url: &str) -> Option<String> {
        let url = Url::parse(url).ok()?;
        self.agents.locked().get(agent_id)?.strip.same_site(&url)
    }

    /// Makes the tab the person's, so tidying up leaves it open.
    pub fn keep(&self, agent_id: &str, tab_id: &str) {
        if let Some(agent) = self.agents.locked().get_mut(agent_id) {
            if agent.strip.pages.contains_key(tab_id) {
                agent.strip.kept.insert(tab_id.to_owned());
            }
        }
    }

    /// Gives the person's tab back to the agent, once the agent takes it to another site.
    pub fn hand_back(&self, agent_id: &str, tab_id: &str) {
        if let Some(agent) = self.agents.locked().get_mut(agent_id) {
            agent.strip.kept.remove(tab_id);
        }
        crate::person::untouched(tab_id);
    }

    /// Opens the desk's latest closed tab again, as the person's.
    pub async fn reopen_closed(&self, agent_id: &str) -> BrowserResult<()> {
        validate_agent_id(agent_id)?;
        let url = self
            .closed
            .locked()
            .get_mut(agent_id)
            .and_then(Vec::pop)
            .ok_or(Error::BadArg("no closed tab to open again"))?;
        let tab_id = self.open_tab(agent_id, Some(&url)).await?;
        self.keep(agent_id, &tab_id);
        Ok(())
    }

    pub fn switch_tab(&self, agent_id: &str, tab_id: &str) -> BrowserResult<()> {
        validate_agent_id(agent_id)?;
        let switched = self
            .agents
            .locked()
            .get_mut(agent_id)
            .is_some_and(|agent| agent.strip.activate(tab_id));
        if !switched {
            return Err(Error::BadArg("unknown browser tab"));
        }
        self.relayout(agent_id);
        self.announce();
        Ok(())
    }

    /// `None` parks every tab of the agent off screen: the pane is hidden,
    /// showing its blank page, or an app overlay needs to paint over it. A
    /// pane squeezed to a sliver parks them too, and is never remembered as
    /// the size a parked page lays out at.
    pub fn set_bounds(&self, agent_id: &str, bounds: Option<BrowserBounds>) -> BrowserResult<()> {
        validate_agent_id(agent_id)?;
        if let Some(bounds) = &bounds {
            validate_bounds(bounds)?;
        }
        let bounds = bounds.filter(|area| !viewport::collapsed(area));
        {
            let mut agents = self.agents.locked();
            let agent = match agents.get_mut(agent_id) {
                Some(agent) => agent,
                None if bounds.is_some() => agents.entry(agent_id.to_owned()).or_default(),
                None => return Ok(()),
            };
            if let Some(area) = &bounds {
                agent.seen = Some((area.width, area.height));
            }
            agent.bounds = bounds;
            agent.desk_coming = false;
        }
        self.relayout(agent_id);
        self.desk_answers.notify_waiters();
        Ok(())
    }

    /// Whether the person can see this tab's page in its pane right now.
    pub fn shown(&self, agent_id: &str, tab_id: &str) -> bool {
        self.agents.locked().get(agent_id).is_some_and(|agent| {
            agent.strip.active.as_deref() == Some(tab_id)
                && agent
                    .bounds
                    .as_ref()
                    .is_some_and(|area| area.clip_left + area.clip_right < area.width)
        })
    }

    /// Opens `input` in the agent's tab and returns the address it went to.
    pub fn navigate(&self, agent_id: &str, input: &str) -> BrowserResult<String> {
        let url = self.page_url(input)?;
        validate_url(&url)?;
        let parsed = Url::parse(&url).map_err(|_| Error::BadArg("invalid browser url"))?;
        let (tab_id, view) = self.active_view(agent_id)?;
        view.navigate(parsed.as_str());
        self.note_page(agent_id, &tab_id, |page| {
            page.url = url.clone();
            page.loading = true;
        });
        Ok(url)
    }

    /// Opens what the person typed in the address bar, and remembers it as typed.
    pub fn navigate_typed(&self, agent_id: &str, input: &str) -> BrowserResult<()> {
        let opened = self.navigate(agent_id, input)?;
        if let Ok((tab_id, _)) = self.active_view(agent_id) {
            self.keep(agent_id, &tab_id);
        }
        self.note_typed(input, &opened);
        Ok(())
    }

    /// Opens what the person typed in a new tab, for a desk with no tab to go to.
    pub async fn open_typed(&self, agent_id: &str, input: &str) -> BrowserResult<()> {
        let tab_id = if input.is_empty() {
            self.open_tab(agent_id, None).await?
        } else {
            let opened = self.page_url(input)?;
            let tab_id = self.open_tab(agent_id, Some(input)).await?;
            self.note_typed(input, &opened);
            tab_id
        };
        self.keep(agent_id, &tab_id);
        Ok(())
    }

    fn note_typed(&self, input: &str, opened: &str) {
        if !matches!(address::resolve(input), address::Address::Local(_)) {
            self.history
                .note_typed(self.host().state_dir().as_deref(), opened);
        }
    }

    /// What the address bar offers for what the person has typed so far.
    pub fn suggest(&self, query: &str) -> history::AddressSuggestions {
        self.history
            .suggest(self.host().state_dir().as_deref(), query)
    }

    pub fn reload(&self, agent_id: &str) -> BrowserResult<()> {
        let (_, view) = self.active_view(agent_id)?;
        view.reload();
        Ok(())
    }

    pub fn history(&self, agent_id: &str, delta: i32) -> BrowserResult<()> {
        let (_, view) = self.active_view(agent_id)?;
        view.run(move |webview| page::history(webview, delta));
        Ok(())
    }

    /// Holds the tab at `fixed`, or with `None` lets it follow the pane again.
    pub fn set_viewport(
        &self,
        agent_id: &str,
        tab_id: &str,
        fixed: Option<viewport::Viewport>,
    ) -> BrowserResult<()> {
        let view = {
            let mut agents = self.agents.locked();
            let agent = agents
                .get_mut(agent_id)
                .ok_or(Error::BadArg("unknown browser tab"))?;
            let view = agent
                .views
                .get(tab_id)
                .cloned()
                .ok_or(Error::BadArg("unknown browser tab"))?;
            match fixed {
                Some(fixed) => agent.viewports.insert(tab_id.to_owned(), fixed),
                None => agent.viewports.remove(tab_id),
            };
            view
        };
        view.set_zoom(1.0);
        let agent = fixed
            .and_then(|fixed| fixed.user_agent())
            .unwrap_or(USER_AGENT);
        view.run(move |webview| page::introduce_as(webview, agent));
        self.relayout(agent_id);
        Ok(())
    }

    pub fn viewport(&self, agent_id: &str, tab_id: &str) -> Option<viewport::Viewport> {
        self.agents
            .locked()
            .get(agent_id)
            .and_then(|agent| agent.viewports.get(tab_id).copied())
    }

    /// The agent's shown tab, with its page loading again if it was unloaded.
    pub fn active_view(&self, agent_id: &str) -> BrowserResult<(String, Tab)> {
        validate_agent_id(agent_id)?;
        let no_tab = || Error::BadArg("this agent has no open browser tab");
        let (id, view) = self
            .agents
            .locked()
            .get(agent_id)
            .and_then(|agent| {
                let id = agent.strip.active.clone()?;
                let view = agent.views.get(&id).cloned();
                Some((id, view))
            })
            .ok_or_else(no_tab)?;
        if let Some(view) = view {
            return Ok((id, view));
        }
        let view = self.restore(agent_id, &id).ok_or_else(no_tab)?;
        self.relayout(agent_id);
        Ok((id, view))
    }

    /// Show the active tab inside the pane's page area and park the rest.
    pub(crate) fn relayout(&self, agent_id: &str) {
        let needed = self
            .agents
            .locked()
            .get(agent_id)
            .map(AgentBrowser::needed_back)
            .unwrap_or_default();
        for tab_id in needed {
            self.restore(agent_id, &tab_id);
        }
        let now = Instant::now();
        let plan: Vec<(Tab, viewport::Layout, bool)> = {
            let mut agents = self.agents.locked();
            let Some(agent) = agents.get_mut(agent_id) else {
                return;
            };
            let wanted: Vec<(String, Tab, viewport::Layout, bool)> = agent
                .views
                .iter()
                .map(|(id, view)| {
                    let layout = agent.layout_of(id);
                    (id.clone(), view.clone(), layout, agent.strip.awake(id))
                })
                .collect();
            for (id, _, layout, awake) in &wanted {
                if *awake || matches!(layout, viewport::Layout::Shown { .. }) {
                    agent.strip.hidden_since.remove(id);
                } else {
                    agent.strip.hidden_since.entry(id.clone()).or_insert(now);
                }
            }
            let views = &agent.views;
            agent.applied.retain(|id, _| views.contains_key(id));
            wanted
                .into_iter()
                .filter_map(|(id, view, layout, awake)| {
                    let next = (layout.clone(), awake);
                    if agent.applied.get(&id) == Some(&next) {
                        return None;
                    }
                    agent.applied.insert(id, next);
                    Some((view, layout, awake))
                })
                .collect()
        };
        for (view, layout, awake) in plan {
            let shown = matches!(layout, viewport::Layout::Shown { .. });
            place(&view, layout, awake);
            if shown || awake {
                view.show();
            } else {
                view.hide();
            }
        }
    }

    /// Tells the app which agent took the wheel, so its browser can come on
    /// screen even before it has a tab.
    pub fn announce_acting(&self, agent_id: &str) {
        {
            let mut agents = self.agents.locked();
            let agent = agents.entry(agent_id.to_owned()).or_default();
            agent.desk_coming = agent.bounds.is_none();
        }
        self.host().acting(agent_id);
    }

    /// Marks the tab the agent's tools are on, so the strip can show it working there.
    pub fn mark_acting(&self, agent_id: &str) -> Option<(String, u64)> {
        let mark = self.next_mark.fetch_add(1, Ordering::AcqRel);
        let (tab, woke) = {
            let mut agents = self.agents.locked();
            let strip = &mut agents.get_mut(agent_id)?.strip;
            let was_awake = strip.active.as_deref().is_some_and(|id| strip.awake(id));
            (strip.mark_acting(mark)?, !was_awake)
        };
        self.announce();
        if woke {
            self.relayout(agent_id);
        }
        Some((tab, mark))
    }

    /// The highlight lingers a moment after the last action, so a run of quick
    /// tool calls reads as one stretch of work rather than a flicker.
    pub fn release_acting(&self, agent_id: &str, marks: Vec<(String, u64)>) {
        if marks.is_empty() {
            return;
        }
        let Some(manager) = self.me.upgrade() else {
            return;
        };
        let agent_id = agent_id.to_owned();
        crate::spawn(async move {
            tokio::time::sleep(ACTING_LINGER).await;
            let mut released = false;
            if let Some(agent) = manager.agents.locked().get_mut(&agent_id) {
                for (tab, mark) in &marks {
                    released |= agent.strip.release_acting(tab, *mark);
                }
            }
            if released {
                manager.announce();
                manager.relayout(&agent_id);
            }
        });
    }

    pub(crate) fn announce(&self) {
        self.host().tabs_changed();
    }

    pub(crate) fn install_shortcuts(&self) {
        if self.shortcuts_installed.swap(true, Ordering::AcqRel) {
            return;
        }
        let host = self.host.0.clone();
        #[cfg(target_os = "macos")]
        dispatch2::DispatchQueue::main()
            .exec_async(move || page::install_shortcuts(move |shortcut| host.shortcut(shortcut)));
        #[cfg(target_os = "linux")]
        page::install_shortcuts(move |shortcut| host.shortcut(shortcut));
    }

    /// Where pages' downloads wait until the person keeps them, so a page can never
    /// drop a file into Downloads by itself. Files left there go after a week.
    pub(crate) fn held_downloads_dir(&self) -> Option<PathBuf> {
        let folder = self.host().cache_dir()?.join("desk-downloads");
        std::fs::create_dir_all(&folder).ok()?;
        let now = std::time::SystemTime::now();
        for entry in std::fs::read_dir(&folder).ok()?.flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| {
                    now.duration_since(modified)
                        .is_ok_and(|age| age > HELD_DOWNLOAD_LIFETIME)
                });
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Some(folder)
    }

    pub fn drain(&self, why: &str) {
        let views: Vec<Tab> = self
            .agents
            .locked()
            .drain()
            .flat_map(|(_, agent)| agent.views.into_values())
            .collect();
        if !views.is_empty() {
            eprintln!("Sikemux closed {} browser tab(s): {why}", views.len());
        }
        for view in views {
            view.close();
        }
    }
}

/// The tab whose page opened another as its window, and what the engine makes that one from.
pub(crate) struct OpenedBy {
    tab_id: String,
    opener: Option<Opener>,
}

/// What one tab's page tells the manager.
struct TabListener {
    manager: Weak<BrowserManager>,
    agent_id: String,
    tab_id: String,
    /// The tab whose page opened this one as its window, which alone may close it.
    opened_from: Option<String>,
    popups: Mutex<burst::Burst>,
    downloads: Mutex<burst::Burst>,
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
        let mut dialogs = manager.dialogs.locked();
        match dialog {
            Some(dialog) => dialogs.insert(self.tab_id.clone(), dialog),
            None => dialogs.remove(&self.tab_id),
        };
    }

    fn upload(&self) -> Option<Vec<PathBuf>> {
        self.manager.upgrade()?.take_upload(&self.tab_id)
    }

    fn document(&self, event: DocumentEvent) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let failed = manager
            .documents
            .locked()
            .entry(self.tab_id.clone())
            .or_default()
            .note(event);
        if failed {
            self.note(|page| page.loading = false);
        }
    }

    fn health(&self, stall: Option<TabStall>) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let mut stalls = manager.stalls.locked();
        match stall {
            Some(stall) => stalls.insert(self.tab_id.clone(), stall),
            None => stalls.remove(&self.tab_id),
        };
    }
}

pub(crate) fn validate_agent_id(agent_id: &str) -> BrowserResult<()> {
    if !sikemux_base::cli::protocol::is_tool_agent_id(agent_id) {
        return Err(Error::BadArg("invalid browser agent id"));
    }
    Ok(())
}

/// Marks a download as from the internet, as Safari does, so Gatekeeper
/// checks it before it first opens.
#[cfg(target_os = "macos")]
pub(crate) fn quarantine(path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let value = format!("0083;{seconds:08x};Sikemux;");
    let (Ok(target), Ok(name)) = (
        std::ffi::CString::new(path.as_os_str().as_bytes()),
        std::ffi::CString::new("com.apple.quarantine"),
    ) else {
        return;
    };
    // SAFETY: `target` and `name` are nul-terminated and live through the call, and
    // `value.len()` is exactly the bytes `value` points at.
    unsafe {
        libc::setxattr(
            target.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        );
    }
}

pub(crate) fn validate_url(url: &str) -> BrowserResult<()> {
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return Err(Error::BadArg("browser url is empty or too long"));
    }
    let parsed = Url::parse(url).map_err(|_| Error::BadArg("invalid browser url"))?;
    let opens = matches!(parsed.scheme(), "http" | "https") || parsed.as_str() == BLANK_URL;
    if !opens {
        return Err(Error::BadArg(
            "browser url scheme is not allowed; to show a local file, pass its path or file:// url to browser_navigate",
        ));
    }
    Ok(())
}

/// What a tab's page or any frame inside it may load. Frames also build
/// `about:srcdoc`, `blob:` and `data:` documents, which take the page's origin
/// or none at all, never the app's.
pub(crate) fn tab_may_load(url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" | "data" => true,
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "blob" => {
            Url::parse(url.path()).is_ok_and(|inner| matches!(inner.scheme(), "http" | "https"))
        }
        _ => false,
    }
}

/// Puts the tab's view where `layout` says, laid out at the page size it names.
#[cfg(target_os = "macos")]
pub(crate) fn place(view: &Tab, layout: viewport::Layout, awake: bool) {
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    view.run(move |tab| {
        macos::keep_running_when_covered(tab, awake);
        match layout {
            viewport::Layout::Shown { frame, page } => {
                macos::place(
                    tab,
                    NSRect::new(
                        NSPoint::new(frame.x, frame.y),
                        NSSize::new(frame.width, frame.height),
                    ),
                    page.map(|(width, height)| NSSize::new(width, height)),
                );
                let holes = frame
                    .holes
                    .iter()
                    .map(|hole| {
                        (
                            NSRect::new(
                                NSPoint::new(hole.x, hole.y),
                                NSSize::new(hole.width, hole.height),
                            ),
                            hole.radius,
                        )
                    })
                    .collect();
                macos::clip(tab, frame.clip_left, frame.clip_right, holes);
                macos::round_bottom(tab, frame.bottom_radius);
                macos::dim(tab, frame.dim);
                // A video's controls blur what is behind them, and that blur
                // draws through the mask even when it cuts the whole page away.
                let page_width = page.map_or(frame.width, |(width, _)| width);
                let nothing_shows = frame.clip_left + frame.clip_right >= page_width;
                macos::fade(tab, if nothing_shows { 0.0 } else { frame.opacity });
            }
            viewport::Layout::Parked {
                page: (width, height),
            } => macos::place(
                tab,
                NSRect::new(
                    NSPoint::new(PARKED_ORIGIN, PARKED_ORIGIN),
                    NSSize::new(width, height),
                ),
                None,
            ),
        }
    });
}

/// Elsewhere a fixed viewport can only be had by zooming the page.
#[cfg(not(target_os = "macos"))]
pub(crate) fn place(view: &Tab, layout: viewport::Layout, _awake: bool) {
    let (frame, sides, zoom) = match layout {
        viewport::Layout::Shown { frame, page } => (
            (frame.x, frame.y, frame.width, frame.height),
            (frame.clip_left, frame.clip_right),
            page.map_or(1.0, |(page_width, _)| frame.width / page_width),
        ),
        viewport::Layout::Parked {
            page: (width, height),
        } => ((PARKED_ORIGIN, PARKED_ORIGIN, width, height), (0., 0.), 1.0),
    };
    view.set_zoom(zoom);
    view.set_bounds(frame, sides);
}

const MAX_HOLES: usize = 32;

pub(crate) fn validate_bounds(bounds: &BrowserBounds) -> BrowserResult<()> {
    let finite = [
        bounds.dim,
        bounds.opacity,
        bounds.bottom_radius,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        bounds.clip_left,
        bounds.clip_right,
    ]
    .into_iter()
    .chain(
        bounds
            .holes
            .iter()
            .flat_map(|hole| [hole.x, hole.y, hole.width, hole.height, hole.radius]),
    )
    .all(|value| value.is_finite() && value.abs() < 1.0e6);
    if !finite
        || bounds.holes.len() > MAX_HOLES
        || bounds
            .holes
            .iter()
            .any(|hole| hole.width <= 0.0 || hole.height <= 0.0 || hole.radius < 0.0)
        || bounds.width < 1.0
        || bounds.height < 1.0
        || bounds.clip_left < 0.0
        || bounds.clip_right < 0.0
        || bounds.clip_left + bounds.clip_right > bounds.width
        || !(0.0..=1.0).contains(&bounds.dim)
        || !(0.0..=1.0).contains(&bounds.opacity)
        || bounds.bottom_radius < 0.0
    {
        return Err(Error::BadArg("invalid browser bounds"));
    }
    Ok(())
}

/// The name the page suggested, else the last path segment of the URL, with
/// anything that could leave the download folder stripped out.
pub(crate) fn download_file_name(url: &Url, suggested: &Path) -> String {
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
pub(crate) fn unique_download_path(folder: &Path, name: &str) -> PathBuf {
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

pub(crate) fn label_safe(agent_id: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserHole;

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
    fn a_tab_sleeps_only_once_it_is_idle_and_its_latest_load_has_settled() {
        let mut strip = TabStrip::default();
        strip.insert("a".into(), page("https://a.test"));
        assert!(!strip.awake("a"));

        strip.pages.get_mut("a").unwrap().loading = true;
        assert!(strip.awake("a"));

        strip.pages.get_mut("a").unwrap().loading = false;
        strip.settle("a", 1);
        strip.settle("a", 2);
        assert!(!strip.release_settled("a", 1));
        assert!(strip.awake("a"));
        assert!(strip.release_settled("a", 2));
        assert!(!strip.awake("a"));

        strip.mark_acting(3);
        assert!(strip.awake("a"));
        strip.release_acting("a", 3);
        strip.settle("a", 4);
        strip.remove("a");
        assert!(strip.settling.is_empty());
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
    fn script_bearing_and_app_schemes_never_load_in_a_tab() {
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("DATA:text/html,hi").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("asset://localhost/x").is_err());
        assert!(validate_url("mailto:a@b.test").is_err());
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("HTTP://localhost:3000").is_ok());
        assert!(validate_url(BLANK_URL).is_ok());
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
    fn a_page_cannot_move_itself_or_a_frame_onto_an_app_origin() {
        let loads = |url: &str| tab_may_load(&Url::parse(url).unwrap());
        for allowed in [
            "https://example.com/",
            "http://localhost:1420/",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hi",
            "blob:https://example.com/0b7e",
        ] {
            assert!(loads(allowed), "{allowed} should load");
        }
        for refused in [
            "ipc://localhost/",
            "asset://localhost/x",
            "file:///etc/passwd",
            "about:config",
            "javascript:alert(1)",
        ] {
            assert!(!loads(refused), "{refused} should be refused");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_finished_download_is_quarantined() {
        use std::os::unix::ffi::OsStrExt;

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("setup.dmg");
        std::fs::write(&path, b"x").unwrap();
        quarantine(&path);
        let target = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = std::ffi::CString::new("com.apple.quarantine").unwrap();
        let mut value = [0u8; 64];
        // SAFETY: `value` is a writable buffer of `value.len()` bytes, and both names are
        // nul-terminated and live through the call.
        let read = unsafe {
            libc::getxattr(
                target.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        let value = String::from_utf8_lossy(&value[..usize::try_from(read).unwrap()]);
        assert!(value.starts_with("0083;"), "{value}");
        assert!(value.ends_with(";Sikemux;"), "{value}");
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
            clip_left: 40.0,
            clip_right: 0.0,
            holes: vec![BrowserHole {
                x: 0.0,
                y: 150.0,
                width: 120.0,
                height: 34.0,
                radius: 13.0,
            }],
            dim: 0.0,
            opacity: 1.0,
            bottom_radius: 0.0,
        };
        assert!(validate_bounds(&good).is_ok());
        assert!(validate_bounds(&BrowserBounds {
            opacity: 0.4,
            ..good.clone()
        })
        .is_ok());
        assert!(validate_bounds(&BrowserBounds {
            opacity: -0.1,
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            dim: 0.2,
            ..good.clone()
        })
        .is_ok());
        assert!(validate_bounds(&BrowserBounds {
            dim: 1.5,
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            holes: vec![BrowserHole {
                width: 0.0,
                ..good.holes[0]
            }],
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            clip_left: -1.0,
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            clip_right: 261.0,
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            width: 0.0,
            ..good.clone()
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            x: f64::NAN,
            ..good
        })
        .is_err());
    }
}
