//! The builds agents start take turns: as many build at once as the host's
//! cores and memory take, and the rest wait in the order they asked.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::cli::protocol::BuildRequest;
use sikemux_view::device::AgentBuild;

use super::changed::What;
use super::Core;

/// The most builds the person can let run at once.
pub(crate) const MOST_RUNS: u32 = 64;
/// How many cores a cold `cargo check` of the core keeps busy on average.
const CORES_PER_BUILD: f64 = 6.;
/// About the most memory that build's compilers hold at once.
const BYTES_PER_BUILD: u64 = 3 << 30;

#[derive(Default)]
pub(crate) struct Builds {
    state: Mutex<State>,
    turned: Notify,
}

#[derive(Default)]
struct State {
    next: u64,
    building: Vec<Build>,
    waiting: VecDeque<Build>,
}

struct Build {
    id: u64,
    agent_id: Option<String>,
    command: String,
    pid: Option<u32>,
    /// When it began to wait, then when it started, in Unix milliseconds.
    since: u64,
}

pub(crate) enum Place {
    Building,
    Waiting {
        ahead: usize,
        building: usize,
        limit: usize,
    },
}

impl Builds {
    /// When the build `agent_id` started began to wait for its turn, in Unix
    /// milliseconds, while it still waits.
    pub(crate) fn waiting_since(&self, agent_id: &str) -> Option<u64> {
        let state = self.state.locked();
        state
            .waiting
            .iter()
            .find(|waiting| waiting.agent_id.as_deref() == Some(agent_id))
            .map(|waiting| waiting.since)
    }

    /// Every build running, then every one waiting in line, as clients show them.
    pub(crate) fn shown(&self) -> Vec<AgentBuild> {
        let state = self.state.locked();
        let running = state.building.iter().map(|build| (build, None));
        let waiting =
            (state.waiting.iter().enumerate()).map(|(ahead, build)| (build, Some(ahead as u32)));
        running
            .chain(waiting)
            .map(|(build, ahead)| AgentBuild {
                id: build.id,
                agent_id: build.agent_id.clone(),
                command: build.command.clone(),
                since: build.since,
                ahead,
            })
            .collect()
    }

    /// Stops the build `id`, running or waiting: what it started, then the build itself.
    pub(crate) fn stop(&self, id: u64) -> Result<(), String> {
        let pid = {
            let state = self.state.locked();
            let build = (state.building.iter().chain(&state.waiting))
                .find(|build| build.id == id)
                .ok_or("that build has finished")?;
            build
                .pid
                .ok_or("that build did not say which process it is")?
        };
        let below = sikemux_process::procs::descendants(pid);
        for target in below.keys().copied().chain([pid]) {
            if let Ok(target) = libc::pid_t::try_from(target) {
                // SAFETY: kill only sends a signal; the pids come from the build's own tree.
                unsafe { libc::kill(target, libc::SIGTERM) };
            }
        }
        Ok(())
    }

    /// Where the build `id` stands, starting it when it is first in line and
    /// fewer than `limit` build.
    fn place(&self, id: u64, limit: impl FnOnce(usize) -> usize) -> Place {
        let mut state = self.state.locked();
        if state.building.iter().any(|build| build.id == id) {
            return Place::Building;
        }
        let ahead = state
            .waiting
            .iter()
            .position(|waiting| waiting.id == id)
            .unwrap_or_default();
        let building = state.building.len();
        let limit = limit(building);
        if ahead == 0 && building < limit {
            if let Some(mut build) = state.waiting.pop_front() {
                build.since = sikemux_process::clock::epoch_ms();
                state.building.push(build);
            }
            return Place::Building;
        }
        Place::Waiting {
            ahead,
            building,
            limit,
        }
    }
}

/// A build's place in line, or its turn, until it is dropped.
pub(crate) struct Turn {
    core: Arc<Core>,
    id: u64,
    agent: bool,
    shown: AtomicBool,
}

impl Turn {
    pub(crate) fn ask(core: &Arc<Core>, request: BuildRequest) -> Self {
        let agent = request.agent_id.is_some();
        let mut state = core.builds.state.locked();
        state.next += 1;
        let id = state.next;
        state.waiting.push_back(Build {
            id,
            agent_id: request.agent_id,
            command: request.command,
            pid: request.pid,
            since: sikemux_process::clock::epoch_ms(),
        });
        drop(state);
        show_while_overloaded(core);
        Turn {
            core: core.clone(),
            id,
            agent,
            shown: AtomicBool::new(false),
        }
    }

    /// Where it stands now, sized by the person's limit, else the host's.
    pub(crate) fn place(&self) -> Place {
        let chosen = self.core.preferences.get().build_runs as usize;
        let place = self.core.builds.place(self.id, |building| match chosen {
            0 => host_limit(building),
            chosen => chosen,
        });
        if matches!(place, Place::Building) {
            show_while_overloaded(&self.core);
        }
        place
    }

    /// Waits until a build ends or a little while passes, as memory frees up too.
    pub(crate) async fn changed(&self) {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.core.builds.turned.notified(),
        )
        .await;
    }

    /// Lists it as waiting on its agent's row, or no longer once it was.
    pub(crate) fn show(&self) {
        if self.agent {
            self.shown.store(true, Ordering::Relaxed);
            self.core.changed(What::View);
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut state = self.core.builds.state.locked();
        state.building.retain(|build| build.id != self.id);
        state.waiting.retain(|build| build.id != self.id);
        drop(state);
        self.core.builds.turned.notify_waiters();
        if self.shown.load(Ordering::Relaxed) {
            self.core.changed(What::View);
        } else {
            show_while_overloaded(&self.core);
        }
    }
}

/// Tells clients the builds moved, while the host is overloaded and they list them.
fn show_while_overloaded(core: &Core) {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    if sikemux_process::machine::overloaded(sikemux_process::machine::load_average(), cores) {
        core.changed(What::View);
    }
}

/// As many builds as the host's cores keep busy, fewer when its free memory
/// would not hold one more beside the `building` ones, and never none.
fn host_limit(building: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    let by_cores = (cores as f64 / CORES_PER_BUILD).round() as usize;
    let by_memory = sikemux_process::machine::memory().map_or(by_cores, |memory| {
        building + (memory.total.saturating_sub(memory.used) / BYTES_PER_BUILD) as usize
    });
    by_cores.min(by_memory).max(1)
}
