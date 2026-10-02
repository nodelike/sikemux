use std::cmp::Ordering;
use std::collections::HashSet;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::async_runtime::spawn_blocking;

use super::claude::claude_recent;
use super::codex::codex_recent;
use super::grok::grok_recent;
use super::hermes::hermes_recent;
use super::omp::omp_recent;
use super::opencode::opencode_recent;
use super::pi::pi_recent;
use crate::agents::{AgentKind, AgentSession};

const MAX_PAGE: usize = 100;
const MAX_PROJECTS: usize = 64;
const MAX_EXCLUDED: usize = 512;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProvider {
    agent: AgentKind,
    config_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRef {
    agent: AgentKind,
    id: String,
}

/// Where the last page ended. Sessions are ordered newest first, then by
/// provider and a key that is stable for each provider, so equal times never
/// repeat or skip a row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentCursor {
    at_ms: u64,
    agent: String,
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRequest {
    providers: Vec<RecentProvider>,
    projects: Vec<String>,
    limit: usize,
    #[serde(default)]
    cursor: Option<RecentCursor>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    exclude: Vec<RecentRef>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecentSession {
    agent: &'static str,
    id: String,
    title: String,
    mtime: u64,
    project: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecentPage {
    sessions: Vec<RecentSession>,
    next: Option<RecentCursor>,
}

/// One entry a provider can list without opening it.
pub(super) struct Listed<T> {
    pub(super) at_ms: u64,
    pub(super) key: String,
    pub(super) item: T,
}

/// A listed entry once opened: the project it belongs to and what the rail shows.
#[derive(Clone)]
pub(super) struct Found {
    pub(super) project: String,
    pub(super) session: AgentSession,
}

pub(super) struct Hit {
    at_ms: u64,
    agent: &'static str,
    key: String,
    found: Found,
}

/// What one request asks every provider for.
pub(super) struct PageScan<'a> {
    agent: &'static str,
    projects: &'a HashSet<String>,
    cursor: Option<&'a RecentCursor>,
    limit: usize,
    query: Option<&'a str>,
    exclude: &'a HashSet<(String, String)>,
}

impl PageScan<'_> {
    pub(super) fn projects(&self) -> impl Iterator<Item = &String> {
        self.projects.iter()
    }

    fn after_cursor(&self, at_ms: u64, key: &str) -> bool {
        match self.cursor {
            None => true,
            Some(cursor) => {
                compare(
                    at_ms,
                    self.agent,
                    key,
                    cursor.at_ms,
                    &cursor.agent,
                    &cursor.key,
                ) == Ordering::Greater
            }
        }
    }

    fn wanted(&self, found: &Found) -> bool {
        if !self.projects.contains(&found.project) {
            return false;
        }
        if self
            .exclude
            .contains(&(self.agent.to_string(), found.session.id.clone()))
        {
            return false;
        }
        self.query
            .is_none_or(|needle| found.session.title.to_lowercase().contains(needle))
    }

    /// Sorts what a provider listed, skips everything up to the cursor, then
    /// opens entries a batch at a time until the page is full.
    pub(super) fn collect<T, F>(&self, mut listed: Vec<Listed<T>>, resolve: F) -> Vec<Hit>
    where
        T: Sync,
        F: Fn(&T) -> Option<Found> + Sync,
    {
        listed.sort_unstable_by(|a, b| {
            compare(a.at_ms, self.agent, &a.key, b.at_ms, self.agent, &b.key)
        });
        let start = listed.partition_point(|entry| !self.after_cursor(entry.at_ms, &entry.key));
        let mut hits = Vec::new();
        for batch in listed[start..].chunks(self.limit) {
            let resolved: Vec<Option<Found>> =
                batch.par_iter().map(|entry| resolve(&entry.item)).collect();
            for (entry, found) in batch.iter().zip(resolved) {
                let Some(found) = found.filter(|found| self.wanted(found)) else {
                    continue;
                };
                hits.push(Hit {
                    at_ms: entry.at_ms,
                    agent: self.agent,
                    key: entry.key.clone(),
                    found,
                });
                if hits.len() == self.limit {
                    return hits;
                }
            }
        }
        hits
    }
}

fn compare(
    a_ms: u64,
    a_agent: &str,
    a_key: &str,
    b_ms: u64,
    b_agent: &str,
    b_key: &str,
) -> Ordering {
    b_ms.cmp(&a_ms)
        .then_with(|| a_agent.cmp(b_agent))
        .then_with(|| a_key.cmp(b_key))
}

fn provider_hits(provider: &RecentProvider, scan: &PageScan<'_>) -> Vec<Hit> {
    let config_path = provider.config_path.as_deref();
    match provider.agent {
        AgentKind::Claude => claude_recent(scan, config_path),
        AgentKind::Codex => codex_recent(scan, config_path),
        AgentKind::Hermes => hermes_recent(scan),
        AgentKind::Pi => pi_recent(scan),
        AgentKind::Opencode => opencode_recent(scan),
        AgentKind::Omp => omp_recent(scan),
        AgentKind::Grok => grok_recent(scan),
    }
}

fn recent_page(request: RecentRequest) -> Result<RecentPage, String> {
    if request.limit == 0 || request.limit > MAX_PAGE {
        return Err(format!("limit must be between 1 and {MAX_PAGE}"));
    }
    if request.projects.len() > MAX_PROJECTS || request.exclude.len() > MAX_EXCLUDED {
        return Err("too many projects or excluded sessions".into());
    }
    let projects: HashSet<String> = request.projects.into_iter().collect();
    let exclude: HashSet<(String, String)> = request
        .exclude
        .into_iter()
        .map(|entry| (entry.agent.as_str().to_string(), entry.id))
        .collect();
    let query = request
        .query
        .map(|query| query.trim().to_lowercase())
        .filter(|query| !query.is_empty());
    let mut seen = HashSet::new();
    let providers: Vec<RecentProvider> = request
        .providers
        .into_iter()
        .filter(|provider| seen.insert(provider.agent.as_str()))
        .collect();

    let mut hits: Vec<Hit> = providers
        .par_iter()
        .flat_map_iter(|provider| {
            let scan = PageScan {
                agent: provider.agent.as_str(),
                projects: &projects,
                cursor: request.cursor.as_ref(),
                limit: request.limit,
                query: query.as_deref(),
                exclude: &exclude,
            };
            provider_hits(provider, &scan)
        })
        .collect();
    hits.sort_unstable_by(|a, b| compare(a.at_ms, a.agent, &a.key, b.at_ms, b.agent, &b.key));
    hits.truncate(request.limit);

    let next = (hits.len() == request.limit)
        .then(|| hits.last())
        .flatten()
        .map(|last| RecentCursor {
            at_ms: last.at_ms,
            agent: last.agent.to_string(),
            key: last.key.clone(),
        });
    let sessions = hits
        .into_iter()
        .map(|hit| RecentSession {
            agent: hit.agent,
            id: hit.found.session.id,
            title: hit.found.session.title,
            mtime: hit.found.session.mtime,
            project: hit.found.project,
        })
        .collect();
    Ok(RecentPage { sessions, next })
}

/// Saved chats across providers and projects, newest first, a page at a time.
#[tauri::command]
pub async fn agent_recent_sessions(request: RecentRequest) -> Result<RecentPage, String> {
    spawn_blocking(move || recent_page(request))
        .await
        .map_err(|error| format!("agent_recent_sessions join: {error}"))?
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use super::*;

    fn session(id: &str, title: &str, mtime: u64) -> AgentSession {
        AgentSession {
            id: id.into(),
            title: title.into(),
            mtime,
        }
    }

    fn listed(project: &str, id: &str, title: &str, at_ms: u64) -> Listed<Found> {
        Listed {
            at_ms,
            key: id.into(),
            item: Found {
                project: project.into(),
                session: session(id, title, at_ms / 1000),
            },
        }
    }

    fn scan<'a>(
        projects: &'a HashSet<String>,
        exclude: &'a HashSet<(String, String)>,
        cursor: Option<&'a RecentCursor>,
        limit: usize,
        query: Option<&'a str>,
    ) -> PageScan<'a> {
        PageScan {
            agent: "claude",
            projects,
            cursor,
            limit,
            query,
            exclude,
        }
    }

    fn ids(hits: &[Hit]) -> Vec<&str> {
        hits.iter()
            .map(|hit| hit.found.session.id.as_str())
            .collect()
    }

    fn many() -> Vec<Listed<Found>> {
        (0..30)
            .map(|index| {
                listed(
                    "/a",
                    &format!("s{index:02}"),
                    &format!("chat {index}"),
                    1_000 * (index / 2 + 1),
                )
            })
            .collect()
    }

    #[test]
    fn pages_follow_on_without_gaps_or_repeats_across_equal_times() {
        let projects = HashSet::from(["/a".to_string()]);
        let exclude = HashSet::new();
        let mut seen = Vec::new();
        let mut cursor: Option<RecentCursor> = None;
        loop {
            let page = scan(&projects, &exclude, cursor.as_ref(), 7, None)
                .collect(many(), |found| Some(found.clone()));
            if page.is_empty() {
                break;
            }
            seen.extend(ids(&page).into_iter().map(String::from));
            let last = page.last().unwrap();
            cursor = Some(RecentCursor {
                at_ms: last.at_ms,
                agent: "claude".into(),
                key: last.key.clone(),
            });
        }
        let mut expected: Vec<String> = (0..30).map(|index| format!("s{index:02}")).collect();
        expected.sort_by(|a, b| {
            let at = |id: &str| 1_000 * (id[1..].parse::<u64>().unwrap() / 2 + 1);
            at(b).cmp(&at(a)).then_with(|| a.cmp(b))
        });
        assert_eq!(seen, expected);
    }

    #[test]
    fn a_page_stops_opening_entries_once_it_is_full() {
        let projects = HashSet::from(["/a".to_string()]);
        let exclude = HashSet::new();
        let opened = AtomicUsize::new(0);
        let page = scan(&projects, &exclude, None, 5, None).collect(many(), |found| {
            opened.fetch_add(1, AtomicOrdering::Relaxed);
            Some(found.clone())
        });
        assert_eq!(page.len(), 5);
        assert_eq!(opened.load(AtomicOrdering::Relaxed), 5);
    }

    #[test]
    fn other_projects_excluded_sessions_and_non_matches_are_skipped_without_shortening_the_page() {
        let projects = HashSet::from(["/a".to_string()]);
        let exclude = HashSet::from([("claude".to_string(), "s29".to_string())]);
        let mut entries = many();
        entries.push(listed("/b", "elsewhere", "chat 2 elsewhere", 99_000));
        let page = scan(&projects, &exclude, None, 3, Some("chat 2"))
            .collect(entries, |found| Some(found.clone()));
        assert_eq!(ids(&page), vec!["s28", "s26", "s27"]);
    }

    fn write_at(path: &std::path::Path, text: &str, secs: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    fn request(
        providers: Vec<RecentProvider>,
        projects: &[&str],
        limit: usize,
        cursor: Option<RecentCursor>,
    ) -> RecentRequest {
        RecentRequest {
            providers,
            projects: projects.iter().map(|project| project.to_string()).collect(),
            limit,
            cursor,
            query: None,
            exclude: vec![],
        }
    }

    #[test]
    fn claude_and_codex_chats_from_several_projects_page_newest_first() {
        let claude = tempfile::tempdir().unwrap();
        let codex = tempfile::tempdir().unwrap();
        let prompt =
            |text: &str| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{text}\"}}}}\n");
        write_at(
            &claude.path().join("projects/-a/c1.jsonl"),
            &prompt("claude in a"),
            400,
        );
        write_at(
            &claude.path().join("projects/-b/c2.jsonl"),
            &prompt("claude in b"),
            200,
        );
        write_at(
            &claude.path().join("projects/-c/c3.jsonl"),
            &prompt("claude in c"),
            500,
        );
        let rollout = |id: &str, cwd: &str| {
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\"}}}}\n"
            )
        };
        write_at(
            &codex.path().join("sessions/2026/09/30/x1.jsonl"),
            &rollout("x1", "/a"),
            300,
        );
        write_at(
            &codex.path().join("sessions/2026/09/30/x2.jsonl"),
            &rollout("x2", "/c"),
            600,
        );
        write_at(
            &codex.path().join("sessions/2026/09/29/x3.jsonl"),
            &rollout("x3", "/b"),
            100,
        );
        let providers = || {
            vec![
                RecentProvider {
                    agent: AgentKind::Claude,
                    config_path: claude.path().to_str().map(String::from),
                },
                RecentProvider {
                    agent: AgentKind::Codex,
                    config_path: codex.path().to_str().map(String::from),
                },
            ]
        };

        let first = recent_page(request(providers(), &["/a", "/b"], 2, None)).unwrap();
        fn listed(page: &RecentPage) -> Vec<(&str, &str, u64)> {
            page.sessions
                .iter()
                .map(|s| (s.agent, s.project.as_str(), s.mtime))
                .collect()
        }
        assert_eq!(
            listed(&first),
            vec![("claude", "/a", 400), ("codex", "/a", 300)]
        );
        let second =
            recent_page(request(providers(), &["/a", "/b"], 2, first.next.clone())).unwrap();
        assert_eq!(
            listed(&second),
            vec![("claude", "/b", 200), ("codex", "/b", 100)]
        );
        let third =
            recent_page(request(providers(), &["/a", "/b"], 2, second.next.clone())).unwrap();
        assert!(third.sessions.is_empty() && third.next.is_none());
    }

    /// Times pages against this machine's real transcripts: `cargo test --lib real_history -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_history_pages_quickly() {
        let projects: Vec<String> = std::env::var("SIKEMUX_RECENT_PROJECTS")
            .map(|value| value.split(':').map(String::from).collect())
            .unwrap_or_default();
        sikemux_pty::user_shell::login_shell_environment();
        let providers = || -> Vec<RecentProvider> {
            [
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Hermes,
                AgentKind::Pi,
                AgentKind::Opencode,
                AgentKind::Omp,
                AgentKind::Grok,
            ]
            .into_iter()
            .map(|agent| RecentProvider {
                agent,
                config_path: None,
            })
            .collect()
        };
        let projects_set: HashSet<String> = projects.iter().cloned().collect();
        let exclude = HashSet::new();
        for provider in providers() {
            let scan = PageScan {
                agent: provider.agent.as_str(),
                projects: &projects_set,
                cursor: None,
                limit: 12,
                query: None,
                exclude: &exclude,
            };
            let started = std::time::Instant::now();
            let found = provider_hits(&provider, &scan).len();
            println!(
                "{}: {found} in {:?}",
                provider.agent.as_str(),
                started.elapsed()
            );
        }
        let mut cursor = None;
        for page in 0..4 {
            let started = std::time::Instant::now();
            let result = recent_page(RecentRequest {
                providers: providers(),
                projects: projects.clone(),
                limit: 12,
                cursor: cursor.clone(),
                query: None,
                exclude: vec![],
            })
            .unwrap();
            println!(
                "page {page}: {} chats in {:?}",
                result.sessions.len(),
                started.elapsed()
            );
            cursor = result.next;
            if cursor.is_none() {
                break;
            }
        }
    }

    #[test]
    fn requests_outside_the_limits_are_refused() {
        let request = |limit| RecentRequest {
            providers: vec![],
            projects: vec![],
            limit,
            cursor: None,
            query: None,
            exclude: vec![],
        };
        assert!(recent_page(request(0)).is_err());
        assert!(recent_page(request(MAX_PAGE + 1)).is_err());
        let empty = recent_page(request(12)).unwrap();
        assert!(empty.sessions.is_empty() && empty.next.is_none());
    }
}
