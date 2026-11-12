//! How busy this host is, for its page on the Hosts tab, the rows that show its load and the
//! alerts the inbox lists. A cheap look at the whole host runs always, so its history is there
//! when the page opens and alerts can start; what each agent uses is read only while a client
//! watches the page, which a `hostStats` read for more than now does for a while.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use sikemux_process::clock::epoch_ms;
use sikemux_process::machine::{self, CpuMeter};
use sikemux_process::memory_pressure::{self, Pressure};
use sikemux_process::procs::{self, Usage};
use sikemux_process::sync::Locked;
use sikemux_view::changed::Topic;
use sikemux_view::device::{HostAlerted, HostLoad};
use sikemux_view::host_stats::{
    AgentUse, AlertLevel, DiskPart, DiskPartKind, HostAbout, HostAlert, HostAlertKind, HostEvent,
    HostEventKind, HostMark, HostSample, HostStats, HostStatsParams, ProcessUse, RestUse,
    StatsSpan,
};
use tokio::task::JoinHandle;

use super::changed::What;
use super::Core;

pub mod apps;

const HISTORY_EVERY: Duration = Duration::from_secs(15);
const WATCHED_EVERY: Duration = Duration::from_secs(3);
/// How often the rows' load may send the device view again, as phones and the notch get it too.
const LOAD_SHOWN_EVERY: Duration = Duration::from_secs(30);
/// After the first look at every process, the next comes sooner, as CPU is told between two.
const FIRST_LOOKS_APART: Duration = Duration::from_secs(1);
const WATCH_LASTS_MS: u64 = 15_000;
/// An hour of samples, one each `HISTORY_EVERY`.
const FINE_KEPT: usize = 240;
/// Fine samples taken together into one for the day's history: five minutes.
const FINE_PER_COARSE: usize = 20;
const COARSE_KEPT: usize = 288;
const EVENTS_KEPT: usize = 64;
const TOP_KEPT: usize = 8;
const REST_NAMED: usize = 3;
const MAX_COMMAND_CHARS: usize = 160;
/// A process busier than this share of one core, in thousandths, counts as running something.
const COMMAND_BUSY: u64 = 20;
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "env"];
const CPU_BUSY: u16 = 900;
const CPU_CALM: u16 = 800;
const CPU_BUSY_FOR_MS: u64 = 10 * 60_000;
const MEMORY_LIMITS: Limits = Limits {
    warn: 92,
    danger: 97,
    calm: 90,
};
const DISK_LIMITS: Limits = Limits {
    warn: 90,
    danger: 97,
    calm: 88,
};
const OFFLINE_AFTER_MS: u64 = 2 * 60_000;
/// How long the system must say memory is short before that alerts, and calm before it ends,
/// so a moment's stall neither starts nor ends one.
const PRESSURE_FOR_MS: u64 = 60_000;
const PRESSURE_CALM_FOR_MS: u64 = 3 * 60_000;
const DISK_PARTS_EVERY: Duration = Duration::from_secs(30 * 60);

pub(crate) struct HostWatch {
    state: Mutex<State>,
    about: OnceLock<HostAbout>,
    watched_until: AtomicU64,
    woken: tokio::sync::Notify,
    measuring: AtomicBool,
    icons: apps::Icons,
}

#[derive(Default)]
struct State {
    now: HostSample,
    memory_total: u64,
    disk_total: u64,
    fine: VecDeque<HostSample>,
    coarse: VecDeque<HostSample>,
    looks: usize,
    pressure: Option<Pressure>,
    /// Since when the pressure has been what it is now.
    pressure_since: u64,
    /// How bad the system's pressure has counted, apart from how much memory is used.
    pressed: Option<AlertLevel>,
    busy_since: Option<u64>,
    alerts: Vec<HostAlert>,
    offline: Vec<(String, u64)>,
    events: VecDeque<HostEvent>,
    agents: Vec<AgentUse>,
    rest: Option<RestUse>,
    top: Vec<ProcessUse>,
    shown: u64,
    disk_parts: Vec<DiskPart>,
    disk_parts_at: Option<Instant>,
}

/// How full something may get before it warns, before it is dangerous, and how far it must
/// drop again before a warning ends.
struct Limits {
    warn: u16,
    danger: u16,
    calm: u16,
}

impl Default for HostWatch {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            about: OnceLock::new(),
            watched_until: AtomicU64::new(0),
            woken: tokio::sync::Notify::new(),
            measuring: AtomicBool::new(false),
            icons: apps::Icons::default(),
        }
    }
}

impl super::part::Part for HostWatch {
    fn start(&self, core: &Arc<Core>) -> Vec<JoinHandle<()>> {
        watch_pressure(Arc::downgrade(core));
        super::worktree::build_output::remove_set_aside(core);
        vec![tokio::spawn(run(core.clone()))]
    }
}

impl HostWatch {
    pub(crate) fn note(&self, what: HostEventKind) {
        self.state.locked().record(epoch_ms(), what);
    }

    /// The disk's free space and its size at the latest look, once it has been looked at.
    fn disk_free(&self) -> Option<(u64, u64)> {
        let state = self.state.locked();
        (state.disk_total > 0).then(|| {
            let free = state.disk_total.saturating_sub(state.now.disk_used);
            (free, state.disk_total)
        })
    }

    fn watched(&self) -> bool {
        epoch_ms() < self.watched_until.load(Ordering::Acquire)
    }

    fn watch(&self) {
        let was = self.watched();
        self.watched_until
            .store(epoch_ms() + WATCH_LASTS_MS, Ordering::Release);
        if !was {
            self.woken.notify_one();
        }
    }
}

pub(crate) fn read(core: &Core, params: HostStatsParams) -> HostStats {
    let watch = &core.host_watch;
    let more = params.span != StatsSpan::Now;
    if more {
        watch.watch();
    }
    let mut about = about(core).clone();
    about.name = Some(super::host::shown_name(core));
    let state = watch.state.locked();
    let history = match params.span {
        StatsSpan::Now => Vec::new(),
        StatsSpan::Minutes15 => {
            let from = state.now.at.saturating_sub(15 * 60_000);
            (state.fine.iter())
                .filter(|sample| sample.at >= from)
                .copied()
                .collect()
        }
        StatsSpan::Hour => state.fine.iter().copied().collect(),
        StatsSpan::Day => state.coarse.iter().copied().collect(),
    };
    HostStats {
        about,
        now: state.now,
        history,
        agents: if more {
            state.agents.clone()
        } else {
            Vec::new()
        },
        rest: state.rest.clone().filter(|_| more),
        top: if more { state.top.clone() } else { Vec::new() },
        disk_parts: if more {
            state.disk_parts.clone()
        } else {
            Vec::new()
        },
        alerts: state.alerts.clone(),
        events: (state.events.iter().rev())
            .filter(|event| more || matches!(event.what, HostEventKind::Killed { .. }))
            .cloned()
            .collect(),
    }
}

/// What the rows that show this host's load say of it.
pub(crate) fn load(core: &Core) -> HostLoad {
    let now = core.host_watch.state.locked().now;
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    let average = sikemux_process::machine::load_average();
    let overloaded = sikemux_process::machine::overloaded(average, cores);
    HostLoad {
        cpu: u32::from(now.cpu.div_ceil(10).min(100)),
        memory_used: (now.memory_used > 0).then_some(now.memory_used),
        overloaded,
        load_average: average
            .filter(|_| overloaded)
            .map(|average| average.round() as u32),
        cores: cores as u32,
        builds: match overloaded {
            true => core.builds.shown(),
            false => Vec::new(),
        },
    }
}

/// What a row shows of `load`: its memory only to a tenth of a gigabyte.
pub(crate) fn shown_load(load: &HostLoad) -> HostLoad {
    let tenths = |bytes: u64| (bytes as f64 / (1u64 << 30) as f64 * 10.).round() as u64;
    HostLoad {
        memory_used: load.memory_used.map(tenths),
        ..load.clone()
    }
}

/// This host's alerts, then each linked host out of reach.
pub(crate) fn alerts(core: &Core) -> Vec<HostAlerted> {
    alerts_named(core)
        .into_iter()
        .map(|(alerted, _)| alerted)
        .collect()
}

/// Every alert the inbox lists, as [`alerts`] has them, with what to call the host each is about.
pub(crate) fn inbox_alerts(core: &Core) -> Vec<(Option<String>, String, HostAlert)> {
    let here = super::host::shown_name(core);
    alerts_named(core)
        .into_iter()
        .map(|(alerted, name)| {
            let name = name.unwrap_or_else(|| here.clone());
            (alerted.host, name, alerted.alert)
        })
        .collect()
}

/// [`alerts`], each linked host's with its name.
fn alerts_named(core: &Core) -> Vec<(HostAlerted, Option<String>)> {
    let (alerts, offline) = {
        let state = core.host_watch.state.locked();
        (state.alerts.clone(), state.offline.clone())
    };
    let mut listed: Vec<_> = (alerts.into_iter())
        .map(|alert| (HostAlerted { host: None, alert }, None))
        .collect();
    if offline.is_empty() {
        return listed;
    }
    let links = core.remote.links();
    for (host, since) in offline {
        let Some(link) = links.iter().find(|link| link.id == host) else {
            continue;
        };
        let alert = HostAlert {
            kind: HostAlertKind::Offline,
            level: AlertLevel::Danger,
            since,
            percent: 0,
            pressure: false,
        };
        listed.push((
            HostAlerted {
                host: Some(host),
                alert,
            },
            Some(link.name.clone()),
        ));
    }
    listed
}

fn about(core: &Core) -> &HostAbout {
    core.host_watch.about.get_or_init(|| {
        let machine = machine::about();
        let mark = if machine.gpu.is_some() {
            HostMark::Nvidia
        } else {
            match (machine.vendor.as_deref(), machine.distribution.as_deref()) {
                (Some(vendor), _) if vendor.starts_with("Hetzner") => HostMark::Hetzner,
                (Some(vendor), _) if vendor.starts_with("DigitalOcean") => HostMark::DigitalOcean,
                _ if cfg!(target_os = "macos") => HostMark::Apple,
                (_, Some("ubuntu")) => HostMark::Ubuntu,
                (_, Some("debian")) => HostMark::Debian,
                _ if super::themes::on_omarchy() => HostMark::Omarchy,
                _ => HostMark::Linux,
            }
        };
        let info = super::host::info();
        HostAbout {
            name: Some(info.name),
            model: machine.model,
            system: machine.system,
            mark,
            cores: machine.cores,
            memory_total: machine.memory_total,
            disk_total: sikemux_process::paths::home()
                .and_then(|home| machine::disk(&home))
                .map_or(0, |disk| disk.total),
            gpu: machine.gpu,
            core_version: info.version,
        }
    })
}

fn watch_pressure(core: Weak<Core>) {
    memory_pressure::watch(move |pressure| {
        let Some(core) = core.upgrade() else {
            return;
        };
        let moved = {
            let mut state = core.host_watch.state.locked();
            let at = epoch_ms();
            if state.pressure != Some(pressure) {
                state.pressure = Some(pressure);
                state.pressure_since = at;
            }
            state.judge(at)
        };
        if moved {
            core.changed(What::View);
            core.changed(What::Read(Topic::HostStats));
        }
    });
}

/// What the looks at the host keep between one and the next. Only the task that looks holds it.
#[derive(Default)]
struct Looker {
    host_cpu: CpuMeter,
    watched_cpu: CpuMeter,
    home: Option<PathBuf>,
    used: HashMap<(u32, u64), u64>,
    looked_at: Option<Instant>,
    commands: HashMap<String, (u32, u64)>,
    apps: apps::Apps,
}

#[derive(Default)]
struct Moved {
    view: bool,
    shown: bool,
}

async fn run(core: Arc<Core>) {
    let mut looker = Looker {
        home: sikemux_process::paths::home(),
        ..Looker::default()
    };
    looker.host_cpu.look();
    let mut next_host = Instant::now() + WATCHED_EVERY;
    let mut next_watched = Instant::now();
    let mut next_shown = Instant::now() + LOAD_SHOWN_EVERY;
    loop {
        let now = Instant::now();
        let watched = core.host_watch.watched();
        let host_due = now >= next_host;
        let watched_due = watched && now >= next_watched;
        if !watched && looker.looked_at.is_some() {
            looker.forget_processes();
            let mut state = core.host_watch.state.locked();
            state.agents = Vec::new();
            state.rest = None;
            state.top = Vec::new();
        }
        if host_due || watched_due {
            let first_look = looker.looked_at.is_none();
            let looking = core.clone();
            let Ok((back, moved)) = tokio::task::spawn_blocking(move || {
                let mut moved = Moved::default();
                if host_due {
                    moved = looker.look_at_host(&looking);
                }
                if watched_due {
                    moved.shown |= looker.look_at_processes(&looking);
                }
                (looker, moved)
            })
            .await
            else {
                return;
            };
            looker = back;
            if host_due {
                next_host = now + HISTORY_EVERY;
                if let Some((free, total)) = core.host_watch.disk_free() {
                    super::worktree::build_output::make_room(&core, free, total);
                }
                if now >= next_shown {
                    next_shown = now + LOAD_SHOWN_EVERY;
                    if core.shows_load() && core.load_moved(&load(&core)) {
                        core.changed(What::View);
                    }
                }
            }
            if watched_due {
                next_watched = now
                    + if first_look {
                        FIRST_LOOKS_APART
                    } else {
                        WATCHED_EVERY
                    };
                measure_disk_parts(&core);
            }
            if moved.view {
                core.changed(What::View);
            }
            if moved.shown || moved.view {
                core.changed(What::Read(Topic::HostStats));
            }
        }
        let wake = if core.host_watch.watched() {
            next_host.min(next_watched)
        } else {
            next_host
        };
        tokio::select! {
            () = tokio::time::sleep_until(wake.into()) => {}
            () = core.host_watch.woken.notified() => {}
        }
    }
}

impl Looker {
    fn look_at_host(&mut self, core: &Core) -> Moved {
        let cpu = self.host_cpu.look();
        let memory = machine::memory().unwrap_or_default();
        let disk = (self.home.as_deref())
            .and_then(machine::disk)
            .unwrap_or_default();
        let offline = core.hosts.out_of_reach(OFFLINE_AFTER_MS);
        let at = epoch_ms();
        let mut state = core.host_watch.state.locked();
        let sample = HostSample {
            at,
            cpu: cpu.unwrap_or(state.now.cpu),
            memory_used: memory.used,
            swap_used: memory.swap_used,
            disk_used: disk.used,
        };
        state.now = sample;
        state.memory_total = memory.total;
        state.disk_total = disk.total;
        state.fine.push_back(sample);
        if state.fine.len() > FINE_KEPT {
            state.fine.pop_front();
        }
        state.looks += 1;
        if state.looks.is_multiple_of(FINE_PER_COARSE) {
            let together = taken_together(state.fine.iter().rev().take(FINE_PER_COARSE));
            state.coarse.push_back(together);
            if state.coarse.len() > COARSE_KEPT {
                state.coarse.pop_front();
            }
        }
        if sample.cpu >= CPU_BUSY {
            state.busy_since.get_or_insert(at);
        } else if sample.cpu < CPU_CALM {
            state.busy_since = None;
        }
        let mut hosts: Vec<&str> = offline.iter().map(|(host, _)| host.as_str()).collect();
        hosts.sort_unstable();
        let mut were: Vec<&str> = (state.offline.iter())
            .map(|(host, _)| host.as_str())
            .collect();
        were.sort_unstable();
        let offline_moved = hosts != were;
        state.offline = offline;
        Moved {
            view: state.judge(at) || offline_moved,
            shown: true,
        }
    }

    fn forget_processes(&mut self) {
        self.used = HashMap::new();
        self.looked_at = None;
        self.commands = HashMap::new();
    }

    /// Reads what every process uses and who it is for; answers whether anything shown moved.
    fn look_at_processes(&mut self, core: &Core) -> bool {
        let cpu = self.watched_cpu.look();
        let memory = machine::memory();
        let usages = procs::usages();
        let at = Instant::now();
        let elapsed = self.looked_at.map(|then| at.duration_since(then));
        self.looked_at = Some(at);
        let mut used = HashMap::with_capacity(usages.len());
        let busy: Vec<u64> = (usages.iter())
            .map(|usage| {
                let key = (usage.pid, usage.started_ms);
                used.insert(key, usage.cpu_ns);
                self.used
                    .get(&key)
                    .map_or(0, |before| usage.cpu_ns.saturating_sub(*before))
            })
            .collect();
        self.used = used;
        let Some(elapsed) = elapsed.filter(|elapsed| !elapsed.is_zero()) else {
            return false;
        };
        let cores = u64::from(about(core).cores.max(1));
        let window = elapsed.as_nanos() as u64;
        let share =
            |nanos: u64| (u128::from(nanos) * 1_000 / u128::from(window * cores)).min(1_000) as u16;
        let roots = super::callers::agent_roots();
        let tree = Tree::new(&usages, &busy, &roots);
        let mut agents: HashMap<&str, AgentUse> = HashMap::new();
        let mut rest = RestUse {
            cpu: 0,
            memory: 0,
            names: Vec::new(),
            count: 0,
        };
        let mut rest_busy = 0u64;
        for (index, usage) in usages.iter().enumerate() {
            match tree.owner[index] {
                Some(agent_id) => {
                    let agent = agents.entry(agent_id).or_insert_with(|| AgentUse {
                        agent_id: agent_id.to_owned(),
                        cpu: 0,
                        memory: 0,
                        command: None,
                        command_since: None,
                        person: None,
                    });
                    agent.memory += usage.memory;
                }
                None => {
                    rest_busy += busy[index];
                    rest.memory += usage.memory;
                    rest.count += 1;
                }
            }
        }
        let mut agent_busy: HashMap<&str, u64> = HashMap::new();
        for (index, owner) in tree.owner.iter().enumerate() {
            if let Some(agent_id) = owner {
                *agent_busy.entry(agent_id).or_default() += busy[index];
            }
        }
        let mut started = Vec::new();
        let mut commands = HashMap::new();
        for (&root, &agent_id) in &tree.roots {
            let Some(agent) = agents.get_mut(agent_id) else {
                continue;
            };
            let Some(command) = tree.command(root, window * COMMAND_BUSY / 1_000) else {
                continue;
            };
            let usage = &usages[command];
            if agent.command.is_some() {
                continue;
            }
            agent.command = Some(command_line(usage));
            agent.command_since = Some(usage.started_ms);
            let key = (usage.pid, usage.started_ms);
            if self.commands.get(agent_id) != Some(&key) {
                started.push((
                    agent_id.to_owned(),
                    agent.command.clone().unwrap_or_default(),
                ));
            }
            commands.insert(agent_id.to_owned(), key);
        }
        self.commands = commands;
        for (agent_id, agent) in &mut agents {
            agent.cpu = share(agent_busy.get(agent_id).copied().unwrap_or(0));
        }
        let mut agents: Vec<AgentUse> = agents.into_values().collect();
        agents.sort_by(|a, b| (b.cpu, b.memory, &a.agent_id).cmp(&(a.cpu, a.memory, &b.agent_id)));
        let mut order: Vec<usize> = (0..usages.len()).collect();
        order.sort_by(|a, b| (busy[*b], usages[*b].memory).cmp(&(busy[*a], usages[*a].memory)));
        rest.cpu = share(rest_busy);
        let mut named = HashSet::new();
        rest.names = (order.iter())
            .filter(|index| tree.owner[**index].is_none())
            .map(|index| usages[*index].name.as_str())
            .filter(|name| named.insert(*name))
            .take(REST_NAMED)
            .map(str::to_owned)
            .collect();
        let (in_apps, met) = self.apps.of(&usages);
        core.host_watch.icons.meet(met);
        let mut grouped: Vec<(ProcessUse, u64, u64)> = Vec::new();
        let mut group_of: HashMap<(&str, Option<&str>), usize> = HashMap::new();
        for (index, usage) in usages.iter().enumerate() {
            let owner = tree.owner[index];
            let app = in_apps[index].as_deref();
            let group = app.map(|app| (app.key.as_str(), owner));
            let at = match group.and_then(|group| group_of.get(&group)) {
                Some(at) => *at,
                None => {
                    grouped.push((
                        ProcessUse {
                            pid: usage.pid,
                            name: app.map_or(&usage.name, |app| &app.name).clone(),
                            cpu: 0,
                            memory: 0,
                            app: app.map(|app| app.key.clone()),
                            agent_id: owner.map(str::to_owned),
                        },
                        0,
                        0,
                    ));
                    group_of.extend(group.map(|group| (group, grouped.len() - 1)));
                    grouped.len() - 1
                }
            };
            let (process, busy_together, heaviest) = &mut grouped[at];
            *busy_together += busy[index];
            process.memory += usage.memory;
            if usage.memory > *heaviest {
                *heaviest = usage.memory;
                process.pid = usage.pid;
            }
        }
        let mut busiest: Vec<usize> = (0..grouped.len()).collect();
        busiest.sort_by(|a, b| {
            (grouped[*b].1, grouped[*b].0.memory).cmp(&(grouped[*a].1, grouped[*a].0.memory))
        });
        let mut heaviest: Vec<usize> = (0..grouped.len()).collect();
        heaviest.sort_by(|a, b| grouped[*b].0.memory.cmp(&grouped[*a].0.memory));
        let mut kept = HashSet::new();
        let top: Vec<ProcessUse> = (busiest.iter().take(TOP_KEPT))
            .chain(heaviest.iter().take(TOP_KEPT))
            .filter(|index| kept.insert(**index))
            .map(|index| {
                let (process, busy_together, _) = &grouped[*index];
                ProcessUse {
                    cpu: share(*busy_together),
                    ..process.clone()
                }
            })
            .collect();
        let shown = shown_of(&agents, &rest, &top);
        let now = epoch_ms();
        let mut state = core.host_watch.state.locked();
        if let Some(cpu) = cpu {
            state.now.cpu = cpu;
        }
        if let Some(memory) = memory {
            state.now.memory_used = memory.used;
            state.now.swap_used = memory.swap_used;
        }
        state.now.at = now;
        for (agent_id, command) in started {
            state.record(now, HostEventKind::CommandStarted { agent_id, command });
        }
        state.agents = agents;
        state.rest = Some(rest);
        state.top = top;
        let shown = {
            let mut hasher = DefaultHasher::new();
            shown.hash(&mut hasher);
            (state.now.cpu / 10, state.now.memory_used >> 27).hash(&mut hasher);
            hasher.finish()
        };
        let moved = state.shown != shown;
        state.shown = shown;
        moved
    }
}

/// What the page shows of the processes, coarsely, so a change too small to see sends nothing.
fn shown_of(agents: &[AgentUse], rest: &RestUse, top: &[ProcessUse]) -> Vec<(String, u16, u64)> {
    let mut shown: Vec<(String, u16, u64)> = (agents.iter())
        .map(|agent| {
            (
                format!(
                    "{}{}",
                    agent.agent_id,
                    agent.command.as_deref().unwrap_or("")
                ),
                agent.cpu / 10,
                agent.memory >> 27,
            )
        })
        .collect();
    shown.push((rest.names.join(","), rest.cpu / 10, rest.memory >> 27));
    shown.extend(top.iter().map(|process| {
        (
            (process.app.clone()).unwrap_or_else(|| process.pid.to_string()),
            process.cpu / 10,
            process.memory >> 27,
        )
    }));
    shown
}

/// The processes below each agent's root, with how busy each subtree was.
struct Tree<'a> {
    owner: Vec<Option<&'a str>>,
    roots: HashMap<usize, &'a str>,
    children: HashMap<usize, Vec<usize>>,
    busy: Vec<u64>,
    names: Vec<&'a str>,
}

impl<'a> Tree<'a> {
    fn new(usages: &'a [Usage], busy: &[u64], roots: &'a HashMap<u32, String>) -> Self {
        let index: HashMap<u32, usize> = (usages.iter().enumerate())
            .map(|(index, usage)| (usage.pid, index))
            .collect();
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for (child, usage) in usages.iter().enumerate() {
            if let Some(parent) = index.get(&usage.parent).filter(|parent| **parent != child) {
                children.entry(*parent).or_default().push(child);
            }
        }
        let mut owner = vec![None; usages.len()];
        let mut subtree = busy.to_vec();
        let mut tree_roots = HashMap::new();
        for (pid, agent_id) in roots {
            let Some(&root) = index.get(pid) else {
                continue;
            };
            tree_roots.insert(root, agent_id.as_str());
            let mut order = Vec::new();
            let mut stack = vec![root];
            while let Some(at) = stack.pop() {
                if owner[at].is_some() {
                    continue;
                }
                owner[at] = Some(agent_id.as_str());
                order.push(at);
                stack.extend(children.get(&at).into_iter().flatten().copied());
            }
            for at in order.into_iter().rev() {
                let below: u64 = (children.get(&at).into_iter().flatten())
                    .filter(|child| owner[**child] == Some(agent_id.as_str()))
                    .map(|child| subtree[*child])
                    .sum();
                subtree[at] = busy[at] + below;
            }
        }
        Self {
            owner,
            roots: tree_roots,
            children,
            busy: subtree,
            names: usages.iter().map(|usage| usage.name.as_str()).collect(),
        }
    }

    fn busiest_child(&self, of: usize) -> Option<usize> {
        (self.children.get(&of).into_iter().flatten())
            .copied()
            .max_by_key(|child| self.busy[*child])
    }

    fn is_shell(&self, at: usize) -> bool {
        let name = self.names[at].trim_start_matches('-');
        SHELLS.contains(&name)
    }

    /// What the agent at `root` runs beyond itself: below the agent's own process, the
    /// busiest of what it started through a shell, or of anything busier than `busy_floor`,
    /// past the shells wrapping it.
    fn command(&self, root: usize, busy_floor: u64) -> Option<usize> {
        let mut agent = root;
        while self.is_shell(agent) {
            agent = self.busiest_child(agent)?;
        }
        let mut command = (self.children.get(&agent).into_iter().flatten())
            .copied()
            .filter(|child| self.is_shell(*child) || self.busy[*child] >= busy_floor)
            .max_by_key(|child| (self.busy[*child], !self.is_shell(*child)))?;
        while self.is_shell(command) {
            match self.busiest_child(command) {
                Some(child) => command = child,
                None => break,
            }
        }
        (!self.is_shell(command)).then_some(command)
    }
}

/// `cargo test --workspace`, from the arguments a process was started with.
fn command_line(usage: &Usage) -> String {
    let Some(arguments) = procs::command_line(usage.pid).filter(|line| !line.is_empty()) else {
        return usage.name.clone();
    };
    let program = Path::new(&arguments[0]).file_name().map_or_else(
        || arguments[0].clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut line = program;
    for argument in &arguments[1..] {
        if line.chars().count() >= MAX_COMMAND_CHARS {
            break;
        }
        line.push(' ');
        line.push_str(argument);
    }
    match line.char_indices().nth(MAX_COMMAND_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

fn taken_together<'a>(samples: impl Iterator<Item = &'a HostSample>) -> HostSample {
    let samples: Vec<&HostSample> = samples.collect();
    let count = samples.len().max(1) as u64;
    HostSample {
        at: samples.first().map_or(0, |sample| sample.at),
        cpu: (samples
            .iter()
            .map(|sample| u64::from(sample.cpu))
            .sum::<u64>()
            / count) as u16,
        memory_used: samples
            .iter()
            .map(|sample| sample.memory_used)
            .max()
            .unwrap_or(0),
        swap_used: samples
            .iter()
            .map(|sample| sample.swap_used)
            .max()
            .unwrap_or(0),
        disk_used: samples.first().map_or(0, |sample| sample.disk_used),
    }
}

fn percent(used: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    let (used, total) = (u128::from(used), u128::from(total));
    ((used * 100 + total / 2) / total).min(100) as u16
}

fn level(percent: u16, limits: &Limits, was: Option<AlertLevel>) -> Option<AlertLevel> {
    if percent >= limits.danger {
        Some(AlertLevel::Danger)
    } else if percent >= limits.warn || (was.is_some() && percent >= limits.calm) {
        Some(AlertLevel::Warn)
    } else {
        None
    }
}

fn worse(one: Option<AlertLevel>, other: Option<AlertLevel>) -> Option<AlertLevel> {
    match (one, other) {
        (Some(AlertLevel::Danger), _) | (_, Some(AlertLevel::Danger)) => Some(AlertLevel::Danger),
        (Some(level), _) | (None, Some(level)) => Some(level),
        (None, None) => None,
    }
}

impl State {
    fn alert(&self, kind: HostAlertKind) -> Option<&HostAlert> {
        self.alerts.iter().find(|alert| alert.kind == kind)
    }

    fn record(&mut self, at: u64, what: HostEventKind) {
        self.events.push_back(HostEvent { at, what });
        if self.events.len() > EVENTS_KEPT {
            self.events.pop_front();
        }
    }

    /// Works out the alerts from the latest sample and the system's memory pressure, recording
    /// those that start and end. Answers whether any moved.
    fn judge(&mut self, at: u64) -> bool {
        let memory_percent = percent(self.now.memory_used, self.memory_total);
        let held = at.saturating_sub(self.pressure_since);
        let was_pressed = self.pressed;
        let pressed = match self.pressure {
            Some(Pressure::Critical) => Some(AlertLevel::Danger),
            Some(Pressure::Warn) if held >= PRESSURE_FOR_MS => Some(AlertLevel::Warn),
            Some(Pressure::Warn) => was_pressed,
            _ if held < PRESSURE_CALM_FOR_MS => was_pressed,
            _ => None,
        };
        let by_use = level(
            memory_percent,
            &MEMORY_LIMITS,
            self.alert(HostAlertKind::Memory).map(|alert| alert.level),
        );
        self.pressed = pressed;
        let memory = worse(pressed, by_use);
        let memory_pressed = pressed.is_some() && worse(pressed, by_use) != by_use;
        let disk_percent = percent(self.now.disk_used, self.disk_total);
        let disk = level(
            disk_percent,
            &DISK_LIMITS,
            self.alert(HostAlertKind::Disk).map(|alert| alert.level),
        );
        let busy_since = self
            .busy_since
            .filter(|since| at.saturating_sub(*since) >= CPU_BUSY_FOR_MS);
        let judged = [
            (HostAlertKind::Memory, memory, memory_percent, None),
            (HostAlertKind::Disk, disk, disk_percent, None),
            (
                HostAlertKind::Cpu,
                busy_since.map(|_| AlertLevel::Warn),
                self.now.cpu / 10,
                busy_since,
            ),
        ];
        let mut alerts = Vec::new();
        for (kind, level, percent, since) in judged {
            let was = self.alert(kind).cloned();
            match (level, was) {
                (Some(level), was) => {
                    if was.is_none() {
                        self.record(
                            at,
                            HostEventKind::AlertStarted {
                                alert: kind,
                                percent,
                            },
                        );
                    }
                    alerts.push(HostAlert {
                        kind,
                        level,
                        since: was.map_or(since.unwrap_or(at), |was| was.since),
                        percent,
                        pressure: kind == HostAlertKind::Memory && memory_pressed,
                    });
                }
                (None, Some(_)) => self.record(at, HostEventKind::AlertEnded { alert: kind }),
                (None, None) => {}
            }
        }
        let moved = alerts != self.alerts;
        self.alerts = alerts;
        moved
    }
}

/// Measures what takes the disk, in the background at the system's lowest priority, at most
/// every so often while the page is watched.
fn measure_disk_parts(core: &Arc<Core>) {
    let watch = &core.host_watch;
    if (watch.state.locked().disk_parts_at).is_some_and(|at| at.elapsed() < DISK_PARTS_EVERY)
        || watch.measuring.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let projects = core.workspaces.project_paths();
    let core = Arc::downgrade(core);
    let spawned = std::thread::Builder::new()
        .name("sikemux-disk-parts".into())
        .spawn(move || {
            lowest_priority();
            let parts = disk_parts(&projects);
            let Some(core) = core.upgrade() else {
                return;
            };
            {
                let mut state = core.host_watch.state.locked();
                let measured: u64 = parts.iter().map(|part| part.bytes).sum();
                let mut parts = parts;
                parts.push(DiskPart {
                    kind: DiskPartKind::Other,
                    bytes: state.now.disk_used.saturating_sub(measured),
                    clearable: false,
                });
                parts.retain(|part| part.bytes > 0);
                parts.sort_by_key(|part| std::cmp::Reverse(part.bytes));
                state.disk_parts = parts;
                state.disk_parts_at = Some(Instant::now());
            }
            core.host_watch.measuring.store(false, Ordering::Release);
            core.changed(What::Read(Topic::HostStats));
        });
    if spawned.is_err() {
        watch.measuring.store(false, Ordering::Release);
    }
}

pub(crate) fn lowest_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: changes only this thread's priority, which throttles its disk reads too.
    unsafe {
        libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG);
    }
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux a thread's nice value is its own; 0 names the calling thread.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// Build output in the projects this host knows and their agents' worktrees, and those
/// worktrees besides their build output.
fn disk_parts(projects: &[PathBuf]) -> Vec<DiskPart> {
    let mut build = 0;
    let mut worktrees = 0;
    let mut seen = HashSet::new();
    for project in projects {
        if !seen.insert(project.clone()) {
            continue;
        }
        build += size_of_folder(&project.join("target"));
        let folder = super::worktree::worktrees_folder(project);
        if !seen.insert(folder.clone()) {
            continue;
        }
        for tree in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let tree = tree.path();
            let tree_build = size_of_folder(&tree.join("target"));
            build += tree_build;
            worktrees += size_of_folder(&tree).saturating_sub(tree_build);
        }
    }
    vec![
        DiskPart {
            kind: DiskPartKind::BuildOutput,
            bytes: build,
            clearable: false,
        },
        DiskPart {
            kind: DiskPartKind::Worktrees,
            bytes: worktrees,
            clearable: false,
        },
    ]
}

/// What the files under `folder` take on the disk, not following links.
pub(crate) fn size_of_folder(folder: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                folders.push(entry.path());
            } else {
                total += metadata.blocks() * 512;
            }
        }
    }
    total
}
