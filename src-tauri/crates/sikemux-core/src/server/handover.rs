//! Replacing a core with another binary in the same process. The old core
//! writes what it holds to a private directory, keeps the descriptors its
//! sessions and listeners need open across `exec`, and starts the new binary
//! as `core --resume <directory>`. Keeping the pid keeps every shell its
//! child, so their exits are still reported.
//!
//! The command line repeats the descriptors and sessions, so a replacement
//! that cannot read the directory still keeps every shell it can find.

use std::collections::{HashMap, HashSet};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{Child, ChildKiller, ExitStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_pty::output_log::OutputLog;
use sikemux_pty::screen::{replay_snapshot, restored_parser};
use sikemux_pty::shell::ShellLaunchIntegration;
use sikemux_pty::shell_protocol::ShellMetadataSnapshot;
use tokio::io::unix::AsyncFd;

use crate::protocol::frozen::RESUME_FORMAT;
use crate::protocol::{BuildIdentity, SessionExit, SessionId, SessionKind};

use super::agent::{self, AgentActivity, AgentRecord};
use super::chat::{self, ChatRecord};
use super::harness::HarnessRecord;
use super::prepare::Owner;
use super::session::{self, Session, SessionParts};
use super::{
    continue_clock, lock_path, now_ms, run_core, tools, Core, CoreError, CoreResult, ServerConfig,
    ServerError,
};

const STATE_FILE: &str = "state.json";
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoverState {
    format: u32,
    from_build: BuildIdentity,
    uptime_ms: u64,
    socket: PathBuf,
    listener_fd: RawFd,
    lock_fd: RawFd,
    idle_exit_ms: u64,
    cli_endpoint: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    manifest_dir: Option<PathBuf>,
    tools: Option<ToolsRecord>,
    next_session_id: u64,
    next_call_id: u64,
    agent_sequence: u64,
    /// Read one at a time, so a damaged entry costs only its own session's
    /// screen and details.
    sessions: Vec<Value>,
    harness: Value,
    /// Chats a hand-over stops, which the replacement starts again on their
    /// provider sessions. Added in format 2.
    chats: Vec<ChatRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolsRecord {
    path: PathBuf,
    fd: RawFd,
    token: String,
}

/// A session's screen and task output travel in files beside the state:
/// `<id>.replay` and `<id>.output`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    id: SessionId,
    kind: SessionKind,
    fd: RawFd,
    pid: Option<u32>,
    rows: u16,
    cols: u16,
    shell_protocol: bool,
    shell: Option<ShellMetadataSnapshot>,
    window_title: String,
    owner: Owner,
    killed: bool,
    exit: Option<SessionExit>,
    exited_at_ms: u64,
    last_activity_ms: u64,
    trimmed: bool,
    task_output_end: Option<u64>,
    agent: Option<AgentRecord>,
    shell_files: Option<PathBuf>,
}

fn blob_path(directory: &Path, id: SessionId, kind: &str) -> PathBuf {
    directory.join(format!("{id}.{kind}"))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn private_directory() -> std::io::Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "sikemux-core-handover-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    DirBuilder::new().mode(0o700).create(&directory)?;
    Ok(directory)
}

fn capture(session: &Session, directory: &Path) -> CoreResult<SessionRecord> {
    let mut parser = session.parser.lock().map_err(CoreError::poisoned)?;
    let (rows, cols) = parser.screen().size();
    let replay = replay_snapshot(&mut parser)?;
    let shell = parser
        .callbacks()
        .shell
        .as_ref()
        .map(|shell| shell.snapshot());
    let window_title = parser.callbacks().window_title.clone();
    drop(parser);
    write_private(&blob_path(directory, session.id, "replay"), &replay)?;
    let task_output_end = match session.task_log() {
        Some((bytes, end)) => {
            write_private(&blob_path(directory, session.id, "output"), &bytes)?;
            Some(end)
        }
        None => None,
    };
    Ok(SessionRecord {
        id: session.id,
        kind: session.kind,
        fd: session.fd(),
        pid: session.pid(),
        rows,
        cols,
        shell_protocol: session.shell_protocol,
        shell,
        window_title,
        owner: session.owner().clone(),
        killed: session.is_killed(),
        exit: session.exit_reported().then(|| session.exit()).flatten(),
        exited_at_ms: session
            .exited_at_ms
            .load(std::sync::atomic::Ordering::Acquire),
        last_activity_ms: session
            .last_activity_ms
            .load(std::sync::atomic::Ordering::Acquire),
        trimmed: session.trimmed.load(std::sync::atomic::Ordering::Acquire),
        task_output_end,
        agent: session.agent.as_ref().map(AgentActivity::record),
        shell_files: session.shell_files().map(Path::to_path_buf),
    })
}

fn set_close_on_exec(fd: RawFd, close: bool) {
    // SAFETY: F_GETFD and F_SETFD read and set one descriptor's flags and
    // touch no memory; an fd that is not open makes them fail harmlessly.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 {
            return;
        }
        let flags = if close {
            flags | libc::FD_CLOEXEC
        } else {
            flags & !libc::FD_CLOEXEC
        };
        libc::fcntl(fd, libc::F_SETFD, flags);
    }
}

/// Writes everything down and replaces the process with `binary`. Returns
/// only if that failed, with every descriptor as it was.
pub(crate) fn replace_process(core: &Core, binary: &Path, chats: &[ChatRecord]) -> CoreError {
    let Some(listening) = core.listening.get() else {
        return "the core is not listening yet".into();
    };
    let directory = match private_directory() {
        Ok(directory) => directory,
        Err(error) => return error.into(),
    };
    let mut sessions = core.all_sessions();
    sessions.sort_by_key(|session| session.id);
    // Held until the exec, so nothing reaps a process whose exit the
    // replacement has to report. A waiter that already holds one is reaping
    // it now; the replacement reports that exit without its code.
    let held: Vec<_> = sessions
        .iter()
        .map(|session| session.hold_child())
        .collect();
    let mut records = Vec::new();
    for session in &sessions {
        match capture(session, &directory).and_then(|record| Ok(serde_json::to_value(record)?)) {
            Ok(record) => records.push(record),
            Err(error) => eprintln!(
                "sikemux core: session {} keeps running but loses its screen in the update: {error}",
                session.id
            ),
        }
    }
    let tools = core.tools.lock().ok().and_then(|tools| {
        tools.as_ref().map(|tools| ToolsRecord {
            path: tools.path().to_path_buf(),
            fd: tools.fd(),
            token: tools.token().to_owned(),
        })
    });
    let config = &listening.config;
    let state = HandoverState {
        format: RESUME_FORMAT,
        from_build: core.build.clone(),
        uptime_ms: now_ms(),
        socket: config.socket.clone(),
        listener_fd: listening.listener_fd,
        lock_fd: listening.lock_fd,
        idle_exit_ms: config.idle_exit.as_millis() as u64,
        cli_endpoint: config.cli_endpoint.clone(),
        data_dir: config.data_dir.clone(),
        manifest_dir: core.manifest_dir(),
        tools,
        next_session_id: core
            .next_session_id
            .load(std::sync::atomic::Ordering::Acquire),
        next_call_id: core.window.call_mark(),
        agent_sequence: agent::sequence_mark(),
        sessions: records,
        harness: serde_json::to_value(core.harness.record()).unwrap_or(Value::Null),
        chats: chats.to_vec(),
    };
    let written = serde_json::to_vec(&state)
        .map_err(std::io::Error::other)
        .and_then(|bytes| write_private(&directory.join(STATE_FILE), &bytes));
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&directory);
        return error.into();
    }

    let mut command = sikemux_process::user_environment::command(binary);
    command
        .arg("core")
        .arg("--resume")
        .arg(&directory)
        .arg("--socket")
        .arg(&config.socket)
        .arg("--listen-fd")
        .arg(listening.listener_fd.to_string())
        .arg("--lock-fd")
        .arg(listening.lock_fd.to_string())
        .arg("--idle-exit-ms")
        .arg(config.idle_exit.as_millis().to_string());
    if let Some(endpoint) = config.cli_endpoint.as_ref() {
        command.arg("--cli-endpoint").arg(endpoint);
    }
    if let Some(data_dir) = config.data_dir.as_ref() {
        command.arg("--data-dir").arg(data_dir);
    }
    let mut keep = vec![listening.listener_fd, listening.lock_fd];
    if let Some(tools) = state.tools.as_ref() {
        command.arg("--tools-fd").arg(tools.fd.to_string());
        keep.push(tools.fd);
    }
    for session in &sessions {
        let pid = session
            .pid()
            .map_or_else(|| "-".to_owned(), |pid| pid.to_string());
        command
            .arg("--session")
            .arg(format!("{}:{}:{pid}", session.id, session.fd()));
        keep.push(session.fd());
    }
    for fd in &keep {
        set_close_on_exec(*fd, false);
    }
    eprintln!(
        "sikemux core: handing {} sessions and {} chats over to {}",
        sessions.len(),
        chats.len(),
        binary.display()
    );
    let error = command.exec();
    for fd in &keep {
        set_close_on_exec(*fd, true);
    }
    let _ = std::fs::remove_dir_all(&directory);
    drop(held);
    CoreError::from(format!("could not start {}: {error}", binary.display()))
}

/// What a replacement core is told on its command line.
pub(crate) struct Recovery {
    pub directory: PathBuf,
    pub socket: Option<PathBuf>,
    pub listener_fd: Option<RawFd>,
    pub lock_fd: Option<RawFd>,
    pub tools_fd: Option<RawFd>,
    pub cli_endpoint: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub idle_exit: Duration,
    pub sessions: Vec<(SessionId, RawFd, Option<u32>)>,
    /// The listed descriptors that were open when the process started, with
    /// their file type. Only these are adopted or closed: anything opened
    /// since may have reused a number that was listed but already closed.
    pub inherited: HashMap<RawFd, libc::mode_t>,
}

pub(crate) fn file_type(fd: RawFd) -> Option<libc::mode_t> {
    if fd <= 2 {
        return None;
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes one stat struct into the buffer it is given and
    // fails without writing for a descriptor that is not open.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: fstat succeeded, so it filled the struct.
    let stat = unsafe { stat.assume_init() };
    Some(stat.st_mode & libc::S_IFMT)
}

impl Recovery {
    pub(crate) fn listed_fds(&self) -> Vec<RawFd> {
        self.listener_fd
            .into_iter()
            .chain(self.lock_fd)
            .chain(self.tools_fd)
            .chain(self.sessions.iter().map(|(_, fd, _)| *fd))
            .collect()
    }
}

struct Inherited {
    open: HashMap<RawFd, libc::mode_t>,
    claimed: HashSet<RawFd>,
}

impl Inherited {
    /// Takes ownership of a descriptor handed over as `kind`, once.
    fn claim(&mut self, fd: RawFd, kind: libc::mode_t) -> bool {
        if self.open.get(&fd) != Some(&kind) || !self.claimed.insert(fd) {
            return false;
        }
        set_close_on_exec(fd, true);
        true
    }

    /// Closes what was handed over but not taken.
    fn close_unclaimed(&self) {
        for fd in self.open.keys() {
            if !self.claimed.contains(fd) {
                eprintln!("sikemux core: closing descriptor {fd}, which the update could not use");
                // SAFETY: the descriptor was open when this process started
                // and nothing here took it, so closing it frees nobody's fd.
                unsafe {
                    libc::close(*fd);
                }
            }
        }
    }
}

/// Brings state an older core wrote up to the current format, one format at
/// a time.
fn migrate(mut state: Value) -> Result<Value, String> {
    loop {
        let format = state
            .get("format")
            .and_then(Value::as_u64)
            .ok_or("it names no format")?;
        match format {
            1 => {
                // Format 1 cores held no chats.
                state["chats"] = Value::Array(Vec::new());
                state["format"] = Value::from(2);
            }
            2 => {
                // Format 2 cores were reached only by the app on this Mac.
                for list in ["sessions", "chats"] {
                    let entries = state.get_mut(list).and_then(Value::as_array_mut);
                    for entry in entries.into_iter().flatten() {
                        let holder = if list == "sessions" {
                            entry.get_mut("owner")
                        } else {
                            Some(entry)
                        };
                        if let Some(holder) = holder.and_then(Value::as_object_mut) {
                            holder.insert("startedBy".into(), Value::Null);
                        }
                    }
                }
                state["format"] = Value::from(3);
            }
            3 => {
                // Format 3 chats were started without a launcher.
                let chats = state.get_mut("chats").and_then(Value::as_array_mut);
                for chat in chats.into_iter().flatten() {
                    if let Some(chat) = chat.as_object_mut() {
                        chat.insert("launcher".into(), Value::Null);
                    }
                }
                state["format"] = Value::from(4);
            }
            4 => return Ok(state),
            other => return Err(format!("it is in format {other}, not {RESUME_FORMAT}")),
        }
    }
}

fn read_state(directory: &Path) -> Option<HandoverState> {
    let bytes = match std::fs::read(directory.join(STATE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("sikemux core: UPDATE STATE UNREADABLE ({error}); keeping the sessions named on the command line without their screens");
            return None;
        }
    };
    let state = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| error.to_string())
        .and_then(migrate)
        .and_then(|state| {
            serde_json::from_value::<HandoverState>(state).map_err(|error| error.to_string())
        });
    match state {
        Ok(state) => Some(state),
        Err(error) => {
            eprintln!("sikemux core: UPDATE STATE DAMAGED ({error}); keeping the sessions named on the command line without their screens");
            None
        }
    }
}

fn adopt_lock(
    fd: Option<RawFd>,
    socket: &Path,
    inherited: &mut Inherited,
) -> std::io::Result<File> {
    if let Some(fd) = fd.filter(|fd| inherited.claim(*fd, libc::S_IFREG)) {
        // SAFETY: the descriptor was handed over open and claimed once, so
        // this File is its only owner.
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    eprintln!("sikemux core: the single-instance lock was not handed over; taking it again");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path(socket))?;
    // SAFETY: flock only reads the integer fd, which `lock` keeps open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("sikemux core: another process holds the single-instance lock");
    }
    Ok(lock)
}

fn adopt_listener(
    fd: Option<RawFd>,
    socket: &Path,
    inherited: &mut Inherited,
) -> std::io::Result<std::os::unix::net::UnixListener> {
    if let Some(fd) = fd.filter(|fd| inherited.claim(*fd, libc::S_IFSOCK)) {
        // SAFETY: the descriptor was handed over open and claimed once, so
        // this listener is its only owner.
        return Ok(unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) });
    }
    eprintln!(
        "sikemux core: the socket was not handed over; listening at {} again",
        socket.display()
    );
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn terminal_io(fd: RawFd, inherited: &mut Inherited) -> Result<AsyncFd<File>, String> {
    if !inherited.claim(fd, libc::S_IFCHR) {
        return Err(format!(
            "descriptor {fd} is not a terminal that was handed over"
        ));
    }
    // SAFETY: the descriptor was handed over open and claimed once, so this
    // File is its only owner and closes it if adopting fails below.
    let file = unsafe { File::from_raw_fd(fd) };
    // SAFETY: F_GETFL/F_SETFL only read and set the flags of the open fd.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    AsyncFd::new(file).map_err(|error| error.to_string())
}

fn adopt_session(
    core: &Arc<Core>,
    record: SessionRecord,
    directory: &Path,
    inherited: &mut Inherited,
) -> Result<(), String> {
    let io = terminal_io(record.fd, inherited)?;
    let permit = core
        .capacity
        .try_acquire()
        .map_err(|error| error.to_string())?;
    let replay = std::fs::read(blob_path(directory, record.id, "replay")).unwrap_or_else(|error| {
        eprintln!(
            "sikemux core: session {} lost its screen in the update: {error}",
            record.id
        );
        Vec::new()
    });
    let task_log = record.task_output_end.map(|end| {
        let bytes = std::fs::read(blob_path(directory, record.id, "output")).unwrap_or_default();
        OutputLog::restored(&bytes, end)
    });
    let agent = record.agent.as_ref().map_or_else(
        || {
            AgentActivity::new(
                record.owner.agent_id.as_deref(),
                record.owner.agent_type.as_deref(),
                false,
            )
        },
        |agent| {
            AgentActivity::restored(
                record.owner.agent_id.as_deref(),
                record.owner.agent_type.as_deref(),
                agent,
            )
        },
    );
    let rows = if record.rows == 0 {
        DEFAULT_ROWS
    } else {
        record.rows
    };
    let cols = if record.cols == 0 {
        DEFAULT_COLS
    } else {
        record.cols
    };
    let child = AdoptedChild::new(record.pid, record.exit.as_ref());
    let (session, input_jobs) = Session::assemble(
        SessionParts {
            id: record.id,
            kind: record.kind,
            pid: record.pid,
            owner: record.owner,
            parser: restored_parser(rows, cols, &replay, record.shell, record.window_title),
            shell_protocol: record.shell_protocol,
            last_activity_ms: record.last_activity_ms,
            trimmed: record.trimmed,
            killed: record.killed,
            exit: record.exit,
            exited_at_ms: record.exited_at_ms,
            task_log,
            agent,
            shell_integration: record
                .shell_files
                .map(|files| ShellLaunchIntegration::adopt(Some(files))),
        },
        io,
        Box::new(child),
        permit,
    );
    core.insert_session(session.clone());
    session::start_adopted(core, session, input_jobs);
    Ok(())
}

/// A session known only from the command line: its terminal and process
/// carry on, with an empty screen and no details.
fn adopt_bare(
    core: &Arc<Core>,
    id: SessionId,
    fd: RawFd,
    pid: Option<u32>,
    inherited: &mut Inherited,
) -> Result<(), String> {
    let io = terminal_io(fd, inherited)?;
    let permit = core
        .capacity
        .try_acquire()
        .map_err(|error| error.to_string())?;
    let (rows, cols) = window_size(fd).unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
    let (session, input_jobs) = Session::assemble(
        SessionParts {
            id,
            kind: SessionKind::Terminal,
            pid,
            owner: Owner::default(),
            parser: restored_parser(rows, cols, &[], None, String::new()),
            shell_protocol: false,
            last_activity_ms: now_ms(),
            trimmed: false,
            killed: false,
            exit: None,
            exited_at_ms: 0,
            task_log: None,
            agent: None,
            shell_integration: None,
        },
        io,
        Box::new(AdoptedChild::new(pid, None)),
        permit,
    );
    core.insert_session(session.clone());
    session::start_adopted(core, session, input_jobs);
    Ok(())
}

fn window_size(fd: RawFd) -> Option<(u16, u16)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one winsize into the live struct on the stack.
    let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size as *mut libc::winsize) };
    (rc == 0 && size.ws_row > 0 && size.ws_col > 0).then_some((size.ws_row, size.ws_col))
}

/// Starts a core from what an earlier one handed over, then serves.
pub(crate) async fn resume(recovery: Recovery, build: BuildIdentity) -> Result<(), ServerError> {
    let state = read_state(&recovery.directory);
    if let Some(state) = state.as_ref() {
        continue_clock(state.uptime_ms);
    }
    let socket = state
        .as_ref()
        .map(|state| state.socket.clone())
        .or_else(|| recovery.socket.clone())
        .ok_or_else(|| std::io::Error::other("the update named no socket"))?;
    let config = ServerConfig {
        socket,
        idle_exit: state.as_ref().map_or(recovery.idle_exit, |state| {
            Duration::from_millis(state.idle_exit_ms)
        }),
        build: build.clone(),
        cli_endpoint: state.as_ref().map_or_else(
            || recovery.cli_endpoint.clone(),
            |state| state.cli_endpoint.clone(),
        ),
        data_dir: state
            .as_ref()
            .map_or_else(|| recovery.data_dir.clone(), |state| state.data_dir.clone()),
        remote_direct_only: false,
    };
    let core = Core::new(build, config.data_dir.as_deref())?;
    let mut inherited = Inherited {
        open: recovery.inherited.clone(),
        claimed: HashSet::new(),
    };
    let lock = adopt_lock(
        state
            .as_ref()
            .map(|state| state.lock_fd)
            .or(recovery.lock_fd),
        &config.socket,
        &mut inherited,
    )?;
    let listener = adopt_listener(
        state
            .as_ref()
            .map(|state| state.listener_fd)
            .or(recovery.listener_fd),
        &config.socket,
        &mut inherited,
    )?;

    let mut from = "an unknown build".to_owned();
    let mut tools_record = None;
    let mut chats = Vec::new();
    if let Some(state) = state {
        from = format!("{} {}", state.from_build.version, state.from_build.commit);
        core.next_session_id
            .fetch_max(state.next_session_id, std::sync::atomic::Ordering::AcqRel);
        core.window.continue_calls(state.next_call_id);
        agent::continue_sequence(state.agent_sequence);
        if state.manifest_dir.is_some() {
            let configuring = core.clone();
            let manifest_dir = state.manifest_dir.clone();
            let configured =
                tokio::task::spawn_blocking(move || configuring.configure_manifests(manifest_dir))
                    .await;
            if !matches!(configured, Ok(Ok(_))) {
                eprintln!("sikemux core: the person's agent detection rules did not load after the update");
            }
        }
        match serde_json::from_value::<HarnessRecord>(state.harness) {
            Ok(record) => core.harness.restore(record),
            Err(error) => eprintln!(
                "sikemux core: UPDATE LOST THE HARNESS RUNS ({error}); task sessions keep running"
            ),
        }
        for entry in state.sessions {
            let record = match serde_json::from_value::<SessionRecord>(entry) {
                Ok(record) => record,
                Err(error) => {
                    eprintln!("sikemux core: a session's details were damaged in the update ({error}); it is kept without them");
                    continue;
                }
            };
            let id = record.id;
            if let Err(error) = adopt_session(&core, record, &recovery.directory, &mut inherited) {
                eprintln!("sikemux core: SESSION {id} WAS NOT KEPT IN THE UPDATE: {error}");
            }
        }
        tools_record = state.tools;
        chats = state.chats;
    }
    for (id, fd, pid) in recovery.sessions.iter().copied() {
        if inherited.claimed.contains(&fd) || core.session(id).is_some() {
            continue;
        }
        if let Err(error) = adopt_bare(&core, id, fd, pid, &mut inherited) {
            eprintln!("sikemux core: SESSION {id} WAS NOT KEPT IN THE UPDATE: {error}");
        }
    }

    let tools_fd = tools_record
        .as_ref()
        .map(|record| record.fd)
        .or(recovery.tools_fd);
    let mut endpoint = None;
    if let (Some(record), Some(fd)) = (tools_record, tools_fd) {
        if inherited.claim(fd, libc::S_IFSOCK) {
            // SAFETY: the descriptor was handed over open and claimed once, so
            // this listener is its only owner.
            let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            match tools::ToolEndpoint::adopt(core.clone(), listener, record.path, record.token)
                .await
            {
                Ok(adopted) => endpoint = Some(adopted),
                Err(error) => eprintln!(
                    "sikemux core: the agents' tool endpoint was not kept in the update: {error}"
                ),
            }
        }
    }
    inherited.close_unclaimed();
    if endpoint.is_none() {
        if let Some(path) = config.cli_endpoint.clone() {
            match tools::ToolEndpoint::start(core.clone(), path).await {
                Ok(started) => endpoint = Some(started),
                Err(error) => eprintln!("sikemux core: agents' tools are unavailable: {error}"),
            }
        }
    }
    if let Ok(mut slot) = core.tools.lock() {
        *slot = endpoint;
    }
    if let Err(error) = std::fs::remove_dir_all(&recovery.directory) {
        eprintln!(
            "sikemux core: could not remove {}: {error}",
            recovery.directory.display()
        );
    }
    eprintln!(
        "sikemux core: took over from {from} with {} sessions and {} chats",
        core.all_sessions().len(),
        chats.len()
    );
    for record in chats {
        chat::resume(&core, record);
    }
    run_core(core, listener, lock, config).await
}

/// The handle of a process this core inherited from the one it replaced. It
/// is still this process's child, so it is waited for like one.
#[derive(Debug)]
pub(crate) struct AdoptedChild {
    pid: Option<u32>,
    status: Option<ExitStatus>,
}

#[derive(Debug)]
struct AdoptedKiller(Option<u32>);

fn exit_status(exit: &SessionExit) -> ExitStatus {
    match exit.signal.as_deref() {
        Some(signal) => ExitStatus::with_signal(signal),
        None => ExitStatus::with_exit_code(exit.code.unwrap_or(1)),
    }
}

fn hang_up(pid: Option<u32>, signal: libc::c_int) -> std::io::Result<()> {
    let Some(pid) = pid.filter(|pid| *pid > 0) else {
        return Ok(());
    };
    // SAFETY: kill only takes integers; the pid is positive, so this never
    // signals a process group or every process.
    if unsafe { libc::kill(pid as libc::pid_t, signal) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl AdoptedChild {
    fn new(pid: Option<u32>, exit: Option<&SessionExit>) -> Self {
        Self {
            pid,
            status: exit.map(exit_status),
        }
    }

    fn reap(&mut self, options: libc::c_int) -> std::io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status.as_ref() {
            return Ok(Some(status.clone()));
        }
        let Some(pid) = self.pid else {
            return Err(std::io::Error::other("the process id is unknown"));
        };
        loop {
            let mut raw = 0;
            // SAFETY: waitpid writes one int status into `raw`.
            let reaped = unsafe { libc::waitpid(pid as libc::pid_t, &mut raw, options) };
            if reaped == 0 {
                return Ok(None);
            }
            if reaped < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            let status = ExitStatus::from(std::process::ExitStatus::from_raw(raw));
            self.status = Some(status.clone());
            return Ok(Some(status));
        }
    }
}

impl ChildKiller for AdoptedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        hang_up(self.pid, libc::SIGHUP)?;
        for attempt in 0..5 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Ok(Some(_)) = self.reap(libc::WNOHANG) {
                return Ok(());
            }
        }
        hang_up(self.pid, libc::SIGKILL)
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller(self.pid))
    }
}

impl ChildKiller for AdoptedKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        hang_up(self.0, libc::SIGHUP)
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller(self.0))
    }
}

impl Child for AdoptedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.reap(0)?
            .ok_or_else(|| std::io::Error::other("the process did not exit"))
    }

    fn process_id(&self) -> Option<u32> {
        self.pid
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::migrate;

    #[test]
    fn format_one_state_gains_an_empty_chat_list() {
        let migrated = migrate(json!({ "format": 1, "sessions": [] })).expect("migrate");
        assert_eq!(migrated["format"], 4);
        assert_eq!(migrated["chats"], json!([]));
        assert_eq!(migrated["sessions"], json!([]));
    }

    #[test]
    fn format_two_sessions_and_chats_were_started_by_the_app() {
        let migrated = migrate(json!({
            "format": 2,
            "sessions": [{ "id": 1, "owner": { "project": "/p" } }],
            "chats": [{ "launch": {} }],
        }))
        .expect("migrate");
        assert_eq!(migrated["format"], 4);
        assert_eq!(
            migrated["sessions"][0]["owner"],
            json!({ "project": "/p", "startedBy": null })
        );
        assert_eq!(
            migrated["chats"][0],
            json!({ "launch": {}, "startedBy": null, "launcher": null })
        );
    }

    #[test]
    fn current_state_is_left_as_it_is_and_unknown_formats_are_refused() {
        let current = json!({
            "format": 4,
            "chats": [{ "launch": {}, "startedBy": "phone", "launcher": "claude" }],
        });
        assert_eq!(migrate(current.clone()).expect("migrate"), current);
        assert!(migrate(json!({ "format": 9 })).is_err());
        assert!(migrate(json!({})).is_err());
    }
}
