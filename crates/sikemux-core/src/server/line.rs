//! Heavy commands agents run take turns on the host. The first in line starts once free memory
//! holds what it took the last times it ran, and checks before a commit go ahead of the rest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::cli::protocol::BuildRequest;
use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use sikemux_view::device::AgentBuild;

use super::changed::What;
use super::Core;

/// The most heavy commands the person can let run at once.
pub(crate) const MOST_RUNS: u32 = 64;
/// What a command never measured is taken to need.
const UNSEEN_BYTES: u64 = 3 << 30;
/// How far apart commands never measured start, since how much they take shows only as they run.
const UNSEEN_APART_MS: u64 = 20_000;
/// How long a check before a commit counts as having waited already.
const BEFORE_COMMIT_HEAD_START_MS: u64 = 15 * 60_000;
/// Past this measured peak in a project, a watched command waits in line there.
const WATCHED_HEAVY_BYTES: u64 = 1 << 30;
/// Where the host cannot say its free memory, one run for this many cores.
const CORES_PER_RUN: usize = 6;
const LOOK_EVERY: Duration = Duration::from_secs(2);
const SIZES_FILE: &str = "line-sizes.json";
/// How many of a command's latest runs its size is judged by.
const RUNS_KEPT: usize = 5;

pub(crate) struct Line {
    state: Mutex<State>,
    turned: Notify,
    sizes: Sizes,
    looking: AtomicBool,
}

#[derive(Default)]
struct State {
    next: u64,
    running: Vec<Run>,
    /// In the order they go, checks before a commit counted as having waited longer.
    waiting: Vec<Run>,
}

struct Run {
    id: u64,
    agent_id: Option<String>,
    command: String,
    pid: Option<u32>,
    /// When it began to wait, then when it started, in Unix milliseconds.
    since: u64,
    /// Its place among those waiting: when it asked, less a check's head start.
    order: u64,
    /// The project and command it is measured as.
    measured: Option<(String, String)>,
    /// What it took the last times it ran.
    expected: Option<Size>,
    before_commit: bool,
    /// Whether it waits for its turn, rather than running at once while it is measured.
    in_line: bool,
    /// The most memory its processes held together, then what they hold now.
    peak: u64,
    holds: u64,
    /// Whether its processes were measured since it started.
    measured_once: bool,
}

impl Run {
    fn expected_peak(&self) -> u64 {
        match (self.expected, self.in_line) {
            (Some(size), _) => size.peak,
            (None, true) => UNSEEN_BYTES,
            (None, false) => 0,
        }
    }

    /// How much more memory it is expected to take than it holds now.
    fn growth(&self) -> u64 {
        self.expected_peak().saturating_sub(self.holds)
    }
}

#[derive(Clone, Copy)]
struct Size {
    peak: u64,
    took_ms: u64,
}

pub(crate) enum Place {
    Running,
    Waiting {
        ahead: usize,
        running: usize,
        why: String,
    },
}

/// What the host can take beside what runs now.
struct Room {
    /// The most the person lets run at once, or none when they left it to the host.
    chosen: usize,
    short_of_memory: bool,
    free: Option<u64>,
}

impl Line {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            state: Mutex::default(),
            turned: Notify::new(),
            sizes: Sizes {
                file: data_dir.map(|dir| dir.join(SIZES_FILE)),
                kept: Mutex::default(),
            },
            looking: AtomicBool::new(false),
        }
    }

    /// When the heavy command `agent_id` started began to wait for its turn, in Unix
    /// milliseconds, while it still waits.
    pub(crate) fn waiting_since(&self, agent_id: &str) -> Option<u64> {
        let state = self.state.locked();
        state
            .waiting
            .iter()
            .find(|waiting| waiting.agent_id.as_deref() == Some(agent_id))
            .map(|waiting| waiting.since)
    }

    /// Every heavy command running, then every one waiting in line, as clients show them.
    pub(crate) fn shown(&self) -> Vec<AgentBuild> {
        let state = self.state.locked();
        let running = state.running.iter().map(|run| (run, None));
        let waiting =
            (state.waiting.iter().enumerate()).map(|(ahead, run)| (run, Some(ahead as u32)));
        running
            .chain(waiting)
            .map(|(run, ahead)| AgentBuild {
                id: run.id,
                agent_id: run.agent_id.clone(),
                command: run.command.clone(),
                since: run.since,
                ahead,
                expected_ms: run.expected.map(|size| size.took_ms),
                before_commit: run.before_commit,
            })
            .collect()
    }

    /// Stops the heavy command `id`, running or waiting: what it started, then the command itself.
    pub(crate) fn stop(&self, id: u64) -> Result<(), String> {
        let pid = {
            let state = self.state.locked();
            let run = (state.running.iter().chain(&state.waiting))
                .find(|run| run.id == id)
                .ok_or("that command has finished")?;
            run.pid
                .ok_or("that command did not say which process it is")?
        };
        let below = sikemux_process::procs::descendants(pid);
        for target in below.keys().copied().chain([pid]) {
            if let Ok(target) = libc::pid_t::try_from(target) {
                // SAFETY: kill only sends a signal; the pids come from the command's own tree.
                unsafe { libc::kill(target, libc::SIGTERM) };
            }
        }
        Ok(())
    }

    /// Where the run `id` stands, starting it when it is first in line and the host has room.
    fn place(&self, id: u64, room: &Room) -> Place {
        let mut state = self.state.locked();
        let Some(ahead) = state.waiting.iter().position(|run| run.id == id) else {
            return Place::Running;
        };
        let running = state.running.len();
        let why = match ahead {
            0 => why_not_yet(&state, &state.waiting[0], room),
            _ => Some("the ones ahead go first".to_owned()),
        };
        if let Some(why) = why {
            return Place::Waiting {
                ahead,
                running,
                why,
            };
        }
        let mut run = state.waiting.remove(0);
        run.since = epoch_ms();
        state.running.push(run);
        Place::Running
    }

    fn ended(&self, id: u64) -> Option<Run> {
        let mut state = self.state.locked();
        if let Some(at) = state.waiting.iter().position(|run| run.id == id) {
            state.waiting.remove(at);
            return None;
        }
        let at = state.running.iter().position(|run| run.id == id)?;
        Some(state.running.remove(at))
    }

    /// Measures each running command's processes every little while, for as long as any runs.
    fn look_while_running(&self, core: &Arc<Core>) {
        if self.looking.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut core = Arc::downgrade(core);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LOOK_EVERY).await;
                let looked = super::connection::blocking(move || Ok(look(&core))).await;
                match looked {
                    Ok(Some(next)) => core = next,
                    _ => return,
                }
            }
        });
    }
}

/// Why the first in line may not start yet, if it may not.
fn why_not_yet(state: &State, first: &Run, room: &Room) -> Option<String> {
    if state.running.is_empty() {
        return None;
    }
    if room.short_of_memory {
        return Some("the host is short of memory".to_owned());
    }
    let in_line = state.running.iter().filter(|run| run.in_line).count();
    if room.chosen > 0 && in_line >= room.chosen {
        return Some(format!(
            "{} run at once, as Settings → Chats → Heavy commands at once has it",
            room.chosen
        ));
    }
    match room.free {
        Some(free) => {
            let growing: u64 = state.running.iter().map(Run::growth).sum();
            let needs = first.expected.map_or(UNSEEN_BYTES, |size| size.peak);
            if free.saturating_sub(growing) < needs {
                return Some("free memory would not hold it beside the ones running".to_owned());
            }
        }
        None => {
            let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
            if in_line >= (cores / CORES_PER_RUN).max(1) {
                return Some("the host's cores are taken".to_owned());
            }
        }
    }
    let now = epoch_ms();
    let young = |run: &&Run| now.saturating_sub(run.since) < UNSEEN_APART_MS;
    if first.expected.is_none() && state.running.iter().any(|run| young(&run)) {
        return Some("commands never measured start 20 s apart".to_owned());
    }
    (state.running.iter().filter(young))
        .any(|run| !run.measured_once || run.expected.is_some_and(|size| run.holds > size.peak))
        .then(|| "the last one started has not shown how much it takes yet".to_owned())
}

/// Takes the measure of every running command's processes, and hands back what to look at
/// next time. None once none runs, and the looking stops.
fn look(weak: &Weak<Core>) -> Option<Weak<Core>> {
    let core = weak.upgrade()?;
    let line = core.line();
    let pids: Vec<(u64, u32)> = {
        let state = line.state.locked();
        if state.running.is_empty() {
            line.looking.store(false, Ordering::Release);
            return None;
        }
        (state.running.iter())
            .filter_map(|run| Some((run.id, run.pid?)))
            .collect()
    };
    let held: Vec<(u64, u64)> = pids
        .into_iter()
        .map(|(id, pid)| (id, tree_footprint(pid)))
        .collect();
    let mut state = line.state.locked();
    for (id, holds) in held {
        if let Some(run) = state.running.iter_mut().find(|run| run.id == id) {
            run.holds = holds;
            run.peak = run.peak.max(holds);
            run.measured_once = true;
        }
    }
    Some(weak.clone())
}

/// The memory `root` and every process under it hold together.
fn tree_footprint(root: u32) -> u64 {
    let below = sikemux_process::procs::descendants(root);
    (below.keys().copied().chain([root]))
        .filter_map(sikemux_process::procs::footprint)
        .sum()
}

/// A heavy command's place in line, or its turn, until it is dropped.
pub(crate) struct Turn {
    core: Arc<Core>,
    id: u64,
    agent: bool,
    shown: AtomicBool,
}

impl Turn {
    pub(crate) fn ask(core: &Arc<Core>, request: BuildRequest) -> Self {
        let line = core.line();
        let measured = request.key.map(|key| {
            let cwd = request.cwd.as_deref().unwrap_or_default();
            (core.workspaces().project_of(Path::new(cwd)), key)
        });
        let expected = (measured.as_ref()).and_then(|(project, key)| line.sizes.of(project, key));
        let agent = request.agent_id.is_some();
        let in_line = agent
            && (!request.watched || expected.is_some_and(|size| size.peak > WATCHED_HEAVY_BYTES));
        let now = epoch_ms();
        let head_start = match request.before_commit {
            true => BEFORE_COMMIT_HEAD_START_MS,
            false => 0,
        };
        let mut state = line.state.locked();
        state.next += 1;
        let id = state.next;
        let run = Run {
            id,
            agent_id: request.agent_id,
            command: request.command,
            pid: request.pid,
            since: now,
            order: now.saturating_sub(head_start),
            measured,
            expected,
            before_commit: request.before_commit,
            in_line,
            peak: 0,
            holds: 0,
            measured_once: false,
        };
        match in_line {
            true => {
                let at = state
                    .waiting
                    .partition_point(|waiting| waiting.order <= run.order);
                state.waiting.insert(at, run);
            }
            false => state.running.push(run),
        }
        drop(state);
        show_while_overloaded(core);
        Turn {
            core: core.clone(),
            id,
            agent,
            shown: AtomicBool::new(false),
        }
    }

    /// Where it stands now, starting it when its turn has come.
    pub(crate) fn place(&self) -> Place {
        let room = Room {
            chosen: self.core.preferences().get().build_runs as usize,
            short_of_memory: self.core.host_watch().short_of_memory(),
            free: sikemux_process::machine::memory()
                .map(|memory| memory.total.saturating_sub(memory.used)),
        };
        let place = self.core.line().place(self.id, &room);
        if matches!(place, Place::Running) {
            self.core.line().look_while_running(&self.core);
            show_while_overloaded(&self.core);
        }
        place
    }

    /// Waits until a heavy command ends or a little while passes, as memory frees up too.
    pub(crate) async fn changed(&self) {
        let _ = tokio::time::timeout(LOOK_EVERY, self.core.line().turned.notified()).await;
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
        let line = self.core.line();
        if let Some(Run {
            measured: Some((project, key)),
            peak,
            since,
            ..
        }) = line.ended(self.id)
        {
            line.sizes
                .record(&project, &key, peak, epoch_ms().saturating_sub(since));
        }
        line.turned.notify_waiters();
        if self.shown.load(Ordering::Relaxed) {
            self.core.changed(What::View);
        } else {
            show_while_overloaded(&self.core);
        }
    }
}

/// Tells clients the line moved, while the host is overloaded and they list it.
fn show_while_overloaded(core: &Core) {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    if sikemux_process::machine::overloaded(sikemux_process::machine::load_average(), cores) {
        core.changed(What::View);
    }
}

/// How much memory and time each project's heavy commands took the last times they ran, kept
/// in the core's data folder.
struct Sizes {
    file: Option<PathBuf>,
    kept: Mutex<Option<Kept>>,
}

/// Project, then command, then its latest runs.
type Kept = BTreeMap<String, BTreeMap<String, Runs>>;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runs {
    peaks: Vec<u64>,
    took_ms: Vec<u64>,
}

impl Sizes {
    fn of(&self, project: &str, key: &str) -> Option<Size> {
        let mut kept = self.kept.locked();
        let runs = self.loaded(&mut kept).get(project)?.get(key)?;
        let mut took = runs.took_ms.clone();
        took.sort_unstable();
        Some(Size {
            peak: runs.peaks.iter().copied().max()?,
            took_ms: took.get(took.len() / 2).copied().unwrap_or_default(),
        })
    }

    fn record(&self, project: &str, key: &str, peak: u64, took_ms: u64) {
        let mut kept = self.kept.locked();
        let all = self.loaded(&mut kept);
        let runs = (all.entry(project.to_owned()).or_default())
            .entry(key.to_owned())
            .or_default();
        for (list, value) in [(&mut runs.peaks, peak), (&mut runs.took_ms, took_ms)] {
            list.push(value);
            if list.len() > RUNS_KEPT {
                list.remove(0);
            }
        }
        let Some(file) = &self.file else {
            return;
        };
        let written = serde_json::to_vec(all)
            .map_err(std::io::Error::other)
            .and_then(|json| sikemux_process::files::write_atomic_unsynced(file, &json, false));
        if let Err(error) = written {
            eprintln!("sikemux core: how much heavy commands take was not kept: {error}");
        }
    }

    fn loaded<'a>(&self, kept: &'a mut Option<Kept>) -> &'a mut Kept {
        kept.get_or_insert_with(|| {
            (self.file.as_ref())
                .and_then(|file| std::fs::read(file).ok())
                .and_then(|json| serde_json::from_slice(&json).ok())
                .unwrap_or_default()
        })
    }
}
