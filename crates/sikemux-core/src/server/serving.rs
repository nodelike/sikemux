//! What the core keeps for its terminals and the clients connected to it, and how the rest
//! reads and changes it.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use sikemux_process::sync::Locked;
use sikemux_pty::agent_detection::ManifestRegistry;
use sikemux_pty::task::{
    task_reclamation_plan, TaskRetentionCandidate, MAX_RETAINED_EXITED_TASK_PTYS,
};
use sikemux_wire::protocol::{BuildIdentity, SessionId};

use super::changed::{self, ChangedCore, What};
use super::connection::{ClientConn, ClientId};
use super::session::{self, Session};
use super::{waits, window, Capacity, Core};

pub(crate) struct ServeFields {
    pub(crate) sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) capacity: Arc<Capacity>,
    pub(crate) connections: Mutex<HashMap<ClientId, Arc<ClientConn>>>,
    pub(crate) next_client_id: AtomicU64,
    pub(crate) detection: RwLock<ManifestRegistry>,
    pub(crate) window: window::Window,
    pub(crate) waits: waits::Waits,
    pub(crate) changes: changed::Changes,
    pub(crate) updating: super::updating::Updating,
    /// The rail numbers running loose terminals have.
    pub(crate) rail_numbers: Mutex<BTreeSet<u32>>,
    pub(crate) above: super::above::Above,
    /// The build this core is, which a client is told as it connects.
    pub(crate) build: BuildIdentity,
}

pub(crate) trait ServeParts {
    fn sessions(&self) -> &Mutex<HashMap<SessionId, Arc<Session>>>;
    fn next_session_id(&self) -> &AtomicU64;
    fn capacity(&self) -> &Arc<Capacity>;
    fn connections(&self) -> &Mutex<HashMap<ClientId, Arc<ClientConn>>>;
    fn next_client_id(&self) -> &AtomicU64;
    fn detection(&self) -> &RwLock<ManifestRegistry>;
    fn window(&self) -> &window::Window;
    fn waits(&self) -> &waits::Waits;
    fn changes(&self) -> &changed::Changes;
    fn updating(&self) -> &super::updating::Updating;
    fn rail_numbers(&self) -> &Mutex<BTreeSet<u32>>;
    fn above(&self) -> &super::above::Above;
    fn build(&self) -> &BuildIdentity;
}

impl ServeParts for Core {
    fn sessions(&self) -> &Mutex<HashMap<SessionId, Arc<Session>>> {
        &self.kept::<ServeFields>().sessions
    }

    fn next_session_id(&self) -> &AtomicU64 {
        &self.kept::<ServeFields>().next_session_id
    }

    fn capacity(&self) -> &Arc<Capacity> {
        &self.kept::<ServeFields>().capacity
    }

    fn connections(&self) -> &Mutex<HashMap<ClientId, Arc<ClientConn>>> {
        &self.kept::<ServeFields>().connections
    }

    fn next_client_id(&self) -> &AtomicU64 {
        &self.kept::<ServeFields>().next_client_id
    }

    fn detection(&self) -> &RwLock<ManifestRegistry> {
        &self.kept::<ServeFields>().detection
    }

    fn window(&self) -> &window::Window {
        &self.kept::<ServeFields>().window
    }

    fn waits(&self) -> &waits::Waits {
        &self.kept::<ServeFields>().waits
    }

    fn changes(&self) -> &changed::Changes {
        &self.kept::<ServeFields>().changes
    }

    fn updating(&self) -> &super::updating::Updating {
        &self.kept::<ServeFields>().updating
    }

    fn rail_numbers(&self) -> &Mutex<BTreeSet<u32>> {
        &self.kept::<ServeFields>().rail_numbers
    }

    fn above(&self) -> &super::above::Above {
        &self.kept::<ServeFields>().above
    }

    fn build(&self) -> &BuildIdentity {
        &self.kept::<ServeFields>().build
    }
}

/// The core's terminals and clients.
pub(crate) trait ServeCore {
    fn clients(&self) -> Vec<Arc<ClientConn>>;

    fn agent_session(&self, agent_id: &str) -> Option<Arc<Session>>;

    fn session(&self, id: SessionId) -> Option<Arc<Session>>;

    fn all_sessions(&self) -> Vec<Arc<Session>>;

    /// The lowest rail number no running loose terminal has, taken now.
    fn take_rail_number(&self) -> u32;

    fn insert_session(&self, session: Arc<Session>);

    fn remove_session(&self, session: &Arc<Session>) -> bool;

    fn take_session_for_kill(&self, id: SessionId) -> Option<Arc<Session>>;

    fn client_conn(&self, id: ClientId) -> Option<Arc<ClientConn>>;

    fn register_client(&self, client: Arc<ClientConn>);

    fn unregister_client(self: &Arc<Self>, client: &ClientConn);

    /// Ends the connections of one paired device, or of every one.
    fn close_device_clients(&self, id: Option<&str>);

    fn schedule_task_output_notice(self: &Arc<Self>, id: SessionId, delay: Duration);

    /// Drops the oldest exited sessions nobody is watching once there are too
    /// many or they have been kept long enough.
    fn reclaim_exited_sessions(&self, now: u64);
}

impl ServeCore for Core {
    fn clients(&self) -> Vec<Arc<ClientConn>> {
        self.connections().locked().values().cloned().collect()
    }

    fn agent_session(&self, agent_id: &str) -> Option<Arc<Session>> {
        self.all_sessions().into_iter().find(|session| {
            session
                .agent
                .as_ref()
                .is_some_and(|agent| agent.agent_id() == agent_id)
        })
    }

    fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions().locked().get(&id).cloned()
    }

    fn all_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions().locked().values().cloned().collect()
    }

    fn take_rail_number(&self) -> u32 {
        let mut taken = self.rail_numbers().locked();
        let number = (1..).find(|number| !taken.contains(number)).unwrap_or(0);
        taken.insert(number);
        number
    }

    fn insert_session(&self, session: Arc<Session>) {
        if let Some(number) = session.rail_number() {
            self.rail_numbers().locked().insert(number);
        }
        let ssh = session.ssh_link().is_some();
        self.sessions().locked().insert(session.id, session);
        if ssh {
            (self.above().ssh_moved)(self);
        }
    }

    fn remove_session(&self, session: &Arc<Session>) -> bool {
        let removed = {
            let mut sessions = self.sessions().locked();
            let current = sessions
                .get(&session.id)
                .is_some_and(|current| Arc::ptr_eq(current, session));
            if current {
                sessions.remove(&session.id);
            }
            current
        };
        if removed {
            session::detach_all(session);
        }
        removed
    }

    fn take_session_for_kill(&self, id: SessionId) -> Option<Arc<Session>> {
        let mut sessions = self.sessions().locked();
        let session = sessions.get(&id)?.clone();
        // A killed task keeps its snapshot and output log for later reads.
        if !session.is_task() {
            sessions.remove(&id);
            drop(sessions);
            session::detach_all(&session);
        }
        Some(session)
    }

    fn client_conn(&self, id: ClientId) -> Option<Arc<ClientConn>> {
        self.connections().locked().get(&id).cloned()
    }

    fn register_client(&self, client: Arc<ClientConn>) {
        self.connections().locked().insert(client.id, client);
        (self.above().client_joined)(self);
    }

    fn unregister_client(self: &Arc<Self>, client: &ClientConn) {
        client.close();
        (self.above().client_gone)(self, client.id);
        self.connections().locked().remove(&client.id);
        for (stream, id) in client.take_streams() {
            if let Some(session) = self.session(id) {
                session::detach(&session, stream);
            }
        }
        self.changed(What::View);
    }

    fn close_device_clients(&self, id: Option<&str>) {
        for client in self.clients() {
            let closing = match id {
                Some(id) => client.peer.is_device(id),
                None => !client.peer.is_local(),
            };
            if closing {
                client.close();
            }
        }
    }

    fn schedule_task_output_notice(self: &Arc<Self>, id: SessionId, delay: Duration) {
        let core = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(session) = core.session(id) {
                session::mark_task_output_noticed(&session);
            }
            (core.above().task_output_noticed)(&core, id);
        });
    }

    fn reclaim_exited_sessions(&self, now: u64) {
        let sessions = self.all_sessions();
        let candidates = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| TaskRetentionCandidate {
                id: index as u32,
                exited_at_ms: session.exited_at_ms.load(Ordering::Acquire),
                has_subscribers: session.has_subscribers(),
            })
            .collect();
        for index in task_reclamation_plan(candidates, now, MAX_RETAINED_EXITED_TASK_PTYS) {
            let Some(session) = sessions.get(index as usize) else {
                continue;
            };
            if session.exited_at_ms.load(Ordering::Acquire) != 0 && !session.has_subscribers() {
                self.remove_session(session);
            }
        }
    }
}
