//! What plugins' panes show, read and kept by the core. A plugin's manifest
//! names its views and how often each is read again while a pane shows it,
//! and its writes, after which its views are read again at once. Clients are
//! told when a view changed, so none keeps time of its own.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use super::changed::Topic;
use super::connection::ClientId;
use super::plugins::{self, Failure, Outcome};
use super::Core;

/// A view no pane shows any more is kept this long after it was asked for.
const GRACE: Duration = Duration::from_secs(60);
/// The soonest and the latest the loop looks again.
const QUICKEST: Duration = Duration::from_secs(1);
const SLOWEST: Duration = Duration::from_secs(15);
/// A view that failed is not read again for longer than this.
const LONGEST_BACKOFF: Duration = Duration::from_secs(300);
const KEPT_AT_MOST: usize = 64;
/// A service lists a change a moment after it took it.
const SETTLING: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    plugin: String,
    method: String,
    /// The view's parameters as JSON, so equal ones share it.
    params: String,
}

impl Key {
    fn path(&self) -> String {
        format!("{}/{}", self.plugin, self.method)
    }
}

struct Kept {
    answer: Value,
    read: Instant,
    /// How long after `read` it is read again while shown.
    fresh: Duration,
    /// What the manifest asks for, which `fresh` returns to after a failure.
    every: Duration,
    asked: Instant,
}

#[derive(Default)]
struct State {
    kept: HashMap<Key, Kept>,
    /// The view each place in each connection last asked for, which it shows
    /// until it asks for another there or leaves.
    shown: HashMap<(ClientId, String), Key>,
}

#[derive(Default)]
pub(crate) struct Views {
    state: Mutex<State>,
    ticking: AtomicBool,
    /// Wakes the reading loop when a view arrives that is read again sooner.
    wake: Arc<Notify>,
}

/// A view of a plugin. `slot` names where the client shows it, such as a pane,
/// so two panes showing two of the same plugin's views each keep theirs.
#[derive(Deserialize)]
pub(crate) struct ViewParams {
    plugin: String,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    slot: Option<String>,
}

/// What the plugin's manifest says of its views and writes.
fn manifest(plugin: &str) -> Option<Value> {
    plugins::manifests()
        .into_iter()
        .find(|manifest| manifest["id"] == plugin)
}

fn every(plugin: &str, method: &str) -> Option<Duration> {
    let secs = manifest(plugin)?["views"][method]["everySecs"].as_u64()?;
    Some(Duration::from_secs(secs.max(1)))
}

pub(crate) async fn view(core: &Arc<Core>, client: ClientId, asked: ViewParams) -> Outcome {
    let Some(every) = every(&asked.plugin, &asked.method) else {
        return Outcome::Failed(Failure::new(
            &asked.plugin,
            "invalid",
            format!("{} is not a view of {}", asked.method, asked.plugin),
        ));
    };
    let slot = asked
        .slot
        .unwrap_or_else(|| format!("{}/{}", asked.plugin, asked.method));
    let key = Key {
        plugin: asked.plugin,
        method: asked.method,
        params: asked.params.to_string(),
    };
    let kept = {
        let mut state = core.plugin_views.state.locked();
        state.shown.insert((client, slot), key.clone());
        state.kept.get_mut(&key).and_then(|kept| {
            kept.asked = Instant::now();
            (kept.read.elapsed() < kept.fresh).then(|| kept.answer.clone())
        })
    };
    if let Some(answer) = kept {
        return Outcome::Value(answer);
    }
    match plugins::call(&key.plugin, &key.method, asked.params).await {
        Ok(answer) => {
            keep(core, key, answer.clone(), every);
            Outcome::Value(answer)
        }
        Err(failure) => Outcome::Failed(failure),
    }
}

/// How long `answer` stays fresh: the manifest's pace, or sooner when the
/// view asks for it, such as while a run it shows goes on.
fn pace(answer: &Value, every: Duration) -> Duration {
    answer["readAgainSecs"]
        .as_u64()
        .map_or(every, |secs| Duration::from_secs(secs.max(1)).min(every))
}

fn keep(core: &Arc<Core>, key: Key, answer: Value, every: Duration) {
    {
        let mut state = core.plugin_views.state.locked();
        let now = Instant::now();
        let kept = Kept {
            fresh: pace(&answer, every),
            answer,
            read: now,
            every,
            asked: now,
        };
        state.kept.insert(key, kept);
        while state.kept.len() > KEPT_AT_MOST {
            let Some(oldest) = state
                .kept
                .iter()
                .min_by_key(|(_, kept)| kept.asked)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            state.kept.remove(&oldest);
        }
    }
    core.plugin_views.wake.notify_one();
    follow(core);
}

/// Reads the views panes still show again as each goes stale, until none
/// shows one.
fn follow(core: &Arc<Core>) {
    if core.plugin_views.ticking.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(core);
    let wake = core.plugin_views.wake.clone();
    tokio::spawn(async move {
        let mut wait = QUICKEST;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wake.notified() => {}
            }
            let Some(core) = weak.upgrade() else { return };
            let connected: HashSet<ClientId> =
                core.clients().iter().map(|client| client.id).collect();
            let due = {
                let mut state = core.plugin_views.state.locked();
                state
                    .shown
                    .retain(|(client, _), _| connected.contains(client));
                let shown: HashSet<Key> = state.shown.values().cloned().collect();
                state
                    .kept
                    .retain(|key, kept| shown.contains(key) || kept.asked.elapsed() < GRACE);
                if state.kept.is_empty() {
                    core.plugin_views.ticking.store(false, Ordering::Release);
                    return;
                }
                let shown_kept = || state.kept.iter().filter(|(key, _)| shown.contains(*key));
                wait = shown_kept()
                    .map(|(_, kept)| kept.fresh.saturating_sub(kept.read.elapsed()))
                    .min()
                    .unwrap_or(SLOWEST)
                    .clamp(QUICKEST, SLOWEST);
                shown_kept()
                    .filter(|(_, kept)| kept.read.elapsed() >= kept.fresh)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>()
            };
            if !due.is_empty() {
                refresh(&core, due).await;
                wait = QUICKEST;
            }
        }
    });
}

/// Reads `keys` again, telling clients about each view that changed.
async fn refresh(core: &Arc<Core>, keys: Vec<Key>) {
    let mut reading = JoinSet::new();
    for key in keys {
        reading.spawn(async move {
            let params = serde_json::from_str(&key.params).unwrap_or_default();
            let read = plugins::call(&key.plugin, &key.method, params).await;
            (key, read)
        });
    }
    let mut moved = HashSet::new();
    while let Some(Ok((key, read))) = reading.join_next().await {
        let mut state = core.plugin_views.state.locked();
        let Some(kept) = state.kept.get_mut(&key) else {
            continue;
        };
        kept.read = Instant::now();
        let Ok(answer) = read else {
            // A service that turned the read down, or limits how often it is asked, is asked less and less often.
            kept.fresh = (kept.fresh * 2).min(LONGEST_BACKOFF.max(kept.every));
            continue;
        };
        kept.fresh = pace(&answer, kept.every);
        if kept.answer != answer {
            kept.answer = answer;
            moved.insert(key.path());
        }
    }
    if !moved.is_empty() {
        core.changes
            .changed(Topic::PluginView, None, Some(moved.into_iter().collect()));
    }
}

/// The plugin `plugin` ran `method`; when the manifest names it as a write,
/// the plugin's views are read again at once, and again once the service has
/// settled.
pub(crate) fn wrote(core: &Arc<Core>, plugin: &str, method: &str) {
    let writes = manifest(plugin).is_some_and(|manifest| {
        manifest["writes"]
            .as_array()
            .is_some_and(|writes| writes.iter().any(|write| write == method))
    });
    if !writes {
        return;
    }
    let keys: Vec<Key> = core
        .plugin_views
        .state
        .locked()
        .kept
        .keys()
        .filter(|key| key.plugin == plugin)
        .cloned()
        .collect();
    if keys.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        refresh(&core, keys.clone()).await;
        tokio::time::sleep(SETTLING).await;
        refresh(&core, keys).await;
    });
}

/// The views of `plugin` to read again now: each of `method`'s, or all.
#[derive(Deserialize)]
pub(crate) struct ReadAgain {
    plugin: String,
    #[serde(default)]
    method: Option<String>,
}

/// The person asked for a plugin's views to be read again, as after a change
/// made outside Sikemux; those that changed are sent.
pub(crate) fn read_again(core: &Arc<Core>, asked: ReadAgain) {
    let keys: Vec<Key> = core
        .plugin_views
        .state
        .locked()
        .kept
        .keys()
        .filter(|key| {
            key.plugin == asked.plugin
                && asked
                    .method
                    .as_ref()
                    .is_none_or(|method| *method == key.method)
        })
        .cloned()
        .collect();
    if keys.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move { refresh(&core, keys).await });
}

/// An agent ran the tool `tool`, which may be one of a plugin's writes.
pub(crate) fn tool_ran(core: &Arc<Core>, tool: &str) {
    let found = plugins::manifests().into_iter().find_map(|manifest| {
        let method = manifest["tools"]
            .as_array()?
            .iter()
            .find(|each| each["name"] == tool)?["method"]
            .as_str()?
            .to_owned();
        Some((manifest["id"].as_str()?.to_owned(), method))
    });
    if let Some((plugin, method)) = found {
        wrote(core, &plugin, &method);
    }
}

/// The person signed in to or out of `plugin`, so what its views showed is
/// forgotten and read afresh.
pub(crate) fn signed(core: &Core, plugin: &str) {
    let forgot: HashSet<String> = {
        let mut state = core.plugin_views.state.locked();
        let paths = state
            .kept
            .keys()
            .filter(|key| key.plugin == plugin)
            .map(Key::path)
            .collect();
        state.kept.retain(|key, _| key.plugin != plugin);
        paths
    };
    if !forgot.is_empty() {
        core.changes
            .changed(Topic::PluginView, None, Some(forgot.into_iter().collect()));
    }
}
