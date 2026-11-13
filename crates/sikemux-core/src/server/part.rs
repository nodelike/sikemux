//! What the core asks of a feature that keeps state, so that starting, stopping, handing over
//! and a client leaving walk one list and a new feature joins them by being on it. `run_core`
//! starts what keeps no state of its own.

use std::sync::Arc;

use tokio::task::JoinHandle;

use super::connection::ClientId;
use super::wake::{self, Wake};
use super::Core;

pub(crate) trait Part: Send + Sync {
    /// The tasks it runs for as long as the core does.
    fn start(&self, _core: &Arc<Core>) -> Vec<JoinHandle<()>> {
        Vec::new()
    }

    /// Waits until every change so far is on disk, for a core about to stop or hand over.
    fn flush(&self) {}

    /// Lets go of what a connection that has gone held.
    fn client_gone(&self, _core: &Arc<Core>, _client: ClientId) {}
}

/// Runs `work` each time the wake `of` the core is woken.
pub(crate) fn on_wake(
    core: &Arc<Core>,
    of: fn(&Core) -> &Wake,
    work: fn(&Arc<Core>),
) -> JoinHandle<()> {
    tokio::spawn(wake::serve(core.clone(), of, work))
}
