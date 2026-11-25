use std::fmt;
use std::io::{self, Read, Write};
use std::process::Output;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub mod android_sdk;
pub mod cgroups;
pub mod clock;
pub mod command;

use command::{Child, Command, Stdio};
pub mod files;
pub mod jobs;
pub mod line;
pub mod machine;
pub mod memory_pressure;
pub mod names;
pub mod paths;
#[cfg(unix)]
pub mod private_socket;
pub mod procs;
pub mod sync;
mod tree;
pub mod user_environment;
pub mod vars;

pub use tree::{spawn_tree, terminate_tree};

#[derive(Debug)]
pub enum ProcessRunError {
    Spawn(io::Error),
    Io(io::Error),
    Timeout(Duration),
    OutputLimit(usize),
    Cancelled,
    WorkerPanic(&'static str),
    MissingPipe(&'static str),
}

impl fmt::Display for ProcessRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) | Self::Io(error) => error.fmt(formatter),
            Self::Timeout(timeout) => write!(
                formatter,
                "subprocess timed out after {}s",
                timeout.as_secs()
            ),
            Self::OutputLimit(limit) => write!(
                formatter,
                "subprocess output exceeds {} MiB limit",
                limit / 1024 / 1024
            ),
            Self::Cancelled => formatter.write_str("subprocess was cancelled"),
            Self::WorkerPanic(worker) => write!(formatter, "{worker} worker panicked"),
            Self::MissingPipe(pipe) => write!(formatter, "subprocess {pipe} unavailable"),
        }
    }
}

impl std::error::Error for ProcessRunError {}

#[derive(Clone, Default)]
pub struct ProcessCancellation(Arc<AtomicBool>);

impl ProcessCancellation {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Kills the child's process group and every group a process below it leads, such as
/// the group nextest gives each test, which the child's own group kill misses.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    let below = procs::descendants(pid);
    terminate_tree(pid, true);
    #[cfg(unix)]
    for &descendant in below.keys() {
        terminate_tree(descendant, true);
    }
}

/// Kills the child and everything it started, then reaps it.
pub fn kill_and_reap(child: &mut Child) {
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

enum RunEvent {
    Exited(io::Result<std::process::ExitStatus>),
    StdoutFinished,
    StderrFinished,
    StdinFinished,
}

/// How often the wait checks a cancellation token. Nothing else needs a timer:
/// the child's exit and each drained pipe arrive as events. A run with no
/// token waits with no timer at all.
const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How often a run whose waits do not count asks whether it waits.
const WAIT_LOOK_INTERVAL: Duration = Duration::from_secs(2);
/// How long to wait for pipes and child reap to drain after a timeout or cancel
/// before detaching any reader held open by an escaped grandchild.
const POST_KILL_GRACE: Duration = Duration::from_secs(1);

#[cfg(unix)]
fn wait_readable(fd: std::os::fd::RawFd, stop: &AtomicBool) -> io::Result<bool> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        // SAFETY: `pollfd` points to one valid `libc::pollfd` for the duration of the call.
        let ready = unsafe { libc::poll(&mut pollfd, 1, 100) };
        if ready > 0 {
            return Ok(true);
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn read_bounded<R: Read + std::os::fd::AsRawFd>(
    mut pipe: R,
    total: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    max_output_bytes: usize,
) -> io::Result<Vec<u8>> {
    let fd = pipe.as_raw_fd();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        if !wait_readable(fd, &stop)? {
            return Ok(bytes);
        }
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(bytes);
        }
        let previous = total.fetch_add(read, Ordering::AcqRel);
        if previous.saturating_add(read) > max_output_bytes {
            exceeded.store(true, Ordering::Release);
            return Err(io::Error::other("subprocess output limit exceeded"));
        }
        bytes.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
}

#[cfg(not(unix))]
fn read_bounded(
    mut pipe: impl Read,
    total: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    max_output_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(bytes);
        }
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(bytes);
        }
        let previous = total.fetch_add(read, Ordering::AcqRel);
        if previous.saturating_add(read) > max_output_bytes {
            exceeded.store(true, Ordering::Release);
            return Err(io::Error::other("subprocess output limit exceeded"));
        }
        bytes.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
}

/// Run a child with concurrent pipe drainage, a combined output ceiling,
/// process-group ownership, a hard deadline, and optional cooperative cancel.
pub fn run(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_output_bytes: usize,
    cancellation: Option<&ProcessCancellation>,
) -> Result<Output, ProcessRunError> {
    run_with_pid(
        command,
        input,
        timeout,
        max_output_bytes,
        cancellation,
        |_| {},
    )
}

/// [`run`], handing `started` the child's pid before the child is given its
/// input.
pub fn run_with_pid(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_output_bytes: usize,
    cancellation: Option<&ProcessCancellation>,
    started: impl FnOnce(u32),
) -> Result<Output, ProcessRunError> {
    run_counting(
        command,
        input,
        timeout,
        max_output_bytes,
        cancellation,
        started,
        None,
    )
}

/// [`run`], whose `timeout` counts only the time `waiting`, asked of the child's pid every
/// few seconds, says the child is not waiting for its turn. `started` hears the child's pid.
pub fn run_not_counting_waits(
    command: &mut Command,
    timeout: Duration,
    max_output_bytes: usize,
    started: impl FnOnce(u32),
    waiting: &dyn Fn(u32) -> bool,
) -> Result<Output, ProcessRunError> {
    run_counting(
        command,
        None,
        timeout,
        max_output_bytes,
        None,
        started,
        Some(waiting),
    )
}

fn run_counting(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_output_bytes: usize,
    cancellation: Option<&ProcessCancellation>,
    started: impl FnOnce(u32),
    waiting: Option<&dyn Fn(u32) -> bool>,
) -> Result<Output, ProcessRunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = spawn_tree(command).map_err(ProcessRunError::Spawn)?;
    started(child.id());
    let stdout = child.stdout.take().ok_or_else(|| {
        kill_and_reap(&mut child);
        ProcessRunError::MissingPipe("stdout")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        kill_and_reap(&mut child);
        ProcessRunError::MissingPipe("stderr")
    })?;

    let total = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let stop_readers = Arc::new(AtomicBool::new(false));
    let (events, arrivals) = mpsc::channel();
    let stdout_reader = {
        let total = Arc::clone(&total);
        let exceeded = Arc::clone(&exceeded);
        let stop = Arc::clone(&stop_readers);
        let events = events.clone();
        std::thread::spawn(move || {
            let result = read_bounded(stdout, total, exceeded, stop, max_output_bytes);
            let _ = events.send(RunEvent::StdoutFinished);
            result
        })
    };
    let stderr_reader = {
        let total = Arc::clone(&total);
        let exceeded = Arc::clone(&exceeded);
        let stop = Arc::clone(&stop_readers);
        let events = events.clone();
        std::thread::spawn(move || {
            let result = read_bounded(stderr, total, exceeded, stop, max_output_bytes);
            let _ = events.send(RunEvent::StderrFinished);
            result
        })
    };
    let stdin_writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let stdin = child.stdin.take();
        let events = events.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let mut stdin = stdin.ok_or(ProcessRunError::MissingPipe("stdin"))?;
                stdin.write_all(&bytes).map_err(ProcessRunError::Io)
            })();
            let _ = events.send(RunEvent::StdinFinished);
            result
        })
    });

    // The child moves to its own thread so the wait below is woken by its exit
    // rather than asking fifty times a second whether it has finished yet.
    let pid = child.id();
    let waiter = std::thread::spawn(move || {
        let _ = events.send(RunEvent::Exited(child.wait()));
    });

    let mut left = timeout;
    let mut counted_to = Instant::now();
    let mut counting = true;
    let mut next_look = counted_to;
    let mut completed_status = None;
    let mut waiter_done = false;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdin_done = stdin_writer.is_none();
    let status = loop {
        if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
            kill_group(pid);
            break Err(ProcessRunError::Cancelled);
        }
        if exceeded.load(Ordering::Acquire) {
            kill_group(pid);
            break Err(ProcessRunError::OutputLimit(max_output_bytes));
        }
        if let Some(status) = completed_status {
            if stdout_done && stderr_done {
                break Ok(status);
            }
        }
        let now = Instant::now();
        if counting {
            left = left.saturating_sub(now - counted_to);
        }
        counted_to = now;
        if let Some(waiting) = waiting.filter(|_| now >= next_look) {
            counting = !waiting(pid);
            next_look = now + WAIT_LOOK_INTERVAL;
        }
        let remaining = left;
        if remaining.is_zero() {
            // The direct child may already have exited while a descendant
            // retains one of its pipes. The process-group kill closes those
            // handles so reader joins remain bounded as well.
            kill_group(pid);
            break Err(ProcessRunError::Timeout(timeout));
        }
        let wait = match cancellation {
            Some(_) => remaining.min(CANCEL_CHECK_INTERVAL),
            None => remaining,
        };
        let wait = match waiting {
            Some(_) => wait.min(WAIT_LOOK_INTERVAL),
            None => wait,
        };
        match arrivals.recv_timeout(wait) {
            Ok(RunEvent::StdoutFinished) => stdout_done = true,
            Ok(RunEvent::StderrFinished) => stderr_done = true,
            Ok(RunEvent::StdinFinished) => stdin_done = true,
            Ok(RunEvent::Exited(Ok(status))) => {
                waiter_done = true;
                completed_status = Some(status);
            }
            Ok(RunEvent::Exited(Err(error))) => {
                waiter_done = true;
                kill_group(pid);
                break Err(ProcessRunError::Io(error));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Every sender is gone, so nothing further can arrive.
            Err(mpsc::RecvTimeoutError::Disconnected) => match completed_status {
                Some(status) => break Ok(status),
                None => {
                    break Err(ProcessRunError::Io(io::Error::other(
                        "subprocess exited without reporting a status",
                    )))
                }
            },
        }
    };

    if let Err(error) = status {
        let mut drain = |deadline: Instant| -> bool {
            while !(waiter_done && stdout_done && stderr_done && stdin_done) {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                match arrivals.recv_timeout(remaining) {
                    Ok(RunEvent::Exited(_)) => waiter_done = true,
                    Ok(RunEvent::StdoutFinished) => stdout_done = true,
                    Ok(RunEvent::StderrFinished) => stderr_done = true,
                    Ok(RunEvent::StdinFinished) => stdin_done = true,
                    Err(_) => break,
                }
            }
            stdout_done && stderr_done
        };
        if !drain(Instant::now() + POST_KILL_GRACE) {
            stop_readers.store(true, Ordering::Release);
            drain(Instant::now() + Duration::from_millis(250));
        }
        if waiter_done {
            let _ = waiter.join();
        }
        if stdin_done {
            if let Some(writer) = stdin_writer {
                let _ = writer.join();
            }
        }
        if stdout_done {
            let _ = stdout_reader.join();
        }
        if stderr_done {
            let _ = stderr_reader.join();
        }
        return Err(error);
    }

    let _ = waiter.join();
    let stdin_result = if let Some(writer) = stdin_writer {
        Some(
            writer
                .join()
                .map_err(|_| ProcessRunError::WorkerPanic("stdin"))?,
        )
    } else {
        None
    };
    let stdout_result = stdout_reader
        .join()
        .map_err(|_| ProcessRunError::WorkerPanic("stdout"))?
        .map_err(ProcessRunError::Io);
    let stderr_result = stderr_reader
        .join()
        .map_err(|_| ProcessRunError::WorkerPanic("stderr"))?
        .map_err(ProcessRunError::Io);
    let status = status?;
    if let Some(result) = stdin_result {
        result?;
    }
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// `text` as one word for a POSIX shell.
pub fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `script` run by the platform's own shell.
    fn shell(script: &str) -> Command {
        #[cfg(unix)]
        let (program, flags) = ("sh", ["-c"].as_slice());
        #[cfg(windows)]
        let (program, flags) = ("cmd", ["/D", "/C"].as_slice());
        let mut command = crate::user_environment::command(program);
        command.args(flags).arg(script);
        command
    }

    /// A script that waits about `seconds`, through a program the shell starts, so the
    /// waiting process is the shell's child rather than the shell itself.
    fn waiting(seconds: u32) -> String {
        if cfg!(windows) {
            format!("ping -n {} 127.0.0.1 >NUL", seconds + 1)
        } else {
            format!("sleep {seconds}")
        }
    }

    #[test]
    fn a_slow_child_is_waited_for_and_its_output_kept() {
        #[cfg(unix)]
        let mut command = shell("sleep 0.3; echo done");
        #[cfg(windows)]
        let mut command = shell("ping -n 2 127.0.0.1 >NUL & echo done");
        let out = run(
            &mut command,
            None,
            Duration::from_secs(5),
            1024 * 1024,
            None,
        )
        .expect("child must run to completion");

        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
    }

    #[test]
    fn a_child_that_outlives_its_deadline_is_killed() {
        let mut command = shell(&waiting(30));
        let started = Instant::now();
        let error = run(
            &mut command,
            None,
            Duration::from_millis(100),
            1024 * 1024,
            None,
        )
        .expect_err("the deadline must be enforced");

        assert!(matches!(error, ProcessRunError::Timeout(_)), "{error}");
        // The waiting grandchild holds the output pipes, so the run ends only once it is gone too.
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_cancelled_child_stops_without_waiting_for_its_deadline() {
        let cancellation = ProcessCancellation::new();
        let trigger = cancellation.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });

        let mut command = shell(&waiting(30));
        let started = Instant::now();
        let error = run(
            &mut command,
            None,
            Duration::from_secs(30),
            1024 * 1024,
            Some(&cancellation),
        )
        .expect_err("cancellation must stop the run");

        assert!(matches!(error, ProcessRunError::Cancelled), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_detached_grandchild_holding_stdout_does_not_hang_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let script = format!(
            "python3 -c 'import os, time; pid = os.fork(); \
             (os._exit(0) if pid else (os.setsid(), open({:?}, \"w\").write(str(os.getpid())), time.sleep(30)))'; sleep 30",
            pid_file.to_string_lossy()
        );
        let mut command = shell(&script);
        let started = Instant::now();
        let error = run(
            &mut command,
            None,
            Duration::from_millis(200),
            1024 * 1024,
            None,
        )
        .expect_err("timeout must return even with a detached grandchild holding stdout");

        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = text.trim().parse::<libc::pid_t>() {
                // SAFETY: sending SIGKILL to the test's own detached grandchild pid.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        assert!(matches!(error, ProcessRunError::Timeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[test]
    fn a_tree_asked_to_stop_stops_without_force() {
        let mut child =
            spawn_tree(shell(&waiting(60)).stdout(Stdio::null())).expect("spawn the tree");
        std::thread::sleep(Duration::from_millis(500));

        terminate_tree(child.id(), false);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut status = None;
        while status.is_none() && Instant::now() < deadline {
            status = child.try_wait().expect("wait for the tree");
            std::thread::sleep(Duration::from_millis(20));
        }
        kill_and_reap(&mut child);
        assert!(status.is_some(), "the tree ignored being asked to stop");
    }
}
