use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use portable_pty::{Child, ExitStatus, NativePtySystem, PtySize, PtySystem};

use super::pty_io::{PtyIo, ReadOutcome};
use sikemux_process::sync::Locked;
use sikemux_pty::output_log::OutputLog;
use sikemux_pty::process::SpawnedChildGuard;
use sikemux_pty::screen::Screen;
use sikemux_pty::shell::ShellLaunchIntegration;
use sikemux_pty::shell_protocol::ShellProtocolParser;
use sikemux_pty::task::TaskProcessExit;
use sikemux_pty::terminal::View;
use sikemux_pty::validate_pty_dimensions;
use sikemux_wire::grid::{FindOptions, FindResult, GridPoint, TerminalInput};
use tokio::sync::{mpsc, Notify};

use crate::protocol::{
    encode_grid, encode_snapshot, AttachHeader, Continuation, Event, RequestId, SessionExit,
    SessionId, SessionInfo, SessionKind, StreamId,
};

use super::agent::{self, AgentActivity};
use super::connection::ClientConn;
use super::prepare::{Owner, PreparedLaunch};
use super::{uptime_ms, CapacityPermit, Core, CoreError, CoreResult};

const OUTPUT_COALESCE: Duration = Duration::from_millis(2);
const OUTPUT_BATCH_BYTES: usize = 64 * 1024;
/// The shortest gap between two frames to one client. The first change after
/// a quiet spell goes out at once, so typing shows without waiting.
const FRAME_GAP: Duration = Duration::from_millis(8);
/// Frames a client may have undrawn before it is sent no more. One that
/// falls behind gets the latest screen once it catches up, never a backlog.
const MAX_FRAMES_IN_FLIGHT: u32 = 2;
pub(crate) const MAX_SUBSCRIBERS_PER_SESSION: usize = 16;
const TASK_OUTPUT_NOTICE_DELAY: Duration = Duration::from_millis(50);

static NEXT_STREAM: AtomicU32 = AtomicU32::new(1);

struct LineWaiter {
    containing: String,
    found: tokio::sync::oneshot::Sender<Option<String>>,
}

pub(crate) struct Subscriber {
    client: Arc<ClientConn>,
    view: View,
    in_flight: u32,
    sent_at: Option<Instant>,
}

/// Wakes a session's frame pump, which holds the session only while it
/// sends, so a session that goes away also ends its pump.
#[derive(Default)]
pub(crate) struct FrameSignal {
    notify: Notify,
    closed: AtomicBool,
}

pub(crate) struct InputJob {
    bytes: Vec<u8>,
    reply: Box<dyn FnOnce(std::io::Result<()>) + Send>,
}

pub(crate) struct TaskOutput {
    log: Mutex<OutputLog>,
    notice_pending: AtomicBool,
}

pub(crate) struct Session {
    pub id: SessionId,
    pub kind: SessionKind,
    io: PtyIo,
    input: mpsc::UnboundedSender<InputJob>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    pid: Option<u32>,
    owner: Owner,
    pub(crate) screen: Mutex<Screen>,
    pub(crate) shell_protocol: bool,
    subscribers: Mutex<HashMap<StreamId, Subscriber>>,
    frames: Arc<FrameSignal>,
    pub(crate) last_activity_ms: AtomicU64,
    pub(crate) trimmed: AtomicBool,
    killed: AtomicBool,
    exit_reported: AtomicBool,
    exited: AtomicBool,
    task: Option<TaskOutput>,
    /// Zero while the process runs; when it was reaped otherwise. Completed
    /// tasks, and terminals that ended with nobody watching, stay attachable
    /// for a while.
    pub(crate) exited_at_ms: AtomicU64,
    exit: Mutex<Option<SessionExit>>,
    stop_reader: Notify,
    /// Present for a terminal launched for an agent.
    pub(crate) agent: Option<AgentActivity>,
    /// The reader stopped for a hand-over that then failed, so it has to be
    /// started again.
    reader_frozen: AtomicBool,
    shell_integration: Option<ShellLaunchIntegration>,
    /// A title change waits to be sent, with any that follow it soon after.
    title_pending: AtomicBool,
    /// Callers waiting for a line containing some text to show on the screen.
    line_waiters: Mutex<Vec<LineWaiter>>,
    /// A draft to type once the shell shows its prompt, and who typed it.
    typed_at_prompt: Mutex<Option<(String, Typist)>>,
    /// Setting a host up over SSH: its output carries the sign-in code the core
    /// approves for the account, so only the person's own app types into it.
    setup: AtomicBool,
    _capacity_permit: CapacityPermit,
}

/// Who types into a terminal: the person's own app on this host, the terminal
/// answering its program's own queries, or anyone else, the core's own typing
/// included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Typist {
    App,
    Terminal,
    Other,
}

pub(crate) const SETUP_LOCKED: &str =
    "this terminal is setting a host up, and only the Sikemux window types into it";

/// Programs that animate their title, such as a spinner, send at most one
/// change per this long.
const TITLE_GAP: Duration = Duration::from_millis(200);

/// Everything a session is made of besides its process handle and terminal,
/// for a session spawned here or adopted from an earlier core.
pub(crate) struct SessionParts {
    pub id: SessionId,
    pub kind: SessionKind,
    pub pid: Option<u32>,
    pub owner: Owner,
    pub screen: Screen,
    pub shell_protocol: bool,
    pub last_activity_ms: u64,
    pub trimmed: bool,
    pub killed: bool,
    pub exit: Option<SessionExit>,
    pub exited_at_ms: u64,
    pub task_log: Option<OutputLog>,
    pub agent: Option<AgentActivity>,
    pub shell_integration: Option<ShellLaunchIntegration>,
    pub setup: bool,
}

impl Session {
    pub(crate) fn assemble(
        parts: SessionParts,
        io: PtyIo,
        child: Box<dyn Child + Send + Sync>,
        capacity_permit: CapacityPermit,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<InputJob>) {
        let (input, input_jobs) = mpsc::unbounded_channel();
        let reported = parts.exit.is_some();
        if let (Some(pid), Some(agent_id), false) = (parts.pid, &parts.owner.agent_id, reported) {
            super::callers::started(pid, agent_id);
        }
        let session = Arc::new(Session {
            id: parts.id,
            kind: parts.kind,
            io,
            input,
            child: Mutex::new(child),
            pid: parts.pid,
            owner: parts.owner,
            screen: Mutex::new(parts.screen),
            shell_protocol: parts.shell_protocol,
            subscribers: Mutex::new(HashMap::new()),
            frames: Arc::default(),
            last_activity_ms: AtomicU64::new(parts.last_activity_ms),
            trimmed: AtomicBool::new(parts.trimmed),
            killed: AtomicBool::new(parts.killed),
            exit_reported: AtomicBool::new(reported),
            exited: AtomicBool::new(reported),
            task: parts.task_log.map(|log| TaskOutput {
                log: Mutex::new(log),
                notice_pending: AtomicBool::new(false),
            }),
            exited_at_ms: AtomicU64::new(parts.exited_at_ms),
            exit: Mutex::new(parts.exit),
            stop_reader: Notify::new(),
            agent: parts.agent,
            reader_frozen: AtomicBool::new(false),
            shell_integration: parts.shell_integration,
            title_pending: AtomicBool::new(false),
            line_waiters: Mutex::new(Vec::new()),
            typed_at_prompt: Mutex::new(None),
            setup: AtomicBool::new(parts.setup),
            _capacity_permit: capacity_permit,
        });
        (session, input_jobs)
    }

    pub(crate) fn may_type(&self, typist: Typist) -> bool {
        typist != Typist::Other || !self.in_setup()
    }

    pub(crate) fn in_setup(&self) -> bool {
        self.setup.load(Ordering::Acquire)
    }

    /// The core has read the sign-in code, or the set-up ended without one.
    pub(crate) fn end_setup(&self) {
        self.setup.store(false, Ordering::Release);
    }

    pub(crate) fn owner(&self) -> &Owner {
        &self.owner
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    #[cfg(unix)]
    pub(crate) fn fd(&self) -> std::os::fd::RawFd {
        self.io.fd()
    }

    pub(crate) fn shell_files(&self) -> Option<&std::path::Path> {
        self.shell_integration
            .as_ref()
            .and_then(ShellLaunchIntegration::files_directory)
    }

    pub(crate) fn is_killed(&self) -> bool {
        self.killed.load(Ordering::Acquire)
    }

    pub(crate) fn exit_reported(&self) -> bool {
        self.exit_reported.load(Ordering::Acquire)
    }

    pub(crate) fn exit(&self) -> Option<SessionExit> {
        self.exit.locked().clone()
    }

    /// Holds the process handle so nothing reaps or signals it, unless a
    /// waiter already holds it.
    pub(crate) fn hold_child(
        &self,
    ) -> Option<std::sync::MutexGuard<'_, Box<dyn Child + Send + Sync>>> {
        self.child.try_lock().ok()
    }

    pub(crate) fn task_log(&self) -> Option<(Vec<u8>, u64)> {
        let task = self.task.as_ref()?;
        Some(task.log.locked().contents())
    }

    pub(crate) fn is_task(&self) -> bool {
        self.task.is_some()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }

    pub(crate) fn shell_at_prompt(&self) -> bool {
        #[cfg(unix)]
        {
            self.is_running()
                && self
                    .pid
                    .is_some_and(|pid| sikemux_pty::process::shell_at_prompt(self.fd(), pid))
        }
        #[cfg(windows)]
        {
            false
        }
    }

    pub(crate) fn has_subscribers(&self) -> bool {
        !self.subscribers.locked().is_empty()
    }

    /// The SSH config host an ssh terminal connects to, and how it reaches
    /// it while ssh is up, on its way, or gave up.
    pub(crate) fn ssh_link(
        &self,
    ) -> Option<(String, Option<sikemux_pty::shell_protocol::SshLink>)> {
        let host = self.owner.ssh_host.clone()?;
        let link = self
            .screen
            .locked()
            .shell
            .as_ref()
            .and_then(|shell| shell.ssh_reach());
        Some((host, link))
    }

    /// The folder its shell last reported, else the project it was started in.
    pub(crate) fn cwd(&self) -> Option<PathBuf> {
        let reported = self
            .screen
            .locked()
            .shell
            .as_ref()
            .and_then(|shell| shell.snapshot().cwd);
        reported
            .or_else(|| self.owner.project.clone())
            .map(PathBuf::from)
    }

    pub(crate) fn rail_number(&self) -> Option<u32> {
        self.owner.rail_number
    }

    pub(crate) fn info(&self) -> SessionInfo {
        let ((rows, cols), ssh, window_title) = {
            let screen = self.screen.locked();
            let ssh = screen.shell.as_ref().and_then(|shell| shell.ssh());
            let title = Some(screen.window_title.clone()).filter(|title| !title.is_empty());
            (screen.terminal.size(), ssh, title)
        };
        SessionInfo {
            id: self.id,
            kind: self.kind,
            pid: self.pid,
            running: self.is_running(),
            cols,
            rows,
            attached: self.subscribers.locked().len(),
            project: self.owner.project.clone(),
            pane_id: self.owner.pane_id.clone(),
            agent_id: self.owner.agent_id.clone(),
            agent_name: None,
            agent_type: self.owner.agent_type.clone(),
            task_execution_id: self.owner.task_execution_id.clone(),
            agent_state: self
                .agent
                .as_ref()
                .and_then(AgentActivity::state_label)
                .map(str::to_string),
            task: self.owner.task.clone(),
            started_by: self.owner.started_by.clone(),
            exit: self.exit.locked().clone(),
            killed: self.is_killed(),
            title: None,
            window_title,
            unread: false,
            active_at: None,
            group: None,
            status: None,
            attention_rank: None,
            ssh,
            ssh_alias: self.owner.ssh_host.clone(),
            rail_name: self.owner.rail_number.map(|number| number.to_string()),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.frames.closed.store(true, Ordering::Release);
        self.frames.notify.notify_one();
    }
}

/// A session that is listed but whose output is not read yet. Starting it
/// after the spawn reply is queued means a client hears about the session
/// before any of its output or its exit.
pub(crate) struct PendingStart {
    session: Arc<Session>,
    input_jobs: mpsc::UnboundedReceiver<InputJob>,
}

impl PendingStart {
    pub(crate) fn id(&self) -> SessionId {
        self.session.id
    }

    pub(crate) fn task(&self) -> Option<&crate::protocol::TaskSessionInfo> {
        self.session.owner.task.as_ref()
    }

    pub(crate) fn start(self, core: &Arc<Core>) {
        if let Some(agent) = self.session.agent.as_ref() {
            agent::publish_start(core, agent);
        }
        tokio::spawn(write_input(Arc::downgrade(&self.session), self.input_jobs));
        tokio::spawn(pump_frames(
            core.clone(),
            Arc::downgrade(&self.session),
            self.session.frames.clone(),
        ));
        tokio::spawn(read_output(core.clone(), self.session));
        core.ports.changed();
    }
}

/// Starts a session adopted from an earlier core. One whose exit was already
/// reported has nothing left to read.
pub(crate) fn start_adopted(
    core: &Arc<Core>,
    session: Arc<Session>,
    input_jobs: mpsc::UnboundedReceiver<InputJob>,
) {
    tokio::spawn(write_input(Arc::downgrade(&session), input_jobs));
    tokio::spawn(pump_frames(
        core.clone(),
        Arc::downgrade(&session),
        session.frames.clone(),
    ));
    if !session.exit_reported() {
        tokio::spawn(read_output(core.clone(), session));
    }
}

/// Starts the readers a failed hand-over stopped.
pub(crate) fn thaw(core: &Arc<Core>, session: Arc<Session>) {
    if session.reader_frozen.swap(false, Ordering::AcqRel) {
        tokio::spawn(read_output(core.clone(), session));
    }
}

/// Opens the terminal and starts its process. Runs on a blocking thread.
pub(crate) fn spawn_session(core: &Arc<Core>, launch: PreparedLaunch) -> CoreResult<PendingStart> {
    validate_pty_dimensions(launch.cols, launch.rows)?;
    let _launching = core.launching.enter();
    if core.is_frozen() {
        return Err(super::upgrade::UPDATING.into());
    }
    core.reclaim_exited_sessions(uptime_ms());
    let capacity_permit = core.capacity.try_acquire()?;
    let PreparedLaunch {
        cols,
        rows,
        command,
        kind,
        mut owner,
        shell_integration,
        ssh,
        initial_prompt_submitted,
        continues,
        setup,
    } = launch;
    let carried = continues
        .as_ref()
        .and_then(|continuation| carried_screen(core, continuation));
    let (rows, cols) = carried
        .as_ref()
        .map_or((rows, cols), |carried| (carried.rows, carried.cols));
    let pair = NativePtySystem::default()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(CoreError::pty)?;
    let shell_metadata_enabled = shell_integration.is_some() || ssh;

    let child = pair.slave.spawn_command(command).map_err(CoreError::pty)?;
    let pid = child.process_id();
    let child = SpawnedChildGuard::new(child);
    drop(pair.slave);

    let io = PtyIo::open(pair.master)?;

    let id = core.next_session_id.fetch_add(1, Ordering::Relaxed);
    if owner.wants_rail_number(kind) {
        owner.rail_number = Some(core.take_rail_number());
    }
    let agent = AgentActivity::new(
        owner.agent_id.as_deref(),
        owner.agent_type.as_deref(),
        initial_prompt_submitted,
    );
    let mut screen = Screen::new(rows, cols, shell_metadata_enabled);
    if let Some(carried) = carried.as_ref() {
        screen.process(&carried.prelude, &core.terminal_colors());
    }
    let (session, input_jobs) = Session::assemble(
        SessionParts {
            id,
            kind,
            pid,
            owner,
            screen,
            shell_protocol: shell_metadata_enabled,
            last_activity_ms: uptime_ms(),
            trimmed: false,
            killed: false,
            exit: None,
            exited_at_ms: 0,
            task_log: (kind == SessionKind::Task).then(OutputLog::default),
            agent,
            shell_integration,
            setup,
        },
        io,
        child.into_inner(),
        capacity_permit,
    );

    // Publish before starting the reader, so a command that exits at once
    // cannot prune itself before it was ever inserted.
    core.insert_session(session.clone());
    if let Some(carried) = carried {
        core.remove_session(&carried.from);
    }
    Ok(PendingStart {
        session,
        input_jobs,
    })
}

struct CarriedScreen {
    from: Arc<Session>,
    rows: u16,
    cols: u16,
    prelude: Vec<u8>,
}

const MAX_CONTINUATION_NOTE_CHARS: usize = 120;

/// The ended terminal's screen and history, then a dim line with the note
/// below everything it showed. The old program's modes and scroll region are
/// reset first, so nothing it turned on reaches the new one.
fn carried_screen(core: &Core, continuation: &Continuation) -> Option<CarriedScreen> {
    let from = core.session(continuation.session)?;
    if from.is_task() || from.is_running() {
        return None;
    }
    let mut screen = from.screen.locked();
    screen.process(RESET_MODES, &core.terminal_colors());
    screen.process(b"\x1b[r", &core.terminal_colors());
    let (rows, cols) = screen.terminal.size();
    let cursor_row = screen.terminal.cursor().0;
    let last_written = screen.terminal.last_written_row();
    let mut prelude = screen.terminal.replay();
    drop(screen);
    let row = last_written.map_or(cursor_row, |row| row.max(cursor_row));
    prelude.extend_from_slice(
        format!("\x1b[{};1H\x1b[0m\x1b[?25h\r\n\x1b[38;5;245m", row + 1).as_bytes(),
    );
    let note: String = continuation
        .note
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_CONTINUATION_NOTE_CHARS)
        .collect();
    prelude.extend_from_slice(note.as_bytes());
    prelude.extend_from_slice(b"\x1b[0m\r\n");
    Some(CarriedScreen {
        from,
        rows,
        cols,
        prelude,
    })
}

/// One writer per session keeps every client's writes in the order they
/// arrived without holding up the connection that sent them.
async fn write_input(session: Weak<Session>, mut jobs: mpsc::UnboundedReceiver<InputJob>) {
    while let Some(job) = jobs.recv().await {
        let result = match session.upgrade() {
            Some(session) => session.io.write_all(&job.bytes).await,
            None => Err(std::io::Error::other("session closed")),
        };
        (job.reply)(result);
    }
}

/// `text` as typed into `session` to arrive as one paste: between paste
/// markers when its program has turned bracketed paste on, else on one line,
/// since a bare newline would send it.
pub(crate) fn pasted(session: &Session, text: &str) -> Vec<u8> {
    pasted_on(&session.screen.locked(), text)
}

fn pasted_on(screen: &Screen, text: &str) -> Vec<u8> {
    let plain: String = text
        .chars()
        .filter(|char| matches!(char, '\n' | '\t') || !char.is_control())
        .collect();
    if screen.terminal.bracketed_paste() {
        screen
            .terminal
            .encode(&TerminalInput::Paste { text: plain })
    } else {
        plain.replace('\n', " ").into_bytes()
    }
}

pub(crate) fn queue_input(
    session: &Session,
    bytes: Vec<u8>,
    typist: Typist,
    reply: Box<dyn FnOnce(std::io::Result<()>) + Send>,
) {
    if !session.may_type(typist) {
        return reply(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            SETUP_LOCKED,
        )));
    }
    if let Err(mpsc::error::SendError(job)) = session.input.send(InputJob { bytes, reply }) {
        (job.reply)(Err(std::io::Error::other("session closed")));
    }
}

enum PumpEnd {
    Eof,
    /// The core is handing itself over: every byte read so far reached the
    /// screen and the subscribers, and nothing more was read.
    Frozen,
}

/// Reads until EOF. The first byte is read into the screen at once; after
/// that, small writes are gathered for up to 2 ms into one pass.
async fn pump_output(core: &Arc<Core>, session: &Session) -> PumpEnd {
    let mut frozen = core.frozen.subscribe();
    let _pumping = core.pumping.enter();
    let mut buf = vec![0u8; OUTPUT_BATCH_BYTES];
    let mut batch = Vec::with_capacity(OUTPUT_BATCH_BYTES);
    loop {
        if *frozen.borrow_and_update() {
            return PumpEnd::Frozen;
        }
        batch.clear();
        let read = tokio::select! {
            read = session.io.read(&mut buf) => read,
            _ = frozen.changed() => return PumpEnd::Frozen,
        };
        let mut eof = match read {
            ReadOutcome::Bytes(n) => {
                batch.extend_from_slice(buf.get(..n).unwrap_or_default());
                false
            }
            ReadOutcome::Eof => true,
        };
        if !eof && batch.len() < OUTPUT_BATCH_BYTES {
            let deadline = tokio::time::sleep(OUTPUT_COALESCE);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    _ = frozen.changed() => break,
                    read = session.io.read(&mut buf) => match read {
                        ReadOutcome::Bytes(n) => {
                            batch.extend_from_slice(buf.get(..n).unwrap_or_default());
                            if batch.len() >= OUTPUT_BATCH_BYTES {
                                break;
                            }
                        }
                        ReadOutcome::Eof => {
                            eof = true;
                            break;
                        }
                    }
                }
            }
        }
        if !batch.is_empty() {
            broadcast_output(core, session, &batch);
        }
        if eof {
            return PumpEnd::Eof;
        }
    }
}

async fn read_output(core: Arc<Core>, session: Arc<Session>) {
    #[cfg(windows)]
    tokio::spawn(close_when_exited(Arc::downgrade(&session)));
    let ended = tokio::select! {
        ended = pump_output(&core, &session) => Some(ended),
        _ = session.stop_reader.notified() => None,
    };
    match ended {
        None => return,
        Some(PumpEnd::Frozen) => {
            session.reader_frozen.store(true, Ordering::Release);
            return;
        }
        Some(PumpEnd::Eof) => {}
    }
    // Terminals leave when their output ends, unless nobody was watching:
    // then the next client to open one sees how it ended. Completed tasks
    // stay, so a command that finished before its spawn reply arrived can
    // still be read, and so do agents, so one resumed after a crash starts
    // below its last screen.
    if !session.is_task() && session.agent.is_none() && session.has_subscribers() {
        core.remove_session(&session);
    }
    let _ = tokio::task::spawn_blocking(move || {
        let status = {
            let mut child = session.child.locked();
            let status = child.wait().ok();
            // Stamped under the child lock, so a concurrent kill or drain
            // never signals a pid that may already be reused.
            stamp_exited(&session);
            status
        };
        report_exit(&core, &session, status.as_ref());
    })
    .await;
}

/// ConPTY never ends a terminal's output while the terminal is open, so the pipe is closed
/// once the program has gone.
#[cfg(windows)]
async fn close_when_exited(session: Weak<Session>) {
    const LOOK_EVERY: Duration = Duration::from_millis(50);
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        let Some(session) = session.upgrade() else {
            return;
        };
        let exited = matches!(session.child.locked().try_wait(), Ok(Some(_)));
        if exited {
            session.io.close();
            return;
        }
    }
}

pub(crate) fn stamp_exited(session: &Session) -> Option<u64> {
    let exited_at = uptime_ms().max(1);
    match session
        .exited_at_ms
        .compare_exchange(0, exited_at, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => {
            session.last_activity_ms.store(exited_at, Ordering::Release);
            Some(exited_at)
        }
        Err(existing) => Some(existing),
    }
}

/// Tells every client the process is gone. Natural exit, kill and shutdown can
/// race here; only the first one is reported. The event goes out under the
/// screen lock, so an attach knows whether its snapshot came after it.
pub(crate) fn report_exit(core: &Core, session: &Session, status: Option<&ExitStatus>) {
    let exited_at = stamp_exited(session);
    session.end_setup();
    let (code, signal) = match status {
        Some(status) => {
            let exit = TaskProcessExit::from_status(Some(status));
            (Some(exit.code), exit.signal)
        }
        None => (None, None),
    };
    let screen = session.screen.locked();
    if session.exit_reported.swap(true, Ordering::AcqRel) {
        session.exited.store(true, Ordering::Release);
        return;
    }
    *session.exit.locked() = Some(SessionExit {
        code,
        signal: signal.clone(),
    });
    session.exited.store(true, Ordering::Release);
    for waiter in std::mem::take(&mut *session.line_waiters.locked()) {
        let _ = waiter.found.send(None);
    }
    core.broadcast_event(&Event::Exited {
        id: session.id,
        code,
        signal: signal.clone(),
        killed: session.killed.load(Ordering::Acquire),
    });
    drop(screen);
    core.ports.changed();
    if let Some(number) = session.owner.rail_number {
        core.rail_numbers.locked().remove(&number);
    }
    if session.owner.ssh_host.is_some() {
        super::ssh::moved(core);
    }
    let failed = match (&code, &signal) {
        (_, Some(signal)) => Some(format!("It stopped from signal {signal}.")),
        (Some(code), None) if *code != 0 => Some(format!("It exited with code {code}.")),
        _ => None,
    };
    if session.is_task() {
        if let Some(failed) = core
            .harness
            .session_exited(session.id, code, signal.clone())
        {
            super::harness::tell_agent_run_failed(core, failed);
        }
        super::tasks::ended(core, session.id);
    }
    if let Some(agent) = session.agent.as_ref() {
        let mid_turn = agent.busy();
        agent::note_exit(core, agent, status);
        super::lifecycle::terminal_exited(
            core,
            session.id,
            agent.agent_id(),
            code,
            signal.as_deref(),
            session.is_killed(),
        );
        let failed = failed.filter(|_| !session.is_killed());
        if let Some(reason) = &failed {
            core.note_activity(agent.agent_id(), super::activity::Activity::Problem(reason));
        }
        core.note_agent_gone(agent.agent_id());
        if mid_turn || !session.is_killed() {
            super::spawn::terminal_turn_ended(
                core,
                agent.agent_id(),
                super::spawn::Ending::Gone {
                    reason: failed,
                    mid_turn,
                },
            );
        }
    }
    if let Some(exited_at) = exited_at {
        core.reclaim_exited_sessions(exited_at);
    }
}

fn broadcast_output(core: &Arc<Core>, session: &Session, bytes: &[u8]) {
    if let Some(task) = session.task.as_ref() {
        task.log.locked().push(bytes);
        if !task.notice_pending.swap(true, Ordering::AcqRel) {
            core.schedule_task_output_notice(session.id, TASK_OUTPUT_NOTICE_DELAY);
        }
    }
    let output_now_ms = uptime_ms();
    session
        .last_activity_ms
        .store(output_now_ms, Ordering::Relaxed);
    if let Some(agent) = session.agent.as_ref() {
        agent::note_output(core, agent);
    }
    let mut screen = session.screen.locked();
    if session.trimmed.swap(false, Ordering::AcqRel) {
        screen.restore_history();
    }
    let shell_output = screen
        .shell
        .as_mut()
        .map(|shell| shell.process_for_events(bytes, output_now_ms))
        .unwrap_or_default();
    let replies = screen.process(bytes, &core.terminal_colors());
    if let Some(agent) = session.agent.as_ref() {
        agent.note_parsed();
    }
    screen.title_from_cwd();
    let retitled = screen.take_title_change().is_some();
    answer_line_waiters(session, &screen);
    if at_prompt(&screen) {
        type_waiting(session, &screen);
    }
    drop(screen);
    session.frames.notify.notify_one();
    reply_to_program(session, replies);
    if retitled && !session.title_pending.swap(true, Ordering::AcqRel) {
        let (core, id) = (core.clone(), session.id);
        tokio::spawn(async move {
            tokio::time::sleep(TITLE_GAP).await;
            let Some(session) = core.session(id) else {
                return;
            };
            session.title_pending.store(false, Ordering::Release);
            let title = session.screen.locked().window_title.clone();
            core.broadcast_local(&Event::TerminalTitle { id, title });
        });
    }
    if let Some(update) = shell_output.ready {
        core.broadcast_event(&Event::ShellMetadata(update.into_event(session.id)));
    }
    if let Some(link) = shell_output.ssh {
        core.broadcast_event(&Event::SshLink {
            id: session.id,
            link,
        });
        if let Some(alias) = &session.owner.ssh_host {
            super::ssh::moved(core);
            if link == sikemux_pty::shell_protocol::SshLink::Connected {
                super::hosts::probe::ask(core, alias, false);
            }
        }
    }
}

/// What the terminal answers a program's queries with goes back to it after
/// anything already on its way in.
fn reply_to_program(session: &Session, replies: Vec<u8>) {
    if !replies.is_empty() {
        queue_input(session, replies, Typist::Terminal, Box::new(|_| ()));
    }
}

pub(crate) const RESET_MODES: &[u8] = b"\x1b>\x1b[4l\x1b[?1l\x1b[?6l\x1b[?7h\x1b[?9l\x1b[?45l\x1b[?66l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l\x1b[?2004l\x1b[?1049l";

pub(crate) fn reset_modes(core: &Core, session: &Session) -> CoreResult<()> {
    session
        .screen
        .locked()
        .process(RESET_MODES, &core.terminal_colors());
    session.frames.notify.notify_one();
    Ok(())
}

pub(crate) fn clear(core: &Core, session: &Session) {
    session
        .screen
        .locked()
        .terminal
        .clear(&core.terminal_colors());
    session.frames.notify.notify_one();
}

fn line_containing(contents: &str, containing: &str) -> Option<String> {
    contents
        .lines()
        .find(|line| line.contains(containing))
        .map(str::to_owned)
}

/// Callers hold the screen lock, so no output can land between a check and
/// a waiter being added.
fn answer_line_waiters(session: &Session, screen: &Screen) {
    let mut waiters = session.line_waiters.locked();
    if waiters.is_empty() {
        return;
    }
    let contents = screen.terminal.contents();
    for waiter in std::mem::take(&mut *waiters) {
        if waiter.found.is_closed() {
            continue;
        }
        match line_containing(&contents, &waiter.containing) {
            Some(line) => {
                let _ = waiter.found.send(Some(line));
            }
            None => waiters.push(waiter),
        }
    }
}

/// How long the core waits for a shell's prompt before typing anyway, for a
/// prompt it never recognises.
const PROMPT_WAIT: Duration = Duration::from_secs(5);
/// How long a program whose prompt the core cannot see, such as an agent's
/// own interface, gets to start before it is typed to.
const START_WAIT: Duration = Duration::from_millis(800);

/// The shell is reading a command: showing its prompt, or past it with
/// nothing typed yet, as an older bash marks it.
fn at_prompt(screen: &Screen) -> bool {
    use sikemux_wire::pty::shell_protocol::ShellPhase;
    screen
        .shell
        .as_ref()
        .is_some_and(|shell| matches!(shell.phase(), ShellPhase::Prompt | ShellPhase::Input))
}

fn type_waiting(session: &Session, screen: &Screen) {
    let draft = session.typed_at_prompt.locked().take();
    if let Some((draft, typist)) = draft {
        type_draft(session, screen, &draft, typist);
    }
}

/// Types `draft` to sit at the prompt unsent: pasted when the program has
/// turned bracketed paste on, else on one line, since a bare newline would
/// send it.
fn type_draft(session: &Session, screen: &Screen, draft: &str, typist: Typist) {
    queue_input(session, pasted_on(screen, draft), typist, Box::new(|_| {}));
}

/// `draft` as it can be typed into a terminal; none when nothing is left.
pub(crate) fn typable(draft: &str) -> Option<String> {
    let draft = draft.replace('\0', "");
    let draft = draft.trim();
    (!draft.is_empty()).then(|| draft.to_owned())
}

/// Types `draft` into the session once it is ready for it: at its shell's
/// prompt, or a moment after it starts when the core cannot see a prompt.
/// How it is typed is decided then, by what the program has turned on.
pub(crate) fn type_when_ready(session: &Arc<Session>, draft: String, typist: Typist) {
    let screen = session.screen.locked();
    if session.shell_protocol && at_prompt(&screen) {
        return type_draft(session, &screen, &draft, typist);
    }
    *session.typed_at_prompt.locked() = Some((draft, typist));
    drop(screen);
    let wait = if session.shell_protocol {
        PROMPT_WAIT
    } else {
        START_WAIT
    };
    let session = Arc::downgrade(session);
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        if let Some(session) = session.upgrade() {
            type_waiting(&session, &session.screen.locked());
        }
    });
}

/// What is typed in the input box of the agent's TUI the session shows.
pub(crate) fn prompt_draft(session: &Session) -> String {
    let rows = session.screen.locked().terminal.read_rows();
    super::prompt_draft::read(&rows.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Connects an ssh terminal again when it has given up and sits at the local
/// shell's prompt.
pub(crate) fn reconnect_ssh(session: &Session) {
    let Some(host) = session.owner.ssh_host.as_deref() else {
        return;
    };
    if session.shell_at_prompt() {
        let command = format!("{}\r", super::ssh::startup(host));
        queue_input(
            session,
            command.into_bytes(),
            Typist::Other,
            Box::new(|_| {}),
        );
    }
}

/// The first line on the screen containing `containing`, once one shows,
/// whether or not anyone watches the terminal. Nothing if it ends first.
pub(crate) fn line_shown(
    session: &Session,
    containing: String,
) -> tokio::sync::oneshot::Receiver<Option<String>> {
    let (found, shown) = tokio::sync::oneshot::channel();
    let screen = session.screen.locked();
    if let Some(line) = line_containing(&screen.terminal.contents(), &containing) {
        let _ = found.send(Some(line));
    } else if session.exit_reported() {
        let _ = found.send(None);
    } else {
        session
            .line_waiters
            .locked()
            .push(LineWaiter { containing, found });
    }
    drop(screen);
    shown
}

/// What the person did, as the bytes the program expects under the modes it
/// turned on.
pub(crate) fn encode(session: &Session, input: &TerminalInput) -> Vec<u8> {
    session.screen.locked().terminal.encode(input)
}

pub(crate) fn lines(session: &Session, start: u64, count: u32) -> Vec<u8> {
    session.screen.locked().terminal.lines(start, count)
}

pub(crate) fn text(session: &Session, range: Option<(GridPoint, GridPoint)>) -> String {
    let screen = session.screen.locked();
    match range {
        Some((from, to)) => screen.terminal.text(from, to),
        None => screen.terminal.all_text(),
    }
}

pub(crate) fn find(
    session: &Session,
    query: &str,
    options: &FindOptions,
    from: Option<GridPoint>,
    backwards: bool,
    inclusive: bool,
) -> CoreResult<FindResult> {
    session
        .screen
        .locked()
        .terminal
        .find(query, options, from, backwards, inclusive)
        .map_err(CoreError::from)
}

/// Sends each subscriber the frame that brings it up to date, unless it has
/// too many undrawn or was sent one too recently. Answers when to try again
/// for those that were sent one too recently.
fn send_frames(core: &Core, session: &Session) -> Option<Instant> {
    let now = Instant::now();
    let mut next: Option<Instant> = None;
    let mut screen = session.screen.locked();
    if let Some(deadline) = screen.terminal.sync_deadline() {
        if deadline <= now {
            let replies = screen.end_sync(&core.terminal_colors());
            reply_to_program(session, replies);
        } else {
            next = Some(deadline);
        }
    }
    let mut subscribers = session.subscribers.locked();
    subscribers.retain(|stream, subscriber| {
        if subscriber.in_flight >= MAX_FRAMES_IN_FLIGHT {
            return true;
        }
        if let Some(due) = subscriber.sent_at.map(|sent| sent + FRAME_GAP) {
            if now < due {
                if screen.terminal.version() != subscriber.view.version() {
                    next = Some(next.map_or(due, |next| next.min(due)));
                }
                return true;
            }
        }
        let Some(frame) = screen.terminal.frame(&mut subscriber.view) else {
            return true;
        };
        if !subscriber.client.send(encode_grid(*stream, &frame).into()) {
            subscriber.client.forget_stream(*stream);
            return false;
        }
        subscriber.in_flight += 1;
        subscriber.sent_at = Some(now);
        true
    });
    next
}

async fn pump_frames(core: Arc<Core>, session: Weak<Session>, signal: Arc<FrameSignal>) {
    let mut wake_at: Option<Instant> = None;
    loop {
        match wake_at {
            Some(at) => {
                tokio::select! {
                    _ = signal.notify.notified() => {}
                    _ = tokio::time::sleep_until(at.into()) => {}
                }
            }
            None => signal.notify.notified().await,
        }
        if signal.closed.load(Ordering::Acquire) {
            return;
        }
        let Some(session) = session.upgrade() else {
            return;
        };
        wake_at = send_frames(&core, &session);
    }
}

pub(crate) fn mark_task_output_noticed(session: &Session) {
    if let Some(task) = session.task.as_ref() {
        task.notice_pending.store(false, Ordering::Release);
    }
}

/// Starts a stream: its first frame, with the whole screen, answers the
/// attach, queued under the screen lock before any later frame can be.
pub(crate) fn attach(
    session: &Session,
    client: &Arc<ClientConn>,
    request_id: RequestId,
) -> CoreResult<()> {
    let mut screen = session.screen.locked();
    let mut subscribers = session.subscribers.locked();
    if subscribers.len() >= MAX_SUBSCRIBERS_PER_SESSION {
        return Err("pty: PTY subscriber capacity reached".into());
    }
    if session.trimmed.swap(false, Ordering::AcqRel) {
        screen.restore_history();
    }
    let stream = NEXT_STREAM.fetch_add(1, Ordering::Relaxed);
    let mut view = View::default();
    let first = screen.terminal.frame(&mut view).unwrap_or_default();
    let shell = screen.shell.as_ref().map(ShellProtocolParser::snapshot);
    let frame = encode_snapshot(
        request_id,
        session.id,
        stream,
        &AttachHeader {
            shell,
            exited: session.exit_reported.load(Ordering::Acquire),
        },
        &first,
    )?;
    if !client.send(frame.into()) {
        return Err("client disconnected".into());
    }
    subscribers.insert(
        stream,
        Subscriber {
            client: client.clone(),
            view,
            in_flight: 1,
            sent_at: Some(Instant::now()),
        },
    );
    client.note_stream(stream, session.id);
    Ok(())
}

pub(crate) fn detach(session: &Session, stream: StreamId) {
    let emptied = {
        let mut subscribers = session.subscribers.locked();
        if subscribers.remove(&stream).is_none() {
            return;
        }
        subscribers.is_empty()
    };
    if !emptied || session.trimmed.load(Ordering::Acquire) {
        return;
    }
    // Nobody is looking any more, so keep only the newest history. Re-check
    // under the screen lock so an attach that raced us keeps its history.
    let mut screen = session.screen.locked();
    if session.has_subscribers() {
        return;
    }
    if screen.compact_for_idle() {
        session.trimmed.store(true, Ordering::Release);
    }
}

pub(crate) fn detach_all(session: &Session) {
    let subscribers = std::mem::take(&mut *session.subscribers.locked());
    for (stream, subscriber) in subscribers {
        subscriber.client.forget_stream(stream);
    }
}

/// The client drew every frame of the stream up to `frame`.
pub(crate) fn ack(session: &Session, stream: StreamId, frame: u32) {
    {
        let mut subscribers = session.subscribers.locked();
        let Some(subscriber) = subscribers.get_mut(&stream) else {
            return;
        };
        let undrawn = subscriber.view.frame().wrapping_sub(frame);
        subscriber.in_flight = subscriber.in_flight.min(undrawn);
    }
    session.frames.notify.notify_one();
}

pub(crate) fn resize(session: &Session, cols: u16, rows: u16) -> CoreResult<()> {
    session.io.resize(cols, rows)?;
    session.screen.locked().terminal.resize(rows, cols);
    session.frames.notify.notify_one();
    Ok(())
}

pub(crate) fn task_output(
    session: &Session,
    query: &sikemux_pty::output_log::OutputQuery,
) -> CoreResult<sikemux_pty::output_log::OutputPage> {
    let task = session
        .task
        .as_ref()
        .ok_or_else(|| CoreError::from("PTY is not a managed task"))?;
    task.log.locked().query(query).map_err(CoreError::from)
}

pub(crate) fn kill(core: &Core, session: &Session) {
    // A terminal kept after its process ended has nothing left to signal, and
    // its pid may belong to someone else by now.
    if !session.is_task() && !session.is_running() {
        session.stop_reader.notify_one();
        return;
    }
    session.killed.store(true, Ordering::Release);
    if let Some(agent) = session.agent.as_ref() {
        agent.silence();
    }
    let status = {
        let mut child = session.child.locked();
        let force_task_tree = sikemux_pty::task::task_process_needs_force_backstop(
            session.is_task(),
            session.exited_at_ms.load(Ordering::Acquire),
        );
        let status = sikemux_pty::process::terminate_and_reap_child(&mut child, force_task_tree);
        stamp_exited(session);
        status
    };
    report_exit(core, session, status.as_ref());
    if !session.is_task() {
        // The shell is gone. Anything still holding the terminal open would
        // keep the reader, and with it the PTY, alive for good.
        session.stop_reader.notify_one();
    }
}

/// First half of shutting everything down: ask the process tree to stop.
pub(crate) fn signal_for_drain(session: &Session) {
    session.killed.store(true, Ordering::Release);
    let child = session.child.locked();
    if sikemux_pty::task::should_signal_process_on_drain(
        session.is_task(),
        session.exited_at_ms.load(Ordering::Acquire),
    ) {
        if let Some(pid) = child.process_id() {
            sikemux_pty::process::terminate_process_tree(pid, false);
        }
    }
}

/// Second half, after the shared grace: force whatever is left and reap it.
pub(crate) fn finish_drain(core: &Core, session: &Session) {
    use sikemux_pty::process::{child_process_id, kill_and_reap_child, terminate_process_tree};
    use sikemux_pty::task::{should_signal_process_on_drain, task_process_needs_force_backstop};
    let status = {
        let mut child = session.child.locked();
        let is_task = session.is_task();
        let exited_at = session.exited_at_ms.load(Ordering::Acquire);
        let status = if !should_signal_process_on_drain(is_task, exited_at) {
            child.try_wait().ok().flatten()
        } else if task_process_needs_force_backstop(is_task, exited_at) {
            let pid = child_process_id(&mut child);
            if let Some(pid) = pid {
                terminate_process_tree(pid, true);
            }
            match child.try_wait() {
                Ok(Some(status)) => Some(status),
                _ => kill_and_reap_child(&mut child, pid),
            }
        } else if let Ok(Some(status)) = child.try_wait() {
            Some(status)
        } else {
            let pid = child_process_id(&mut child);
            kill_and_reap_child(&mut child, pid)
        };
        stamp_exited(session);
        status
    };
    report_exit(core, session, status.as_ref());
    session.stop_reader.notify_one();
}

pub(crate) fn trim_if_idle(session: &Session, now: u64, idle: Duration) {
    let idle_ms = idle.as_millis() as u64;
    let quiet = || now.saturating_sub(session.last_activity_ms.load(Ordering::Relaxed)) >= idle_ms;
    if session.trimmed.load(Ordering::Relaxed) || !quiet() || session.has_subscribers() {
        return;
    }
    let mut screen = session.screen.locked();
    // Output and attaches take the screen lock first, so re-checking under it
    // means neither can slip in between the check and the compaction.
    if !quiet() || session.has_subscribers() {
        return;
    }
    if screen.compact_for_idle() {
        session.trimmed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::RESET_MODES;
    use sikemux_pty::screen::Screen;
    use sikemux_wire::grid::{Mods, MouseAction, MouseButton, TerminalInput};

    // The single-fd design rests on this: after the master is dup'd and
    // portable_pty's `MasterPty` dropped, the dup keeps the child's terminal
    // open. The child sleeps before printing, so a hangup on drop would end
    // the read before the marker arrives.
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
        // SAFETY: `pair.master` still owns `master_fd`, so it is open; dup only
        // creates a new fd and touches no memory.
        let dup_fd = unsafe { libc::dup(master_fd) };
        assert!(dup_fd >= 0, "dup failed");
        drop(pair.master);

        // SAFETY: `dup_fd` is a fresh fd, checked above, that nothing else owns,
        // so the File is its only owner and closes it exactly once.
        let mut file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
        let mut got = String::new();
        let mut buf = [0u8; 256];
        loop {
            match file.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if got.contains("MARKER") {
                        break;
                    }
                }
            }
        }
        let _ = child.wait();
        assert!(
            got.contains("MARKER"),
            "the child lost its terminal; got {got:?}"
        );
    }

    #[test]
    fn reset_modes_disables_interaction_modes_without_losing_normal_history() {
        let mut screen = Screen::new(5, 20, false);
        let colors = sikemux_wire::grid::TerminalColors::default();
        for i in 0..20 {
            screen.process(format!("line {i:02}\r\n").as_bytes(), &colors);
        }
        screen.process(
            b"\x1b=\x1b[?1h\x1b[?9h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1005h\x1b[?1006h\x1b[?2004h\x1b[?1049halt",
            &colors,
        );
        screen.process(RESET_MODES, &colors);

        assert!(!screen.terminal.alternate_screen());
        assert!(!screen.terminal.bracketed_paste());
        let up = TerminalInput::Key {
            key: "ArrowUp".into(),
            mods: Mods::default(),
        };
        assert_eq!(screen.terminal.encode(&up), b"\x1b[A");
        let click = TerminalInput::Mouse {
            action: MouseAction::Press,
            button: MouseButton::Left,
            col: 1,
            row: 1,
            mods: Mods::default(),
        };
        assert!(screen.terminal.encode(&click).is_empty());
        assert!(screen.terminal.all_text().contains("line 00"));
    }
}
