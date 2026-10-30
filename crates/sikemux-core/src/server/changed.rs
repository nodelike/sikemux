//! Tells every client that something it reads through a `Call` changed, so
//! it reads that again. A burst of changes to one topic and project goes out
//! as one notice.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::protocol::Event;
pub(crate) use sikemux_wire::changed::Topic;

use super::Core;

const GATHER: Duration = Duration::from_millis(50);
/// Past this many paths a notice says anything may have changed.
const MAX_PATHS: usize = 512;

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
            core.broadcast_event(&Event::Changed {
                topic,
                project,
                paths: paths.map(|paths| paths.into_iter().collect()),
            });
        }
    }
}
