//! What the core asks of a feature that keeps state, so that starting, stopping, handing over
//! and a client leaving walk one list and a new feature joins them by being on it.

use std::future::Future;
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

    /// Whether it has work that keeps a core nothing else is using running.
    fn has_work(&self) -> bool {
        false
    }
}

/// Runs `work` each time the wake `of` the core is woken.
pub(crate) fn on_wake(
    core: &Arc<Core>,
    of: fn(&Core) -> &Wake,
    work: fn(&Arc<Core>),
) -> JoinHandle<()> {
    tokio::spawn(wake::serve(core.clone(), of, work))
}

/// A task that takes the core and runs for as long as it does.
pub(crate) fn serving<F: Future<Output = ()> + Send + 'static>(
    core: &Arc<Core>,
    serve: impl FnOnce(Arc<Core>) -> F,
) -> JoinHandle<()> {
    tokio::spawn(serve(core.clone()))
}

impl Core {
    fn parts(&self) -> [&dyn Part; 25] {
        [
            &self.harness,
            &self.boards,
            &self.memory,
            &self.triggers,
            &self.collisions,
            &self.checks,
            &self.workspaces,
            &self.hosts,
            &self.ports,
            &self.spares,
            &self.device_view,
            &self.lifecycle,
            &self.updates,
            &self.changes,
            &self.tasks,
            &self.notify,
            &self.window,
            &self.chats,
            &self.seen,
            &self.open_files,
            &self.mods,
            &self.diagrams,
            &self.preferences,
            &self.drafts,
            &self.code_host_lists,
        ]
    }

    /// Every feature's tasks, to end when the core stops.
    pub(super) fn start_parts(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        self.parts()
            .iter()
            .flat_map(|part| part.start(self))
            .collect()
    }

    /// Waits until everything every feature changed is on disk.
    pub(crate) fn flush_parts(&self) {
        for part in self.parts() {
            part.flush();
        }
    }

    pub(super) fn tell_parts_client_gone(self: &Arc<Self>, client: ClientId) {
        for part in self.parts() {
            part.client_gone(self, client);
        }
        super::lsp::orphan(client);
    }

    pub(super) fn parts_have_work(&self) -> bool {
        self.parts().iter().any(|part| part.has_work())
    }
}
