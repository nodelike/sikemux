//! Heavy commands agents run take turns on the host. The first in line starts once free memory
//! holds what it took the last times it ran, and checks before a commit go ahead of the rest.
//! The same command asked again in one checkout is one run, whose output each caller gets.
//! An agent that would wait long is sent away and woken when its turn comes. A run also holds
//! the things it needs that only so many use at once, and an agent can hold one for its turn.

mod saved;
mod sizes;
mod things;

use crate::server::chat_state::ChatParts;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::cli::protocol::BuildRequest;
use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use sikemux_view::device::{AgentBuild, InUse};
use sikemux_view::inbox::InLine;

use super::changed::What;
use super::Core;
use sizes::{Size, Sizes};
pub(crate) use things::{resolve, Need};
use things::{Around, Wanted};

/// An agent's own command that would wait longer than this is sent away, to be woken when its
/// turn comes, rather than wait where it is.
pub(crate) const COME_BACK_AFTER_MS: u64 = 45_000;
/// How long the host keeps room for an agent woken for its turn.
pub(crate) const ROOM_KEPT_MS: u64 = 90_000;
/// How long a waiting command its agent's tool gave up on keeps its place.
const PLACE_KEPT_MS: u64 = 2 * 60_000;
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
/// How long a finished run's exit code and output wait for the callers following it.
const FINISHED_KEPT_MS: u64 = 60_000;
const LOOK_EVERY: Duration = Duration::from_secs(2);
const LOGS: &str = "line-logs";
/// The most tool calls kept as still running for one agent.
const MOST_OPEN_CALLS: usize = 64;
/// The most rows clients are sent of what runs and waits.
const SHOWN_ROWS: usize = 32;

pub(crate) struct Line {
    state: Mutex<State>,
    turned: Notify,
    sizes: Sizes,
    saved: saved::Saved,
    logs: Option<PathBuf>,
    tending: AtomicBool,
}

#[derive(Default)]
struct State {
    next: u64,
    running: Vec<Run>,
    /// In the order they go, checks before a commit counted as having waited longer.
    waiting: Vec<Run>,
    /// Runs that ended lately, for the callers following them: id, exit code, log, when.
    finished: Vec<(u64, Option<i32>, Option<PathBuf>, u64)>,
    /// Why the first in line did not start at the last look.
    why: String,
    /// When each agent last started or ended a tool call of any kind, in Unix milliseconds, so a
    /// turn's hold it forgot ends.
    called: HashMap<String, u64>,
    /// The tool calls each agent has started and not ended, by id.
    open_calls: HashMap<String, std::collections::HashSet<String>>,
    /// Programs outside Sikemux listening on the ports of things runs wait for.
    outside: HashMap<u16, String>,
}

struct Run {
    id: u64,
    /// The agent it runs for, which a run with no caller left still keeps.
    agent_id: Option<String>,
    command: String,
    cwd: String,
    /// Who asked for it: the first runs it, the rest follow its output.
    callers: Vec<Caller>,
    held: Held,
    pid: Option<u32>,
    /// When its process started, in the system's own terms, to know it again after a restart.
    started: Option<u64>,
    /// When it began to wait, then when it started, in Unix milliseconds.
    since: u64,
    /// Its place among those waiting: when it asked, less a check's head start.
    order: u64,
    /// The project and command it is measured as.
    measured: Option<(String, String)>,
    expected: Option<Size>,
    before_commit: bool,
    /// Whether it waits for its turn, rather than running at once.
    in_line: bool,
    /// Its build is over and what it built runs now, which needs no turn.
    released: bool,
    /// The same command already running in this checkout, which it waits to end.
    after: Option<u64>,
    copies_output: bool,
    log: Option<PathBuf>,
    execution_id: Option<String>,
    /// Why the core stopped it.
    stopped: Option<String>,
    /// The most memory its processes held together, then what they hold now.
    peak: u64,
    holds: u64,
    measured_once: bool,
    /// The things only so many use at once that it holds while it runs.
    needs: Vec<Need>,
    kind: Kind,
    /// What it waits for of its needs, while it does.
    wants: Option<Wanted>,
    /// Its holder's lead, or the person, heard it held a thing long while others wait.
    told_long: bool,
}

/// What a run in line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A heavy command, through the shim.
    Command,
    /// A process run, which the core starts once what it needs is free.
    Process,
    /// An agent holding a thing for its turn.
    Hold,
}

#[derive(Clone)]
struct Caller {
    id: u64,
    agent_id: Option<String>,
    exit: Option<i32>,
}

/// What keeps a run whose callers are gone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    /// A caller is connected.
    No,
    /// Its agent was sent away, to be woken when its turn comes.
    ComesBack,
    /// Its agent's tool gave up on it while it waited; it keeps its place until then.
    PlaceUntil(u64),
    /// Its turn came while its agent was away; the room is kept until then.
    RoomUntil(u64),
    /// It ran when the core restarted, and is counted until its process ends.
    Taken,
    /// An agent holds it for its turn.
    Turn,
    /// The core holds it for a process run until the run ends.
    Core,
}

impl Run {
    fn is_for(&self, agent_id: &str) -> bool {
        self.agent_id.as_deref() == Some(agent_id)
    }

    fn expected_peak(&self) -> u64 {
        match (self.expected, self.in_line) {
            (Some(size), _) => size.peak,
            (None, true) => UNSEEN_BYTES,
            (None, false) => 0,
        }
    }

    /// How much more memory it is expected to take than it holds now.
    fn growth(&self) -> u64 {
        match self.released {
            true => 0,
            false => self.expected_peak().saturating_sub(self.holds),
        }
    }

    fn same(&self, cwd: &str, command: &str) -> bool {
        self.cwd == cwd && self.command == command
    }

    /// Moves the caller waiting on its turn out of its agent's cgroup before it starts the
    /// command, so when memory runs out systemd-oomd stops the build and not the agent with it.
    fn take_own_cgroup(&self) {
        let Some((pid, started)) = self.pid.zip(self.started) else {
            return;
        };
        if self.kind == Kind::Command && sikemux_process::procs::started(pid) == Some(started) {
            sikemux_process::cgroups::move_into_own_cgroup(pid);
        }
    }
}

pub(crate) enum Place {
    Running {
        log: Option<String>,
    },
    Following {
        log: String,
    },
    Ended {
        code: Option<i32>,
        log: Option<String>,
    },
    Waiting {
        ahead: usize,
        running: usize,
        why: String,
        expected_ms: Option<u64>,
    },
}

/// What the host can take beside what runs now.
struct Room {
    /// The most the person lets run at once, or none when they left it to the host.
    chosen: usize,
    short_of_memory: bool,
    free: Option<u64>,
    around: Around,
}

impl Room {
    fn now(core: &Core, outside: HashMap<u16, String>) -> Self {
        let memory = sikemux_process::machine::memory();
        Room {
            chosen: core.preferences().get().build_runs as usize,
            short_of_memory: core.host_watch().short_of_memory(),
            free: memory.map(|memory| memory.total.saturating_sub(memory.used)),
            around: Around { outside },
        }
    }
}

/// An agent whose turn came while it was away.
struct Woken {
    agent_id: String,
    cwd: String,
    command: String,
    /// What it held for its turn, when it was a thing rather than a command.
    thing: Option<String>,
}

/// A number of milliseconds a debug build reads from the environment variable `name`, so a
/// throwaway core can show what happens after a wait too long to sit through.
fn debug_ms(name: &str) -> Option<u64> {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var(name).ok().and_then(|ms| ms.parse().ok()) {
        return Some(ms);
    }
    let _ = name;
    None
}

impl Line {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let saved = saved::Saved::new(data_dir);
        let mut state = State::default();
        for run in saved.load() {
            state.next = state.next.max(run.id);
            match run.held {
                Held::Taken => state.running.push(run),
                _ => state.waiting.push(run),
            }
        }
        state.waiting.sort_by_key(|run| run.order);
        Self {
            state: Mutex::new(state),
            turned: Notify::new(),
            sizes: Sizes::new(data_dir),
            saved,
            logs: data_dir.map(|dir| dir.join(LOGS)),
            tending: AtomicBool::new(false),
        }
    }

    /// Where the heavy command `agent_id` started stands in line, or what it waits for that
    /// another holds, while it waits.
    pub(crate) fn waiting(&self, core: &Core, agent_id: &str) -> Option<InLine> {
        let state = self.state.locked();
        let at = (state.waiting.iter()).position(|run| run.is_for(agent_id))?;
        let run = &state.waiting[at];
        let by = (run.wants.as_ref())
            .and_then(Wanted::holder)
            .map(|holder| name_of(core, holder));
        let what = match (&run.wants, run.kind) {
            (Some(Wanted::InUse { label, .. }), _) => label.clone(),
            (_, Kind::Hold) => things::labels(run),
            _ => run.command.clone(),
        };
        Some(InLine {
            what,
            place: at as u32 + 1,
            expected_ms: expected_wait(&state, at),
            by,
        })
    }

    /// Where the heavy command of the `process_start` run `execution_id` stands, while it waits.
    pub(crate) fn waiting_for_run(&self, execution_id: &str) -> Option<InLine> {
        let state = self.state.locked();
        let at = (state.waiting.iter())
            .position(|run| run.execution_id.as_deref() == Some(execution_id))?;
        Some(InLine {
            what: state.waiting[at].command.clone(),
            place: at as u32 + 1,
            expected_ms: expected_wait(&state, at),
            by: None,
        })
    }

    /// Why the `process_start` run `execution_id` waits, in words, while it waits for a thing.
    pub(crate) fn wanted_by_run(&self, core: &Core, execution_id: &str) -> Option<String> {
        let state = self.state.locked();
        let run =
            (state.waiting.iter()).find(|run| run.execution_id.as_deref() == Some(execution_id))?;
        Some(run.wants.as_ref()?.words(&|agent| name_of(core, agent)))
    }

    /// What is held of the things only so many use at once, oldest first, as clients show it.
    pub(crate) fn in_use(&self) -> Vec<InUse> {
        let state = self.state.locked();
        let mut held: Vec<InUse> = (state.running.iter())
            .filter(|run| !run.needs.is_empty())
            .map(|run| InUse {
                id: run.id,
                label: things::labels(run),
                agent_id: run.agent_id.clone(),
                since: run.since,
                waiting: things::waiting_for(&state, run),
                until_turn_ends: run.held == Held::Turn,
            })
            .collect();
        held.sort_by_key(|held| held.since);
        held
    }

    /// Every heavy command running, then every one waiting in line, then every run and agent
    /// waiting for a thing another holds, as clients show them.
    pub(crate) fn shown(&self, core: &Core) -> Vec<AgentBuild> {
        let state = self.state.locked();
        let commands = |run: &&Run| run.kind == Kind::Command && run.wants.is_none();
        let running = (state.running.iter())
            .filter(|run| run.kind == Kind::Command)
            .map(|run| (run, None));
        let waiting = (state.waiting.iter().filter(commands).enumerate())
            .map(|(ahead, run)| (run, Some(ahead as u32)));
        let for_things = (state.waiting.iter().enumerate())
            .filter(|(_, run)| run.wants.is_some())
            .map(|(ahead, run)| (run, Some(ahead as u32)));
        running
            .chain(waiting)
            .chain(for_things)
            .take(SHOWN_ROWS)
            .map(|(run, ahead)| {
                let wants = run.wants.as_ref();
                AgentBuild {
                    id: run.id,
                    agent_id: run.agent_id.clone(),
                    command: match wants {
                        Some(Wanted::InUse { label, .. } | Wanted::PortTaken { label, .. }) => {
                            label.clone()
                        }
                        None => run.command.clone(),
                    },
                    since: run.since,
                    ahead,
                    expected_ms: run.expected.map(|size| size.took_ms),
                    before_commit: run.before_commit,
                    joined: (run.callers.iter().skip(1))
                        .filter_map(|caller| caller.agent_id.clone())
                        .filter(|agent| Some(agent) != run.agent_id.as_ref())
                        .collect(),
                    waits_for_thing: wants.is_some(),
                    by: (wants.and_then(Wanted::holder)).map(|holder| name_of(core, holder)),
                }
            })
            .collect()
    }

    /// Moves the waiting heavy command `id` to the front of the line.
    pub(crate) fn go_first(&self, core: &Core, id: u64) -> Result<(), String> {
        let mut state = self.state.locked();
        let at = (state.waiting.iter())
            .position(|run| run.id == id)
            .ok_or("that command is not waiting")?;
        let mut run = state.waiting.remove(at);
        run.order = state.waiting.first().map_or(run.order, |first| {
            first.order.min(run.order).saturating_sub(1)
        });
        state.waiting.insert(0, run);
        drop(state);
        if let Some(core) = core.arc() {
            tidy(&core);
        }
        Ok(())
    }

    /// Stops the heavy command `id`: what it started and the command itself, or its place in line.
    /// A thing an agent holds for its turn goes to the next in line, and a process run holding
    /// one stops.
    pub(crate) fn stop(&self, core: &Core, id: u64) -> Result<(), String> {
        let mut state = self.state.locked();
        if let Some(at) = state.waiting.iter().position(|run| run.id == id) {
            let run = &state.waiting[at];
            if run.callers.is_empty() && run.kind != Kind::Process {
                state.waiting.remove(at);
                drop(state);
                self.turned.notify_waiters();
                return Ok(());
            }
        }
        let process = (state.running.iter().chain(&state.waiting))
            .find(|run| run.id == id && run.kind == Kind::Process)
            .and_then(|run| run.execution_id.clone());
        if let Some(execution_id) = process {
            drop(state);
            let core = core.arc().ok_or("the core is stopping")?;
            tokio::spawn(async move {
                let _ = super::harness::stop_run(&core, &execution_id).await;
            });
            return Ok(());
        }
        if let Some(at) =
            (state.running.iter()).position(|run| run.id == id && run.kind == Kind::Hold)
        {
            let run = state.running.remove(at);
            drop(state);
            if let Some(core) = core.arc() {
                let words = format!(
                    "The person stopped your hold on {}, so it went to whoever waits for it next.",
                    things::labels(&run)
                );
                tell(&core, run.agent_id, words);
                tidy(&core);
            }
            return Ok(());
        }
        let run = (state.running.iter().chain(&state.waiting))
            .find(|run| run.id == id)
            .ok_or("that command has finished")?;
        let pid = run
            .pid
            .ok_or("that command did not say which process it is")?;
        drop(state);
        stop_tree(pid);
        Ok(())
    }

    /// Starts every waiting run whose turn has come, while the host has room, and lets go of
    /// places and room kept too long.
    fn admit(&self, state: &mut State, room: &Room) -> Vec<Woken> {
        let now = epoch_ms();
        let promote = |run: &mut Run| run.held = Held::No;
        state.waiting.retain_mut(|run| match run.held {
            Held::PlaceUntil(until) if until <= now => {
                if run.callers.is_empty() {
                    return false;
                }
                promote(run);
                true
            }
            _ => true,
        });
        state.running.retain_mut(|run| match run.held {
            Held::RoomUntil(until) if until <= now => {
                if run.callers.is_empty() {
                    return false;
                }
                promote(run);
                run.since = now;
                true
            }
            Held::Taken => run.pid.zip(run.started).is_some_and(|(pid, started)| {
                sikemux_process::procs::started(pid) == Some(started)
            }),
            _ => true,
        });
        state.finished.retain(
            |(_, _, log, at)| match now.saturating_sub(*at) < FINISHED_KEPT_MS {
                true => true,
                false => {
                    if let Some(log) = log {
                        let _ = std::fs::remove_file(log);
                    }
                    false
                }
            },
        );
        let mut woken = Vec::new();
        let mut blocked = false;
        let mut at = 0;
        while at < state.waiting.len() {
            let running: Vec<u64> = state.running.iter().map(|run| run.id).collect();
            let run = &state.waiting[at];
            let set_aside = matches!(run.held, Held::PlaceUntil(_))
                || run.after.is_some_and(|after| running.contains(&after));
            if set_aside {
                at += 1;
                continue;
            }
            let wants = things::unfree(state, run, &room.around);
            let waits_for_thing = wants.is_some();
            state.waiting[at].wants = wants;
            if waits_for_thing {
                at += 1;
                continue;
            }
            if state.waiting[at].in_line {
                if blocked {
                    at += 1;
                    continue;
                }
                if let Some(why) = why_not_yet(state, &state.waiting[at], room) {
                    state.why = why;
                    blocked = true;
                    at += 1;
                    continue;
                }
            }
            let mut run = state.waiting.remove(at);
            run.since = now;
            run.after = None;
            run.held = match (run.held, run.kind) {
                (Held::ComesBack, _) => {
                    if let Some(agent_id) = &run.agent_id {
                        woken.push(Woken {
                            agent_id: agent_id.clone(),
                            cwd: run.cwd.clone(),
                            command: run.command.clone(),
                            thing: (run.kind == Kind::Hold).then(|| things::labels(&run)),
                        });
                    }
                    Held::RoomUntil(now + ROOM_KEPT_MS)
                }
                (held, _) => held,
            };
            if run.copies_output && run.log.is_none() {
                run.log = self.new_log(run.id);
            }
            run.take_own_cgroup();
            state.running.push(run);
        }
        if !blocked {
            state.why.clear();
        }
        woken
    }

    /// `agent_id` holds the thing `name` of `project` for its turn, now when it is free, or once it
    /// is: it is sent away, woken when the thing is free, and holds it on asking again. Answers
    /// what the agent is told.
    pub(crate) fn hold(
        &self,
        core: &Arc<Core>,
        agent_id: &str,
        project: &str,
        name: &str,
    ) -> Result<(bool, String), String> {
        let need = things::resolve(project, Path::new(project), &[name.to_owned()])?
            .pop()
            .ok_or("hold needs the name of one of the project's resources")?;
        let label = need.label.clone();
        let key = need.key.clone();
        let mut state = self.state.locked();
        let mine = |run: &Run| {
            run.kind == Kind::Hold
                && run.is_for(agent_id)
                && run.needs.iter().any(|held| held.key == key)
        };
        if let Some(run) = state.running.iter_mut().find(|run| mine(run)) {
            run.held = Held::Turn;
            run.since = epoch_ms();
            drop(state);
            tidy(core);
            return Ok((true, format!("You hold {label} until your turn ends. Nobody else gets it meanwhile; it goes to whoever waits next once your turn ends, you close, or you do no work for ten minutes.")));
        }
        if !state.waiting.iter().any(&mine) {
            if let Some(other) = things::would_deadlock(&state, agent_id, &key) {
                let other = name_of(core, &other);
                return Err(format!(
                    "Refused: @{other} holds {label} for its turn and waits for something you hold for yours, so each of you would wait on the other. End your turn, or finish with what you hold, before asking for {label}."
                ));
            }
            state.next += 1;
            let id = state.next;
            state
                .waiting
                .push(things::turn_hold(id, agent_id, project, need));
        }
        let room = Room::now(core, state.outside.clone());
        let woken = self.admit(&mut state, &room);
        self.tend(core);
        let granted = state.running.iter_mut().find(|run| mine(run));
        let answer = match granted {
            Some(run) => {
                run.held = Held::Turn;
                Ok((true, format!("You hold {label} until your turn ends. Nobody else gets it meanwhile; it goes to whoever waits next once your turn ends, you close, or you do no work for ten minutes.")))
            }
            None => {
                let at = (state.waiting.iter()).position(mine).unwrap_or_default();
                let wants = (state.waiting.get(at))
                    .and_then(|run| run.wants.clone())
                    .map(|wants| wants.words(&|agent| name_of(core, agent)))
                    .unwrap_or_else(|| format!("{label} is in use"));
                if let Some(run) = state.waiting.get_mut(at) {
                    run.held = Held::ComesBack;
                }
                Ok((false, format!(
                    "Not yet: {wants}. You are {} in line for it. Sikemux tells you when it is free; call hold again then, within {} s. Do other work or end your turn meanwhile.",
                    place_word(at + 1),
                    ROOM_KEPT_MS / 1000
                )))
            }
        };
        let waiting = matches!(answer, Ok((false, _)));
        self.changed(core, state);
        for woken in woken.into_iter().filter(|woken| woken.agent_id != agent_id) {
            match woken.thing {
                Some(thing) => super::triggers::in_use_freed(
                    core,
                    &woken.agent_id,
                    &woken.cwd,
                    &woken.command,
                    &thing,
                ),
                None => super::triggers::line_turn_came(
                    core,
                    &woken.agent_id,
                    &woken.cwd,
                    &woken.command,
                ),
            }
        }
        if waiting {
            super::triggers::park_for_thing(core, agent_id, project, name, &label);
        }
        answer
    }

    /// `agent_id` called one of Sikemux's tools, so a thing it holds for its turn stays held.
    pub(crate) fn called(&self, agent_id: &str) {
        self.state
            .locked()
            .called
            .insert(agent_id.to_owned(), epoch_ms());
    }

    /// One of `agent_id`'s tool calls, of any kind, its own shell's included, began, moved or
    /// ended, as `update` from its chat says: while one runs, a thing it holds for its turn stays
    /// held.
    pub(crate) fn tool_update(&self, agent_id: &str, update: &serde_json::Value) {
        let Some(id) = update.get("toolCallId").and_then(serde_json::Value::as_str) else {
            return;
        };
        let status = update.get("status").and_then(serde_json::Value::as_str);
        let ended = matches!(status, Some("completed" | "failed"));
        let mut state = self.state.locked();
        state.called.insert(agent_id.to_owned(), epoch_ms());
        let open = state.open_calls.entry(agent_id.to_owned()).or_default();
        match ended {
            true => {
                open.remove(id);
            }
            false if open.len() < MOST_OPEN_CALLS => {
                open.insert(id.to_owned());
            }
            false => {}
        }
    }

    /// `agent_id`'s turn ended, and with it what it held for the turn.
    pub(crate) fn turn_ended(&self, core: &Core, agent_id: &str) {
        let mut state = self.state.locked();
        state.open_calls.remove(agent_id);
        let before = state.running.len();
        state.running.retain(|run| {
            !(run.kind == Kind::Hold && run.held == Held::Turn && run.is_for(agent_id))
        });
        if state.running.len() != before {
            drop(state);
            tidy_soon(core);
        }
    }

    /// `agent_id` closed: what it holds goes, and its places in line with it.
    pub(crate) fn agent_closed(&self, core: &Core, agent_id: &str) {
        let mut state = self.state.locked();
        let theirs =
            |run: &Run| run.is_for(agent_id) && run.callers.is_empty() && run.kind != Kind::Process;
        let before = state.running.len() + state.waiting.len();
        state.running.retain(|run| !theirs(run));
        state.waiting.retain(|run| !theirs(run));
        state.called.remove(agent_id);
        state.open_calls.remove(agent_id);
        if state.running.len() + state.waiting.len() != before {
            drop(state);
            tidy_soon(core);
        }
    }

    /// Waits until the `process_start` run `execution_id` of `agent_id`, running `label` in `cwd`,
    /// may have everything in `needs`, which it then holds until it ends. Err when the run was
    /// stopped meanwhile.
    pub(crate) async fn wait_for_needs(
        &self,
        core: &Arc<Core>,
        execution_id: &str,
        agent_id: Option<String>,
        cwd: &str,
        label: &str,
        needs: Vec<Need>,
    ) -> Result<(), String> {
        let ports: Vec<u16> = needs.iter().filter_map(|need| need.port).collect();
        let outside = super::connection::blocking(move || Ok(things::outside_holders(&ports)))
            .await
            .unwrap_or_default();
        let id = {
            let mut state = self.state.locked();
            state.outside.extend(outside);
            state.next += 1;
            let id = state.next;
            let first = needs.first().cloned().ok_or("no needs")?;
            let mut run =
                things::turn_hold(id, agent_id.as_deref().unwrap_or_default(), cwd, first);
            run.needs = needs;
            run.agent_id = agent_id;
            run.kind = Kind::Process;
            run.held = Held::Core;
            run.command = label.to_owned();
            run.execution_id = Some(execution_id.to_owned());
            state.waiting.push(run);
            id
        };
        self.tend(core);
        tidy(core);
        loop {
            if self.state.locked().running.iter().any(|run| run.id == id) {
                return Ok(());
            }
            let stopped = core
                .harness()
                .run_of(execution_id)
                .is_none_or(|run| !run.status.is_active());
            if stopped {
                self.state.locked().waiting.retain(|run| run.id != id);
                tidy(core);
                return Err("it was stopped while it waited for what it needs".to_owned());
            }
            let _ = tokio::time::timeout(LOOK_EVERY, self.turned.notified()).await;
        }
    }

    /// The process run `execution_id` ended, and with it what it held.
    pub(crate) fn run_ended(&self, core: &Core, execution_id: &str) {
        let mut state = self.state.locked();
        let before = state.running.len() + state.waiting.len();
        let ended = |run: &Run| {
            run.kind == Kind::Process && run.execution_id.as_deref() == Some(execution_id)
        };
        state.running.retain(|run| !ended(run));
        state.waiting.retain(|run| !ended(run));
        if state.running.len() + state.waiting.len() != before {
            drop(state);
            tidy_soon(core);
        }
    }

    fn new_log(&self, id: u64) -> Option<PathBuf> {
        let folder = self.logs.as_ref()?;
        std::fs::create_dir_all(folder).ok()?;
        let log = folder.join(format!("{}-{id}.log", epoch_ms()));
        std::fs::File::create(&log).ok()?;
        Some(log)
    }

    /// Looks after the line every little while, for as long as anything runs or waits: measures
    /// running commands, stops one that takes too much, starts what may start and wakes agents
    /// whose turn came while they were away.
    fn tend(&self, core: &Arc<Core>) {
        if self.tending.swap(true, Ordering::AcqRel) {
            return;
        }
        let core = Arc::downgrade(core);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LOOK_EVERY).await;
                let Some(owner) = core.upgrade() else {
                    return;
                };
                let measured = {
                    let owner = owner.clone();
                    super::connection::blocking(move || Ok(measure(&owner))).await
                };
                if !matches!(measured, Ok(true)) {
                    return;
                }
                tidy(&owner);
            }
        });
    }

    /// Keeps the line as it is now, then tells the callers and clients it moved.
    fn changed(&self, core: &Core, state: std::sync::MutexGuard<'_, State>) {
        self.saved.save(&state);
        drop(state);
        self.turned.notify_waiters();
        core.changed(What::View);
    }
}

impl super::part::Part for Line {
    /// Takes up what an earlier core left running or waiting for an agent sent away.
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        self.tend(core);
        Vec::new()
    }
}

/// Starts what may start, wakes the agents whose turn came, lets go of turn holds whose agent
/// went quiet, and tells callers the line moved.
fn tidy(core: &Arc<Core>) {
    let line = core.line();
    let outside = line.state.locked().outside.clone();
    let room = Room::now(core, outside);
    let mut state = line.state.locked();
    let now = epoch_ms();
    let idle = things::idle_holds(&state, now);
    let mut quiet = Vec::new();
    state.running.retain(|run| match idle.contains(&run.id) {
        true => {
            quiet.push((run.agent_id.clone(), things::labels(run)));
            false
        }
        false => true,
    });
    let long_ids = things::held_long(&state, now);
    let long: Vec<(Option<String>, String, u32)> = (state.running.iter())
        .filter(|run| long_ids.contains(&run.id))
        .map(|run| {
            (
                run.agent_id.clone(),
                things::labels(run),
                things::waiting_for(&state, run),
            )
        })
        .collect();
    for run in (state.running.iter_mut()).filter(|run| long_ids.contains(&run.id)) {
        run.told_long = true;
    }
    let woken = line.admit(&mut state, &room);
    line.changed(core, state);
    for woken in woken {
        match woken.thing {
            Some(thing) => super::triggers::in_use_freed(
                core,
                &woken.agent_id,
                &woken.cwd,
                &woken.command,
                &thing,
            ),
            None => {
                super::triggers::line_turn_came(core, &woken.agent_id, &woken.cwd, &woken.command)
            }
        }
    }
    for (agent, thing) in quiet {
        let words = format!("You held {thing} for your turn but did no work for a while, so it went to whoever waits for it next. Call hold again when you need it.");
        tell(core, agent, words);
    }
    for (agent, thing, waiting) in long {
        held_long(core, agent, &thing, waiting);
    }
}

/// Looks at the line again, from where only the core itself is at hand.
fn tidy_soon(core: &Core) {
    if let Some(core) = core.arc() {
        tidy(&core);
    }
}

/// Hands `words` to `agent`, from Sikemux, when there is an agent.
fn tell(core: &Arc<Core>, agent: Option<String>, words: String) {
    let Some(agent) = agent else {
        return;
    };
    let core = core.clone();
    tokio::spawn(async move {
        let why = "about something it holds that only one agent uses at a time";
        let _ = super::messages::work_from_sikemux(&core, agent, words, why).await;
    });
}

/// Tells the lead of `agent`, or the person when it has none, that it has held `thing` long
/// while others wait. The holder is never stopped for it.
fn held_long(core: &Arc<Core>, agent: Option<String>, thing: &str, waiting: u32) {
    let who = agent
        .as_deref()
        .map(|agent| format!("@{}", name_of(core, agent)));
    let words = format!(
        "{} has held {thing} for over half an hour while {waiting} {} for it. Nothing stops it for taking long; ask it to finish, or stop it from the host's list of what is in use.",
        who.as_deref().unwrap_or("A process"),
        if waiting == 1 { "waits" } else { "wait" },
    );
    let lead = agent.as_deref().and_then(|agent| lead_of(core, agent));
    match (lead, agent) {
        (Some(lead), _) => tell(core, Some(lead), words),
        (None, Some(agent)) => core.notify().send(super::notify::Signal::Problem {
            agent_id: agent,
            message: words,
        }),
        (None, None) => {}
    }
}

/// The agent that started `agent_id`, on this host or a linked one.
fn lead_of(core: &Core, agent_id: &str) -> Option<String> {
    let started_by = (core.chats().get(agent_id)).and_then(|chat| chat.origin.started_by.clone());
    started_by.or_else(|| core.hosts().team.get(agent_id)?.parent)
}

/// What the person calls `agent_id`.
fn name_of(core: &Core, agent_id: &str) -> String {
    core.names()
        .get(agent_id)
        .map_or_else(|| agent_id.to_owned(), |named| named.name)
}

/// About how long the run waiting at `at` waits from now: what the ones ahead of it take, once
/// every one of them was measured.
fn expected_wait(state: &State, at: usize) -> Option<u64> {
    let now = epoch_ms();
    let ahead = (state.waiting[..at].iter())
        .map(|run| run.expected.map(|size| size.took_ms))
        .sum::<Option<u64>>()?;
    let soonest_free = (state
        .running
        .iter()
        .filter(|run| run.in_line && !run.released))
    .map(|run| {
        let size = run.expected?;
        Some(size.took_ms.saturating_sub(now.saturating_sub(run.since)))
    })
    .collect::<Option<Vec<u64>>>()?
    .into_iter()
    .min()
    .unwrap_or(0);
    Some(ahead + soonest_free)
}

/// Why the run `first` may not start yet, if it may not.
fn why_not_yet(state: &State, first: &Run, room: &Room) -> Option<String> {
    let busy: Vec<&Run> = (state.running.iter())
        .filter(|run| !run.released && run.kind == Kind::Command)
        .collect();
    if busy.is_empty() {
        return None;
    }
    if room.short_of_memory {
        return Some("the host is short of memory".to_owned());
    }
    let in_line = busy.iter().filter(|run| run.in_line).count();
    if room.chosen > 0 && in_line >= room.chosen {
        return Some(format!(
            "{} run at once, as Settings → Chats → Heavy commands at once has it",
            room.chosen
        ));
    }
    match room.free {
        Some(free) => {
            let growing: u64 = busy.iter().map(|run| run.growth()).sum();
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
    let young = |run: &&&Run| now.saturating_sub(run.since) < UNSEEN_APART_MS;
    if first.expected.is_none() && busy.iter().any(|run| young(&run)) {
        return Some("commands never measured start 20 s apart".to_owned());
    }
    (busy.iter().filter(young))
        .any(|run| !run.measured_once || run.expected.is_some_and(|size| run.holds > size.peak))
        .then(|| "the last one started has not shown how much it takes yet".to_owned())
}

/// Takes the measure of every running command's processes, and stops one past half the host's
/// memory. False once nothing runs or waits, and the tending stops.
fn measure(core: &Arc<Core>) -> bool {
    let line = core.line();
    let ports: Vec<u16> = (line.state.locked().waiting.iter())
        .flat_map(|run| run.needs.iter().filter_map(|need| need.port))
        .collect();
    let outside = things::outside_holders(&ports);
    let pids: Vec<(u64, u32)> = {
        let mut state = line.state.locked();
        state.outside = outside;
        if state.running.is_empty() && state.waiting.is_empty() && state.finished.is_empty() {
            line.tending.store(false, Ordering::Release);
            return false;
        }
        (state.running.iter())
            .filter(|run| run.held == Held::No || run.held == Held::Taken)
            .filter_map(|run| Some((run.id, run.pid?)))
            .collect()
    };
    let ceiling = ceiling();
    let held: Vec<(u64, u32, u64)> = pids
        .into_iter()
        .map(|(id, pid)| (id, pid, tree_footprint(pid)))
        .collect();
    let mut stopping = Vec::new();
    let mut state = line.state.locked();
    for (id, pid, holds) in held {
        let Some(run) = state.running.iter_mut().find(|run| run.id == id) else {
            continue;
        };
        run.holds = holds;
        run.peak = run.peak.max(holds);
        run.measured_once = true;
        if ceiling.is_some_and(|ceiling| holds > ceiling) && run.stopped.is_none() {
            run.stopped = Some(format!(
                "its processes held {} GB, past half of this host's memory, so Sikemux stopped it before the host ran out",
                holds >> 30
            ));
            stopping.push(pid);
        }
    }
    drop(state);
    if !stopping.is_empty() {
        line.turned.notify_waiters();
    }
    for pid in stopping {
        stop_below(pid);
    }
    true
}

/// How much memory one heavy command's processes may hold together: half the host's.
fn ceiling() -> Option<u64> {
    #[cfg(debug_assertions)]
    if let Some(bytes) = std::env::var("SIKEMUX_LINE_CEILING_BYTES")
        .ok()
        .and_then(|bytes| bytes.parse().ok())
    {
        return Some(bytes);
    }
    sikemux_process::machine::memory().map(|memory| memory.total / 2)
}

/// The memory `root` and every process under it hold together.
fn tree_footprint(root: u32) -> u64 {
    let below = sikemux_process::procs::descendants(root);
    (below.keys().copied().chain([root]))
        .filter_map(sikemux_process::procs::footprint)
        .sum()
}

/// Ends what `root` started, leaving `root` to say why its command ended.
fn stop_below(root: u32) {
    for target in sikemux_process::procs::descendants(root).into_keys() {
        if let Ok(target) = libc::pid_t::try_from(target) {
            // SAFETY: kill only sends a signal; the pids come from the command's own tree.
            unsafe { libc::kill(target, libc::SIGTERM) };
        }
    }
}

/// Ends `root`'s processes: what it started, then itself.
fn stop_tree(root: u32) {
    let below = sikemux_process::procs::descendants(root);
    for target in below.keys().copied().chain([root]) {
        if let Ok(target) = libc::pid_t::try_from(target) {
            // SAFETY: kill only sends a signal; the pids come from the command's own tree.
            unsafe { libc::kill(target, libc::SIGTERM) };
        }
    }
}

/// One caller's place in line, its turn, or the run it follows, until it is dropped.
pub(crate) struct Turn {
    core: Arc<Core>,
    caller: u64,
    run: u64,
    agent_id: Option<String>,
    /// It was sent away, so its run keeps its place for its agent.
    sent_away: AtomicBool,
}

impl Turn {
    pub(crate) fn ask(core: &Arc<Core>, request: BuildRequest) -> Self {
        let line = core.line();
        let now = epoch_ms();
        let cwd = request.cwd.clone().unwrap_or_default();
        let measured = request
            .key
            .clone()
            .map(|key| (core.workspaces().project_of(Path::new(&cwd)), key));
        let expected = (measured.as_ref()).and_then(|(project, key)| line.sizes.of(project, key));
        let agent = request.agent_id.clone();
        let needs = match &measured {
            Some((project, _)) => {
                things::resolve(project, Path::new(&cwd), &request.needs).unwrap_or_default()
            }
            None => Vec::new(),
        };
        let same_since = {
            let state = line.state.locked();
            (state.running.iter())
                .find(|run| run.same(&cwd, &request.command) && run.held == Held::No)
                .map(|run| run.since)
        };
        let unchanged = same_since.and_then(|since| unchanged_since(core, Path::new(&cwd), since));
        let in_line = agent.is_some()
            && (!request.watched || expected.is_some_and(|size| size.peak > WATCHED_HEAVY_BYTES));
        let mut state = line.state.locked();
        state.next += 1;
        let caller = Caller {
            id: state.next,
            agent_id: agent.clone(),
            exit: None,
        };
        let back = taken_back(&mut state, &request, &cwd, caller.clone());
        let run = match back {
            Some(run) => run,
            None => match joined(&mut state, &request, &cwd, caller.clone(), unchanged) {
                Joined::Run(run) => run,
                Joined::No { after } => {
                    state.next += 1;
                    let id = state.next;
                    let head_start = match request.before_commit {
                        true => BEFORE_COMMIT_HEAD_START_MS,
                        false => 0,
                    };
                    let asked_at = request.asked_at.unwrap_or(now);
                    let run = Run {
                        id,
                        agent_id: agent.clone(),
                        command: request.command.clone(),
                        cwd: cwd.clone(),
                        callers: vec![caller.clone()],
                        held: Held::No,
                        pid: request.pid,
                        started: request.pid.and_then(sikemux_process::procs::started),
                        since: now,
                        order: asked_at.saturating_sub(head_start),
                        measured,
                        expected,
                        before_commit: request.before_commit,
                        in_line,
                        released: false,
                        after,
                        copies_output: request.copies_output,
                        log: None,
                        execution_id: request.execution_id.clone(),
                        stopped: None,
                        peak: 0,
                        holds: 0,
                        measured_once: false,
                        needs: needs.clone(),
                        kind: Kind::Command,
                        wants: None,
                        told_long: false,
                    };
                    let at = state
                        .waiting
                        .partition_point(|waiting| waiting.order <= run.order);
                    state.waiting.insert(at, run);
                    id
                }
            },
        };
        drop(state);
        if let (Some(_), Some(agent_id)) = (back, &agent) {
            super::triggers::line_left(core, agent_id, &cwd, &request.command);
        }
        line.tend(core);
        tidy(core);
        Turn {
            core: core.clone(),
            caller: caller.id,
            run,
            agent_id: agent,
            sent_away: AtomicBool::new(false),
        }
    }

    /// Where it stands now.
    pub(crate) fn place(&self) -> Place {
        let line = self.core.line();
        let state = line.state.locked();
        if let Some(run) = state.running.iter().find(|run| run.id == self.run) {
            let first = run
                .callers
                .first()
                .is_some_and(|caller| caller.id == self.caller);
            let log = run
                .log
                .as_ref()
                .map(|log| log.to_string_lossy().into_owned());
            return match (first, log) {
                (true, log) => Place::Running { log },
                (false, Some(log)) => Place::Following { log },
                (false, None) => Place::Waiting {
                    ahead: 0,
                    running: state.running.len(),
                    why: "the same command runs for another caller".to_owned(),
                    expected_ms: None,
                },
            };
        }
        if let Some(at) = state.waiting.iter().position(|run| run.id == self.run) {
            let after = state.waiting[at].after;
            let wants = (state.waiting[at].wants.as_ref())
                .map(|wants| wants.words(&|agent| name_of(&self.core, agent)));
            let why = match at {
                _ if wants.is_some() => wants.unwrap_or_default(),
                _ if after.is_some_and(|after| state.running.iter().any(|run| run.id == after)) => {
                    "the same command runs here already, started before a file changed, so this one runs once it ends".to_owned()
                }
                0 if !state.why.is_empty() => state.why.clone(),
                0 => "its turn is about to come".to_owned(),
                _ => "the ones ahead go first".to_owned(),
            };
            return Place::Waiting {
                ahead: at,
                running: state.running.len(),
                why,
                expected_ms: expected_wait(&state, at),
            };
        }
        let finished = (state.finished.iter()).find(|(id, ..)| *id == self.run);
        Place::Ended {
            code: finished.and_then(|(_, code, ..)| *code),
            log: finished
                .and_then(|(_, _, log, _)| log.as_ref())
                .map(|log| log.to_string_lossy().into_owned()),
        }
    }

    /// Why the core stopped the command it runs, once it did.
    pub(crate) fn stopped(&self) -> Option<String> {
        let state = self.core.line().state.locked();
        (state.running.iter())
            .find(|run| run.id == self.run)
            .and_then(|run| run.stopped.clone())
    }

    /// Waits until the line moves or a little while passes, as memory frees up too.
    pub(crate) async fn changed(&self) {
        let _ = tokio::time::timeout(LOOK_EVERY, self.core.line().turned.notified()).await;
    }

    /// What it built runs now, which needs no turn.
    pub(crate) fn released(&self) {
        let line = self.core.line();
        let mut state = line.state.locked();
        if let Some(run) = state.running.iter_mut().find(|run| run.id == self.run) {
            run.released = true;
        }
        line.changed(&self.core, state);
    }

    pub(crate) fn exited(&self, code: i32) {
        let mut state = self.core.line().state.locked();
        let run = (state.running.iter_mut()).find(|run| run.id == self.run);
        if let Some(caller) = run.and_then(|run| {
            run.callers
                .iter_mut()
                .find(|caller| caller.id == self.caller)
        }) {
            caller.exit = Some(code);
        }
    }

    /// Sends its agent away until its turn comes, keeping its place, and says so.
    pub(crate) fn send_away(&self, ahead: usize, expected_ms: u64) -> Option<String> {
        let agent_id = self.agent_id.as_ref()?;
        let (cwd, command, project) = {
            let state = self.core.line().state.locked();
            let run = state.waiting.iter().find(|run| run.id == self.run)?;
            if run.callers.len() > 1 {
                return None;
            }
            let project = (run.measured.as_ref()).map(|(project, _)| project.clone());
            (run.cwd.clone(), run.command.clone(), project)
        };
        self.sent_away.store(true, Ordering::Relaxed);
        let project = project.unwrap_or_else(|| cwd.clone());
        super::triggers::park_in_line(&self.core, agent_id, &project, &cwd, &command);
        Some(format!(
            "Not started: {} in line, {}. Sikemux starts you again when it is your turn.",
            place_word(ahead + 1),
            about(expected_ms)
        ))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let line = self.core.line();
        let now = epoch_ms();
        let mut state = line.state.locked();
        if let Some(at) = state.running.iter().position(|run| run.id == self.run) {
            let run = &mut state.running[at];
            let first = run
                .callers
                .first()
                .is_some_and(|caller| caller.id == self.caller);
            let exit = (run.callers.iter())
                .find(|caller| caller.id == self.caller)
                .and_then(|caller| caller.exit);
            run.callers.retain(|caller| caller.id != self.caller);
            if first {
                let run = state.running.remove(at);
                if let Some((project, key)) = &run.measured {
                    let took = now.saturating_sub(run.since);
                    line.sizes.record(project, key, run.peak, took);
                }
                state.finished.push((run.id, exit, run.log, now));
            }
        } else if let Some(at) = state.waiting.iter().position(|run| run.id == self.run) {
            let run = &mut state.waiting[at];
            run.callers.retain(|caller| caller.id != self.caller);
            if let Some(next) = run.callers.first() {
                run.agent_id = next.agent_id.clone();
            } else {
                run.held = match (self.sent_away.load(Ordering::Relaxed), &self.agent_id) {
                    (true, _) => Held::ComesBack,
                    (false, Some(_)) => Held::PlaceUntil(now + PLACE_KEPT_MS),
                    (false, None) => Held::No,
                };
                if run.held == Held::No {
                    state.waiting.remove(at);
                }
            }
        }
        drop(state);
        tidy(&self.core);
    }
}

/// The run `request`'s agent left in line or was woken for, taken back by this caller.
fn taken_back(state: &mut State, request: &BuildRequest, cwd: &str, caller: Caller) -> Option<u64> {
    let agent_id = request.agent_id.as_deref()?;
    let mine =
        |run: &Run| run.is_for(agent_id) && run.same(cwd, &request.command) && run.held != Held::No;
    if let Some(run) = state.running.iter_mut().find(|run| mine(run)) {
        run.held = Held::No;
        run.callers.insert(0, caller);
        run.pid = request.pid;
        run.started = request.pid.and_then(sikemux_process::procs::started);
        run.since = epoch_ms();
        run.copies_output = request.copies_output;
        run.take_own_cgroup();
        return Some(run.id);
    }
    let run = state.waiting.iter_mut().find(|run| mine(run))?;
    run.held = Held::No;
    run.callers.insert(0, caller);
    run.pid = request.pid;
    run.copies_output = request.copies_output;
    Some(run.id)
}

enum Joined {
    Run(u64),
    /// Not joined; it waits for the same command running now, when one does.
    No {
        after: Option<u64>,
    },
}

/// Whether no file changed in the checkout `dir` is in since `since`: neither its watch, when
/// agents work there, nor git, by the files it lists as changed, saw one change. The watch
/// hears of a change a moment late, so git is asked as well.
fn unchanged_since(core: &Core, dir: &Path, since: u64) -> Option<bool> {
    if core.edits().unchanged_since(dir, since) == Some(false) {
        return Some(false);
    }
    let top = sikemux_process::files::repo_root(dir)?;
    let listed = super::edits::git(top, &["status", "--porcelain", "-z"]).ok()?;
    let changed_since = |path: &str| {
        std::fs::metadata(top.join(path))
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| sikemux_process::clock::epoch_ms_of(at) >= since)
    };
    Some(
        !listed
            .split('\0')
            .filter_map(|entry| entry.get(3..))
            .any(changed_since),
    )
}

/// The run of the same command in the same checkout this caller follows, when it may.
/// `unchanged` says whether the checkout's files are as they were when that run started.
fn joined(
    state: &mut State,
    request: &BuildRequest,
    cwd: &str,
    caller: Caller,
    unchanged: Option<bool>,
) -> Joined {
    let same = |run: &Run| run.same(cwd, &request.command) && run.held == Held::No;
    if let Some(run) = state.waiting.iter_mut().find(|run| same(run)) {
        if run.copies_output && request.copies_output {
            run.callers.push(caller);
            return Joined::Run(run.id);
        }
    }
    let Some(run) = state.running.iter_mut().find(|run| same(run)) else {
        return Joined::No { after: None };
    };
    if run.log.is_some() && run.stopped.is_none() && unchanged == Some(true) {
        run.callers.push(caller);
        return Joined::Run(run.id);
    }
    Joined::No {
        after: Some(run.id),
    }
}

/// A place in line as every screen words it: next, then 2nd, 3rd and so on.
fn place_word(place: usize) -> String {
    if place == 1 {
        return "next".to_owned();
    }
    let suffix = match (place % 10, place % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{place}{suffix}")
}

/// Rounded up to whole minutes, as the apps word the same wait.
fn about(ms: u64) -> String {
    format!("about {} min", ms.div_ceil(60_000).max(1))
}

/// The `hold` tool: the calling agent holds one of its project's things for its turn.
pub(crate) fn hold_tool(
    core: &Arc<Core>,
    request: &crate::cli::protocol::HarnessRequest,
) -> Result<serde_json::Value, String> {
    let agent_id = super::caller::agent_of(request, "hold")?;
    let name = (request.params.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or("hold needs name: one of the resources in sikemux.json")?;
    let project = super::caller::project_of(core, request);
    let (held, note) = core.line().hold(core, &agent_id, &project, name)?;
    Ok(serde_json::json!({ "held": held, "note": note }))
}
