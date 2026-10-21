//! Checks the CI of every code-host repository a client has on screen or an
//! Auto card waits on, once however many look, and tells them all when its
//! runs move. A client says which repositories it shows; the list goes with
//! its connection.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::changed::Topic;
use super::connection::ClientId;
use super::{plugins, Core};

const TICK: Duration = Duration::from_secs(5);
/// How often a repository is checked while one of its runs is going.
const LIVE: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(20);
const RUNS_CHECKED: u32 = 20;

/// A repository on a code host, as the client names it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shown {
    /// The plugin that speaks for the host, such as `sikemux.github`.
    pub provider: String,
    pub owner: String,
    pub name: String,
    pub account: Option<String>,
    pub host: Option<String>,
}

impl Shown {
    /// What a `ci` change names it by.
    fn key(&self) -> String {
        format!("{}:{}/{}", self.provider, self.owner, self.name)
    }
}

struct Seen {
    runs: u64,
    going: bool,
    at: Instant,
}

#[derive(Default)]
struct State {
    shown: HashMap<ClientId, HashSet<Shown>>,
    seen: HashMap<Shown, Seen>,
    checking: bool,
}

#[derive(Default)]
pub(crate) struct Ci(Mutex<State>);

#[derive(Deserialize)]
pub(crate) struct ShowParams {
    repos: Vec<Shown>,
}

/// The repositories `client` shows now, in place of those it showed before.
pub(crate) fn show(core: &Arc<Core>, client: ClientId, params: ShowParams) {
    {
        let mut state = core.ci.0.locked();
        if params.repos.is_empty() {
            state.shown.remove(&client);
        } else {
            state
                .shown
                .insert(client, params.repos.into_iter().collect());
        }
    }
    start(core);
}

/// Starts checking, unless it already runs; it stops by itself once nothing
/// is looked at.
pub(crate) fn start(core: &Arc<Core>) {
    if !std::mem::replace(&mut core.ci.0.locked().checking, true) {
        tokio::spawn(serve(core.clone()));
    }
}

async fn serve(core: Arc<Core>) {
    loop {
        let due = {
            let connected: HashSet<ClientId> =
                core.clients().iter().map(|client| client.id).collect();
            let waiting = super::board::waiting_on_ci(&core);
            let mut state = core.ci.0.locked();
            state.shown.retain(|client, _| connected.contains(client));
            let shown: HashSet<Shown> = state
                .shown
                .values()
                .flatten()
                .cloned()
                .chain(waiting)
                .collect();
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
                        seen.at.elapsed() >= if seen.going { LIVE } else { IDLE }
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
        "owner": repo.owner,
        "name": repo.name,
        "account": repo.account,
        "host": repo.host,
        "perPage": RUNS_CHECKED,
    });
    let Ok(page) = plugins::call(&repo.provider, "runs", params).await else {
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
        let mut state = core.ci.0.locked();
        let before = state.seen.insert(
            repo.clone(),
            Seen {
                runs: fingerprint,
                going,
                at: Instant::now(),
            },
        );
        before.is_some_and(|before| before.runs != fingerprint)
    };
    if moved {
        core.changes
            .changed(Topic::Ci, None, Some(vec![repo.key()]));
        super::board::ci_moved(core, &repo);
    }
}
