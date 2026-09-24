use std::fmt;
use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

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

/// Kill the child and everything it started. The waiter thread owns the child
/// and reaps it once this lands.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
}

fn kill_and_reap(child: &mut std::process::Child) {
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
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(ProcessRunError::Spawn)?;
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

    let deadline = Instant::now() + timeout;
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
        let remaining = deadline.saturating_duration_since(Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow child must not be polled awake: the wait returns when the
    /// process exits, not when a timer next fires.
    #[test]
    fn a_slow_child_is_waited_on_not_polled() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 0.3; echo done"]);
        let started = Instant::now();
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
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_child_that_outlives_its_deadline_is_killed() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let error = run(
            &mut command,
            None,
            Duration::from_millis(100),
            1024 * 1024,
            None,
        )
        .expect_err("the deadline must be enforced");

        assert!(matches!(error, ProcessRunError::Timeout(_)), "{error}");
    }

    #[test]
    fn a_cancelled_child_stops_without_waiting_for_its_deadline() {
        let cancellation = ProcessCancellation::new();
        let trigger = cancellation.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });

        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
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
}
