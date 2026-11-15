use std::fmt;
use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub mod android_sdk;
pub mod cgroups;
pub mod clock;
pub mod files;
pub mod jobs;
pub mod line;
pub mod machine;
pub mod memory_pressure;
pub mod names;
pub mod paths;
pub mod procs;
pub mod sync;
mod tree;
pub mod user_environment;
pub mod vars;

pub use tree::{own_tree, spawn_tree, terminate_tree};

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

fn kill_group(pid: u32) {
    terminate_tree(pid, true);
}

/// Kills the child and everything it started, then reaps it.
pub fn kill_and_reap(child: &mut std::process::Child) {
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

enum RunEvent {
    Exited(io::Result<std::process::ExitStatus>),
    ReaderFinished,
}

/// How often the wait checks a cancellation token. Nothing else needs a timer:
/// the child's exit and each drained pipe arrive as events. A run with no
/// token waits with no timer at all.
const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How often a run whose waits do not count asks whether it waits.
const WAIT_LOOK_INTERVAL: Duration = Duration::from_secs(2);

fn read_bounded(
    mut pipe: impl Read,
    total: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    max_output_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(bytes);
        }
        let previous = total.fetch_add(read, Ordering::AcqRel);
        if previous.saturating_add(read) > max_output_bytes {
            exceeded.store(true, Ordering::Release);
            return Err(io::Error::other("subprocess output limit exceeded"));
        }
        bytes.extend_from_slice(&chunk[..read]);
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
    let (events, arrivals) = mpsc::channel();
    let stdout_reader = {
        let total = Arc::clone(&total);
        let exceeded = Arc::clone(&exceeded);
        let events = events.clone();
        std::thread::spawn(move || {
            let result = read_bounded(stdout, total, exceeded, max_output_bytes);
            let _ = events.send(RunEvent::ReaderFinished);
            result
        })
    };
    let stderr_reader = {
        let total = Arc::clone(&total);
        let exceeded = Arc::clone(&exceeded);
        let events = events.clone();
        std::thread::spawn(move || {
            let result = read_bounded(stderr, total, exceeded, max_output_bytes);
            let _ = events.send(RunEvent::ReaderFinished);
            result
        })
    };
    let stdin_writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let stdin = child.stdin.take();
        std::thread::spawn(move || {
            let mut stdin = stdin.ok_or(ProcessRunError::MissingPipe("stdin"))?;
            stdin.write_all(&bytes).map_err(ProcessRunError::Io)
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
    let mut readers_finished = 0_u8;
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
            if readers_finished == 2 {
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
            Ok(RunEvent::ReaderFinished) => readers_finished += 1,
            Ok(RunEvent::Exited(Ok(status))) => completed_status = Some(status),
            Ok(RunEvent::Exited(Err(error))) => {
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
    // Process-level policy errors are more useful than the expected broken
    // pipe errors caused by enforcing them.
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

    #[cfg(windows)]
    fn stops_when_asked(clear: bool) -> bool {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        if clear { unsafe { SetConsoleCtrlHandler(None, 0) }; }
        let spawned = spawn_tree(shell(&waiting(60)).stdout(Stdio::null()));
        if clear { unsafe { SetConsoleCtrlHandler(None, 1) }; }
        let mut child = spawned.expect("spawn the tree");
        std::thread::sleep(Duration::from_millis(500));
        terminate_tree(child.id(), false);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status = None;
        while status.is_none() && Instant::now() < deadline {
            status = child.try_wait().expect("wait for the tree");
            std::thread::sleep(Duration::from_millis(20));
        }
        kill_and_reap(&mut child);
        status.is_some()
    }

    #[cfg(windows)]
    #[test]
    fn probe() {
        let plain = stops_when_asked(false);
        let cleared = stops_when_asked(true);
        let log = std::fs::read_to_string(std::env::temp_dir().join("probe.txt")).unwrap_or_default();
        panic!("plain {plain} cleared {cleared}\n{log}");
    }
}
