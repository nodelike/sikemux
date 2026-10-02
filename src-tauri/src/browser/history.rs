//! Pages the desk tabs have shown, kept on disk so the address bar can
//! suggest them while someone types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Url};

use super::{browser_state_dir, normalize_url};

const FILE_NAME: &str = "history.json";
const MAX_PAGES: usize = 3000;
const MAX_SUGGESTIONS: usize = 6;
const MAX_TOP_SITES: usize = 8;
const MAX_TITLE_LEN: usize = 300;
/// Visits land in bursts while a page redirects, so they are written together.
const SAVE_DELAY: Duration = Duration::from_secs(2);
/// A typed address says far more about what someone wants back than a page an
/// agent or a link happened to pass through.
const TYPED_WEIGHT: f64 = 4.0;
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
struct Visit {
    url: String,
    title: String,
    visits: u32,
    typed: u32,
    last: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Store {
    pages: Vec<Visit>,
    /// Site icons as data URLs, one per host, so a suggestion can show one
    /// without the page being open.
    icons: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub url: String,
    pub title: String,
    /// The address as the bar writes it: no scheme and no `www.`.
    pub address: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AddressSuggestions {
    /// A remembered address that begins with what was typed, which the bar
    /// finishes in place.
    pub completion: Option<Suggestion>,
    pub pages: Vec<Suggestion>,
    /// Whether the typed text, sent as it is, would be a web search.
    pub searches: bool,
    pub search_url: String,
}

#[derive(Default)]
pub(super) struct History {
    store: Mutex<Option<Store>>,
    saving: AtomicBool,
}

impl History {
    /// Remembers a page a tab finished showing, or only freshens the title and
    /// icon of one already remembered when `visited` is false.
    pub fn note(&self, app: &AppHandle, url: &str, title: &str, icon: Option<&str>, visited: bool) {
        let Some(url) = remembered(url) else {
            return;
        };
        let changed = self.with_store(app, |store| store.note(&url, title, icon, visited, now()));
        if changed {
            self.save_soon(app);
        }
    }

    pub fn note_typed(&self, app: &AppHandle, url: &str) {
        let Some(url) = remembered(url) else {
            return;
        };
        self.with_store(app, |store| store.note_typed(&url, now()));
        self.save_soon(app);
    }

    pub fn suggest(&self, app: &AppHandle, query: &str) -> AddressSuggestions {
        self.with_store(app, |store| store.suggest(query, now()))
    }

    fn with_store<R>(&self, app: &AppHandle, work: impl FnOnce(&mut Store) -> R) -> R {
        let mut slot = self
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let store =
            slot.get_or_insert_with(|| path(app).map(|path| load(&path)).unwrap_or_default());
        work(store)
    }

    fn save_soon(&self, app: &AppHandle) {
        if self.saving.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            let manager = tauri::Manager::state::<super::BrowserManager>(&app);
            manager.history.saving.store(false, Ordering::Release);
            let Some(path) = path(&app) else {
                return;
            };
            let data = {
                let slot = manager
                    .history
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match slot.as_ref().map(serde_json::to_vec) {
                    Some(Ok(data)) => data,
                    _ => return,
                }
            };
            let written = tauri::async_runtime::spawn_blocking(move || save(&path, &data)).await;
            if let Ok(Err(error)) = written {
                eprintln!("could not save browser history: {error}");
            }
        });
    }
}

impl Store {
    fn note(
        &mut self,
        url: &str,
        title: &str,
        icon: Option<&str>,
        visited: bool,
        now: u64,
    ) -> bool {
        let mut changed = false;
        if let (Some(host), Some(icon)) = (host_of(url), icon) {
            if self.icons.get(&host).map(String::as_str) != Some(icon) {
                self.icons.insert(host, icon.to_owned());
                changed = true;
            }
        }
        let title: String = title.trim().chars().take(MAX_TITLE_LEN).collect();
        match self.pages.iter_mut().find(|page| page.url == url) {
            Some(page) => {
                if visited {
                    page.visits = page.visits.saturating_add(1);
                    page.last = now;
                    changed = true;
                }
                if !title.is_empty() && page.title != title {
                    page.title = title;
                    changed = true;
                }
            }
            None if visited => {
                self.pages.push(Visit {
                    url: url.to_owned(),
                    title,
                    visits: 1,
                    typed: 0,
                    last: now,
                });
                self.trim();
                changed = true;
            }
            None => {}
        }
        changed
    }

    fn note_typed(&mut self, url: &str, now: u64) {
        match self.pages.iter_mut().find(|page| page.url == url) {
            Some(page) => {
                page.typed = page.typed.saturating_add(1);
                page.last = now;
            }
            None => {
                self.pages.push(Visit {
                    url: url.to_owned(),
                    typed: 1,
                    last: now,
                    ..Visit::default()
                });
                self.trim();
            }
        }
    }

    fn trim(&mut self) {
        if self.pages.len() <= MAX_PAGES {
            return;
        }
        self.pages.sort_by_key(|page| std::cmp::Reverse(page.last));
        self.pages.truncate(MAX_PAGES);
        let hosts: std::collections::HashSet<String> = self
            .pages
            .iter()
            .filter_map(|page| host_of(&page.url))
            .collect();
        self.icons.retain(|host, _| hosts.contains(host));
    }

    fn suggest(&self, query: &str, now: u64) -> AddressSuggestions {
        let typed = query.trim();
        let search_url = search_url(typed);
        let searches = normalize_url(typed) == search_url;
        let needle = typed.to_lowercase();
        let words: Vec<&str> = needle.split_whitespace().collect();
        if words.is_empty() {
            return AddressSuggestions {
                completion: None,
                pages: self.top_sites(now),
                searches,
                search_url,
            };
        }

        let mut scored: Vec<(f64, &Visit)> = self
            .pages
            .iter()
            .filter(|page| page.visits > 0)
            .filter_map(|page| {
                let address = address_of(&page.url).to_lowercase();
                let title = page.title.to_lowercase();
                if !words
                    .iter()
                    .all(|word| address.contains(word) || title.contains(word))
                {
                    return None;
                }
                let fit = if address.starts_with(&needle) {
                    4.0
                } else if words
                    .iter()
                    .any(|word| starts_a_word(&address, word) || starts_a_word(&title, word))
                {
                    2.0
                } else {
                    1.0
                };
                Some((fit * frecency(page, now), page))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        let completion = if needle.contains(char::is_whitespace) {
            None
        } else {
            self.complete(&needle, now)
        };
        let pages = scored
            .into_iter()
            .map(|(_, page)| page)
            .filter(|page| completion.as_ref().is_none_or(|done| done.url != page.url))
            .take(MAX_SUGGESTIONS)
            .map(|page| self.suggestion(&page.url, &page.title))
            .collect();
        AddressSuggestions {
            completion,
            pages,
            searches,
            search_url,
        }
    }

    /// The best remembered address that begins with what was typed, cut back
    /// to the site alone unless the typing has already gone past it.
    fn complete(&self, needle: &str, now: u64) -> Option<Suggestion> {
        let mut typed_on_site: HashMap<String, u32> = HashMap::new();
        for page in self.pages.iter().filter(|page| page.typed > 0) {
            if let Some(host) = host_of(&page.url) {
                *typed_on_site.entry(host).or_default() += page.typed;
            }
        }
        let score = |page: &Visit| {
            let typed = host_of(&page.url).and_then(|host| typed_on_site.get(&host).copied());
            frecency(page, now) + TYPED_WEIGHT * f64::from(typed.unwrap_or(0))
        };
        let (page, address) = self
            .pages
            .iter()
            .filter(|page| page.visits > 0)
            .filter_map(|page| {
                let address = address_of(&page.url);
                let from = [address.as_str(), full_address(&page.url).as_str()]
                    .into_iter()
                    .find(|candidate| {
                        candidate.to_lowercase().starts_with(needle)
                            && candidate.len() > needle.len()
                    })?
                    .to_owned();
                Some((page, from))
            })
            .max_by(|a, b| score(a.0).total_cmp(&score(b.0)))?;

        let site = root_of(&page.url)?;
        let site_address = address_of(&site);
        if needle.contains('/')
            || !site_address.to_lowercase().starts_with(needle)
                && !full_address(&site).to_lowercase().starts_with(needle)
        {
            return Some(Suggestion {
                address,
                ..self.suggestion(&page.url, &page.title)
            });
        }
        let title = self.site_title(&site);
        let address = if address.to_lowercase().starts_with("www.") {
            full_address(&site)
        } else {
            site_address
        };
        Some(Suggestion {
            address,
            ..self.suggestion(&site, &title)
        })
    }

    /// The sites visited most, each offered at its front page, for when nothing
    /// has been typed yet.
    fn top_sites(&self, now: u64) -> Vec<Suggestion> {
        let mut sites: HashMap<String, f64> = HashMap::new();
        for page in &self.pages {
            if let Some(site) = root_of(&page.url) {
                *sites.entry(site).or_default() += frecency(page, now);
            }
        }
        let visited: std::collections::HashSet<String> = self
            .pages
            .iter()
            .filter(|page| page.visits > 0)
            .filter_map(|page| root_of(&page.url))
            .collect();
        let mut ranked: Vec<(String, f64)> = sites
            .into_iter()
            .filter(|(site, _)| visited.contains(site))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked
            .into_iter()
            .take(MAX_TOP_SITES)
            .map(|(site, _)| self.suggestion(&site, &self.site_title(&site)))
            .collect()
    }

    /// The front page's own title when it was ever opened, else the site's name.
    fn site_title(&self, site: &str) -> String {
        self.pages
            .iter()
            .find(|page| page.url == site && !page.title.is_empty())
            .map_or_else(
                || address_of(site).trim_end_matches('/').to_owned(),
                |page| page.title.clone(),
            )
    }

    fn suggestion(&self, url: &str, title: &str) -> Suggestion {
        Suggestion {
            url: url.to_owned(),
            title: title.to_owned(),
            address: address_of(url),
            icon: host_of(url).and_then(|host| self.icons.get(&host).cloned()),
        }
    }
}

/// Recent and often-visited pages come first; a page not seen for months
/// keeps only a sliver of what its visits earned.
fn frecency(page: &Visit, now: u64) -> f64 {
    let age = now.saturating_sub(page.last) / DAY;
    let recency = match age {
        0..=3 => 1.0,
        4..=14 => 0.7,
        15..=31 => 0.5,
        32..=90 => 0.3,
        _ => 0.1,
    };
    (f64::from(page.visits) + TYPED_WEIGHT * f64::from(page.typed)) * recency
}

fn starts_a_word(text: &str, word: &str) -> bool {
    text.match_indices(word)
        .any(|(at, _)| at == 0 || text[..at].ends_with(|c: char| !c.is_alphanumeric()))
}

/// Only web pages are worth suggesting, and a page is the same page whichever
/// part of it the address pointed at.
fn remembered(url: &str) -> Option<String> {
    let mut parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

fn root_of(url: &str) -> Option<String> {
    let mut url = Url::parse(url).ok()?;
    url.host_str()?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .host_str()
        .map(|host| host.trim_start_matches("www.").to_owned())
}

fn full_address(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .to_owned()
}

fn address_of(url: &str) -> String {
    let rest = full_address(url);
    rest.strip_prefix("www.")
        .map_or(rest.clone(), str::to_owned)
}

pub(super) fn search_url(query: &str) -> String {
    let query = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    format!("https://www.google.com/search?q={query}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    browser_state_dir(app).ok().map(|dir| dir.join(FILE_NAME))
}

fn load(path: &Path) -> Store {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn save(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, data)?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn visit(store: &mut Store, url: &str, title: &str, times: u32) {
        for _ in 0..times {
            store.note(url, title, None, true, NOW);
        }
    }

    #[test]
    fn finishes_a_typed_site_to_its_root() {
        let mut store = Store::default();
        visit(&mut store, "https://www.youtube.com/", "YouTube", 3);
        visit(
            &mut store,
            "https://www.youtube.com/watch?v=abc",
            "A video - YouTube",
            5,
        );

        let found = store.suggest("you", NOW);
        let completion = found.completion.unwrap();
        assert_eq!(completion.address, "youtube.com/");
        assert_eq!(completion.url, "https://www.youtube.com/");
        assert_eq!(completion.title, "YouTube");
        assert_eq!(
            found
                .pages
                .iter()
                .map(|page| page.url.as_str())
                .collect::<Vec<_>>(),
            ["https://www.youtube.com/watch?v=abc"]
        );
        assert!(found.searches);
    }

    #[test]
    fn keeps_www_when_it_was_typed() {
        let mut store = Store::default();
        visit(&mut store, "https://www.youtube.com/", "YouTube", 1);
        assert_eq!(
            store.suggest("www.yo", NOW).completion.unwrap().address,
            "www.youtube.com/"
        );
    }

    #[test]
    fn follows_a_path_once_it_is_typed() {
        let mut store = Store::default();
        visit(
            &mut store,
            "https://github.com/nodelike?tab=repositories",
            "Your Repositories",
            2,
        );
        let completion = store.suggest("github.com/no", NOW).completion.unwrap();
        assert_eq!(completion.address, "github.com/nodelike?tab=repositories");
        assert_eq!(
            completion.url,
            "https://github.com/nodelike?tab=repositories"
        );
    }

    #[test]
    fn matches_titles_and_ranks_by_use() {
        let mut store = Store::default();
        visit(
            &mut store,
            "https://studio.youtube.com/analytics",
            "Video analytics - YouTube Studio",
            1,
        );
        visit(
            &mut store,
            "https://www.youtube.com/watch?v=abc",
            "KREAM - YouTube",
            6,
        );
        let found = store.suggest("youtube", NOW);
        assert_eq!(
            found
                .pages
                .iter()
                .map(|page| page.title.as_str())
                .collect::<Vec<_>>(),
            ["KREAM - YouTube", "Video analytics - YouTube Studio"]
        );
        assert!(store
            .suggest("youtube kream", NOW)
            .pages
            .iter()
            .all(|page| page.title == "KREAM - YouTube"));
    }

    #[test]
    fn typed_addresses_outrank_pages_passed_through() {
        let mut store = Store::default();
        visit(&mut store, "https://www.example.com/", "Example", 1);
        visit(&mut store, "https://example.org/", "Other", 3);
        store.note_typed("https://example.com/", NOW);
        assert_eq!(
            store.suggest("exa", NOW).completion.unwrap().url,
            "https://www.example.com/"
        );
    }

    #[test]
    fn a_typed_address_never_opened_is_not_suggested() {
        let mut store = Store::default();
        store.note_typed("https://typo.example/", NOW);
        let found = store.suggest("typo", NOW);
        assert!(found.pages.is_empty());
        assert!(found.completion.is_none());
    }

    #[test]
    fn only_web_pages_are_remembered_and_fragments_fold_together() {
        assert_eq!(remembered("about:blank"), None);
        assert_eq!(remembered("file:///tmp/a.html"), None);
        assert_eq!(
            remembered("https://a.dev/docs#intro").as_deref(),
            Some("https://a.dev/docs")
        );
    }

    #[test]
    fn title_changes_do_not_count_as_visits() {
        let mut store = Store::default();
        assert!(!store.note("https://a.dev/", "A", None, false, NOW));
        visit(&mut store, "https://a.dev/", "", 1);
        assert!(store.note(
            "https://a.dev/",
            "A dev",
            Some("data:image/png;base64,x"),
            false,
            NOW
        ));
        assert_eq!(store.pages[0].visits, 1);
        assert_eq!(store.pages[0].title, "A dev");
        assert_eq!(
            store
                .suggest("a.d", NOW)
                .completion
                .unwrap()
                .icon
                .as_deref(),
            Some("data:image/png;base64,x")
        );
    }

    #[test]
    fn offers_the_most_visited_sites_when_nothing_is_typed() {
        let mut store = Store::default();
        visit(&mut store, "https://www.youtube.com/", "YouTube", 1);
        visit(
            &mut store,
            "https://www.youtube.com/watch?v=a",
            "A - YouTube",
            3,
        );
        visit(
            &mut store,
            "https://www.youtube.com/watch?v=b",
            "B - YouTube",
            3,
        );
        visit(&mut store, "https://github.com/nodelike", "nodelike", 4);
        visit(&mut store, "http://localhost:3000/app", "Dev", 2);
        store.note_typed("https://never.example/", NOW);

        let found = store.suggest("  ", NOW);
        assert_eq!(found.completion, None);
        assert_eq!(
            found
                .pages
                .iter()
                .map(|page| (
                    page.url.as_str(),
                    page.title.as_str(),
                    page.address.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("https://www.youtube.com/", "YouTube", "youtube.com/"),
                ("https://github.com/", "github.com", "github.com/"),
                (
                    "http://localhost:3000/",
                    "localhost:3000",
                    "localhost:3000/"
                ),
            ]
        );
    }

    #[test]
    fn a_search_row_is_offered_for_the_typed_text() {
        let found = Store::default().suggest("rust lifetimes", NOW);
        assert!(found.searches);
        assert_eq!(
            found.search_url,
            "https://www.google.com/search?q=rust+lifetimes"
        );
        assert!(!Store::default().suggest("example.com", NOW).searches);
    }

    #[test]
    fn forgets_the_oldest_pages_past_the_limit() {
        let mut store = Store::default();
        for index in 0..=MAX_PAGES {
            store.note(
                &format!("https://site{index}.dev/"),
                "",
                None,
                true,
                NOW + index as u64,
            );
        }
        assert_eq!(store.pages.len(), MAX_PAGES);
        assert!(store
            .pages
            .iter()
            .all(|page| page.url != "https://site0.dev/"));
    }
}
