//! Tells every client that something it reads through a `Call` changed, so
//! it reads that again. A burst of changes to one topic and project goes out
//! as one notice.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::protocol::Event;

use super::Core;

const GATHER: Duration = Duration::from_millis(50);
/// Past this many paths a notice says anything may have changed.
const MAX_PATHS: usize = 512;

/// What a client reads again, as `Event::Changed` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Topic {
    Activity,
    AgentAccounts,
    AgentSignIn,
    AgentStatus,
    Boards,
    Ci,
    Git,
    Ports,
    Runs,
    Shares,
    Skills,
    Tasks,
    Trust,
    /// How much of their plans the agent accounts have used.
    Usage,
    /// The SSH config's hosts, or how a terminal reaches one.
    Ssh,
    /// Which plugins are on, and whether each is signed in.
    Plugins,
    /// The questions agents put to the person, and the assumptions they went on.
    Questions,
    /// Who this host is signed in to Sikemux as.
    Account,
    /// The hosts linked with this one, the SSH hosts a terminal is open to,
    /// and how each is reached.
    Hosts,
    /// A language server's diagnostics for the files in `paths` moved.
    Lsp,
    /// How the person wants Sikemux to behave.
    Preferences,
    /// What the person has typed and not sent yet; `paths` names the drafts' keys.
    Drafts,
    /// A project's pull requests, runs, issues or releases on its code host.
    CodeHost,
    /// What a plugin's pane shows; `paths` names each view as `plugin/method`.
    PluginView,
    /// Files clients have open moved on disk; `paths` names them as the clients did.
    Files,
    /// Whether an update is out, and how installing it goes.
    Update,
}

/// Paths by topic and project; none means anything may have changed.
type Pending = BTreeMap<(Topic, Option<String>), Option<BTreeSet<String>>>;

#[derive(Default)]
pub(crate) struct Changes {
    pending: Mutex<Pending>,
    notify: Notify,
}

impl Changes {
    /// `topic` changed for `project`, or for every project when it is none.
    /// `paths` names what moved inside it; none means anything may have.
    pub(crate) fn changed(&self, topic: Topic, project: Option<&str>, paths: Option<Vec<String>>) {
        let mut pending = self.pending.locked();
        let key = (topic, project.map(str::to_owned));
        let known = pending.get(&key);
        let merged = match (known, paths) {
            (Some(None), _) | (_, None) => None,
            (Some(Some(known)), Some(paths)) => {
                let mut known = known.clone();
                known.extend(paths);
                Some(known)
            }
            (None, Some(paths)) => Some(paths.into_iter().collect()),
        };
        pending.insert(key, merged.filter(|paths| paths.len() <= MAX_PATHS));
        drop(pending);
        self.notify.notify_one();
    }
}

pub(crate) async fn serve(core: Arc<Core>) {
    loop {
        core.changes.notify.notified().await;
        tokio::time::sleep(GATHER).await;
        let pending = std::mem::take(&mut *core.changes.pending.locked());
        for ((topic, project), paths) in pending {
            let Ok(Value::String(topic)) = serde_json::to_value(topic) else {
                continue;
            };
            core.broadcast_event(&Event::Changed {
                topic,
                project,
                paths: paths.map(|paths| paths.into_iter().collect()),
            });
        }
    }
}
