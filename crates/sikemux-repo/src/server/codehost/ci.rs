//! Watches the CI of every code-host repository an Auto card, a project's check,
//! a pushed branch or a CI trigger waits on, once however many wait, and tells them all when
//! its runs move. A repository whose host can send CI webhooks gets one, through the accounts
//! server, and is never asked again; its plugin keeps the runs the hook tells of. Any other is
//! asked every few seconds while something waits on it.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::repo::RepoId;
use crate::server::{plugged, Core};

const TICK: Duration = Duration::from_secs(5);
/// How often a repository is checked while one of its runs is going.
const LIVE: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(20);
const RUNS_CHECKED: u32 = 20;
/// A host that would not make a repository's hook is asked again after this.
const HOOK_REFUSED: Duration = Duration::from_secs(6 * 60 * 60);
const FEEDS_FILE: &str = "ci-feeds.json";

/// A repository on a code host, as the core names it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Shown {
    #[serde(flatten)]
    pub id: RepoId,
    pub account: Option<String>,
    pub host: Option<String>,
}

struct Seen {
    runs: u64,
    going: bool,
    /// Its runs moved at the last check, so the next check comes sooner.
    moved: bool,
    at: Instant,
}

/// A repository's CI webhook: the accounts server's address for it, and the secret its host
/// signs each request with.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    repo: Shown,
    hook_id: String,
    secret: String,
}

#[derive(Default)]
struct State {
    seen: HashMap<Shown, Seen>,
    checking: bool,
    feeds: Option<Vec<Feed>>,
    adding: HashSet<RepoId>,
    /// Hosts that would not make a hook, and when; never again when they have no hooks at all.
    refused: HashMap<RepoId, Option<Instant>>,
}

impl State {
    fn feeds(&mut self, core: &Core) -> &mut Vec<Feed> {
        self.feeds.get_or_insert_with(|| {
            (core.data_dir().as_ref())
                .and_then(|dir| {
                    sikemux_process::files::read_or_set_aside(&dir.join(FEEDS_FILE), |bytes| {
                        serde_json::from_slice(bytes)
                    })
                })
                .unwrap_or_default()
        })
    }

    fn fed(&mut self, core: &Core, repo: &RepoId) -> bool {
        self.feeds(core).iter().any(|feed| &feed.repo.id == repo)
    }

    /// Whether to ask the host for a hook on `repo` now.
    fn wants_hook(&mut self, core: &Core, repo: &RepoId) -> bool {
        let refused = self
            .refused
            .get(repo)
            .is_some_and(|at| at.is_none_or(|at| at.elapsed() < HOOK_REFUSED));
        !refused && !self.adding.contains(repo) && !self.fed(core, repo)
    }
}

fn save_feeds(core: &Core, feeds: &[Feed]) {
    let Some(path) = core.data_dir().as_ref().map(|dir| dir.join(FEEDS_FILE)) else {
        return;
    };
    let written = serde_json::to_vec(feeds)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            sikemux_process::files::write_atomic(&path, &bytes, true)
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!("sikemux core: the CI webhooks were not saved: {error}");
    }
}

#[derive(Default)]
pub struct Ci(Mutex<State>);

/// Starts checking, unless it already runs; it stops by itself once nothing
/// waits.
pub fn start(core: &Arc<Core>) {
    if !std::mem::replace(&mut core.ci().0.locked().checking, true) {
        tokio::spawn(serve(core.clone()));
    }
}

async fn serve(core: Arc<Core>) {
    loop {
        let due = {
            let shown: HashSet<Shown> = (core.repo_above().git_pending_pulls)(&core)
                .into_iter()
                .chain(core.checks().waiting_on_ci())
                .chain((core.repo_above().triggers_waiting_on_ci)(&core))
                .collect();
            let mut state = core.ci().0.locked();
            if shown.is_empty() {
                state.seen.clear();
                state.checking = false;
                return;
            }
            state.seen.retain(|repo, _| shown.contains(repo));
            for repo in &shown {
                if state.wants_hook(&core, &repo.id) {
                    state.adding.insert(repo.id.clone());
                    tokio::spawn(add_feed(core.clone(), repo.clone()));
                }
            }
            let unfed: Vec<Shown> = (shown.into_iter())
                .filter(|repo| !state.fed(&core, &repo.id))
                .collect();
            unfed
                .into_iter()
                .filter(|repo| {
                    state.seen.get(repo).is_none_or(|seen| {
                        seen.at.elapsed() >= if seen.going || seen.moved { LIVE } else { IDLE }
                    })
                })
                .collect::<Vec<_>>()
        };
        for repo in due {
            check(&core, repo).await;
        }
        tokio::time::sleep(TICK).await;
    }
}

async fn check(core: &Arc<Core>, repo: Shown) {
    let params = json!({
        "owner": repo.id.owner,
        "name": repo.id.name,
        "account": repo.account,
        "host": repo.host,
        "perPage": RUNS_CHECKED,
    });
    let Ok(page) = plugged::call(core, &repo.id.provider, "runs", params).await else {
        return;
    };
    let runs = page.get("runs").cloned().unwrap_or(Value::Null);
    let mut hasher = DefaultHasher::new();
    runs.to_string().hash(&mut hasher);
    let fingerprint = hasher.finish();
    let going = runs.as_array().is_some_and(|runs| {
        runs.iter()
            .any(|run| run.get("status").and_then(Value::as_str) != Some("completed"))
    });
    let moved = {
        let mut state = core.ci().0.locked();
        let before = state.seen.get(&repo);
        let moved = before.is_some_and(|before| before.runs != fingerprint);
        state.seen.insert(
            repo.clone(),
            Seen {
                runs: fingerprint,
                going,
                moved,
                at: Instant::now(),
            },
        );
        moved
    };
    if moved {
        tell_moved(core, &repo);
    }
}

/// Has the repository's host send its CI events to a webhook of this host, so it need not be
/// asked again.
async fn add_feed(core: Arc<Core>, repo: Shown) {
    let refused = match hook(&core, &repo).await {
        Ok(feed) => {
            let mut state = core.ci().0.locked();
            state.feeds(&core).push(feed);
            let feeds = state.feeds(&core).clone();
            save_feeds(&core, &feeds);
            None
        }
        Err(refused) => Some(refused),
    };
    let mut state = core.ci().0.locked();
    state.adding.remove(&repo.id);
    if let Some(retry) = refused {
        state.refused.insert(repo.id, retry.then(Instant::now));
    }
}

/// The hook made for `repo`, or whether to try again later when none could be.
async fn hook(core: &Arc<Core>, repo: &Shown) -> Result<Feed, bool> {
    let added = (core.repo_above().hooks_register)(core)
        .await
        .map_err(|_| true)?;
    let (hook_id, url) = (added.id, added.url);
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let params = json!({
        "owner": repo.id.owner,
        "name": repo.id.name,
        "account": repo.account,
        "host": repo.host,
        "url": url,
        "secret": secret,
    });
    match plugged::call(core, &repo.id.provider, "addCiHook", params).await {
        Ok(_) => Ok(Feed {
            repo: repo.clone(),
            hook_id,
            secret,
        }),
        Err(failure) => {
            drop((core.repo_above().hooks_remove)(core, &hook_id).await);
            let retry = failure.category != sikemux_plugin_api::FailureCategory::UnknownMethod;
            if retry {
                eprintln!(
                    "sikemux core: {} would not send CI webhooks, so its CI is asked for instead: {}",
                    repo.id, failure.message
                );
            }
            Err(retry)
        }
    }
}

/// Whether a webhook of this host is one a repository's CI events come to.
pub fn feeds_hook(core: &Core, hook_id: &str) -> bool {
    let mut state = core.ci().0.locked();
    state.feeds(core).iter().any(|feed| feed.hook_id == hook_id)
}

/// Hands a request to a repository's CI webhook to the plugin that keeps its runs, and tells
/// whatever waits on the repository when one of its runs moved.
pub async fn delivered(
    core: &Arc<Core>,
    hook_id: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(), String> {
    let feed = {
        let mut state = core.ci().0.locked();
        let feeds = state.feeds(core);
        feeds.iter().find(|feed| feed.hook_id == hook_id).cloned()
    };
    let Some(feed) = feed else {
        return Ok(());
    };
    let header = |name: &str| {
        headers
            .iter()
            .find(|(given, _)| given.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    let params = json!({
        "event": header("x-github-event").unwrap_or_default(),
        "signature": header("x-hub-signature-256"),
        "secret": feed.secret,
        "body": String::from_utf8_lossy(body),
    });
    let moved = plugged::call(core, &feed.repo.id.provider, "ciEvent", params)
        .await
        .map_err(|failure| failure.message)?;
    if moved.get("runMoved").and_then(Value::as_bool) == Some(true) {
        let watched: Vec<Shown> = {
            let state = core.ci().0.locked();
            state
                .seen
                .keys()
                .filter(|repo| repo.id == feed.repo.id)
                .cloned()
                .chain(std::iter::once(feed.repo.clone()))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        for repo in watched {
            tell_moved(core, &repo);
        }
    }
    Ok(())
}

fn tell_moved(core: &Arc<Core>, repo: &Shown) {
    (core.repo_above().git_ci_moved)(core, repo);
    core.checks().ci_moved(repo);
    (core.repo_above().triggers_ci_moved)(core, repo);
}

/// Whether the repository's CI events come to this host, so nothing need ask for them.
pub fn is_fed(core: &Core, repo: &RepoId) -> bool {
    core.ci().0.locked().fed(core, repo)
}
