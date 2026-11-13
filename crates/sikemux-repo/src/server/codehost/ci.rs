//! Checks the CI of every code-host repository an Auto card, a project's check,
//! a pushed branch or a CI trigger waits on, once however many wait, and tells them all when
//! its runs move.

use crate::server::repo_state::RepoParts;
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

#[derive(Default)]
struct State {
    seen: HashMap<Shown, Seen>,
    checking: bool,
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
            let shown: HashSet<Shown> = (core.repo_above().board_waiting_on_ci)(&core)
                .into_iter()
                .chain((core.repo_above().git_pending_pulls)(&core))
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
            shown
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
        (core.repo_above().board_ci_moved)(core, &repo);
        (core.repo_above().git_ci_moved)(core, &repo);
        core.checks().ci_moved(&repo);
        (core.repo_above().triggers_ci_moved)(core, &repo);
    }
}
