// PTY layer that scales to 100s of concurrent shells without saturating the
// webview's WebGL context budget.
//
// Architecture:
//
//   * Every PTY in Rust owns a `vt100::Parser` — a headless terminal
//     emulator that maintains the current screen grid + scrollback as
//     bytes arrive. ~80-160 KB per PTY total. No rendering, no DOM, no
//     GPU. Always up to date regardless of whether anyone is looking.
//
//   * The PTY can have ZERO, ONE, or MANY subscribers. A subscriber is a
//     Tauri `Channel<Vec<u8>>` registered by the frontend when a
//     TerminalPane mounts an xterm. When the pane unmounts (user
//     switched away) the subscriber is dropped — the PTY keeps running
//     in the background, parser keeps grid up to date, nothing is lost.
//     On re-focus the pane calls `pty_snapshot` to fetch a fresh ANSI
//     dump of the current grid + scrollback, writes it to a freshly
//     spawned xterm, then subscribes for live output.
//
//   * Result: the only live xterm + WebGL contexts in the app are the
//     ones the user is actually looking at. Hidden PTYs cost ~150 KB of
//     Rust heap and zero rendering work.
//
// Commands surfaced to the frontend:
//
//   pty_spawn       — create a new PTY, returns ptyId
//   task_spawn      — run one non-interactive task in a durable PTY
//   pty_attach      — atomic snapshot + subscribe; returns { subId, snapshot }
//   pty_subscribe   — attach a Channel to a PTY, returns subId
//                     (kept for cases where the caller already has the
//                      screen state from a prior attach — e.g. theme reload)
//   pty_unsubscribe — detach a Channel by subId
//   pty_write       — send bytes to the PTY's stdin
//   pty_resize      — change rows/cols (also resizes the parser)
//   pty_kill        — terminate the PTY process

use std::collections::{hash_map::Entry, HashMap, HashSet, VecDeque};
#[cfg(unix)]
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
#[cfg(windows)]
use portable_pty::MasterPty;
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;

use crate::agent_detection::{
    AgentDetection, AgentDetectionState, AgentKind, DetectionConfidence, DetectionExplain,
    DetectionInput, ManifestRegistry, ManifestReloadReport,
};
use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, Metadata, ScalarValue, SpanOutcome};

fn pty_err<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Pty(e.to_string())
}

/// One running pseudo-terminal: the master fd + child + headless parser +
/// the set of frontend Channels currently subscribed to live output.
///
/// I/O model: the master fd is set non-blocking and wrapped in a SINGLE
/// `AsyncFd`. The reader tokio task awaits its `readable()` side; every
/// `pty_write` awaits its `writable()` side under `write_lock` so writes
/// never block a worker thread and never interleave. We therefore hold
/// exactly one fd per PTY (down from three: master + read-dup + write-dup)
/// — see `pty_spawn` for how the lone dup keeps the child's controlling
/// terminal alive after portable_pty's `MasterPty` is dropped.
struct Pty {
    id: u32,
    app: AppHandle,
    /// The PTY master as one non-blocking fd, servicing both directions.
    /// Resize is an ioctl straight on this fd (see `pty_resize`).
    #[cfg(unix)]
    io: AsyncFd<File>,
    /// Serialises concurrent writers so two `pty_write`s can't interleave
    /// bytes on the shared fd. Reads need no guard — only the reader task
    /// reads.
    #[cfg(unix)]
    write_lock: tokio::sync::Mutex<()>,
    /// ConPTY exposes separate blocking pipe handles. They stay behind a
    /// platform boundary so Unix keeps its single-fd async fast path.
    #[cfg(windows)]
    master: Mutex<Box<dyn MasterPty + Send>>,
    #[cfg(windows)]
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    /// Tracks the current screen grid + scrollback. Always up to date,
    /// even when no one's subscribed — that's the whole point.
    parser: Mutex<SemanticParser>,
    /// Live xterm subscribers. Empty = PTY runs invisibly.
    subscribers: Mutex<HashMap<u32, Channel<Vec<u8>>>>,
    /// Millis-since-process-start of the last chunk processed. The idle
    /// sweeper reads this without contending with the reader because it's
    /// an atomic, not a Mutex.
    last_activity_ms: AtomicU64,
    /// Set true once the sweeper has reseeded the parser at the smaller
    /// scrollback so we don't repeatedly rebuild a parser that's already
    /// at idle size. Cleared on any new activity.
    trimmed: AtomicBool,
    /// Present only for agent PTYs. Activity is inferred natively so it
    /// remains observable after the heavyweight xterm renderer detaches.
    activity_key: Option<String>,
    agent_kind: Option<AgentKind>,
    activity_armed: AtomicBool,
    activity_state: AtomicU8,
    /// Explicit frontend teardown must not masquerade as a natural provider
    /// exit after a replacement PTY has already started.
    report_exit: AtomicBool,
    last_published_fingerprint: AtomicU64,
    idle_confirmations: AtomicU8,
    /// Advances whenever submitted input or parsed output changes the
    /// semantic evidence. Combined with screen/title contents below to make
    /// settled detection edge-triggered instead of a perpetual 4 Hz rescan.
    activity_revision: AtomicU64,
    last_detection_fingerprint: AtomicU64,
    /// Present only for a durable task PTY. The atomic gate makes natural
    /// exit, explicit kill, and app drain race to one channel delivery.
    task_exit: Option<TaskExitReporter>,
    harness_output: Mutex<crate::harness::OutputLog>,
    harness_output_pending: Arc<AtomicBool>,
    /// Monotonic task completion timestamp. Zero means the task is still
    /// running; completed task snapshots remain attachable for a fixed grace.
    task_exited_at_ms: AtomicU64,
    /// Keeps any per-process shell startup files alive for exactly as long as
    /// the PTY. `TempDir` removes them automatically; user dotfiles are never
    /// written or replaced.
    _shell_integration: Option<ShellLaunchIntegration>,
    /// Acquired before allocating an OS PTY and released only after the last
    /// native owner drops. This counts launch and reap windows where resources
    /// exist but no entry is currently published in the manager map.
    _capacity_permit: PtyCapacityPermit,
}

#[derive(Debug)]
struct PtyCapacity {
    active: AtomicUsize,
    limit: usize,
}

impl PtyCapacity {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicUsize::new(0),
            limit,
        })
    }

    fn try_acquire(self: &Arc<Self>) -> AppResult<PtyCapacityPermit> {
        let mut active = self.active.load(Ordering::Acquire);
        loop {
            if active >= self.limit {
                return Err(AppError::Pty("PTY capacity reached".into()));
            }
            match self.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(PtyCapacityPermit {
                        capacity: self.clone(),
                    });
                }
                Err(current) => active = current,
            }
        }
    }
}

#[derive(Debug)]
struct PtyCapacityPermit {
    capacity: Arc<PtyCapacity>,
}

impl Drop for PtyCapacityPermit {
    fn drop(&mut self) {
        let previous = self.capacity.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PTY capacity permit underflow");
    }
}

/// All live PTYs, keyed by an id handed back to the frontend.
pub struct PtyManager {
    ptys: DashMap<u32, Arc<Pty>>,
    capacity: Arc<PtyCapacity>,
    detection_registry: RwLock<ManifestRegistry>,
}

impl Default for PtyManager {
    fn default() -> Self {
        Self {
            ptys: DashMap::new(),
            capacity: PtyCapacity::new(MAX_ACTIVE_PTYS),
            detection_registry: RwLock::new(
                ManifestRegistry::bundled().expect("bundled agent manifests must be valid"),
            ),
        }
    }
}

impl PtyManager {
    pub fn counts(&self) -> (usize, usize) {
        let mut subscribers = 0usize;
        for entry in self.ptys.iter() {
            if let Ok(subs) = entry.value().subscribers.lock() {
                subscribers += subs.len();
            }
        }
        (self.ptys.len(), subscribers)
    }

    /// Tear down every live PTY: SIGTERM each child's process group, allow a
    /// brief grace window for well-behaved programs (editors, agents, builds)
    /// to catch the signal and clean up, then SIGKILL whatever's left and reap
    /// it. Called from the window-close hook, the reload hook, AND the
    /// `RunEvent::Exit` hook — so no exit path (quit, `exit()`, or the
    /// updater's `relaunch()` → `app.restart()`) abandons orphan shells/agents
    /// to burn resources or AI tokens until the OS reaps them.
    ///
    /// SIGTERM rather than a bare SIGKILL is deliberate: it's catchable, and —
    /// unlike the kernel's SIGHUP-on-master-close we'd otherwise rely on — it
    /// also terminates `nohup`'d processes (they ignore SIGHUP, not SIGTERM).
    /// The negative pid targets the child's whole process group (it's a
    /// session/group leader via `setsid` in `pty_spawn`), so a foreground job
    /// dies with its shell. SIGKILL is the guaranteed backstop for holdouts.
    pub fn drain(&self) {
        // Phase 1: pull every entry out of the map — releasing the DashMap
        // shards before we sleep — and politely ask each to terminate. The
        // child lock is held only long enough to read the pid.
        let ids: Vec<u32> = self.ptys.iter().map(|e| *e.key()).collect();
        let mut draining: Vec<Arc<Pty>> = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some((_, pty)) = self.ptys.remove(&id) {
                if let Ok(child) = pty.child.lock() {
                    // Retained task snapshots outlive their reaped process.
                    // Their numeric pid may already belong to an unrelated
                    // process group, so never signal after the completion
                    // stamp has been published. The natural-exit waiter sets
                    // that stamp before releasing this same child lock.
                    if should_signal_process_on_drain(
                        pty.task_exit.is_some(),
                        pty.task_exited_at_ms.load(Ordering::Acquire),
                    ) {
                        if let Some(pid) = child.process_id() {
                            terminate_process_tree(pid, false);
                        }
                    }
                }
                draining.push(pty);
            }
        }
        if draining.is_empty() {
            return;
        }
        // Phase 2: a single shared grace window (not per-PTY) keeps quit /
        // relaunch latency bounded no matter how many shells are open.
        std::thread::sleep(DRAIN_GRACE);
        // Phase 3: SIGKILL the holdouts and reap them, so the reload path
        // (where the app keeps running) doesn't accumulate zombies. Dropping
        // each `pty` afterwards closes the retained master fd, which HUPs any
        // job-control children that landed in their own process groups.
        for pty in draining {
            let status = if let Ok(mut child) = pty.child.lock() {
                let is_task = pty.task_exit.is_some();
                let task_exited_at_ms = pty.task_exited_at_ms.load(Ordering::Acquire);
                let task_already_exited =
                    !should_signal_process_on_drain(is_task, task_exited_at_ms);
                let force_task_tree = task_process_needs_force_backstop(is_task, task_exited_at_ms);
                if task_already_exited {
                    // `wait` has already run for a retained task. Poll only to
                    // recover its cached status; never route a stale pid into
                    // the force-kill fallback if that poll itself fails.
                    child.try_wait().ok().flatten()
                } else if force_task_tree {
                    // The task shell may have exited while a descendant that
                    // retained the PTY ignored SIGTERM. Force the still-owned
                    // process group before reaping its leader; otherwise the
                    // numeric group id could be reused and the descendant
                    // would keep the reader and PTY alive indefinitely.
                    let pid = child_process_id(&mut child);
                    if let Some(pid) = pid {
                        terminate_process_tree(pid, true);
                    }
                    if let Ok(Some(status)) = child.try_wait() {
                        Some(status)
                    } else {
                        kill_and_reap_child(&mut child, pid)
                    }
                } else if let Ok(Some(status)) = child.try_wait() {
                    Some(status)
                } else {
                    let pid = child_process_id(&mut child);
                    kill_and_reap_child(&mut child, pid) // SIGKILL + reap the zombie
                }
            } else {
                None
            };
            notify_task_process_exited(&pty, status.as_ref());
        }
    }
}

static NEXT_PTY_ID: AtomicU32 = AtomicU32::new(1);
/// State events from replacement PTYs share one monotonic ordering so a late
/// delivery from the old process can never overwrite the newer process state.
static NEXT_ACTIVITY_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static NEXT_SUB_ID: AtomicU32 = AtomicU32::new(1);
static OUTPUT_READS: AtomicU64 = AtomicU64::new(0);
static OUTPUT_BROADCASTS: AtomicU64 = AtomicU64::new(0);
static OUTPUT_BYTES: AtomicU64 = AtomicU64::new(0);
const MAX_PTY_ID_COLLISION_PROBES: usize = 4_096;
/// Includes launching, live, draining, and retained task PTYs. The separate
/// retained-task cap leaves at least half this budget available for live work.
const MAX_ACTIVE_PTYS: usize = 256;
const MAX_PTY_SUBSCRIBERS_PER_PTY: usize = 16;
const MAX_SUB_ID_COLLISION_PROBES: usize = MAX_PTY_SUBSCRIBERS_PER_PTY + 1;
const MAX_ATTACH_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PTY_DIMENSION: u16 = 1_000;

const MAX_TASK_EXECUTION_ID_BYTES: usize = 8 * 1024;
const MAX_TASK_TERMINAL_KEY_BYTES: usize = 8 * 1024;
const MAX_TASK_ID_BYTES: usize = 128;
const MAX_TASK_LABEL_BYTES: usize = 256;
const MAX_TASK_PROJECT_BYTES: usize = 4 * 1024;
const MAX_TASK_COMMAND_BYTES: usize = 16 * 1024;
const MAX_TASK_CWD_BYTES: usize = 4 * 1024;
const MAX_TASK_ENV_ENTRIES: usize = 128;
const MAX_TASK_ENV_KEY_BYTES: usize = 256;
const MAX_TASK_ENV_VALUE_BYTES: usize = 8 * 1024;
const MAX_TASK_ENV_TOTAL_BYTES: usize = 64 * 1024;
const MAX_TASK_SIGNAL_BYTES: usize = 128;
const TASK_EXIT_RETENTION: Duration = Duration::from_secs(10 * 60);
const MAX_RETAINED_EXITED_TASK_PTYS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TaskSource {
    BuiltIn,
    Project,
    Recent,
}

impl TaskSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Project => "project",
            Self::Recent => "recent",
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSpawnRequest {
    execution_id: String,
    terminal_key: String,
    task_id: String,
    label: String,
    project: String,
    source: TaskSource,
    command: String,
    cwd: String,
    env: HashMap<String, String>,
    cols: u16,
    rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProcessExit {
    code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<String>,
}

impl TaskProcessExit {
    fn from_status(status: Option<&portable_pty::ExitStatus>) -> Self {
        let signal = status
            .and_then(portable_pty::ExitStatus::signal)
            .and_then(|signal| {
                let mut remaining = MAX_TASK_SIGNAL_BYTES;
                let bounded = signal
                    .chars()
                    .filter(|character| !character.is_control())
                    .take_while(|character| {
                        let bytes = character.len_utf8();
                        if bytes > remaining {
                            return false;
                        }
                        remaining -= bytes;
                        true
                    })
                    .collect::<String>();
                (!bounded.is_empty()).then_some(bounded)
            });
        Self {
            code: status.map_or(1, portable_pty::ExitStatus::exit_code),
            signal,
        }
    }
}

struct TaskExitReporter {
    channel: Channel<TaskProcessExit>,
    sent: AtomicBool,
}

impl TaskExitReporter {
    fn new(channel: Channel<TaskProcessExit>) -> Self {
        Self {
            channel,
            sent: AtomicBool::new(false),
        }
    }

    fn send_once(&self, status: Option<&portable_pty::ExitStatus>) -> bool {
        if self.sent.swap(true, Ordering::AcqRel) {
            return false;
        }
        let _ = self.channel.send(TaskProcessExit::from_status(status));
        true
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpawnResult {
    pty_id: u32,
}

struct ValidatedTaskPaths {
    project: PathBuf,
    cwd: PathBuf,
}

fn valid_task_text(value: &str, max_bytes: usize, require_trimmed: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value.trim().is_empty()
        && (!require_trimmed || value.trim() == value)
        && !value.chars().any(char::is_control)
}

fn valid_task_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TASK_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
        })
        && !matches!(value, "__proto__" | "constructor" | "prototype")
}

fn validate_task_environment(
    environment: &HashMap<String, String>,
    windows: bool,
) -> AppResult<()> {
    if environment.len() > MAX_TASK_ENV_ENTRIES {
        return Err(AppError::BadArg("task environment has too many entries"));
    }
    let mut total_bytes = 0usize;
    let mut normalized_keys = HashSet::with_capacity(environment.len());
    for (key, value) in environment {
        if key.is_empty()
            || key.len() > MAX_TASK_ENV_KEY_BYTES
            || key.contains('=')
            || key.chars().any(char::is_control)
            || matches!(key.as_str(), "__proto__" | "constructor" | "prototype")
        {
            return Err(AppError::BadArg("task environment contains an invalid key"));
        }
        if value.len() > MAX_TASK_ENV_VALUE_BYTES || value.contains('\0') {
            return Err(AppError::BadArg(
                "task environment contains an invalid value",
            ));
        }
        let normalized = if windows {
            key.to_lowercase()
        } else {
            key.clone()
        };
        if !normalized_keys.insert(normalized) {
            return Err(AppError::BadArg("task environment contains duplicate keys"));
        }
        total_bytes = total_bytes
            .checked_add(key.len())
            .and_then(|total| total.checked_add(value.len()))
            .ok_or(AppError::BadArg("task environment is too large"))?;
        if total_bytes > MAX_TASK_ENV_TOTAL_BYTES {
            return Err(AppError::BadArg("task environment is too large"));
        }
    }
    Ok(())
}

fn validate_pty_dimensions(cols: u16, rows: u16) -> AppResult<()> {
    if cols == 0 || cols > MAX_PTY_DIMENSION || rows == 0 || rows > MAX_PTY_DIMENSION {
        return Err(AppError::BadArg("invalid pty terminal dimensions"));
    }
    Ok(())
}

fn validate_task_request(request: &TaskSpawnRequest) -> AppResult<ValidatedTaskPaths> {
    if !valid_task_text(&request.execution_id, MAX_TASK_EXECUTION_ID_BYTES, true) {
        return Err(AppError::BadArg("invalid task execution id"));
    }
    if !valid_task_text(&request.terminal_key, MAX_TASK_TERMINAL_KEY_BYTES, true) {
        return Err(AppError::BadArg("invalid task terminal key"));
    }
    if !valid_task_id(&request.task_id) {
        return Err(AppError::BadArg("invalid task id"));
    }
    if !valid_task_text(&request.label, MAX_TASK_LABEL_BYTES, true) {
        return Err(AppError::BadArg("invalid task label"));
    }
    if !valid_task_text(&request.project, MAX_TASK_PROJECT_BYTES, false)
        || !Path::new(&request.project).is_absolute()
    {
        return Err(AppError::BadArg("invalid task project"));
    }
    if request.command.is_empty()
        || request.command.len() > MAX_TASK_COMMAND_BYTES
        || request.command.trim().is_empty()
        || request.command.contains('\0')
    {
        return Err(AppError::BadArg("invalid task command"));
    }
    if !valid_task_text(&request.cwd, MAX_TASK_CWD_BYTES, false)
        || !Path::new(&request.cwd).is_absolute()
    {
        return Err(AppError::BadArg("invalid task working directory"));
    }
    validate_pty_dimensions(request.cols, request.rows)?;
    validate_task_environment(&request.env, cfg!(windows))?;
    let project = std::fs::canonicalize(&request.project)
        .map_err(|_| AppError::BadArg("invalid task project"))?;
    if !project.is_dir() {
        return Err(AppError::BadArg("invalid task project"));
    }
    let cwd = std::fs::canonicalize(&request.cwd)
        .map_err(|_| AppError::BadArg("invalid task working directory"))?;
    if !cwd.is_dir() || !cwd.starts_with(&project) {
        return Err(AppError::BadArg(
            "task working directory must be inside its project",
        ));
    }
    Ok(ValidatedTaskPaths { project, cwd })
}

fn allocate_pty_id(manager: &PtyManager) -> AppResult<u32> {
    for _ in 0..MAX_PTY_ID_COLLISION_PROBES {
        let id = NEXT_PTY_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 && !manager.ptys.contains_key(&id) {
            return Ok(id);
        }
    }
    Err(AppError::Pty("PTY id capacity exhausted".into()))
}

fn task_retention_elapsed(exited_at_ms: u64, now_ms: u64, has_subscribers: bool) -> bool {
    exited_at_ms != 0
        && !has_subscribers
        && now_ms.saturating_sub(exited_at_ms) >= TASK_EXIT_RETENTION.as_millis() as u64
}

fn should_signal_process_on_drain(is_task: bool, task_exited_at_ms: u64) -> bool {
    !is_task || task_exited_at_ms == 0
}

fn task_process_needs_force_backstop(is_task: bool, task_exited_at_ms: u64) -> bool {
    is_task && task_exited_at_ms == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskRetentionCandidate {
    id: u32,
    exited_at_ms: u64,
    has_subscribers: bool,
}

fn task_reclamation_plan(
    mut candidates: Vec<TaskRetentionCandidate>,
    now: u64,
    max_retained: usize,
) -> Vec<u32> {
    candidates.retain(|candidate| candidate.exited_at_ms != 0 && !candidate.has_subscribers);
    candidates.sort_unstable_by_key(|candidate| (candidate.exited_at_ms, candidate.id));
    let excess = candidates.len().saturating_sub(max_retained);
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (index < excess || task_retention_elapsed(candidate.exited_at_ms, now, false))
                .then_some(candidate.id)
        })
        .collect()
}

fn task_retention_candidate(pty: &Pty) -> Option<TaskRetentionCandidate> {
    pty.task_exit.as_ref()?;
    let has_subscribers = match pty.subscribers.lock() {
        Ok(subscribers) => !subscribers.is_empty(),
        Err(_) => return None,
    };
    Some(TaskRetentionCandidate {
        id: pty.id,
        exited_at_ms: pty.task_exited_at_ms.load(Ordering::Acquire),
        has_subscribers,
    })
}

fn reclaim_completed_task_ptys(manager: &PtyManager, now: u64) {
    // Clone identities out of DashMap before taking subscriber locks. The
    // conditional removal below rechecks both the exact Arc and eligibility,
    // so a concurrent attach or wrapped/reused PTY id always wins safely.
    let snapshot: Vec<(u32, Arc<Pty>)> = manager
        .ptys
        .iter()
        .map(|entry| (*entry.key(), entry.value().clone()))
        .collect();
    let mut identities = HashMap::with_capacity(snapshot.len());
    let mut candidates = Vec::new();
    for (id, pty) in snapshot {
        if let Some(candidate) = task_retention_candidate(&pty) {
            identities.insert(id, pty);
            candidates.push(candidate);
        }
    }
    for id in task_reclamation_plan(candidates, now, MAX_RETAINED_EXITED_TASK_PTYS) {
        let Some(candidate) = identities.remove(&id) else {
            continue;
        };
        let _ = manager.ptys.remove_if(&id, |_, current| {
            Arc::ptr_eq(current, &candidate)
                && task_retention_candidate(current)
                    .is_some_and(|state| state.exited_at_ms != 0 && !state.has_subscribers)
        });
    }
}

#[derive(serde::Serialize)]
pub struct PtyDiagnostics {
    pub output_reads: u64,
    pub output_broadcasts: u64,
    pub output_bytes: u64,
    pub working_agents: usize,
    pub blocked_agents: usize,
    pub idle_agents: usize,
    pub unknown_agents: usize,
}

impl PtyManager {
    pub fn diagnostics(&self) -> PtyDiagnostics {
        let count_state = |state| {
            self.ptys
                .iter()
                .filter(|entry| {
                    entry.value().agent_kind.is_some()
                        && entry.value().activity_state.load(Ordering::Relaxed) == state
                })
                .count()
        };
        PtyDiagnostics {
            output_reads: OUTPUT_READS.load(Ordering::Relaxed),
            output_broadcasts: OUTPUT_BROADCASTS.load(Ordering::Relaxed),
            output_bytes: OUTPUT_BYTES.load(Ordering::Relaxed),
            working_agents: count_state(ACTIVITY_WORKING),
            blocked_agents: count_state(ACTIVITY_BLOCKED),
            idle_agents: count_state(ACTIVITY_IDLE),
            unknown_agents: count_state(ACTIVITY_UNKNOWN),
        }
    }
}

#[tauri::command]
pub fn agent_detection_manifests(
    manager: State<'_, PtyManager>,
) -> AppResult<ManifestReloadReport> {
    manager
        .detection_registry
        .read()
        .map(|registry| registry.report())
        .map_err(|_| AppError::Other("agent detection registry lock poisoned".into()))
}

#[tauri::command]
pub fn agent_detection_reload(
    app: AppHandle,
    manager: State<'_, PtyManager>,
) -> AppResult<ManifestReloadReport> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| AppError::Other(format!("agent detection config path: {error}")))?
        .join("agent-detection");
    let mut replacement = ManifestRegistry::with_override_dir(directory)
        .map_err(|error| AppError::Other(format!("agent detection manifests: {error}")))?;
    let report = replacement
        .reload()
        .map_err(|error| AppError::Other(format!("agent detection manifests: {error}")))?;
    *manager
        .detection_registry
        .write()
        .map_err(|_| AppError::Other("agent detection registry lock poisoned".into()))? =
        replacement;
    // The terminal evidence may be unchanged while the matching rules have
    // changed. Invalidate every settled scan so the new registry takes effect
    // on the next sweeper tick without requiring fresh PTY output.
    for entry in manager.ptys.iter() {
        entry
            .value()
            .last_detection_fingerprint
            .store(0, Ordering::Release);
    }
    Ok(report)
}

#[tauri::command]
pub fn agent_detection_explain(
    manager: State<'_, PtyManager>,
    agent_id: String,
) -> AppResult<DetectionExplain> {
    let pty = manager
        .ptys
        .iter()
        .find(|entry| entry.value().activity_key.as_deref() == Some(agent_id.as_str()))
        .map(|entry| entry.value().clone())
        .ok_or(AppError::BadArg("agent has no live terminal"))?;
    let kind = pty
        .agent_kind
        .ok_or(AppError::BadArg("terminal has no known agent type"))?;
    let (recent, title) = pty
        .parser
        .lock()
        .map(|parser| {
            (
                parser.screen().contents(),
                parser.callbacks().window_title.clone(),
            )
        })
        .map_err(|_| AppError::Other("agent terminal parser lock poisoned".into()))?;
    manager
        .detection_registry
        .read()
        .map(|registry| {
            let input = if title.is_empty() {
                DetectionInput::screen(&recent)
            } else {
                DetectionInput {
                    recent_screen: &recent,
                    osc_title: &title,
                    osc_progress: "",
                }
            };
            registry.explain(kind, input)
        })
        .map_err(|_| AppError::Other("agent detection registry lock poisoned".into()))
}

// Scrollback held in the headless vt100 parser. Must match (or exceed)
// the frontend's xterm scrollback (`TerminalPane.tsx`'s SCROLLBACK) so a
// reattach can repaint the full visible history. At 80 cols of mostly-
// cleared cells, 10k rows ≈ 1.6 MB per PTY. 100 PTYs → ~160 MB worst-case.
// The sweeper below reclaims this back to IDLE_SCROLLBACK for any PTY
// that's been silent for IDLE_TRIM and has no subscribers attached.
pub const PARSER_SCROLLBACK: usize = 10_000;
const IDLE_SCROLLBACK: usize = 2_000;
const IDLE_TRIM: Duration = Duration::from_secs(10 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ACTIVITY_SETTLE: Duration = Duration::from_secs(2);
const ACTIVITY_UNKNOWN: u8 = 0;
const ACTIVITY_IDLE: u8 = 1;
const ACTIVITY_WORKING: u8 = 2;
const ACTIVITY_BLOCKED: u8 = 3;
const ACTIVITY_STOPPED: u8 = 4;
#[cfg(unix)]
const OUTPUT_COALESCE: Duration = Duration::from_millis(2);
#[cfg(unix)]
const OUTPUT_BATCH_BYTES: usize = 64 * 1024;

/// Stable frontend event for opt-in local shell metadata. The terminal byte
/// stream remains untouched; this is a second, typed signal derived from it.
pub const PTY_SHELL_METADATA_EVENT: &str = "pty_shell_metadata";
const MAX_SHELL_OSC_BYTES: usize = 8 * 1024;
const MAX_SHELL_PATH_BYTES: usize = 4 * 1024;
const MAX_SHELL_EXIT_CODE_BYTES: usize = 11;
const SHELL_EVENT_MIN_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellPhase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Running,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellMetadataSnapshot {
    pub revision: u64,
    pub cwd: Option<String>,
    pub phase: ShellPhase,
    pub last_exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ShellBoundary {
    Cwd,
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShellProtocolUpdate {
    boundary: ShellBoundary,
    metadata: ShellMetadataSnapshot,
}

#[derive(Default)]
struct ShellProtocolBatch {
    latest: Option<ShellProtocolUpdate>,
    coalesced: usize,
    dropped: usize,
}

impl ShellProtocolBatch {
    fn push(&mut self, update: ShellProtocolUpdate) {
        if self.latest.replace(update).is_some() {
            self.coalesced = self.coalesced.saturating_add(1);
        }
    }
}

#[derive(Default)]
struct ShellEventCoalescer {
    last_emitted_ms: Option<u64>,
    pending: Option<ShellProtocolUpdate>,
}

struct ShellEventDecision {
    ready: Option<ShellProtocolUpdate>,
    replaced_pending: bool,
}

impl ShellEventCoalescer {
    fn submit(&mut self, now_ms: u64, update: ShellProtocolUpdate) -> ShellEventDecision {
        let replaced_pending = self.pending.replace(update).is_some();
        ShellEventDecision {
            ready: self.take_due(now_ms),
            replaced_pending,
        }
    }

    fn take_due(&mut self, now_ms: u64) -> Option<ShellProtocolUpdate> {
        let due = self.last_emitted_ms.is_none_or(|last| {
            now_ms.saturating_sub(last) >= SHELL_EVENT_MIN_INTERVAL.as_millis() as u64
        });
        if !due {
            return None;
        }
        let ready = self.pending.take()?;
        self.last_emitted_ms = Some(now_ms);
        Some(ready)
    }
}

#[derive(Default)]
struct ShellProtocolOutput {
    ready: Option<ShellProtocolUpdate>,
    coalesced: usize,
    dropped: usize,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyShellMetadataEvent {
    pty_id: u32,
    revision: u64,
    boundary: ShellBoundary,
    cwd: Option<String>,
    phase: ShellPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

impl PtyShellMetadataEvent {
    fn from_update(pty_id: u32, update: ShellProtocolUpdate) -> Self {
        let exit_code = (update.boundary == ShellBoundary::CommandFinished)
            .then_some(update.metadata.last_exit_code)
            .flatten();
        Self {
            pty_id,
            revision: update.metadata.revision,
            boundary: update.boundary,
            cwd: update.metadata.cwd,
            phase: update.metadata.phase,
            exit_code,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ShellScanState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

enum ShellSignal {
    Cwd(String),
    Boundary(ShellBoundary, ShellPhase, Option<i32>),
}

struct ShellProtocolParser {
    scan_state: ShellScanState,
    osc: Vec<u8>,
    osc_overflowed: bool,
    metadata: ShellMetadataSnapshot,
    events: ShellEventCoalescer,
}

impl Default for ShellProtocolParser {
    fn default() -> Self {
        Self {
            scan_state: ShellScanState::Ground,
            osc: Vec::with_capacity(256),
            osc_overflowed: false,
            metadata: ShellMetadataSnapshot {
                revision: 0,
                cwd: None,
                phase: ShellPhase::Unknown,
                last_exit_code: None,
            },
            events: ShellEventCoalescer::default(),
        }
    }
}

impl ShellProtocolParser {
    fn snapshot(&self) -> ShellMetadataSnapshot {
        self.metadata.clone()
    }

    fn process(&mut self, bytes: &[u8]) -> ShellProtocolBatch {
        let mut batch = ShellProtocolBatch::default();
        for &byte in bytes {
            match self.scan_state {
                ShellScanState::Ground => {
                    if byte == b'\x1b' {
                        self.scan_state = ShellScanState::Escape;
                    }
                }
                ShellScanState::Escape => {
                    if byte == b']' {
                        self.osc.clear();
                        self.osc_overflowed = false;
                        self.scan_state = ShellScanState::Osc;
                    } else if byte != b'\x1b' {
                        self.scan_state = ShellScanState::Ground;
                    }
                }
                ShellScanState::Osc => match byte {
                    b'\x07' => self.finish_osc(&mut batch),
                    b'\x1b' => self.scan_state = ShellScanState::OscEscape,
                    _ => self.push_osc_byte(byte),
                },
                ShellScanState::OscEscape => match byte {
                    b'\\' | b'\x07' => self.finish_osc(&mut batch),
                    b'\x1b' => {
                        self.push_osc_byte(b'\x1b');
                    }
                    _ => {
                        self.push_osc_byte(b'\x1b');
                        self.push_osc_byte(byte);
                        self.scan_state = ShellScanState::Osc;
                    }
                },
            }
        }
        batch
    }

    fn process_for_events(&mut self, bytes: &[u8], now_ms: u64) -> ShellProtocolOutput {
        let mut batch = self.process(bytes);
        let Some(latest) = batch.latest.take() else {
            return ShellProtocolOutput {
                ready: self.events.take_due(now_ms),
                coalesced: batch.coalesced,
                dropped: batch.dropped,
            };
        };
        let decision = self.events.submit(now_ms, latest);
        ShellProtocolOutput {
            ready: decision.ready,
            coalesced: batch
                .coalesced
                .saturating_add(usize::from(decision.replaced_pending)),
            dropped: batch.dropped,
        }
    }

    fn take_due_event(&mut self, now_ms: u64) -> Option<ShellProtocolUpdate> {
        self.events.take_due(now_ms)
    }

    fn push_osc_byte(&mut self, byte: u8) {
        if self.osc.len() < MAX_SHELL_OSC_BYTES {
            self.osc.push(byte);
        } else {
            self.osc_overflowed = true;
        }
    }

    fn finish_osc(&mut self, batch: &mut ShellProtocolBatch) {
        if self.osc_overflowed {
            batch.dropped = batch.dropped.saturating_add(1);
        } else if let Some(signal) = parse_shell_signal(&self.osc) {
            self.apply_signal(signal, batch);
        }
        self.osc.clear();
        self.osc_overflowed = false;
        self.scan_state = ShellScanState::Ground;
    }

    fn apply_signal(&mut self, signal: ShellSignal, batch: &mut ShellProtocolBatch) {
        let boundary = match signal {
            ShellSignal::Cwd(cwd) => {
                if self.metadata.cwd.as_deref() == Some(cwd.as_str()) {
                    return;
                }
                self.metadata.cwd = Some(cwd);
                ShellBoundary::Cwd
            }
            ShellSignal::Boundary(boundary, phase, exit_code) => {
                self.metadata.phase = phase;
                if boundary == ShellBoundary::CommandFinished {
                    self.metadata.last_exit_code = exit_code;
                }
                boundary
            }
        };
        self.metadata.revision = self.metadata.revision.saturating_add(1);
        let update = ShellProtocolUpdate {
            boundary,
            metadata: self.metadata.clone(),
        };
        batch.push(update);
    }
}

fn parse_shell_signal(payload: &[u8]) -> Option<ShellSignal> {
    if let Some(uri) = payload.strip_prefix(b"7;") {
        return parse_shell_cwd(uri).map(ShellSignal::Cwd);
    }
    let payload = payload.strip_prefix(b"133;")?;
    let mut fields = payload.split(|byte| *byte == b';');
    let marker = fields.next()?;
    match marker {
        b"A" => Some(ShellSignal::Boundary(
            ShellBoundary::PromptStart,
            ShellPhase::Prompt,
            None,
        )),
        b"B" => Some(ShellSignal::Boundary(
            ShellBoundary::CommandStart,
            ShellPhase::Input,
            None,
        )),
        b"C" => Some(ShellSignal::Boundary(
            ShellBoundary::CommandExecuted,
            ShellPhase::Running,
            None,
        )),
        b"D" => {
            let exit_code = fields.next().and_then(parse_shell_exit_code);
            Some(ShellSignal::Boundary(
                ShellBoundary::CommandFinished,
                ShellPhase::Finished,
                exit_code,
            ))
        }
        _ => None,
    }
}

fn parse_shell_exit_code(value: &[u8]) -> Option<i32> {
    if value.is_empty() || value.len() > MAX_SHELL_EXIT_CODE_BYTES {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_shell_cwd(uri: &[u8]) -> Option<String> {
    if uri.is_empty()
        || uri.len() > MAX_SHELL_OSC_BYTES
        || uri.iter().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    let uri = std::str::from_utf8(uri).ok()?;
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if url
        .host_str()
        .is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
    {
        return None;
    }
    let path = url.to_file_path().ok()?;
    if !path.is_absolute() {
        return None;
    }
    let raw = path.to_string_lossy();
    if raw.is_empty() || raw.len() > MAX_SHELL_PATH_BYTES || raw.chars().any(char::is_control) {
        return None;
    }
    Some(raw.into_owned())
}

#[derive(Default)]
struct SemanticCallbacks {
    window_title: String,
    shell: Option<ShellProtocolParser>,
}

impl vt100::Callbacks for SemanticCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        // Titles are untrusted child-process output. Keep only printable text
        // and impose a small scalar limit before retaining it for detection.
        self.window_title = String::from_utf8_lossy(title)
            .chars()
            .filter(|ch| !ch.is_control())
            .take(512)
            .collect();
    }
}

type SemanticParser = vt100::Parser<SemanticCallbacks>;
type SubscriberSnapshot = Vec<(u32, Channel<Vec<u8>>)>;

#[cfg(test)]
fn semantic_parser(rows: u16, cols: u16, scrollback: usize) -> SemanticParser {
    semantic_parser_with_shell(rows, cols, scrollback, false)
}

fn semantic_parser_with_shell(
    rows: u16,
    cols: u16,
    scrollback: usize,
    enabled: bool,
) -> SemanticParser {
    SemanticParser::new_with_callbacks(
        rows,
        cols,
        scrollback,
        SemanticCallbacks {
            window_title: String::new(),
            shell: enabled.then(ShellProtocolParser::default),
        },
    )
}

fn semantic_fingerprint(revision: u64, screen: &str, title: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    revision.hash(&mut hasher);
    screen.hash(&mut hasher);
    title.hash(&mut hasher);
    // Zero is the initial "never evaluated" sentinel.
    hasher.finish().max(1)
}

fn event_fingerprint(
    state: u8,
    label: &str,
    source: &str,
    confidence: &str,
    reason: &str,
    matched_rule: Option<&str>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    state.hash(&mut hasher);
    label.hash(&mut hasher);
    source.hash(&mut hasher);
    confidence.hash(&mut hasher);
    reason.hash(&mut hasher);
    matched_rule.hash(&mut hasher);
    hasher.finish().max(1)
}

fn detection_reason(detection: &AgentDetection) -> String {
    if let Some(fallback) = detection.fallback_reason.as_deref() {
        return format!("agent detection fallback: {fallback}");
    }
    let Some(rule) = detection.matched_rule.as_deref() else {
        return format!(
            "agent screen evaluated with manifest {}",
            detection.manifest_version
        );
    };
    let evidence = &detection.evidence;
    let visible = if evidence.visible_blocker {
        "visible blocker"
    } else if evidence.visible_working {
        "visible working status"
    } else if evidence.visible_idle {
        "visible idle prompt"
    } else {
        "screen evidence"
    };
    match evidence.region.as_deref() {
        Some(region) => format!("manifest rule {rule} matched {visible} in {region}"),
        None => format!("manifest rule {rule} matched {visible}"),
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentStateEvent<'a> {
    agent_id: &'a str,
    state: &'static str,
    sequence: u64,
    source: &'static str,
    confidence: &'static str,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_rule: Option<String>,
}

fn publish_agent_state(
    pty: &Pty,
    next: u8,
    label: &'static str,
    source: &'static str,
    confidence: &'static str,
    reason: impl Into<String>,
    matched_rule: Option<String>,
) {
    if !pty.report_exit.load(Ordering::Acquire) {
        return;
    }
    let Some(agent_id) = pty.activity_key.as_deref() else {
        return;
    };
    let reason = reason.into();
    let fingerprint = event_fingerprint(
        next,
        label,
        source,
        confidence,
        &reason,
        matched_rule.as_deref(),
    );
    if pty
        .last_published_fingerprint
        .swap(fingerprint, Ordering::AcqRel)
        == fingerprint
    {
        return;
    }
    pty.activity_state.store(next, Ordering::Release);
    let sequence = NEXT_ACTIVITY_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    let _ = pty.app.emit(
        "agent_state_changed",
        AgentStateEvent {
            agent_id,
            state: label,
            sequence,
            source,
            confidence,
            reason,
            matched_rule,
        },
    );
}

fn arm_agent_activity(pty: &Pty) {
    if pty.activity_key.is_none() {
        return;
    }
    pty.activity_armed.store(true, Ordering::Release);
    pty.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    pty.idle_confirmations.store(0, Ordering::Release);
    pty.activity_revision.fetch_add(1, Ordering::AcqRel);
    publish_agent_state(
        pty,
        ACTIVITY_WORKING,
        "working",
        "activity",
        "high",
        "command submitted",
        None,
    );
}

fn submits_line(data: &str) -> bool {
    data.contains('\r') || data.contains('\n')
}

fn note_agent_output(pty: &Pty) {
    // Startup banners, model discovery, and the first TUI paint are output,
    // but they are not work. A fresh agent stays Ready until Sikemux observes
    // an actual submitted line. Once armed, output is meaningful activity.
    if pty.agent_kind.is_some() && pty.activity_armed.load(Ordering::Acquire) {
        pty.idle_confirmations.store(0, Ordering::Release);
        publish_agent_state(
            pty,
            ACTIVITY_WORKING,
            "working",
            "activity",
            "medium",
            "agent produced output",
            None,
        );
    }
}

// Grace window between the SIGTERM and the SIGKILL backstop in `drain`.
// Long enough for a foreground program (editor, agent, build) to catch
// SIGTERM and flush; short enough that app quit / update-relaunch doesn't
// feel laggy. One shared window, not per-PTY — see `drain`.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

fn child_process_id(child: &mut Box<dyn Child + Send + Sync>) -> Option<u32> {
    child.process_id()
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int) {
    if pid == 0 {
        return;
    }
    // Negative pid targets the process group. portable_pty/forkpty makes the
    // shell the session/group leader on Unix; if that assumption ever fails,
    // Child::kill below still targets the direct child as a fallback.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(unix)]
fn terminate_process_tree(pid: u32, force: bool) {
    signal_process_group(pid, if force { libc::SIGKILL } else { libc::SIGTERM });
}

#[cfg(windows)]
fn terminate_process_tree(pid: u32, force: bool) {
    // ConPTY's portable child handle terminates only the direct shell. Use
    // taskkill's tree mode so foreground commands and agent subprocesses do
    // not survive an app close. Child::kill remains the fallback below.
    let mut command = std::process::Command::new("taskkill");
    let pid = pid.to_string();
    command.args(["/PID", &pid, "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.status();
}

fn kill_and_reap_child(
    child: &mut Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
) -> Option<portable_pty::ExitStatus> {
    let _ = child.kill();
    if let Some(pid) = pid {
        terminate_process_tree(pid, true);
    }
    child.wait().ok()
}

fn terminate_and_reap_child(
    child: &mut Box<dyn Child + Send + Sync>,
    force_process_tree_after_grace: bool,
) -> Option<portable_pty::ExitStatus> {
    // For a task, do not reap an exited shell until after its process group
    // receives the force backstop: a descendant may still retain the PTY and
    // ignore SIGTERM. Non-task callers preserve the historical fast path.
    if !force_process_tree_after_grace {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
    }
    let pid = child_process_id(child);
    if let Some(pid) = pid {
        terminate_process_tree(pid, false);
    }
    std::thread::sleep(DRAIN_GRACE);
    if force_process_tree_after_grace {
        if let Some(pid) = pid {
            terminate_process_tree(pid, true);
        }
    }
    if let Ok(Some(status)) = child.try_wait() {
        return Some(status);
    }
    kill_and_reap_child(child, pid)
}

/// Owns a freshly-spawned child until the fully-initialized `Pty` takes it.
/// Child handles do not kill on drop, so every fallible setup step after spawn
/// must be guarded explicitly.
struct SpawnedChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl SpawnedChildGuard {
    fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self(Some(child))
    }

    fn into_inner(mut self) -> Box<dyn Child + Send + Sync> {
        self.0.take().expect("spawned child guard already empty")
    }
}

impl Drop for SpawnedChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let pid = child_process_id(&mut child);
            let _ = kill_and_reap_child(&mut child, pid);
        }
    }
}

// Process-start anchor so all `last_activity_ms` values are monotonic
// deltas in ms — immune to wall-clock jumps (NTP, sleep/resume).
fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Execute startup commands before the interactive shell is launched. Sending
/// text to readline makes it visible in the terminal (and multi-line commands
/// are especially fragile), so startup must be a shell argument, never PTY
/// input. Once it returns, replace the bootstrap shell with the normal local
/// shell so users always land at a usable prompt.
#[cfg(unix)]
fn startup_bootstrap(startup: &str, login_shell: bool) -> String {
    let login = if login_shell { " -l" } else { "" };
    format!("{startup}\nexec \"$SIKEMUX_SHELL\"{login}")
}

/// Shells launched without an injected rc file need `-l` to read their profile,
/// the way a normal terminal emulator starts them. Shells that do get an
/// injected rc file are already reading the user's chain through it.
#[cfg(unix)]
fn shell_wants_login_flag(shell: &str) -> bool {
    matches!(detect_shell_kind(shell), Some(ShellKind::Zsh))
}

/// Drive a non-blocking write to completion against a tokio `AsyncFd`.
/// Loops on EAGAIN via the readiness machinery; returns once every byte
/// has been written or the kernel reports an I/O error.
#[cfg(unix)]
async fn write_all_async(writer: &AsyncFd<File>, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let mut guard = writer.writable().await?;
        let res = guard.try_io(|inner| {
            let mut f = inner.get_ref();
            f.write(data)
        });
        match res {
            Ok(Ok(0)) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "pty write returned 0",
                ));
            }
            Ok(Ok(n)) => {
                data = &data[n..];
            }
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue, // readiness cleared; loop
        }
    }
    Ok(())
}

/// One-shot sweeper kickoff. Spawns a single background task on the first
/// `pty_spawn` of the process; subsequent calls are no-ops.
fn ensure_sweeper(app: AppHandle) {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let activity_app = app.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ACTIVITY_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(mgr) = activity_app.try_state::<PtyManager>() else {
                return;
            };
            let now = now_ms();
            for entry in mgr.ptys.iter() {
                let pty = entry.value();
                // A quiet prompt may leave one coalesced update after its last
                // output chunk. Flush it from the existing bounded sweeper so
                // rate limiting never means "latest state is never emitted".
                let shell_update = pty.parser.lock().ok().and_then(|mut parser| {
                    parser
                        .callbacks_mut()
                        .shell
                        .as_mut()
                        .and_then(|shell| shell.take_due_event(now))
                });
                if let Some(update) = shell_update {
                    publish_shell_metadata(pty, update);
                }
                if !pty.activity_armed.load(Ordering::Acquire)
                    || now.saturating_sub(pty.last_activity_ms.load(Ordering::Relaxed))
                        < ACTIVITY_SETTLE.as_millis() as u64
                {
                    continue;
                }
                let Some(agent) = pty.agent_kind else {
                    continue;
                };
                let (recent, title) = match pty.parser.lock() {
                    Ok(parser) => (
                        parser.screen().contents(),
                        parser.callbacks().window_title.clone(),
                    ),
                    Err(_) => continue,
                };
                let fingerprint = semantic_fingerprint(
                    pty.activity_revision.load(Ordering::Acquire),
                    &recent,
                    &title,
                );
                if pty.last_detection_fingerprint.load(Ordering::Acquire) == fingerprint {
                    continue;
                }
                let detection = match mgr.detection_registry.read() {
                    Ok(registry) => {
                        let input = if title.is_empty() {
                            DetectionInput::screen(&recent)
                        } else {
                            DetectionInput {
                                recent_screen: &recent,
                                osc_title: &title,
                                osc_progress: "",
                            }
                        };
                        registry.detect(agent, input)
                    }
                    Err(_) => continue,
                };
                if detection.skip_state_update {
                    pty.last_detection_fingerprint
                        .store(fingerprint, Ordering::Release);
                    continue;
                }
                let (next, label) = match detection.state {
                    AgentDetectionState::Unknown => (ACTIVITY_UNKNOWN, "unknown"),
                    AgentDetectionState::Idle => (ACTIVITY_IDLE, "idle"),
                    AgentDetectionState::Working => (ACTIVITY_WORKING, "working"),
                    AgentDetectionState::Blocked => (ACTIVITY_BLOCKED, "blocked"),
                };
                if next == ACTIVITY_IDLE {
                    let confirmations = pty.idle_confirmations.fetch_add(1, Ordering::AcqRel) + 1;
                    if confirmations < 2 {
                        continue;
                    }
                } else {
                    pty.idle_confirmations.store(0, Ordering::Release);
                }
                let source = if detection.fallback_reason.is_some() {
                    "fallback"
                } else {
                    "screen"
                };
                let confidence = match detection.confidence {
                    DetectionConfidence::Authoritative | DetectionConfidence::Strong => "high",
                    DetectionConfidence::Fallback => "low",
                };
                let reason = detection_reason(&detection);
                publish_agent_state(
                    pty,
                    next,
                    label,
                    source,
                    confidence,
                    reason,
                    detection.matched_rule,
                );
                pty.last_detection_fingerprint
                    .store(fingerprint, Ordering::Release);
            }
        }
    });
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // skip the immediate first tick
        loop {
            ticker.tick().await;
            let Some(mgr) = app.try_state::<PtyManager>() else {
                return;
            };
            let now = now_ms();
            reclaim_completed_task_ptys(&mgr, now);
            // Snapshot identities after reclamation so parser compaction never
            // holds a DashMap shard across parser/subscriber locks.
            let candidates: Vec<Arc<Pty>> =
                mgr.ptys.iter().map(|entry| entry.value().clone()).collect();
            for pty in candidates {
                if pty.trimmed.load(Ordering::Relaxed) {
                    continue;
                }
                let last = pty.last_activity_ms.load(Ordering::Relaxed);
                if now.saturating_sub(last) < IDLE_TRIM.as_millis() as u64 {
                    continue;
                }
                let has_subs = match pty.subscribers.lock() {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true, // be conservative on poison
                };
                if has_subs {
                    // Don't shrink under an attached xterm — a reattach
                    // would lose scrollback the user might be scrolling
                    // through right now.
                    continue;
                }
                // Re-seed the parser at the smaller scrollback. Alternate
                // screen applications must not be compacted: rebuilding an
                // alternate buffer can destroy the saved normal buffer that
                // 1049l is expected to restore.
                if let Ok(mut parser) = pty.parser.lock() {
                    // Output records activity before taking this lock. Re-read
                    // it here so bytes that raced the optimistic check above
                    // cannot be immediately compacted back to 2,000 rows.
                    let last = pty.last_activity_ms.load(Ordering::Relaxed);
                    if now.saturating_sub(last) < IDLE_TRIM.as_millis() as u64 {
                        continue;
                    }
                    // `pty_attach` takes parser -> subscribers in this same
                    // order. Re-check under the parser lock so an attach that
                    // raced the optimistic check above cannot receive a
                    // snapshot and then have its backing parser compacted.
                    let has_subs = match pty.subscribers.lock() {
                        Ok(s) => !s.is_empty(),
                        Err(_) => true,
                    };
                    if has_subs {
                        continue;
                    }
                    if compact_parser_for_idle(&mut parser) {
                        // Publish the capacity change while still holding the
                        // parser lock. The reader clears this flag and grows
                        // the parser under the same lock before processing its
                        // first new bytes, so no output can land in between.
                        pty.trimmed.store(true, Ordering::Release);
                    }
                }
            }
        }
    });
}

/// Update the headless terminal and fan one output chunk to live subscribers.
/// Both Unix's readiness task and Windows' ConPTY reader thread share this
/// path, preserving the snapshot/subscription ordering invariant.
fn publish_shell_metadata(pty: &Pty, update: ShellProtocolUpdate) {
    if pty
        .app
        .emit(
            PTY_SHELL_METADATA_EVENT,
            PtyShellMetadataEvent::from_update(pty.id, update),
        )
        .is_err()
    {
        let _ = global_observability().increment_counter("pty.shell_protocol.emit_errors", 1);
    }
}

fn broadcast_output(pty: &Pty, bytes: &[u8]) {
    if pty.task_exit.is_some() {
        if let Ok(mut log) = pty.harness_output.lock() {
            log.push(bytes);
        }
        if !pty.harness_output_pending.swap(true, Ordering::AcqRel) {
            let pending = pty.harness_output_pending.clone();
            let app = pty.app.clone();
            let id = pty.id;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                pending.store(false, Ordering::Release);
                let _ = app.emit("harness-task-output", id);
            });
        }
    }
    let observer = global_observability();
    let mut metadata = Metadata::new();
    metadata.insert("bytes".to_owned(), ScalarValue::from(bytes.len()));
    let operation =
        observer.slow_operation("pty.broadcast", Duration::from_millis(8), None, metadata);
    OUTPUT_BROADCASTS.fetch_add(1, Ordering::Relaxed);
    OUTPUT_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let output_now_ms = now_ms();
    pty.last_activity_ms.store(output_now_ms, Ordering::Relaxed);
    note_agent_output(pty);
    let (snapshot, shell_output): (SubscriberSnapshot, ShellProtocolOutput) = {
        let parser_started = Instant::now();
        let Ok(mut parser) = pty.parser.lock() else {
            let _ = observer.increment_counter("pty.parser.lock_errors", 1);
            operation.finish(SpanOutcome::Error);
            return;
        };
        if pty.trimmed.swap(false, Ordering::AcqRel) {
            reseed_parser(&mut parser, PARSER_SCROLLBACK);
        }
        // This bounded side parser observes OSC 7/133 without filtering or
        // rewriting output. The vt100 model and every attached xterm still
        // receive the original byte slice verbatim.
        let shell_batch = parser
            .callbacks_mut()
            .shell
            .as_mut()
            .map(|shell| shell.process_for_events(bytes, output_now_ms))
            .unwrap_or_default();
        parser.process(bytes);
        observer.observe_latency("pty.parser", parser_started.elapsed());
        pty.activity_revision.fetch_add(1, Ordering::AcqRel);
        let subscribers = match pty.subscribers.lock() {
            Ok(subs) => subs.iter().map(|(id, ch)| (*id, ch.clone())).collect(),
            Err(_) => Vec::new(),
        };
        (subscribers, shell_batch)
    };
    if shell_output.coalesced > 0 {
        let _ = observer.increment_counter(
            "pty.shell_protocol.coalesced",
            shell_output.coalesced as u64,
        );
    }
    if shell_output.dropped > 0 {
        let _ =
            observer.increment_counter("pty.shell_protocol.dropped", shell_output.dropped as u64);
    }
    if let Some(update) = shell_output.ready {
        publish_shell_metadata(pty, update);
    }
    if snapshot.is_empty() {
        operation.finish(SpanOutcome::Success);
        return;
    }
    observer.set_gauge("pty.last_subscriber_fanout", snapshot.len() as f64);
    let send_started = Instant::now();
    let dead: Vec<u32> = if snapshot.len() == 1 {
        let (sub_id, channel) = &snapshot[0];
        channel
            .send(bytes.to_vec())
            .err()
            .map(|_| vec![*sub_id])
            .unwrap_or_default()
    } else {
        let chunk = bytes.to_vec();
        snapshot
            .iter()
            .filter_map(|(sub_id, channel)| channel.send(chunk.clone()).err().map(|_| *sub_id))
            .collect()
    };
    observer.observe_latency("pty.channel_send", send_started.elapsed());
    if !dead.is_empty() {
        let _ = observer.increment_counter("pty.channel_send_errors", dead.len() as u64);
    }
    if let Ok(mut subscribers) = pty.subscribers.lock() {
        for sub_id in dead {
            subscribers.remove(&sub_id);
        }
    }
    operation.finish(SpanOutcome::Success);
}

fn notify_process_exited(pty: &Pty, status: Option<&portable_pty::ExitStatus>) {
    notify_task_process_exited(pty, status);
    if let Ok(subscribers) = pty.subscribers.lock() {
        for channel in subscribers.values() {
            let _ = channel.send(Vec::new());
        }
    }
    if pty.agent_kind.is_some() && pty.report_exit.load(Ordering::Acquire) {
        let reason = status.map_or_else(
            || "agent process stopped; exit status unavailable".to_string(),
            |status| {
                if status.success() {
                    "agent process stopped".to_string()
                } else {
                    status.signal().map_or_else(
                        || format!("agent process stopped with code {}", status.exit_code()),
                        |signal| format!("agent process stopped from signal {signal}"),
                    )
                }
            },
        );
        publish_agent_state(
            pty,
            ACTIVITY_STOPPED,
            "stopped",
            "process",
            "high",
            reason,
            None,
        );
    }
}

fn stamp_task_process_exited(pty: &Pty) -> Option<u64> {
    pty.task_exit.as_ref()?;
    let exited_at = now_ms().max(1);
    match pty
        .task_exited_at_ms
        .compare_exchange(0, exited_at, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => {
            pty.last_activity_ms.store(exited_at, Ordering::Release);
            Some(exited_at)
        }
        Err(existing) => Some(existing),
    }
}

fn notify_task_process_exited(pty: &Pty, status: Option<&portable_pty::ExitStatus>) {
    let Some(reporter) = pty.task_exit.as_ref() else {
        return;
    };
    let Some(exited_at) = stamp_task_process_exited(pty) else {
        return;
    };
    let first_delivery = reporter.send_once(status);
    // Enforce the count bound immediately rather than waiting for the periodic
    // sweeper: a storm of zero-duration tasks must not retain one parser per
    // completion for an entire sweep interval.
    if first_delivery {
        if let Some(manager) = pty.app.try_state::<PtyManager>() {
            reclaim_completed_task_ptys(&manager, exited_at);
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyContext {
    session_id: String,
    session_name: String,
    session_kind: String,
    project: Option<String>,
    window_id: Option<String>,
    pane_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    #[serde(default)]
    initial_prompt_submitted: bool,
    /// Explicit opt-in. Absent/false preserves the exact historical shell
    /// launch path and performs no startup-file or argv injection.
    #[serde(default)]
    shell_integration: bool,
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PtyAgentProfile {
    config_path: Option<String>,
    #[serde(default)]
    environment_keys: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyDirectCommand {
    program: String,
    args: Vec<String>,
    profile: Option<PtyAgentProfile>,
}

fn validate_direct_command(
    command: &PtyDirectCommand,
    context: Option<&PtyContext>,
) -> AppResult<()> {
    if context
        .and_then(|value| value.agent_id.as_deref())
        .is_none()
        || context
            .and_then(|value| value.agent_type.as_deref())
            .is_none()
    {
        return Err(AppError::BadArg(
            "direct PTY commands require an explicit agent context",
        ));
    }
    if command.program.is_empty()
        || command.program.len() > 4_096
        || command.program.contains('\0')
        || command.args.len() > 128
    {
        return Err(AppError::BadArg("invalid direct PTY command"));
    }
    let mut total = command.program.len();
    for argument in &command.args {
        if argument.len() > 8_192 || argument.contains('\0') {
            return Err(AppError::BadArg("invalid direct PTY command argument"));
        }
        total = total.saturating_add(argument.len());
    }
    if let Some(profile) = command.profile.as_ref() {
        if profile
            .config_path
            .as_deref()
            .is_some_and(|path| path.len() > 4_096 || path.contains('\0'))
            || profile.environment_keys.len() > 64
            || profile.environment_keys.iter().any(|key| {
                key.len() > 128
                    || !key.chars().enumerate().all(|(index, character)| {
                        character == '_'
                            || character.is_ascii_alphanumeric()
                                && (index > 0 || !character.is_ascii_digit())
                    })
            })
        {
            return Err(AppError::BadArg("invalid direct PTY agent profile"));
        }
    }
    if total > 64 * 1_024 {
        return Err(AppError::BadArg("direct PTY command is too large"));
    }
    Ok(())
}

fn configure_interactive_command(command: &mut CommandBuilder, launch: &PtyDirectCommand) {
    let invocation = std::iter::once(&launch.program)
        .chain(launch.args.iter())
        .map(|value| {
            #[cfg(unix)]
            {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
            #[cfg(windows)]
            {
                format!("'{}'", value.replace('\'', "''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(unix)]
    command.args(["-l", "-i", "-c", &invocation]);
    #[cfg(windows)]
    command.args(["-NoLogo", "-NoExit", "-Command", &format!("& {invocation}")]);
}

fn agent_profile_config_root(path: &str) -> PathBuf {
    let trimmed = path.trim();
    let expanded = if trimmed == "~" {
        crate::system::user_home()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        crate::system::user_home().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if matches!(
        expanded.file_name().and_then(|value| value.to_str()),
        Some("config.toml" | "settings.json" | "settings.local.json")
    ) {
        expanded.parent().map(Path::to_path_buf).unwrap_or(expanded)
    } else {
        expanded
    }
}

fn apply_agent_profile(
    command: &mut CommandBuilder,
    context: Option<&PtyContext>,
    profile: Option<&PtyAgentProfile>,
) {
    let Some(root) = profile
        .and_then(|profile| profile.config_path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(agent_profile_config_root)
    else {
        return;
    };
    match context.and_then(|context| context.agent_type.as_deref()) {
        Some("codex") => command.env("CODEX_HOME", root),
        Some("claude") => command.env("CLAUDE_CONFIG_DIR", root),
        _ => {}
    }
}

pub(crate) const OPTIONAL_PTY_ENV: &[&str] = &[
    "SIKEMUX_SHELL",
    "SIKEMUX_SESSION_ID",
    "SIKEMUX_SESSION_NAME",
    "SIKEMUX_SESSION_KIND",
    "SIKEMUX_PROJECT",
    "SIKEMUX_WINDOW_ID",
    "SIKEMUX_PANE_ID",
    "SIKEMUX_AGENT_ID",
    "SIKEMUX_AGENT_TYPE",
    "SIKEMUX_BIN_PATH",
    "SIKEMUX_CLI_ENDPOINT",
    "SIKEMUX_SHELL_INTEGRATION",
    "SIKEMUX_ORIGINAL_ZDOTDIR",
    "SIKEMUX_ORIGINAL_ZDOTDIR_SET",
    "SIKEMUX_TEMP_ZDOTDIR",
    "SIKEMUX_ORIGINAL_XDG_CONFIG_HOME",
    "SIKEMUX_ORIGINAL_FISH_CONFIG",
    "SIKEMUX_TASK_EXECUTION_ID",
    "SIKEMUX_TASK_TERMINAL_KEY",
    "SIKEMUX_TASK_ID",
    "SIKEMUX_TASK_SOURCE",
    "SIKEMUX_BROWSER_STATE_DIR",
    "SIKEMUX_BROWSER_CDP_URL",
    "SIKEMUX_BROWSER_BROKER_URL",
    "SIKEMUX_BROWSER_BROKER_TOKEN",
    "SIKEMUX_BROWSER_MCP_COMMAND",
    "SIKEMUX_BROWSER_MCP_ARGS",
    "SIKEMUX_BROWSER_AGENT_ID",
];

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

fn editor_command(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let shell_safe = raw.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'\\' | b':' | b'.' | b'_' | b'-')
    });
    if shell_safe {
        return raw.into_owned();
    }
    #[cfg(windows)]
    {
        format!("\"{}\"", raw.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", raw.replace('\'', "'\\''"))
    }
}

/// Apply a clean, typed Sikemux identity to a PTY command. Optional fields are
/// removed before being rebuilt so a terminal can never inherit the identity
/// of the app's parent terminal (or a Codex thread that launched the app).
fn configure_pty_environment(
    cmd: &mut CommandBuilder,
    context: Option<&PtyContext>,
    version: &str,
    cli_executable: Option<&Path>,
    cli_endpoint: Option<&Path>,
    profile_env: &HashMap<String, String>,
) {
    for (key, value) in profile_env {
        if cmd.get_env(key).is_none() {
            cmd.env(key, value);
        }
    }

    for key in OPTIONAL_PTY_ENV {
        cmd.env_remove(key);
    }

    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Sikemux");
    cmd.env("TERM_PROGRAM_VERSION", version);
    cmd.env("SIKEMUX", "1");
    cmd.env("SIKEMUX_VERSION", version);

    if let Some(context) = context {
        cmd.env("SIKEMUX_SESSION_ID", &context.session_id);
        cmd.env("SIKEMUX_SESSION_NAME", &context.session_name);
        cmd.env("SIKEMUX_SESSION_KIND", &context.session_kind);
        if let Some(project) = non_empty(&context.project) {
            cmd.env("SIKEMUX_PROJECT", project);
        }
        if let Some(window_id) = non_empty(&context.window_id) {
            cmd.env("SIKEMUX_WINDOW_ID", window_id);
        }
        if let Some(pane_id) = non_empty(&context.pane_id) {
            cmd.env("SIKEMUX_PANE_ID", pane_id);
        }
        if let Some(agent_id) = non_empty(&context.agent_id) {
            cmd.env("SIKEMUX_AGENT_ID", agent_id);
        }
        if let Some(agent_type) = non_empty(&context.agent_type) {
            cmd.env("SIKEMUX_AGENT_TYPE", agent_type);
        }
    }

    if let Some(path) = cli_executable {
        cmd.env("SIKEMUX_BIN_PATH", path);
        if cmd.get_env("EDITOR").is_none() && cmd.get_env("VISUAL").is_none() {
            let editor = editor_command(path);
            cmd.env("EDITOR", &editor);
            cmd.env("VISUAL", &editor);
        }
    }
    if let Some(path) = cli_endpoint {
        cmd.env("SIKEMUX_CLI_ENDPOINT", path);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
}

/// Lifetime guard for startup files. Fish and PowerShell use argv hooks and
/// therefore have no directory, but still return a guard to mark parsing active.
struct ShellLaunchIntegration {
    _files: Option<tempfile::TempDir>,
}

const BASH_INTEGRATION: &str = r#"# Sikemux ephemeral shell integration; generated per PTY.
[[ -r "$HOME/.bashrc" ]] && source "$HOME/.bashrc"

__sikemux_emit_cwd() {
  local path="${PWD//[[:cntrl:]]/}"
  path="${path//%/%25}"
  path="${path//\\/%5C}"
  path="${path// /%20}"
  path="${path//#/%23}"
  path="${path//\?/%3F}"
  builtin printf '\e]7;file://localhost%s\a' "$path"
}
__sikemux_prompt() {
  local command_status=$?
  builtin printf '\e]133;D;%d\a' "$command_status"
  __sikemux_emit_cwd
  builtin printf '\e]133;A\a'
  return "$command_status"
}
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
  "declare -a"*) PROMPT_COMMAND=(__sikemux_prompt "${PROMPT_COMMAND[@]}") ;;
  *) PROMPT_COMMAND="__sikemux_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
  PS0="${PS0-}"$'\e]133;B\a\e]133;C\a'
else
  PS1="${PS1-}"'\[\e]133;B\a\]'
fi
"#;

const FISH_INTEGRATION: &str = r#"# Sikemux post-config init command; no config paths are replaced.
function __sikemux_emit_cwd
    set -l path (string replace -ar '[[:cntrl:]]' '' -- "$PWD")
    set path (string replace -a '%' '%25' -- "$path")
    set path (string replace -a '\\' '%5C' -- "$path")
    set path (string replace -a ' ' '%20' -- "$path")
    set path (string replace -a '#' '%23' -- "$path")
    set path (string replace -a '?' '%3F' -- "$path")
    printf '\e]7;file://localhost%s\a' "$path"
end
function __sikemux_prompt --on-event fish_prompt
    set -l command_status $status
    __sikemux_emit_cwd
    printf '\e]133;A\a'
    return $command_status
end
function __sikemux_preexec --on-event fish_preexec
    printf '\e]133;B\a\e]133;C\a'
end
function __sikemux_postexec --on-event fish_postexec
    set -l command_status $status
    printf '\e]133;D;%d\a' $command_status
    return $command_status
end
"#;

const POWERSHELL_INTEGRATION: &str = r#"$global:__sikemux_original_prompt = $function:global:prompt
if ($null -eq $global:__sikemux_original_prompt) {
    $global:__sikemux_original_prompt = { "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> " }
}
function global:prompt {
    $sikemux_ok = $?
    $sikemux_text = & $global:__sikemux_original_prompt
    $sikemux_code = if ($sikemux_ok) { 0 } else { 1 }
    [Console]::Write("$([char]27)]133;D;$sikemux_code$([char]7)")
    try {
        $sikemux_path = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.Path
        $sikemux_encoded = [Uri]::EscapeDataString([string]$sikemux_path).Replace('%2F', '/').Replace('%5C', '/')
        if (-not $sikemux_encoded.StartsWith('/')) { $sikemux_encoded = '/' + $sikemux_encoded }
        [Console]::Write("$([char]27)]7;file://localhost$sikemux_encoded$([char]7)")
    } catch {}
    [Console]::Write("$([char]27)]133;A$([char]7)")
    return "$sikemux_text$([char]27)]133;B$([char]7)"
}"#;

fn detect_shell_kind(shell: &str) -> Option<ShellKind> {
    let name = shell.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "zsh" => Some(ShellKind::Zsh),
        "bash" => Some(ShellKind::Bash),
        "fish" => Some(ShellKind::Fish),
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskShellPlatform {
    Unix,
    Windows,
}

const CURRENT_TASK_SHELL_PLATFORM: TaskShellPlatform = if cfg!(windows) {
    TaskShellPlatform::Windows
} else {
    TaskShellPlatform::Unix
};

fn task_shell_arguments(
    shell: &str,
    command: &str,
    platform: TaskShellPlatform,
) -> AppResult<Vec<String>> {
    if matches!(detect_shell_kind(shell), Some(ShellKind::PowerShell)) {
        return Ok(vec![
            "-NoLogo".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            command.into(),
        ]);
    }
    if platform == TaskShellPlatform::Unix
        || matches!(
            detect_shell_kind(shell),
            Some(ShellKind::Zsh | ShellKind::Bash | ShellKind::Fish)
        )
    {
        return Ok(vec!["-c".into(), command.into()]);
    }
    let executable = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(executable.as_str(), "cmd" | "cmd.exe") {
        return Ok(vec!["/D".into(), "/S".into(), "/C".into(), command.into()]);
    }
    Err(AppError::BadArg(
        "configured shell does not support task execution",
    ))
}

fn configure_task_command(command: &mut CommandBuilder, shell: &str, task: &str) -> AppResult<()> {
    command.args(task_shell_arguments(
        shell,
        task,
        CURRENT_TASK_SHELL_PLATFORM,
    )?);
    Ok(())
}

fn shell_integration_requested(
    context: Option<&PtyContext>,
    has_startup: bool,
    inherited_ssh: bool,
) -> bool {
    let Some(context) = context else {
        return false;
    };
    context.shell_integration
        && !has_startup
        && !inherited_ssh
        && context.agent_id.is_none()
        && context.agent_type.is_none()
        && matches!(context.session_kind.as_str(), "project" | "command")
}

fn inherited_ssh_environment() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

fn temporary_shell_file(
    relative: &Path,
    contents: &str,
) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let directory = temporary_shell_directory()?;
    let path = write_temporary_shell_file(&directory, relative, contents)?;
    Ok((directory, path))
}

fn temporary_shell_directory() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new().prefix("sikemux-shell-").tempdir()
}

fn write_temporary_shell_file(
    directory: &tempfile::TempDir,
    relative: &Path,
    contents: &str,
) -> std::io::Result<PathBuf> {
    let path = directory.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    Ok(path)
}

fn configure_shell_integration(
    cmd: &mut CommandBuilder,
    shell: &str,
) -> std::io::Result<Option<ShellLaunchIntegration>> {
    let Some(kind) = detect_shell_kind(shell) else {
        return Ok(None);
    };
    match kind {
        // zsh gets no integration: it is driven by ZDOTDIR, and pointing that
        // at a scratch directory silently redirects everything a user's config
        // derives from it — `HISTFILE` above all, so a session's history was
        // written into a temp dir and deleted on exit. zsh is launched exactly
        // as Terminal, Ghostty and Alacritty launch it, and the cwd/exit
        // metadata is given up rather than paid for with the user's shell.
        ShellKind::Zsh => Ok(None),
        ShellKind::Bash => {
            let (directory, path) = temporary_shell_file(Path::new("bashrc"), BASH_INTEGRATION)?;
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            cmd.arg("--rcfile");
            cmd.arg(path);
            Ok(Some(ShellLaunchIntegration {
                _files: Some(directory),
            }))
        }
        ShellKind::Fish => {
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            // Fish's native init command runs after its normal configuration
            // chain, preserving user/vendor conf.d scripts and autoload paths.
            cmd.args(["--init-command", FISH_INTEGRATION]);
            Ok(Some(ShellLaunchIntegration { _files: None }))
        }
        ShellKind::PowerShell => {
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            cmd.args(["-NoExit", "-Command", POWERSHELL_INTEGRATION]);
            Ok(Some(ShellLaunchIntegration { _files: None }))
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_spawn(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    browser: State<'_, crate::browser::BrowserManager>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    startup: Option<String>,
    direct_command: Option<PtyDirectCommand>,
    context: Option<PtyContext>,
) -> AppResult<u32> {
    validate_pty_dimensions(cols, rows)?;
    let startup = startup.filter(|value| !value.is_empty());
    if startup.is_some() && direct_command.is_some() {
        return Err(AppError::BadArg(
            "PTY startup and direct command are mutually exclusive",
        ));
    }
    if let Some(command) = direct_command.as_ref() {
        validate_direct_command(command, context.as_ref())?;
    }
    let browser_environment = if let Some(agent_id) = context
        .as_ref()
        .and_then(|context| context.agent_id.as_deref())
    {
        browser.environment(&app, agent_id).await.ok()
    } else {
        None
    };
    let shell = crate::system::configured_shell();
    let direct_profile = direct_command
        .as_ref()
        .and_then(|command| command.profile.clone());
    #[cfg(unix)]
    let has_direct_command = direct_command.is_some();
    let mut cmd = CommandBuilder::new(&shell);
    if let Some(command) = direct_command.as_ref() {
        configure_interactive_command(&mut cmd, command);
    }
    let cli_executable = crate::cli_server::cli_executable_path();
    let cli_endpoint = crate::cli_server::cli_endpoint_path();
    configure_pty_environment(
        &mut cmd,
        context.as_ref(),
        &app.package_info().version.to_string(),
        cli_executable.as_deref(),
        cli_endpoint.as_deref(),
        &HashMap::new(),
    );
    apply_agent_profile(&mut cmd, context.as_ref(), direct_profile.as_ref());
    if let Some(environment) = browser_environment {
        for (key, value) in environment {
            cmd.env(key, value);
        }
    }
    #[cfg(windows)]
    if direct_command.is_none() {
        cmd.arg("-NoLogo");
    }
    let shell_integration = if shell_integration_requested(
        context.as_ref(),
        startup.is_some(),
        inherited_ssh_environment(),
    ) {
        match configure_shell_integration(&mut cmd, &shell) {
            Ok(integration) => integration,
            Err(_) => {
                // Shell integration is an enhancement, never a reason to lose
                // the user's terminal. Record only a count; setup errors can
                // contain temporary paths and must not enter trace metadata.
                let _ = global_observability()
                    .increment_counter("pty.shell_integration.setup_errors", 1);
                None
            }
        }
    } else {
        None
    };
    let cwd = cwd.unwrap_or_else(|| crate::system::user_home().to_string_lossy().into_owned());
    cmd.cwd(cwd);
    // Terminal, Ghostty and Alacritty all start a login shell, which is how
    // /etc/zprofile and ~/.zprofile get read — on macOS that is where
    // path_helper builds PATH. Without it the shell came up missing both the
    // user's profile and half their PATH.
    #[cfg(unix)]
    let login_shell = shell_wants_login_flag(&shell);
    #[cfg(unix)]
    if login_shell && startup.is_none() && !has_direct_command {
        cmd.arg("-l");
    }
    if let Some(startup) = startup.as_deref() {
        #[cfg(unix)]
        {
            cmd.env("SIKEMUX_SHELL", &shell);
            cmd.arg("-c");
            cmd.arg(startup_bootstrap(startup, login_shell));
        }
        #[cfg(windows)]
        {
            // -NoExit runs the requested startup action and then leaves the
            // user at a normal interactive PowerShell prompt.
            cmd.args(["-NoExit", "-Command", startup]);
        }
    }

    spawn_prepared_pty(
        app,
        &manager,
        PreparedPtyLaunch {
            cols,
            rows,
            command: cmd,
            context,
            shell_integration,
            task_exit: None,
        },
    )
    .await
}

#[tauri::command]
pub async fn task_spawn(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    request: TaskSpawnRequest,
    on_exit: Channel<TaskProcessExit>,
) -> AppResult<TaskSpawnResult> {
    let paths = validate_task_request(&request)?;
    let TaskSpawnRequest {
        execution_id,
        terminal_key,
        task_id,
        label,
        project: _,
        source,
        command,
        cwd: _,
        env,
        cols,
        rows,
    } = request;

    let shell = crate::system::configured_shell();
    let mut task_command = CommandBuilder::new(&shell);
    let context = PtyContext {
        session_id: execution_id.clone(),
        session_name: label,
        session_kind: "task".into(),
        project: Some(paths.project.to_string_lossy().into_owned()),
        window_id: None,
        pane_id: None,
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: false,
    };
    let cli_executable = crate::cli_server::cli_executable_path();
    let cli_endpoint = crate::cli_server::cli_endpoint_path();
    configure_pty_environment(
        &mut task_command,
        Some(&context),
        &app.package_info().version.to_string(),
        cli_executable.as_deref(),
        cli_endpoint.as_deref(),
        crate::system::login_shell_environment(),
    );
    task_command.env("SIKEMUX_TASK_EXECUTION_ID", execution_id);
    task_command.env("SIKEMUX_TASK_TERMINAL_KEY", terminal_key);
    task_command.env("SIKEMUX_TASK_ID", task_id);
    task_command.env("SIKEMUX_TASK_SOURCE", source.as_str());
    for (key, value) in env {
        task_command.env(key, value);
    }
    task_command.cwd(paths.cwd);
    configure_task_command(&mut task_command, &shell, &command)?;

    let operation = global_observability().slow_operation(
        "pty.task_spawn",
        Duration::from_millis(50),
        None,
        Metadata::new(),
    );
    let result = spawn_prepared_pty(
        app,
        &manager,
        PreparedPtyLaunch {
            cols,
            rows,
            command: task_command,
            context: None,
            shell_integration: None,
            task_exit: Some(TaskExitReporter::new(on_exit)),
        },
    )
    .await;
    operation.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result.map(|pty_id| TaskSpawnResult { pty_id })
}

struct PreparedPtyLaunch {
    cols: u16,
    rows: u16,
    command: CommandBuilder,
    context: Option<PtyContext>,
    shell_integration: Option<ShellLaunchIntegration>,
    task_exit: Option<TaskExitReporter>,
}

async fn spawn_prepared_pty(
    app: AppHandle,
    manager: &PtyManager,
    launch: PreparedPtyLaunch,
) -> AppResult<u32> {
    validate_pty_dimensions(launch.cols, launch.rows)?;
    // Reclaim eligible completed tasks before applying the hard process/parser
    // budget, then reserve capacity before any OS handle or child is created.
    reclaim_completed_task_ptys(manager, now_ms());
    let capacity_permit = manager.capacity.try_acquire()?;
    ensure_sweeper(app.clone());
    // Has to run inside Tauri's tokio runtime — both `AsyncFd::new` and
    // `tokio::spawn` below panic when there is no reactor.
    let pair = NativePtySystem::default()
        .openpty(pty_size(launch.cols, launch.rows))
        .map_err(pty_err)?;
    let PreparedPtyLaunch {
        cols,
        rows,
        command,
        context,
        shell_integration,
        task_exit,
    } = launch;
    let shell_metadata_enabled = shell_integration.is_some();

    let child = pair.slave.spawn_command(command).map_err(pty_err)?;
    let child = SpawnedChildGuard::new(child);
    drop(pair.slave);

    #[cfg(unix)]
    // Get the master fd and set the underlying open-file-description to
    // O_NONBLOCK. This is shared across every dup of the master (a
    // property of the file description, not the fd), which is exactly
    // what we need: the reader and writer dups below inherit it, and the
    // master fd itself only ever sees ioctl (resize) — unaffected by
    // O_NONBLOCK.
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or_else(|| pty_err("master pty has no fd"))?;
    #[cfg(unix)]
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL);
        if flags < 0 {
            return Err(pty_err(std::io::Error::last_os_error()));
        }
        // O_NONBLOCK lives on the open-file-description, so the dup below
        // inherits it — one fd, both directions, never blocking.
        if libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(pty_err(std::io::Error::last_os_error()));
        }
    }

    // ONE fd per PTY (was three). dup() the master once, then drop
    // portable_pty's `MasterPty`: that closes the fd it owned, but our dup
    // refers to the SAME kernel open-file-description, so the master end
    // stays open and the child keeps its controlling terminal (no SIGHUP).
    // The single `AsyncFd` services both reads and writes; resize is an
    // ioctl on this fd. At ~50+ live shells this is the difference between
    // ~150 fds and ~50 — the headroom that keeps a heavy session off the
    // process fd limit.
    #[cfg(unix)]
    let dup_fd = unsafe { libc::dup(master_fd) };
    #[cfg(unix)]
    if dup_fd < 0 {
        return Err(pty_err(std::io::Error::last_os_error()));
    }
    #[cfg(unix)]
    drop(pair.master);
    #[cfg(unix)]
    let io_file = unsafe { File::from_raw_fd(dup_fd) };
    #[cfg(windows)]
    let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
    #[cfg(windows)]
    let writer = pair.master.take_writer().map_err(pty_err)?;
    let id = allocate_pty_id(manager)?;

    let parsed_agent_kind = context
        .as_ref()
        .and_then(|context| context.agent_type.as_deref())
        .and_then(AgentKind::from_label);
    let initial_prompt_submitted = context
        .as_ref()
        .is_some_and(|context| context.initial_prompt_submitted);
    let activity_key = context
        .and_then(|context| context.agent_id)
        .filter(|key| !key.is_empty());
    let pty = Arc::new(Pty {
        id,
        app: app.clone(),
        #[cfg(unix)]
        io: AsyncFd::new(io_file).map_err(pty_err)?,
        #[cfg(unix)]
        write_lock: tokio::sync::Mutex::new(()),
        #[cfg(windows)]
        master: Mutex::new(pair.master),
        #[cfg(windows)]
        writer: Mutex::new(writer),
        child: Mutex::new(child.into_inner()),
        parser: Mutex::new(semantic_parser_with_shell(
            rows,
            cols,
            PARSER_SCROLLBACK,
            shell_metadata_enabled,
        )),
        subscribers: Mutex::new(HashMap::new()),
        last_activity_ms: AtomicU64::new(now_ms()),
        trimmed: AtomicBool::new(false),
        activity_key,
        agent_kind: parsed_agent_kind,
        activity_armed: AtomicBool::new(initial_prompt_submitted),
        activity_state: AtomicU8::new(ACTIVITY_UNKNOWN),
        report_exit: AtomicBool::new(true),
        last_published_fingerprint: AtomicU64::new(0),
        idle_confirmations: AtomicU8::new(0),
        activity_revision: AtomicU64::new(0),
        last_detection_fingerprint: AtomicU64::new(0),
        task_exit,
        task_exited_at_ms: AtomicU64::new(0),
        harness_output: Mutex::new(crate::harness::OutputLog::default()),
        harness_output_pending: Arc::new(AtomicBool::new(false)),
        _shell_integration: shell_integration,
        _capacity_permit: capacity_permit,
    });

    // Publish before starting the reader. A short-lived command can reach EOF
    // immediately; starting first lets its self-prune remove nothing and then
    // leaves a dead PTY inserted forever.
    manager.ptys.insert(id, pty.clone());
    if pty.agent_kind.is_some() && pty.activity_key.is_some() {
        if initial_prompt_submitted {
            publish_agent_state(
                &pty,
                ACTIVITY_WORKING,
                "working",
                "activity",
                "high",
                "initial prompt submitted",
                None,
            );
        } else {
            publish_agent_state(
                &pty,
                ACTIVITY_IDLE,
                "idle",
                "process",
                "high",
                "agent ready; no prompt submitted",
                None,
            );
        }
    }

    // Reader — feeds parser then fans bytes out to subscribers.
    //
    // Runs as a plain tokio task (no dedicated OS thread). `AsyncFd`
    // parks the task until the kernel signals readability via kqueue
    // (macOS) / epoll (linux), at which point `try_io` does a single
    // non-blocking read. WouldBlock just loops back to `readable().await`
    // after clearing the readiness flag, so we never busy-spin.
    //
    // Atomicity invariant (vs `pty_attach`): a freshly-attached subscriber
    // must see EXACTLY the bytes NOT present in the snapshot it got back.
    // We achieve that by, under the parser lock:
    //   1. processing the chunk into the parser
    //   2. cloning the subscriber list (Tauri Channels are Arc-internal
    //      and cheap to clone)
    // Both locks are then dropped BEFORE the channel sends — so one slow
    // subscriber can't stall the parser or block another PTY's reattach.
    //
    // Dead subscribers (channel closed because the JS xterm unmounted
    // without explicit unsub) are GC'd on send error so the map stays
    // bounded.
    #[cfg(unix)]
    let pty_reader = pty.clone();
    #[cfg(unix)]
    let app_reader = app.clone();
    #[cfg(unix)]
    tokio::spawn(async move {
        let mut buf = [0u8; OUTPUT_BATCH_BYTES];
        'reader: loop {
            let mut batch = Vec::with_capacity(OUTPUT_BATCH_BYTES);
            let mut eof = false;
            // The first byte arrives without an artificial delay. Once output
            // starts, collect the tiny writes produced by line-buffered tools
            // for at most 2 ms before one parser pass and one Tauri delivery.
            loop {
                let mut guard = match pty_reader.io.readable().await {
                    Ok(g) => g,
                    Err(_) => break 'reader,
                };
                match guard.try_io(|inner| {
                    let mut f = inner.get_ref();
                    f.read(&mut buf)
                }) {
                    Ok(Ok(0)) => {
                        eof = true;
                        break;
                    }
                    Ok(Ok(n)) => {
                        OUTPUT_READS.fetch_add(1, Ordering::Relaxed);
                        batch.extend_from_slice(&buf[..n]);
                        break;
                    }
                    Ok(Err(_)) => {
                        eof = true;
                        break;
                    }
                    Err(_would_block) => continue,
                }
            }
            if !eof && batch.len() < OUTPUT_BATCH_BYTES {
                let deadline = tokio::time::sleep(OUTPUT_COALESCE);
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        _ = &mut deadline => break,
                        ready = pty_reader.io.readable() => {
                            let mut guard = match ready {
                                Ok(g) => g,
                                Err(_) => { eof = true; break; }
                            };
                            match guard.try_io(|inner| {
                                let mut f = inner.get_ref();
                                f.read(&mut buf)
                            }) {
                                Ok(Ok(0)) => { eof = true; break; }
                                Ok(Ok(n)) => {
                                    OUTPUT_READS.fetch_add(1, Ordering::Relaxed);
                                    batch.extend_from_slice(&buf[..n]);
                                    if batch.len() >= OUTPUT_BATCH_BYTES { break; }
                                }
                                Ok(Err(_)) => { eof = true; break; }
                                Err(_would_block) => continue,
                            }
                        }
                    }
                }
            }
            if !batch.is_empty() {
                broadcast_output(&pty_reader, &batch);
            }
            if eof {
                break;
            }
        }
        // Interactive shells self-prune. Completed task PTYs intentionally
        // stay addressable: a zero-duration command can reach EOF before the
        // invoke response crosses into JS, and the frontend must still be able
        // to attach to its bounded parser snapshot by the returned exact ID.
        if pty_reader.task_exit.is_none() {
            if let Some(mgr) = app_reader.try_state::<PtyManager>() {
                mgr.ptys.remove(&id);
            }
        }
        let reap_pty = pty_reader.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let status = if let Ok(mut child) = reap_pty.child.lock() {
                let status = child.wait().ok();
                // Publish while the child lock still proves this pid cannot be
                // concurrently treated as live by app drain.
                let _ = stamp_task_process_exited(&reap_pty);
                status
            } else {
                None
            };
            // Empty payload remains the frontend's "process exited" signal.
            // Waiting first lets the semantic event distinguish a successful
            // completion from a crash/signal instead of always saying unknown.
            notify_process_exited(&reap_pty, status.as_ref());
        });
    });

    #[cfg(windows)]
    {
        let pty_reader = pty.clone();
        let app_reader = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut buf = [0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        OUTPUT_READS.fetch_add(1, Ordering::Relaxed);
                        broadcast_output(&pty_reader, &buf[..n]);
                    }
                    Err(_) => break,
                }
            }
            if pty_reader.task_exit.is_none() {
                if let Some(mgr) = app_reader.try_state::<PtyManager>() {
                    mgr.ptys.remove(&id);
                }
            }
            let status = if let Ok(mut child) = pty_reader.child.lock() {
                let status = child.wait().ok();
                // Keep the completion stamp ordered before a concurrent app
                // drain can acquire the child lock and inspect the stale pid.
                let _ = stamp_task_process_exited(&pty_reader);
                status
            } else {
                None
            };
            notify_process_exited(&pty_reader, status.as_ref());
        });
    }

    Ok(id)
}

fn insert_subscriber(
    subscribers: &mut HashMap<u32, Channel<Vec<u8>>>,
    next_id: &AtomicU32,
    on_event: Channel<Vec<u8>>,
) -> AppResult<u32> {
    if subscribers.len() >= MAX_PTY_SUBSCRIBERS_PER_PTY {
        return Err(AppError::Pty("PTY subscriber capacity reached".into()));
    }
    // Allocation happens under the same map lock as insertion. A wrapped
    // process-global counter can therefore skip zero and every still-live ID
    // instead of replacing a channel that a stale unsubscribe still names.
    for _ in 0..MAX_SUB_ID_COLLISION_PROBES {
        let sub_id = next_id.fetch_add(1, Ordering::Relaxed);
        if sub_id == 0 {
            continue;
        }
        if let Entry::Vacant(entry) = subscribers.entry(sub_id) {
            entry.insert(on_event);
            return Ok(sub_id);
        }
    }
    Err(AppError::Pty("PTY subscriber id capacity exhausted".into()))
}

#[tauri::command]
pub fn pty_subscribe(
    manager: State<'_, PtyManager>,
    id: u32,
    on_event: Channel<Vec<u8>>,
) -> AppResult<u32> {
    let pty = manager
        .ptys
        .get(&id)
        .ok_or(AppError::BadArg("pty not found"))?;
    let mut subscribers = pty.subscribers.lock().map_err(pty_err)?;
    insert_subscriber(&mut subscribers, &NEXT_SUB_ID, on_event)
}

#[tauri::command]
pub fn pty_unsubscribe(manager: State<'_, PtyManager>, id: u32, sub_id: u32) -> AppResult<()> {
    if let Some(pty) = manager.ptys.get(&id) {
        if let Ok(mut subs) = pty.subscribers.lock() {
            subs.remove(&sub_id);
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    pub sub_id: u32,
    pub snapshot: Vec<u8>,
    pub alternate_screen: bool,
    /// Latest headless shell state, present only for an explicitly enabled,
    /// supported local shell. This lets a remounted frontend recover metadata
    /// even though historical OSC bytes are intentionally not replayed.
    pub shell: Option<ShellMetadataSnapshot>,
}

fn screen_scrollback_len(screen: &vt100::Screen) -> usize {
    let mut s = screen.clone();
    s.set_scrollback(usize::MAX);
    s.scrollback()
}

#[derive(Debug)]
struct BoundedAttachSnapshot {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Produce a replay stream whose returned allocation never exceeds
/// `max_bytes`. When all history cannot fit, only a contiguous newest suffix
/// is retained; the live viewport, terminal modes, and cursor state are never
/// selectively truncated. If that non-negotiable viewport does not fit, the
/// caller gets an error and can leave parser/subscriber state untouched.
fn bounded_attach_snapshot(
    screen: &vt100::Screen,
    max_bytes: usize,
) -> AppResult<BoundedAttachSnapshot> {
    const ALT_SCREEN_PREFIX: &[u8] = b"\x1b[?1049h";
    const HISTORY_ROW_SUFFIX: &[u8] = b"\x1b[0m\r\n";

    let alternate_screen = screen.alternate_screen();
    let prefix = if alternate_screen {
        ALT_SCREEN_PREFIX
    } else {
        &[]
    };
    let viewport = screen.state_formatted();
    let fixed_bytes = prefix
        .len()
        .checked_add(viewport.len())
        .ok_or_else(|| AppError::Pty("PTY attach snapshot size overflow".into()))?;
    if fixed_bytes > max_bytes {
        return Err(AppError::Pty(
            "PTY attach viewport exceeds snapshot capacity".into(),
        ));
    }

    let history_rows = if alternate_screen {
        0
    } else {
        screen_scrollback_len(screen)
    };
    if history_rows == 0 {
        let mut bytes = Vec::with_capacity(fixed_bytes);
        bytes.extend_from_slice(prefix);
        bytes.extend_from_slice(&viewport);
        return Ok(BoundedAttachSnapshot {
            bytes,
            truncated: false,
        });
    }

    let (rows, cols) = screen.size();
    let separator_bytes = usize::from(rows.saturating_sub(1))
        .checked_mul(b"\r\n".len())
        .ok_or_else(|| AppError::Pty("PTY attach snapshot size overflow".into()))?;
    let history_budget = max_bytes.saturating_sub(fixed_bytes.saturating_add(separator_bytes));
    let mut retained = VecDeque::<Vec<u8>>::new();
    let mut retained_bytes = 0usize;
    let mut truncated = history_budget == 0;
    let mut seen_rows = 0usize;
    let mut scrolled = screen.clone();

    // Iterate in the same oldest-to-newest page order as the full replay.
    // The deque never retains more than the remaining byte budget; evicting
    // from its front leaves a deterministic newest suffix.
    let page_rows = usize::from(rows).max(1);
    let mut start = 0usize;
    while start < history_rows {
        scrolled.set_scrollback(history_rows - start);
        let take = (history_rows - start).min(page_rows);
        for mut row in scrolled.rows_formatted(0, cols).take(take) {
            seen_rows += 1;
            row.extend_from_slice(HISTORY_ROW_SUFFIX);
            if row.len() > history_budget {
                retained.clear();
                retained_bytes = 0;
                truncated = true;
                continue;
            }
            while retained_bytes.saturating_add(row.len()) > history_budget {
                let Some(evicted) = retained.pop_front() else {
                    break;
                };
                retained_bytes = retained_bytes.saturating_sub(evicted.len());
                truncated = true;
            }
            retained_bytes += row.len();
            retained.push_back(row);
        }
        start += take;
    }
    truncated |= seen_rows < history_rows || retained.len() < history_rows;

    let include_history = !retained.is_empty();
    let final_separator_bytes = if include_history { separator_bytes } else { 0 };
    let final_capacity = fixed_bytes
        .checked_add(retained_bytes)
        .and_then(|size| size.checked_add(final_separator_bytes))
        .ok_or_else(|| AppError::Pty("PTY attach snapshot size overflow".into()))?;
    debug_assert!(final_capacity <= max_bytes);
    let mut bytes = Vec::with_capacity(final_capacity);
    bytes.extend_from_slice(prefix);
    for row in retained {
        bytes.extend_from_slice(&row);
    }
    if include_history {
        for _ in 1..rows {
            bytes.extend_from_slice(b"\r\n");
        }
    }
    bytes.extend_from_slice(&viewport);
    debug_assert!(bytes.len() <= max_bytes);
    Ok(BoundedAttachSnapshot { bytes, truncated })
}

fn attach_snapshot(screen: &vt100::Screen) -> Vec<u8> {
    let mut snapshot = Vec::new();
    if screen.alternate_screen() {
        // vt100::Screen::state_formatted() restores contents and input
        // modes, but not which screen buffer is active. Re-enter alt
        // screen before replaying alt-buffer contents so xterm's wheel
        // behavior matches the live PTY after a hidden-pane reattach.
        snapshot.extend_from_slice(b"\x1b[?1049h");
    }
    let history_rows = if screen.alternate_screen() {
        0
    } else {
        screen_scrollback_len(screen)
    };
    if history_rows > 0 {
        let (rows, cols) = screen.size();
        let mut scrolled = screen.clone();

        // Seed xterm's scrollback cheaply from vt100's formatted semantic
        // history rows only. `state_formatted` below clears/repaints the live
        // viewport with cursor and input modes; replaying the current viewport
        // here would push a duplicate prompt/input line into scrollback on every
        // reattach, which looks like terminal text repeating after tab switches.
        // Reset between rows because each formatted row is generated from
        // default attrs.
        let page_rows = usize::from(rows).max(1);
        let mut start = 0usize;
        while start < history_rows {
            scrolled.set_scrollback(history_rows - start);
            let take = (history_rows - start).min(page_rows);
            for row in scrolled.rows_formatted(0, cols).take(take) {
                snapshot.extend(row);
                snapshot.extend_from_slice(b"\x1b[0m\r\n");
            }
            start += take;
        }

        // Move the replay cursor far enough that state_formatted's viewport
        // repaint does not overwrite the newest history rows. Without this
        // separator, a fresh parser has a rows-1 hole between the replayed
        // history and the restored live viewport.
        for _ in 1..rows {
            snapshot.extend_from_slice(b"\r\n");
        }
    }
    snapshot.extend(screen.state_formatted());
    snapshot
}

fn reseed_parser_from_snapshot(parser: &mut SemanticParser, snapshot: &[u8], scrollback: usize) {
    let (rows, cols) = parser.screen().size();
    let callbacks = std::mem::take(parser.callbacks_mut());
    let mut fresh = SemanticParser::new_with_callbacks(rows, cols, scrollback, callbacks);
    fresh.process(snapshot);
    *parser = fresh;
}

fn reseed_parser(parser: &mut SemanticParser, scrollback: usize) {
    let snapshot = attach_snapshot(parser.screen());
    reseed_parser_from_snapshot(parser, &snapshot, scrollback);
}

fn attach_snapshot_with_compaction(
    parser: &mut SemanticParser,
    max_bytes: usize,
) -> AppResult<Vec<u8>> {
    let snapshot = bounded_attach_snapshot(parser.screen(), max_bytes)?;
    if snapshot.truncated {
        reseed_parser_from_snapshot(parser, &snapshot.bytes, PARSER_SCROLLBACK);
    }
    Ok(snapshot.bytes)
}

fn compact_parser_for_idle(parser: &mut SemanticParser) -> bool {
    if parser.screen().alternate_screen() {
        return false;
    }
    reseed_parser(parser, IDLE_SCROLLBACK);
    true
}

/// Atomic snapshot + subscribe. The parser lock is held while we both
/// capture the screen contents AND insert the subscriber into the
/// fan-out map, so the reader task (which holds parser → broadcast in
/// the same nested order) cannot interleave a byte that ends up both in
/// the snapshot and in the channel — or one that's in neither.
///
/// `snapshot` is the visible screen + scrollback plus input modes as an
/// ANSI byte stream. Writing it to a fresh xterm reproduces the visual
/// state and the mode state that affects input/wheel handling at the
/// moment of the call. The native byte budget is authoritative: oversized
/// history is compacted to a newest suffix under this same parser lock, while
/// an oversized live viewport fails before a subscriber is registered.
#[tauri::command]
pub fn pty_attach(
    manager: State<'_, PtyManager>,
    id: u32,
    on_event: Channel<Vec<u8>>,
) -> AppResult<AttachResult> {
    let observer = global_observability();
    let operation = observer.slow_operation(
        "pty.attach",
        Duration::from_millis(16),
        None,
        Metadata::new(),
    );
    let result = (|| {
        let pty = manager
            .ptys
            .get(&id)
            .ok_or(AppError::BadArg("pty not found"))?;
        let mut parser = pty.parser.lock().map_err(pty_err)?;
        let alternate_screen = parser.screen().alternate_screen();
        let mut subs = pty.subscribers.lock().map_err(pty_err)?;
        if subs.len() >= MAX_PTY_SUBSCRIBERS_PER_PTY {
            return Err(AppError::Pty("PTY subscriber capacity reached".into()));
        }
        // Make a bounded replay the authoritative parser state when history
        // must be truncated, so repeated attaches do not repeatedly scan
        // discarded history. Output cannot interleave because parser ->
        // subscribers is the reader/broadcast lock order too.
        let snapshot = attach_snapshot_with_compaction(&mut parser, MAX_ATTACH_SNAPSHOT_BYTES)?;
        let shell = parser
            .callbacks()
            .shell
            .as_ref()
            .map(ShellProtocolParser::snapshot);
        let sub_id = insert_subscriber(&mut subs, &NEXT_SUB_ID, on_event)?;
        drop(subs);
        drop(parser);
        Ok(AttachResult {
            sub_id,
            snapshot,
            alternate_screen,
            shell,
        })
    })();
    operation.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

const RESET_MODES: &[u8] = b"\x1b>\x1b[4l\x1b[?1l\x1b[?6l\x1b[?7h\x1b[?9l\x1b[?45l\x1b[?66l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l\x1b[?2004l\x1b[?1049l";

#[tauri::command]
pub fn pty_reset_modes(manager: State<'_, PtyManager>, id: u32) -> AppResult<()> {
    let pty = manager
        .ptys
        .get(&id)
        .ok_or(AppError::BadArg("pty not found"))?;
    let mut parser = pty.parser.lock().map_err(pty_err)?;
    parser.process(RESET_MODES);
    // Queue the exact same bytes to every attached xterm while the parser
    // lock is still held. The reader cannot process and broadcast newer PTY
    // output until this reset is queued, so frontend and backend mode state
    // cannot be reordered around a concurrent child write.
    let mut dead = Vec::new();
    {
        let subscribers = pty.subscribers.lock().map_err(pty_err)?;
        for (sub_id, channel) in subscribers.iter() {
            if channel.send(RESET_MODES.to_vec()).is_err() {
                dead.push(*sub_id);
            }
        }
    }
    drop(parser);
    if !dead.is_empty() {
        let mut subscribers = pty.subscribers.lock().map_err(pty_err)?;
        for sub_id in dead {
            subscribers.remove(&sub_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn pty_write(manager: State<'_, PtyManager>, id: u32, data: String) -> AppResult<()> {
    let observer = global_observability();
    let operation =
        observer.slow_operation("pty.write", Duration::from_millis(8), None, Metadata::new());
    // Clone the Arc out of DashMap immediately so we don't hold a shard
    // across .await points (which would risk deadlocking the manager).
    let pty = manager
        .ptys
        .get(&id)
        .map(|r| r.clone())
        .ok_or(AppError::BadArg("pty not found"))?;
    if submits_line(&data) {
        arm_agent_activity(&pty);
    }
    #[cfg(unix)]
    // Serialise writers on the shared fd; the reader's readable() side is
    // unaffected and keeps draining concurrently.
    let _guard = {
        let wait_started = Instant::now();
        let guard = pty.write_lock.lock().await;
        observer.observe_latency("pty.write_lock_wait", wait_started.elapsed());
        guard
    };
    #[cfg(unix)]
    {
        let write_started = Instant::now();
        write_all_async(&pty.io, data.as_bytes())
            .await
            .map_err(AppError::from)?;
        observer.observe_latency("pty.os_write", write_started.elapsed());
    }
    #[cfg(windows)]
    tauri::async_runtime::spawn_blocking(move || {
        let mut writer = pty.writer.lock().map_err(pty_err)?;
        writer.write_all(data.as_bytes()).map_err(AppError::from)?;
        writer.flush().map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Pty(format!("pty_write join: {e}")))??;
    operation.finish(SpanOutcome::Success);
    Ok(())
}

#[tauri::command]
pub fn pty_resize(manager: State<'_, PtyManager>, id: u32, cols: u16, rows: u16) -> AppResult<()> {
    validate_pty_dimensions(cols, rows)?;
    let pty = manager
        .ptys
        .get(&id)
        .ok_or(AppError::BadArg("pty not found"))?;
    #[cfg(unix)]
    {
        // Resize via TIOCSWINSZ straight on the master fd (the kernel also
        // raises SIGWINCH on the foreground process group).
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe {
            libc::ioctl(
                pty.io.get_ref().as_raw_fd(),
                libc::TIOCSWINSZ,
                &ws as *const _,
            )
        };
        if rc != 0 {
            return Err(pty_err(std::io::Error::last_os_error()));
        }
    }
    #[cfg(windows)]
    pty.master
        .lock()
        .map_err(pty_err)?
        .resize(pty_size(cols, rows))
        .map_err(pty_err)?;
    // Resize the parser too so the grid the snapshot returns matches the
    // xterm's geometry — otherwise re-attach lands on a mis-sized canvas.
    // `set_size` lives on the Screen, not the Parser itself.
    if let Ok(mut parser) = pty.parser.lock() {
        parser.screen_mut().set_size(rows, cols);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::shell_wants_login_flag;
    #[cfg(unix)]
    use super::startup_bootstrap;
    use super::{
        apply_agent_profile, attach_snapshot, attach_snapshot_with_compaction,
        compact_parser_for_idle, configure_pty_environment, configure_shell_integration,
        configure_task_command, detect_shell_kind, event_fingerprint, insert_subscriber,
        parse_shell_cwd, reseed_parser, screen_scrollback_len, semantic_fingerprint,
        semantic_parser, semantic_parser_with_shell, shell_integration_requested,
        should_signal_process_on_drain, submits_line, task_process_needs_force_backstop,
        task_reclamation_plan, task_retention_elapsed, task_shell_arguments,
        validate_pty_dimensions, validate_task_environment, validate_task_request, AttachResult,
        PtyAgentProfile, PtyCapacity, PtyContext, PtyShellMetadataEvent, ShellBoundary, ShellKind,
        ShellPhase, ShellProtocolParser, TaskExitReporter, TaskProcessExit, TaskRetentionCandidate,
        TaskShellPlatform, TaskSource, TaskSpawnRequest, TaskSpawnResult, IDLE_SCROLLBACK,
        MAX_ATTACH_SNAPSHOT_BYTES, MAX_PTY_DIMENSION, MAX_PTY_SUBSCRIBERS_PER_PTY,
        MAX_RETAINED_EXITED_TASK_PTYS, MAX_SHELL_OSC_BYTES, MAX_SHELL_PATH_BYTES,
        MAX_TASK_COMMAND_BYTES, MAX_TASK_ENV_ENTRIES, MAX_TASK_ENV_TOTAL_BYTES, PARSER_SCROLLBACK,
        RESET_MODES, SHELL_EVENT_MIN_INTERVAL, TASK_EXIT_RETENTION,
    };
    use portable_pty::CommandBuilder;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    fn env(command: &CommandBuilder, key: &str) -> Option<String> {
        command
            .get_env(key)
            .map(|value| value.to_string_lossy().into_owned())
    }

    fn output_channel() -> tauri::ipc::Channel<Vec<u8>> {
        tauri::ipc::Channel::new(|_| Ok(()))
    }

    fn local_shell_context() -> PtyContext {
        PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: Some("window-1".into()),
            pane_id: Some("pane-1".into()),
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: true,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_command_reads_zshrc_and_preserves_literal_arguments() {
        use portable_pty::{NativePtySystem, PtySize, PtySystem};
        use std::io::Read;

        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".zshrc"),
            "export SIKEMUX_TEST_AUTH=from-zshrc\nprobe() { printf '%s|%s' \"$SIKEMUX_TEST_AUTH\" \"$1\"; }\n").unwrap();
        let mut command = CommandBuilder::new("/bin/zsh");
        command.env("ZDOTDIR", root.path());
        command.env_remove("SIKEMUX_TEST_AUTH");
        let argument = "spaces ' quotes; $(printf injected) \n next";
        super::configure_interactive_command(
            &mut command,
            &super::PtyDirectCommand {
                program: "probe".into(),
                args: vec![argument.into()],
                profile: None,
            },
        );
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        assert_eq!(child.wait().unwrap().exit_code(), 0);
        assert!(String::from_utf8_lossy(&output)
            .replace("\r\n", "\n")
            .contains(&format!("from-zshrc|{argument}")));
    }

    #[test]
    fn agent_profiles_pin_provider_config_directories() {
        let profile = PtyAgentProfile {
            config_path: Some("/profiles/work/config.toml".into()),
            environment_keys: Vec::new(),
        };
        let mut codex = CommandBuilder::new("codex");
        let mut context = local_shell_context();
        context.agent_id = Some("agent-1".into());
        context.agent_type = Some("codex".into());
        apply_agent_profile(&mut codex, Some(&context), Some(&profile));
        assert_eq!(env(&codex, "CODEX_HOME"), Some("/profiles/work".into()));

        let mut claude = CommandBuilder::new("claude");
        context.agent_type = Some("claude".into());
        apply_agent_profile(&mut claude, Some(&context), Some(&profile));
        assert_eq!(
            env(&claude, "CLAUDE_CONFIG_DIR"),
            Some("/profiles/work".into())
        );
    }

    fn task_request(cwd: &Path) -> TaskSpawnRequest {
        TaskSpawnRequest {
            execution_id: "[\"task\",1]".into(),
            terminal_key: "[\"task\",\"/repo\",\"test\"]".into(),
            task_id: "test:unit".into(),
            label: "Unit tests".into(),
            project: cwd.to_string_lossy().into_owned(),
            source: TaskSource::Project,
            command: "printf '%s' \"$TOKEN\"".into(),
            cwd: cwd.to_string_lossy().into_owned(),
            env: HashMap::from([("TOKEN".into(), "not-logged".into())]),
            cols: 120,
            rows: 40,
        }
    }

    #[test]
    fn active_pty_capacity_is_hard_under_concurrent_admission() {
        const LIMIT: usize = 7;
        const CONTENDERS: usize = 64;
        let capacity = PtyCapacity::new(LIMIT);
        let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
        let results = std::thread::scope(|scope| {
            let handles = (0..CONTENDERS)
                .map(|_| {
                    let capacity = capacity.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        capacity.try_acquire().ok()
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("capacity contender"))
                .collect::<Vec<_>>()
        });
        let mut permits = results.into_iter().flatten().collect::<Vec<_>>();

        assert_eq!(permits.len(), LIMIT);
        assert_eq!(capacity.active.load(Ordering::Acquire), LIMIT);
        assert!(capacity.try_acquire().is_err());

        permits.pop();
        let replacement = capacity.try_acquire().expect("released slot is reusable");
        assert_eq!(capacity.active.load(Ordering::Acquire), LIMIT);
        drop(replacement);
        drop(permits);
        assert_eq!(capacity.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn terminal_geometry_is_strict_and_shared_with_tasks() {
        assert!(validate_pty_dimensions(1, 1).is_ok());
        assert!(validate_pty_dimensions(MAX_PTY_DIMENSION, MAX_PTY_DIMENSION).is_ok());
        for (cols, rows) in [
            (0, 1),
            (1, 0),
            (MAX_PTY_DIMENSION + 1, 1),
            (1, MAX_PTY_DIMENSION + 1),
        ] {
            assert!(validate_pty_dimensions(cols, rows).is_err());
        }

        let directory = tempfile::tempdir().expect("task cwd");
        let mut request = task_request(directory.path());
        request.cols = MAX_PTY_DIMENSION;
        request.rows = MAX_PTY_DIMENSION;
        validate_task_request(&request).expect("shared maximum is valid for tasks");
        request.rows = MAX_PTY_DIMENSION + 1;
        assert!(validate_task_request(&request).is_err());
    }

    #[test]
    fn subscriber_capacity_and_wrapped_ids_never_replace_a_live_channel() {
        let next_id = AtomicU32::new(1);
        let mut subscribers = HashMap::new();
        let mut live_ids = Vec::new();
        for _ in 0..MAX_PTY_SUBSCRIBERS_PER_PTY {
            live_ids.push(
                insert_subscriber(&mut subscribers, &next_id, output_channel())
                    .expect("subscriber below cap"),
            );
        }
        assert_eq!(subscribers.len(), MAX_PTY_SUBSCRIBERS_PER_PTY);
        assert!(insert_subscriber(&mut subscribers, &next_id, output_channel()).is_err());
        assert_eq!(subscribers.len(), MAX_PTY_SUBSCRIBERS_PER_PTY);

        let removed = live_ids.remove(0);
        assert!(subscribers.remove(&removed).is_some());
        let replacement = insert_subscriber(&mut subscribers, &next_id, output_channel())
            .expect("released subscriber slot");
        assert!(!live_ids.contains(&replacement));
        assert_eq!(subscribers.len(), MAX_PTY_SUBSCRIBERS_PER_PTY);

        let wrapped_next = AtomicU32::new(u32::MAX);
        let mut wrapped = HashMap::from([(u32::MAX, output_channel())]);
        let wrapped_id = insert_subscriber(&mut wrapped, &wrapped_next, output_channel())
            .expect("wrap skips zero and live id");
        assert_eq!(wrapped_id, 1);
        assert!(wrapped.contains_key(&u32::MAX));
        assert!(wrapped.contains_key(&wrapped_id));
    }

    #[test]
    fn task_request_is_camel_case_bounded_and_requires_a_real_absolute_cwd() {
        let directory = tempfile::tempdir().expect("task cwd");
        let value = serde_json::json!({
            "executionId": "[\"task\",1]",
            "terminalKey": "[\"task\",\"repo\",\"check\"]",
            "taskId": "check:all",
            "label": "Check all",
            "project": directory.path(),
            "source": "built-in",
            "command": "cargo test\nprintf done",
            "cwd": directory.path(),
            "env": { "TOKEN": "secret\nvalue" },
            "cols": 132,
            "rows": 43
        });
        let request: TaskSpawnRequest =
            serde_json::from_value(value.clone()).expect("deserialize task request");
        validate_task_request(&request).expect("valid task request");
        assert_eq!(request.source, TaskSource::BuiltIn);

        let mut unknown = value;
        unknown["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<TaskSpawnRequest>(unknown).is_err());

        let mut invalid = task_request(directory.path());
        invalid.task_id = "bad/id".into();
        assert!(validate_task_request(&invalid).is_err());
        invalid = task_request(directory.path());
        invalid.command = "\0".repeat(MAX_TASK_COMMAND_BYTES);
        assert!(validate_task_request(&invalid).is_err());
        invalid = task_request(directory.path());
        invalid.cwd = "relative/path".into();
        assert!(validate_task_request(&invalid).is_err());
        invalid = task_request(directory.path());
        invalid.cols = 0;
        assert!(validate_task_request(&invalid).is_err());
    }

    #[test]
    fn task_environment_caps_entries_bytes_and_windows_aliases_without_exposing_values() {
        let too_many = (0..=MAX_TASK_ENV_ENTRIES)
            .map(|index| (format!("KEY_{index}"), String::new()))
            .collect();
        assert!(validate_task_environment(&too_many, false).is_err());

        let oversized = (0..9)
            .map(|index| {
                (
                    format!("KEY_{index}"),
                    "x".repeat(MAX_TASK_ENV_TOTAL_BYTES / 8),
                )
            })
            .collect();
        assert!(validate_task_environment(&oversized, false).is_err());

        let aliases = HashMap::from([("Path".into(), "one".into()), ("PATH".into(), "two".into())]);
        validate_task_environment(&aliases, false).expect("Unix keys are case-sensitive");
        assert!(validate_task_environment(&aliases, true).is_err());
        let unicode_aliases = HashMap::from([
            ("Ä_KEY".into(), "one".into()),
            ("ä_key".into(), "two".into()),
        ]);
        assert!(validate_task_environment(&unicode_aliases, true).is_err());

        for key in ["BAD=KEY", "bad\nkey", "__proto__"] {
            assert!(validate_task_environment(
                &HashMap::from([(key.into(), "value".into())]),
                false
            )
            .is_err());
        }
    }

    #[test]
    fn completed_task_retention_has_exact_grace_and_subscriber_boundaries() {
        let grace = TASK_EXIT_RETENTION.as_millis() as u64;
        assert!(!task_retention_elapsed(0, u64::MAX, false));
        assert!(!task_retention_elapsed(100, 100 + grace - 1, false));
        assert!(task_retention_elapsed(100, 100 + grace, false));
        assert!(!task_retention_elapsed(100, 100 + grace, true));
        assert!(!task_retention_elapsed(500, 100, false));
    }

    #[test]
    fn completed_task_reclamation_is_age_and_cardinality_bounded() {
        let grace = TASK_EXIT_RETENTION.as_millis() as u64;
        let now = grace + 1_000;
        let candidate = |id, exited_at_ms, has_subscribers| TaskRetentionCandidate {
            id,
            exited_at_ms,
            has_subscribers,
        };

        // Running and attached tasks are never part of the reclaimable pool.
        // Two expired entries are removed even though only one entry exceeds
        // the cap; the remaining three exactly fill it.
        let plan = task_reclamation_plan(
            vec![
                candidate(9, now - 100, false),
                candidate(1, 1, false),
                candidate(8, now - 200, true),
                candidate(2, 500, false),
                candidate(7, 0, false),
                candidate(4, now - 300, false),
                candidate(3, now - 400, false),
            ],
            now,
            3,
        );
        assert_eq!(plan, vec![1, 2]);

        // Equal completion times use PTY id as a deterministic tie-breaker.
        assert_eq!(
            task_reclamation_plan(
                vec![candidate(9, now, false), candidate(3, now, false)],
                now,
                1,
            ),
            vec![3]
        );

        let fixed_cap = (1..=(MAX_RETAINED_EXITED_TASK_PTYS as u32 + 1))
            .map(|id| candidate(id, now, false))
            .collect();
        assert_eq!(
            task_reclamation_plan(fixed_cap, now, MAX_RETAINED_EXITED_TASK_PTYS),
            vec![1]
        );
    }

    #[test]
    fn drain_never_signals_a_retained_completed_task_pid() {
        assert!(should_signal_process_on_drain(false, 0));
        assert!(should_signal_process_on_drain(false, 42));
        assert!(should_signal_process_on_drain(true, 0));
        assert!(!should_signal_process_on_drain(true, 42));
        assert!(!task_process_needs_force_backstop(false, 0));
        assert!(task_process_needs_force_backstop(true, 0));
        assert!(!task_process_needs_force_backstop(true, 42));
    }

    #[test]
    fn task_cwd_must_resolve_inside_the_real_project_directory() {
        let root = tempfile::tempdir().expect("task roots");
        let project = root.path().join("project");
        let nested = project.join("packages/app");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&nested).expect("nested task cwd");
        std::fs::create_dir(&outside).expect("outside task cwd");

        let mut request = task_request(&project);
        request.cwd = nested.to_string_lossy().into_owned();
        let paths = validate_task_request(&request).expect("nested cwd is valid");
        assert_eq!(
            paths.project,
            project.canonicalize().expect("canonical project")
        );
        assert_eq!(paths.cwd, nested.canonicalize().expect("canonical cwd"));

        request.cwd = outside.to_string_lossy().into_owned();
        assert!(validate_task_request(&request).is_err());

        let file = project.join("not-a-directory");
        std::fs::write(&file, b"file").expect("project file");
        request.cwd = file.to_string_lossy().into_owned();
        assert!(validate_task_request(&request).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn task_cwd_cannot_symlink_escape_the_project() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("task roots");
        let project = root.path().join("project");
        let outside = root.path().join("outside");
        std::fs::create_dir(&project).expect("project");
        std::fs::create_dir(&outside).expect("outside");
        let escaped = project.join("escaped");
        symlink(&outside, &escaped).expect("escaped cwd symlink");

        let mut request = task_request(&project);
        request.cwd = escaped.to_string_lossy().into_owned();
        assert!(validate_task_request(&request).is_err());
    }

    #[test]
    fn task_shell_uses_direct_arguments_without_requoting_or_interactive_flags() {
        let task = "printf '%s' \"a b;$TOKEN\"";
        assert_eq!(
            task_shell_arguments("/bin/zsh", task, TaskShellPlatform::Unix).expect("zsh task args"),
            ["-c", task]
        );
        assert_eq!(
            task_shell_arguments("pwsh.exe", task, TaskShellPlatform::Windows)
                .expect("PowerShell task args"),
            ["-NoLogo", "-NonInteractive", "-Command", task]
        );
        assert_eq!(
            task_shell_arguments("cmd.exe", task, TaskShellPlatform::Windows)
                .expect("cmd task args"),
            ["/D", "/S", "/C", task]
        );
        assert!(task_shell_arguments("custom.exe", task, TaskShellPlatform::Windows).is_err());

        let mut command = CommandBuilder::new("/bin/zsh");
        configure_task_command(&mut command, "/bin/zsh", task).expect("configure task");
        let argv: Vec<String> = command
            .get_argv()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, ["/bin/zsh", "-c", task]);
        assert_eq!(env(&command, "SIKEMUX_SHELL_INTEGRATION"), None);
        assert!(!argv
            .iter()
            .any(|argument| argument == "-i" || argument == "-NoExit"));
    }

    #[test]
    fn task_exit_reporter_delivers_one_typed_exit_under_racing_completion_paths() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let received = messages.clone();
        let channel = tauri::ipc::Channel::new(move |body| {
            let value = body.deserialize::<serde_json::Value>()?;
            received.lock().expect("messages lock").push(value);
            Ok(())
        });
        let reporter = Arc::new(TaskExitReporter::new(channel));

        std::thread::scope(|scope| {
            for index in 0..16u32 {
                let reporter = reporter.clone();
                scope.spawn(move || {
                    let status = portable_pty::ExitStatus::with_exit_code(index);
                    reporter.send_once(Some(&status));
                });
            }
        });

        let messages = messages.lock().expect("messages lock");
        assert_eq!(messages.len(), 1);
        assert!(messages[0]["code"].as_u64().is_some());
        assert!(messages[0].get("signal").is_none());
    }

    #[test]
    fn task_wire_results_are_exact_and_signal_is_optional() {
        assert_eq!(
            serde_json::to_value(TaskSpawnResult { pty_id: 42 }).expect("serialize spawn"),
            serde_json::json!({ "ptyId": 42 })
        );
        assert_eq!(
            serde_json::to_value(TaskProcessExit::from_status(Some(
                &portable_pty::ExitStatus::with_exit_code(0)
            )))
            .expect("serialize success"),
            serde_json::json!({ "code": 0 })
        );
        assert_eq!(
            serde_json::to_value(TaskProcessExit::from_status(Some(
                &portable_pty::ExitStatus::with_signal("SIGTERM")
            )))
            .expect("serialize signal"),
            serde_json::json!({ "code": 1, "signal": "SIGTERM" })
        );
    }

    #[test]
    fn pty_environment_has_terminal_and_typed_sikemux_identity() {
        let mut command = CommandBuilder::new("shell");
        command.env("SIKEMUX_AGENT_ID", "stale-agent");
        command.env("CODEX_THREAD_ID", "parent-thread");
        command.env("CLAUDECODE", "1");
        command.env("CLAUDE_CODE_CHILD_SESSION", "1");
        command.env("CLAUDE_CODE_SESSION_ID", "parent-session");
        command.env("CLAUDE_CODE_MESSAGING_TOKEN", "parent-token");
        command.env("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/parent.sock");
        command.env_remove("EDITOR");
        command.env_remove("VISUAL");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: Some("window-1".into()),
            pane_id: Some("pane-1".into()),
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: false,
        };

        configure_pty_environment(
            &mut command,
            Some(&context),
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            Some(Path::new("/runtime/cli.json")),
            &HashMap::new(),
        );

        assert_eq!(env(&command, "TERM"), Some("xterm-256color".into()));
        assert_eq!(env(&command, "COLORTERM"), Some("truecolor".into()));
        assert_eq!(env(&command, "TERM_PROGRAM"), Some("Sikemux".into()));
        assert_eq!(env(&command, "TERM_PROGRAM_VERSION"), Some("1.2.3".into()));
        assert_eq!(env(&command, "SIKEMUX"), Some("1".into()));
        assert_eq!(env(&command, "SIKEMUX_VERSION"), Some("1.2.3".into()));
        assert_eq!(
            env(&command, "SIKEMUX_SESSION_ID"),
            Some("session-1".into())
        );
        assert_eq!(env(&command, "SIKEMUX_SESSION_NAME"), Some("repo".into()));
        assert_eq!(
            env(&command, "SIKEMUX_SESSION_KIND"),
            Some("project".into())
        );
        assert_eq!(env(&command, "SIKEMUX_PROJECT"), Some("/repo".into()));
        assert_eq!(env(&command, "SIKEMUX_WINDOW_ID"), Some("window-1".into()));
        assert_eq!(env(&command, "SIKEMUX_PANE_ID"), Some("pane-1".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), None);
        assert_eq!(
            env(&command, "CODEX_THREAD_ID"),
            Some("parent-thread".into())
        );
        for marker in [
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_MESSAGING_SOCKET",
        ] {
            assert!(env(&command, marker).is_some());
        }
        assert_eq!(
            env(&command, "SIKEMUX_BIN_PATH"),
            Some("/app/sikemux-editor".into())
        );
        assert_eq!(
            env(&command, "SIKEMUX_CLI_ENDPOINT"),
            Some("/runtime/cli.json".into())
        );
        assert_eq!(env(&command, "EDITOR"), Some("/app/sikemux-editor".into()));
        assert_eq!(env(&command, "VISUAL"), Some("/app/sikemux-editor".into()));
    }

    #[test]
    fn pty_environment_preserves_user_editor_choice_if_either_var_exists() {
        let mut editor_only = CommandBuilder::new("shell");
        editor_only.env("EDITOR", "nvim");
        editor_only.env_remove("VISUAL");
        configure_pty_environment(
            &mut editor_only,
            None,
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            None,
            &HashMap::new(),
        );
        assert_eq!(env(&editor_only, "EDITOR"), Some("nvim".into()));
        assert_eq!(env(&editor_only, "VISUAL"), None);

        let mut visual_only = CommandBuilder::new("shell");
        visual_only.env_remove("EDITOR");
        visual_only.env("VISUAL", "code --wait");
        configure_pty_environment(
            &mut visual_only,
            None,
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            None,
            &HashMap::new(),
        );
        assert_eq!(env(&visual_only, "EDITOR"), None);
        assert_eq!(env(&visual_only, "VISUAL"), Some("code --wait".into()));
    }

    #[test]
    fn pty_environment_quotes_editor_command_paths_with_spaces() {
        let mut command = CommandBuilder::new("shell");
        command.env_remove("EDITOR");
        command.env_remove("VISUAL");
        configure_pty_environment(
            &mut command,
            None,
            "1.2.3",
            Some(Path::new(
                "/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor",
            )),
            None,
            &HashMap::new(),
        );

        assert_eq!(
            env(&command, "SIKEMUX_BIN_PATH"),
            Some("/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor".into())
        );
        #[cfg(not(windows))]
        assert_eq!(
            env(&command, "EDITOR"),
            Some("'/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor'".into())
        );
        #[cfg(windows)]
        assert_eq!(
            env(&command, "EDITOR"),
            Some("\"/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor\"".into())
        );
    }

    /// The bug this guards: an agent CLI spawned as a direct command reads no
    /// shell profile, so a credential the user keeps in `.zshrc` never reaches
    /// it and the CLI demands a login that the user's own terminal never asks
    /// for.
    #[test]
    fn pty_environment_supplies_profile_exports_a_shell_free_pane_cannot_read() {
        let mut command = CommandBuilder::new("claude");
        command.env_remove("ANTHROPIC_API_KEY");
        let profile = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string()),
            ("HTTPS_PROXY".to_string(), "http://proxy:8080".to_string()),
        ]);

        configure_pty_environment(&mut command, None, "1.2.3", None, None, &profile);

        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-profile".into())
        );
        assert_eq!(
            env(&command, "HTTPS_PROXY"),
            Some("http://proxy:8080".into())
        );
    }

    /// The profile fills gaps; it never overrules a value this layer sets. A
    /// profile that exports TERM_PROGRAM must not make a pane claim to be
    /// another terminal, and one that exports SIKEMUX_AGENT_ID must not let a
    /// pane forge another pane's identity.
    #[test]
    fn pty_environment_profile_never_overrides_sikemux_owned_identity() {
        let mut command = CommandBuilder::new("shell");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "one".into(),
            session_kind: "agent".into(),
            project: None,
            window_id: None,
            pane_id: None,
            agent_id: Some("agent-real".into()),
            agent_type: Some("claude".into()),
            initial_prompt_submitted: false,
            shell_integration: false,
        };
        let profile = HashMap::from([
            ("TERM_PROGRAM".to_string(), "Ghostty".to_string()),
            ("TERM".to_string(), "xterm-kitty".to_string()),
            ("SIKEMUX_AGENT_ID".to_string(), "agent-forged".to_string()),
        ]);

        configure_pty_environment(&mut command, Some(&context), "1.2.3", None, None, &profile);

        assert_eq!(env(&command, "TERM_PROGRAM"), Some("Sikemux".into()));
        assert_eq!(env(&command, "TERM"), Some("xterm-256color".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), Some("agent-real".into()));
    }

    #[test]
    fn pty_environment_preserves_provider_environment_and_resets_own_identity() {
        let mut command = CommandBuilder::new("claude");
        // `CommandBuilder::new` seeds itself from this process's environment,
        // so a developer running the suite from their own terminal already has
        // these set and the profile fill would correctly decline to touch them.
        // Clear them to model the launchd-minimal environment a GUI app sees.
        command.env_remove("ANTHROPIC_API_KEY");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "one".into(),
            session_kind: "agent".into(),
            project: None,
            window_id: None,
            pane_id: None,
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: false,
        };
        let profile = HashMap::from([
            ("SIKEMUX_AGENT_ID".to_string(), "agent-forged".to_string()),
            ("SIKEMUX_PANE_ID".to_string(), "pane-forged".to_string()),
            ("SIKEMUX_SHELL_INTEGRATION".to_string(), "1".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_MESSAGING_SOCKET".to_string(),
                "/tmp/parent.sock".to_string(),
            ),
            (
                "CLAUDE_CODE_MESSAGING_TOKEN".to_string(),
                "parent-token".to_string(),
            ),
            ("CODEX_THREAD_ID".to_string(), "thread-parent".to_string()),
            // A real credential in the same map still has to arrive, or the
            // scrub would be passing by discarding everything.
            ("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string()),
        ]);

        configure_pty_environment(&mut command, Some(&context), "1.2.3", None, None, &profile);

        for scrubbed in [
            "SIKEMUX_AGENT_ID",
            "SIKEMUX_PANE_ID",
            "SIKEMUX_SHELL_INTEGRATION",
        ] {
            assert_eq!(env(&command, scrubbed), None);
        }
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CODEX_THREAD_ID",
        ] {
            assert!(env(&command, key).is_some());
        }
        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-profile".into())
        );
    }

    /// An explicit value already on the command outranks the profile, so a
    /// per-pane assignment is never silently replaced by a global export.
    #[test]
    fn pty_environment_profile_yields_to_an_explicit_command_value() {
        let mut command = CommandBuilder::new("shell");
        command.env("ANTHROPIC_API_KEY", "sk-explicit");
        let profile = HashMap::from([("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string())]);

        configure_pty_environment(&mut command, None, "1.2.3", None, None, &profile);

        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-explicit".into())
        );
    }

    #[test]
    fn pty_environment_rebuilds_agent_identity_without_fake_pane_identity() {
        let mut command = CommandBuilder::new("shell");
        command.env("SIKEMUX_WINDOW_ID", "parent-window");
        command.env("SIKEMUX_PANE_ID", "parent-pane");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: None,
            pane_id: None,
            agent_id: Some("agent-1".into()),
            agent_type: Some("codex".into()),
            initial_prompt_submitted: false,
            shell_integration: false,
        };

        configure_pty_environment(
            &mut command,
            Some(&context),
            "1.2.3",
            None,
            None,
            &HashMap::new(),
        );

        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), Some("agent-1".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_TYPE"), Some("codex".into()));
        assert_eq!(env(&command, "SIKEMUX_WINDOW_ID"), None);
        assert_eq!(env(&command, "SIKEMUX_PANE_ID"), None);
    }

    #[test]
    fn shell_integration_is_strictly_opt_in_and_local_interactive_only() {
        let mut context = local_shell_context();
        assert!(shell_integration_requested(Some(&context), false, false));

        context.shell_integration = false;
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.shell_integration = true;
        assert!(!shell_integration_requested(Some(&context), true, false));
        assert!(!shell_integration_requested(Some(&context), false, true));

        context.session_kind = "ssh".into();
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.session_kind = "project".into();
        context.agent_id = Some("agent-1".into());
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.agent_id = None;
        context.agent_type = Some("codex".into());
        assert!(!shell_integration_requested(Some(&context), false, false));
        assert!(!shell_integration_requested(None, false, false));
    }

    #[test]
    fn shell_integration_context_flag_is_default_off_and_camel_case() {
        let base = serde_json::json!({
            "sessionId": "session-1",
            "sessionName": "repo",
            "sessionKind": "project",
            "project": "/repo",
            "windowId": "window-1",
            "paneId": "pane-1"
        });
        let disabled: PtyContext =
            serde_json::from_value(base.clone()).expect("deserialize default context");
        assert!(!disabled.shell_integration);

        let mut enabled_value = base;
        enabled_value["shellIntegration"] = serde_json::Value::Bool(true);
        let enabled: PtyContext =
            serde_json::from_value(enabled_value).expect("deserialize opt-in context");
        assert!(enabled.shell_integration);
    }

    #[test]
    fn shell_detection_claims_only_exact_supported_executables() {
        assert_eq!(detect_shell_kind("/bin/zsh"), Some(ShellKind::Zsh));
        assert_eq!(detect_shell_kind("bash"), Some(ShellKind::Bash));
        assert_eq!(
            detect_shell_kind("/opt/homebrew/bin/fish"),
            Some(ShellKind::Fish)
        );
        assert_eq!(
            detect_shell_kind(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(
            detect_shell_kind("powershell.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(detect_shell_kind("/bin/sh"), None);
        assert_eq!(detect_shell_kind("/usr/local/bin/my-zsh-wrapper"), None);
    }

    #[test]
    fn supported_shells_receive_ephemeral_hooks_without_dotfile_writes() {
        // zsh is deliberately left alone: hooking it means owning ZDOTDIR, and
        // a user's config derives real paths from that.
        let mut zsh = CommandBuilder::new("/bin/zsh");
        assert!(configure_shell_integration(&mut zsh, "/bin/zsh")
            .expect("configure zsh")
            .is_none());
        assert_eq!(env(&zsh, "ZDOTDIR"), None);
        assert_eq!(env(&zsh, "SIKEMUX_SHELL_INTEGRATION"), None);
        #[cfg(unix)]
        assert!(shell_wants_login_flag("/bin/zsh"));

        let mut bash = CommandBuilder::new("/bin/bash");
        let bash_guard = configure_shell_integration(&mut bash, "/bin/bash")
            .expect("configure bash")
            .expect("supported bash");
        let bash_args: Vec<String> = bash
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(bash_args.get(1).map(String::as_str), Some("--rcfile"));
        let bash_hook = std::fs::read_to_string(&bash_args[2]).expect("read bash hook");
        assert!(bash_hook.contains("PROMPT_COMMAND"));
        assert!(bash_hook.contains("PS0="));

        let mut fish = CommandBuilder::new("fish");
        let original_fish_xdg = env(&fish, "XDG_CONFIG_HOME");
        let fish_guard = configure_shell_integration(&mut fish, "fish")
            .expect("configure fish")
            .expect("supported fish");
        assert_eq!(env(&fish, "XDG_CONFIG_HOME"), original_fish_xdg);
        let fish_args: Vec<String> = fish
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(fish_args.get(1).map(String::as_str), Some("--init-command"));
        assert!(fish_args[2].contains("fish_preexec"));
        assert!(fish_args[2].contains("fish_postexec"));
        assert!(!fish_args[2].contains("XDG_CONFIG_HOME"));

        let mut powershell = CommandBuilder::new("pwsh");
        let powershell_guard = configure_shell_integration(&mut powershell, "pwsh")
            .expect("configure PowerShell")
            .expect("supported PowerShell");
        let powershell_args: Vec<String> = powershell
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &powershell_args[1..3],
            &["-NoExit".to_string(), "-Command".to_string()]
        );
        assert!(powershell_args[3].contains("function global:prompt"));

        let mut unsupported = CommandBuilder::new("/bin/sh");
        assert!(configure_shell_integration(&mut unsupported, "/bin/sh")
            .expect("unsupported shell is not an error")
            .is_none());
        assert_eq!(unsupported.get_argv().len(), 1);
        assert_eq!(env(&unsupported, "SIKEMUX_SHELL_INTEGRATION"), None);

        drop((bash_guard, fish_guard, powershell_guard));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_protocol_parses_chunked_cwd_and_command_boundaries() {
        let mut parser = ShellProtocolParser::default();
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\repo root")
        } else {
            PathBuf::from("/tmp/repo root")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let cwd_signal = format!("plain\x1b]7;{cwd_uri}");
        let split = cwd_signal.len() - 2;
        let first = parser.process(&cwd_signal.as_bytes()[..split]);
        assert!(first.latest.is_none());

        let mut second_chunk = cwd_signal.as_bytes()[split..].to_vec();
        second_chunk.extend_from_slice(
            b"\x1b\\\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x1b\\\x1b]133;D;7\x07",
        );
        let second = parser.process(&second_chunk);
        assert_eq!(second.dropped, 0);
        assert_eq!(second.coalesced, 4);
        let latest = second.latest.expect("latest coalesced update");
        assert_eq!(latest.boundary, ShellBoundary::CommandFinished);
        assert_eq!(latest.metadata.revision, 5);
        assert_eq!(latest.metadata.cwd.as_deref(), cwd.to_str());
        assert_eq!(latest.metadata.phase, ShellPhase::Finished);
        assert_eq!(latest.metadata.last_exit_code, Some(7));

        for (signal, expected) in [
            (b"\x1b]133;A\x07".as_slice(), ShellBoundary::PromptStart),
            (b"\x1b]133;B\x07".as_slice(), ShellBoundary::CommandStart),
            (b"\x1b]133;C\x07".as_slice(), ShellBoundary::CommandExecuted),
            (
                b"\x1b]133;D;0\x07".as_slice(),
                ShellBoundary::CommandFinished,
            ),
        ] {
            assert_eq!(
                ShellProtocolParser::default()
                    .process(signal)
                    .latest
                    .expect("boundary update")
                    .boundary,
                expected
            );
        }
    }

    #[test]
    fn shell_protocol_rejects_remote_and_bounds_untrusted_state() {
        assert_eq!(parse_shell_cwd(b"https://localhost/tmp"), None);
        assert_eq!(parse_shell_cwd(b"file://remote-host/tmp"), None);

        assert_eq!(parse_shell_cwd(b"file:///tmp/a%0Ab"), None);

        let oversized_path = format!("file:///{}", "a".repeat(MAX_SHELL_PATH_BYTES + 1));
        assert_eq!(parse_shell_cwd(oversized_path.as_bytes()), None);

        let mut parser = ShellProtocolParser::default();
        let mut oversized_osc = b"\x1b]7;file:///".to_vec();
        oversized_osc.extend(std::iter::repeat_n(b'a', MAX_SHELL_OSC_BYTES + 1));
        let partial = parser.process(&oversized_osc);
        assert!(partial.latest.is_none());
        assert!(parser.osc.len() <= MAX_SHELL_OSC_BYTES);
        let recovered = parser.process(b"\x07\x1b]133;A\x07");
        assert_eq!(recovered.dropped, 1);
        assert_eq!(
            recovered.latest.expect("recovered update").boundary,
            ShellBoundary::PromptStart
        );
    }

    #[test]
    fn shell_protocol_coalesces_hostile_batches_to_latest_headless_state() {
        let mut parser = ShellProtocolParser::default();
        let signal_count = 1_000usize;
        let signals = b"\x1b]133;A\x07".repeat(signal_count);
        let batch = parser.process(&signals);
        assert!(batch.latest.is_some());
        assert_eq!(batch.coalesced, signal_count - 1);
        assert_eq!(batch.dropped, 0);
        assert_eq!(parser.snapshot().revision, signal_count as u64);
        assert_eq!(parser.snapshot().phase, ShellPhase::Prompt);
    }

    #[test]
    fn shell_event_gate_rate_limits_and_flushes_one_bounded_pending_update() {
        let mut parser = ShellProtocolParser::default();
        let first = parser.process_for_events(b"\x1b]133;A\x07", 1_000);
        assert_eq!(
            first.ready.expect("first update is immediate").boundary,
            ShellBoundary::PromptStart
        );

        let second = parser.process_for_events(b"\x1b]133;B\x07", 1_001);
        assert!(second.ready.is_none());
        assert_eq!(second.coalesced, 0);
        let third = parser.process_for_events(b"\x1b]133;C\x07", 1_050);
        assert!(third.ready.is_none());
        assert_eq!(third.coalesced, 1, "newest update replaces one pending");
        assert!(parser.take_due_event(1_099).is_none());
        assert_eq!(
            parser
                .take_due_event(1_000 + SHELL_EVENT_MIN_INTERVAL.as_millis() as u64)
                .expect("latest pending update becomes due")
                .boundary,
            ShellBoundary::CommandExecuted
        );
        assert!(parser.events.pending.is_none());
    }

    #[test]
    fn hostile_osc_stream_cannot_exceed_the_per_pty_event_rate() {
        let mut parser = ShellProtocolParser::default();
        let mut emitted = 0usize;
        for now in 0..1_000u64 {
            let output = parser.process_for_events(b"\x1b]133;A\x07", now);
            emitted += usize::from(output.ready.is_some());
        }
        let maximum = 1_000usize / SHELL_EVENT_MIN_INTERVAL.as_millis() as usize;
        assert_eq!(emitted, maximum);
        assert!(parser.events.pending.is_some());
    }

    #[test]
    fn shell_protocol_side_parse_preserves_visible_terminal_output() {
        let mut parser = semantic_parser_with_shell(24, 80, PARSER_SCROLLBACK, true);
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\project")
        } else {
            PathBuf::from("/tmp/project")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let output = format!("before\x1b]7;{cwd_uri}\x07after");
        let split = output.len() / 2;
        for chunk in [&output.as_bytes()[..split], &output.as_bytes()[split..]] {
            let batch = parser
                .callbacks_mut()
                .shell
                .as_mut()
                .expect("enabled shell parser")
                .process(chunk);
            parser.process(chunk);
            assert_eq!(batch.dropped, 0);
        }
        assert_eq!(parser.screen().contents().trim(), "beforeafter");
        assert_eq!(
            parser
                .callbacks()
                .shell
                .as_ref()
                .expect("enabled shell parser")
                .snapshot()
                .cwd
                .as_deref(),
            cwd.to_str()
        );
    }

    #[test]
    fn shell_metadata_event_is_typed_bounded_frontend_payload() {
        let mut parser = ShellProtocolParser::default();
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\project")
        } else {
            PathBuf::from("/tmp/project")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let update = parser
            .process(format!("\x1b]7;{cwd_uri}\x07").as_bytes())
            .latest
            .expect("cwd update");
        let value = serde_json::to_value(PtyShellMetadataEvent::from_update(42, update))
            .expect("serialize shell metadata event");
        assert_eq!(value["ptyId"], 42);
        assert_eq!(value["revision"], 1);
        assert_eq!(value["boundary"], "cwd");
        assert_eq!(value["cwd"], cwd.to_string_lossy().as_ref());
        assert_eq!(value["phase"], "unknown");
        assert!(value.get("exitCode").is_none());
    }

    #[test]
    fn only_submitted_input_arms_agent_activity() {
        assert!(submits_line("ship it\r"));
        assert!(submits_line("first\nsecond"));
        assert!(!submits_line("still typing"));
        assert!(!submits_line("\x1b[A"));
    }

    #[test]
    fn semantic_fingerprint_changes_with_evidence_or_revision() {
        let base = semantic_fingerprint(1, "prompt", "Codex");
        assert_eq!(base, semantic_fingerprint(1, "prompt", "Codex"));
        assert_ne!(base, semantic_fingerprint(2, "prompt", "Codex"));
        assert_ne!(base, semantic_fingerprint(1, "working", "Codex"));
        assert_ne!(base, semantic_fingerprint(1, "prompt", "Action required"));
    }

    #[test]
    fn event_fingerprint_preserves_same_state_evidence_upgrades() {
        let activity =
            event_fingerprint(1, "working", "activity", "high", "command submitted", None);
        let screen = event_fingerprint(
            1,
            "working",
            "screen",
            "high",
            "manifest rule spinner matched visible working status",
            Some("spinner"),
        );
        let changed_reason = event_fingerprint(
            1,
            "working",
            "screen",
            "high",
            "manifest rule tool matched visible working status",
            Some("tool"),
        );
        assert_ne!(activity, screen);
        assert_ne!(screen, changed_reason);
        assert_eq!(
            screen,
            event_fingerprint(
                1,
                "working",
                "screen",
                "high",
                "manifest rule spinner matched visible working status",
                Some("spinner")
            )
        );
    }

    #[test]
    fn semantic_parser_captures_and_sanitizes_osc_title() {
        let mut parser = semantic_parser(24, 80, PARSER_SCROLLBACK);
        parser.process(b"\x1b]2;Action\n required\x07");
        assert_eq!(parser.callbacks().window_title, "Action required");
    }

    #[test]
    fn snapshot_round_trips_visible_state() {
        // Smoke-check the contract pty_attach relies on: a parser whose
        // bytes were processed re-emits an ANSI stream that reproduces the
        // visible state when written back into a fresh parser. The full
        // attach/snapshot path can't be exercised without a real PTY, but
        // the parser invariant is the load-bearing piece.
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"hello world\r\nsecond line\r\n");
        let dump = a.screen().contents_formatted();

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);
        assert_eq!(
            a.screen().contents(),
            b.screen().contents(),
            "snapshot did not round-trip cleanly",
        );
    }

    #[test]
    fn attach_snapshot_restores_input_modes() {
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        let dump = attach_snapshot(a.screen());

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);

        assert!(b.screen().bracketed_paste());
        assert_eq!(
            b.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease,
        );
        assert_eq!(
            b.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr,
        );
    }

    #[test]
    fn attach_snapshot_restores_scrollback() {
        let mut a = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..20 {
            a.process(format!("line {i:02}\r\n").as_bytes());
        }
        let dump = attach_snapshot(a.screen());

        let mut b = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        b.process(&dump);

        assert_eq!(b.screen().contents(), a.screen().contents());

        let screen = b.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(
            screen.scrollback() >= 10,
            "reattach snapshot should seed xterm/vt100 scrollback; got {} rows",
            screen.scrollback()
        );
        assert!(
            screen.contents().contains("line 00"),
            "oldest retained output should be reachable after scrolling"
        );
    }

    #[test]
    fn attach_snapshot_restores_scrollback_attrs() {
        let mut a = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..20 {
            let color = 31 + (i % 6);
            a.process(format!("\x1b[{color}mline {i:02}\x1b[0m\r\n").as_bytes());
        }
        let dump = attach_snapshot(a.screen());

        let mut b = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        b.process(&dump);

        assert_eq!(b.screen().contents(), a.screen().contents());

        let screen = b.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(
            screen.contents().contains("line 00"),
            "oldest retained output should be reachable after scrolling"
        );
        assert_eq!(
            screen
                .cell(0, 0)
                .expect("top-left scrollback cell")
                .fgcolor(),
            vt100::Color::Idx(1),
            "reattach snapshot should preserve attrs for scrolled-out rows"
        );
    }

    #[test]
    fn attach_snapshot_restores_alternate_screen() {
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"normal\r\n\x1b[?1049halt");
        let dump = attach_snapshot(a.screen());

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);

        assert!(b.screen().alternate_screen());
        assert_eq!(b.screen().contents(), a.screen().contents());
    }

    #[test]
    fn attach_snapshot_has_exact_history_continuity_without_blank_hole() {
        let mut source = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..30 {
            source.process(format!("line {i:02}\r\n").as_bytes());
        }

        let mut restored = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        restored.process(&attach_snapshot(source.screen()));

        let source_screen = source.screen_mut();
        source_screen.set_scrollback(5);
        let expected = source_screen.contents();
        source_screen.set_scrollback(0);
        let expected_viewport = source_screen.contents();

        let screen = restored.screen_mut();
        screen.set_scrollback(5);
        let joined = screen.contents();
        assert_eq!(joined, expected, "history-to-viewport boundary has a hole");
        assert!(!joined.lines().any(|line| line.trim().is_empty()));
        screen.set_scrollback(0);
        assert_eq!(screen.contents(), expected_viewport);
    }

    #[test]
    fn attach_snapshot_byte_budget_compacts_only_old_history() {
        let mut parser = semantic_parser(6, 40, PARSER_SCROLLBACK);
        for index in 0..500 {
            parser.process(
                format!(
                    "\x1b[{}mline {index:04} {}\x1b[0m\r\n",
                    31 + index % 7,
                    "x".repeat(32)
                )
                .as_bytes(),
            );
        }
        parser.process(b"\x1b[?2004h");

        let visible_before = parser.screen().contents();
        let history_before = screen_scrollback_len(parser.screen());
        let full_before = attach_snapshot(parser.screen());
        let viewport_bytes = parser.screen().state_formatted().len();
        let budget = viewport_bytes + 512;
        assert!(full_before.len() > budget, "fixture must force compaction");

        let snapshot = attach_snapshot_with_compaction(&mut parser, budget)
            .expect("bounded snapshot retains viewport");
        assert!(snapshot.len() <= budget);
        assert_eq!(parser.screen().contents(), visible_before);
        assert!(parser.screen().bracketed_paste());
        assert!(screen_scrollback_len(parser.screen()) < history_before);

        let mut restored = vt100::Parser::new(6, 40, PARSER_SCROLLBACK);
        restored.process(&snapshot);
        assert_eq!(restored.screen().contents(), visible_before);
        assert!(restored.screen().bracketed_paste());
        assert_eq!(MAX_ATTACH_SNAPSHOT_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn attach_snapshot_viewport_over_budget_fails_without_mutation() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        for index in 0..20 {
            parser.process(format!("line {index:02}\r\n").as_bytes());
        }
        parser.process(b"\x1b[?1000h\x1b[?1006h");
        let before = attach_snapshot(parser.screen());
        let viewport_bytes = parser.screen().state_formatted().len();
        assert!(viewport_bytes > 0);

        assert!(attach_snapshot_with_compaction(&mut parser, viewport_bytes - 1).is_err());
        assert_eq!(attach_snapshot(parser.screen()), before);
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease
        );
        assert_eq!(
            parser.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr
        );
    }

    #[test]
    fn idle_compaction_retains_tail_and_modes() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        for i in 0..2_100 {
            parser.process(format!("line {i:04}\r\n").as_bytes());
        }
        parser.process(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");

        assert!(compact_parser_for_idle(&mut parser));
        assert!(parser.screen().application_cursor());
        assert!(parser.screen().bracketed_paste());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::ButtonMotion
        );
        assert_eq!(
            parser.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr
        );
        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(screen.scrollback() <= IDLE_SCROLLBACK);
        assert!(screen.contents().contains("line 0100"));
    }

    #[test]
    fn idle_compaction_skips_alternate_screen() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        parser.process(b"normal history\r\n\x1b[?1049halt screen");
        let before = attach_snapshot(parser.screen());

        assert!(!compact_parser_for_idle(&mut parser));
        assert_eq!(attach_snapshot(parser.screen()), before);
        assert!(parser.screen().alternate_screen());
    }

    #[test]
    fn reseed_restores_full_future_scrollback_capacity() {
        let mut parser = semantic_parser(5, 20, IDLE_SCROLLBACK);
        for i in 0..1_000 {
            parser.process(format!("old {i:04}\r\n").as_bytes());
        }
        reseed_parser(&mut parser, PARSER_SCROLLBACK);
        for i in 0..3_000 {
            parser.process(format!("new {i:04}\r\n").as_bytes());
        }

        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(screen.scrollback() > IDLE_SCROLLBACK);
        assert!(screen.contents().contains("old 0000"));
    }

    #[test]
    fn attach_result_serializes_alternate_screen_camel_case() {
        let value = serde_json::to_value(AttachResult {
            sub_id: 7,
            snapshot: Vec::new(),
            alternate_screen: true,
            shell: None,
        })
        .expect("serialize attach result");
        assert_eq!(value["alternateScreen"], true);
        assert!(value.get("alternate_screen").is_none());
    }

    #[test]
    fn reset_modes_disables_interaction_modes_without_losing_normal_history() {
        let mut parser = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..20 {
            parser.process(format!("line {i:02}\r\n").as_bytes());
        }
        parser.process(
            b"\x1b=\x1b[?1h\x1b[?9h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1005h\x1b[?1006h\x1b[?2004h\x1b[?1049halt",
        );
        parser.process(RESET_MODES);

        assert!(!parser.screen().alternate_screen());
        assert!(!parser.screen().application_keypad());
        assert!(!parser.screen().application_cursor());
        assert!(!parser.screen().bracketed_paste());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::None
        );
        assert_eq!(
            parser.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Default
        );
        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(screen.contents().contains("line 00"));
    }

    #[test]
    fn scrollback_matches_frontend() {
        // If this number changes, update SCROLLBACK in TerminalPane.tsx
        // so reattach doesn't repaint a truncated history.
        assert_eq!(PARSER_SCROLLBACK, 10_000);
    }

    #[cfg(unix)]
    #[test]
    fn startup_runs_before_the_interactive_shell_without_pty_input() {
        let bootstrap = startup_bootstrap("ssh prod-db", false);
        assert_eq!(bootstrap, "ssh prod-db\nexec \"$SIKEMUX_SHELL\"");
        assert!(!bootstrap.contains('\r'));

        // A shell that reads its profile keeps doing so after the startup
        // command hands over, or the user lands in a stripped environment.
        let login = startup_bootstrap("ssh prod-db", true);
        assert_eq!(login, "ssh prod-db\nexec \"$SIKEMUX_SHELL\" -l");
    }

    #[cfg(unix)]
    #[test]
    fn task_command_runs_noninteractive_with_exact_cwd_env_output_and_status() {
        use portable_pty::{NativePtySystem, PtySize, PtySystem};
        use std::io::Read;

        let root = tempfile::tempdir().expect("task root");
        let cwd = root.path().join("project with spaces");
        std::fs::create_dir(&cwd).expect("task cwd");
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open task pty");
        let mut command = CommandBuilder::new("/bin/sh");
        command.cwd(&cwd);
        command.env("TASK_TEST_VALUE", "value with spaces");
        configure_task_command(
            &mut command,
            "/bin/sh",
            "printf '%s|%s' \"$PWD\" \"$TASK_TEST_VALUE\"; exit 7",
        )
        .expect("configure task command");
        let mut reader = pair.master.try_clone_reader().expect("clone task reader");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn task command");
        drop(pair.slave);

        let mut bytes = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
                Err(_) => break,
            }
        }
        let status = child.wait().expect("wait for task command");
        let output = String::from_utf8_lossy(&bytes);
        assert_eq!(status.exit_code(), 7);
        assert!(output.contains(cwd.to_string_lossy().as_ref()));
        assert!(output.contains("value with spaces"));
    }

    // The load-bearing invariant of the single-fd PTY design: after we dup
    // the master and drop portable_pty's `MasterPty`, the dup must keep the
    // master open-file-description (and therefore the child's controlling
    // terminal) alive. The child sleeps, THEN prints — so if dropping the
    // MasterPty had hung up the terminal, the child would take SIGHUP during
    // the sleep and the read below would hit EOF before the marker arrives.
    #[cfg(unix)]
    #[test]
    fn lone_master_dup_keeps_child_alive_after_masterpty_drop() {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
        use std::io::Read;
        use std::os::fd::FromRawFd;

        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("sleep 0.2; printf MARKER");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        let master_fd = pair.master.as_raw_fd().expect("master fd");
        let dup_fd = unsafe { libc::dup(master_fd) };
        assert!(dup_fd >= 0, "dup failed");
        // The whole point: drop the MasterPty (closes the fd it owned) while
        // our dup still references the same OFD.
        drop(pair.master);

        let mut file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
        let mut got = String::new();
        let mut buf = [0u8; 256];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break, // EOF — child gone / pty closed
                Ok(n) => {
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if got.contains("MARKER") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = child.wait();
        assert!(
            got.contains("MARKER"),
            "child did not survive MasterPty drop / output never reached the lone dup; got {got:?}"
        );
    }
}

#[tauri::command]
pub async fn pty_kill(manager: State<'_, PtyManager>, id: u32) -> AppResult<()> {
    let task = manager
        .ptys
        .get(&id)
        .and_then(|entry| entry.task_exit.as_ref().map(|_| entry.value().clone()));
    let target = task.or_else(|| manager.ptys.remove(&id).map(|(_, pty)| pty));
    if let Some(pty) = target {
        pty.report_exit.store(false, Ordering::Release);
        // Notify any remaining subscribers so their xterms render
        // "[process exited]" before the unmount tears them down.
        if let Ok(subs) = pty.subscribers.lock() {
            for ch in subs.values() {
                let _ = ch.send(Vec::new());
            }
        }
        // Killing without wait() leaves zombies. Do the potentially-slow
        // SIGTERM grace + SIGKILL backstop on the blocking pool, not on the
        // async runtime worker.
        tauri::async_runtime::spawn_blocking(move || {
            let status = if let Ok(mut child) = pty.child.lock() {
                // Read the completion stamp only after taking the child lock.
                // The natural waiter publishes it before releasing this lock,
                // closing the stale-pid race with a concurrent explicit kill.
                let force_task_tree = task_process_needs_force_backstop(
                    pty.task_exit.is_some(),
                    pty.task_exited_at_ms.load(Ordering::Acquire),
                );
                terminate_and_reap_child(&mut child, force_task_tree)
            } else {
                None
            };
            notify_task_process_exited(&pty, status.as_ref());
        })
        .await
        .map_err(|e| AppError::Pty(format!("pty_kill join: {e}")))?;
    }
    Ok(())
}

#[tauri::command]
pub fn harness_task_output(
    manager: State<'_, PtyManager>,
    id: u32,
    cursor: u64,
    limit: usize,
) -> Result<crate::harness::OutputPage, String> {
    let pty = manager
        .ptys
        .get(&id)
        .ok_or("Task output expired or task no longer exists")?;
    if pty.task_exit.is_none() {
        return Err("PTY is not a managed task".into());
    }
    let result = pty
        .harness_output
        .lock()
        .map_err(|_| "output lock poisoned")?
        .read(cursor, limit);
    result
}
