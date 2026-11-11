//! Puts the time at the start of every line the core writes to its log. The log is whatever
//! file launchd or systemd opened as the core's stderr; the core puts a pipe in its place and
//! a thread copies each line across with the time in front. Updating the core hands the pipe
//! and the file to the new one, which carries on copying, so no line is lost or left untimed.

use std::os::fd::RawFd;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

const LOG_FD: &str = "SIKEMUX_CORE_LOG_FD";
const PIPE_FD: &str = "SIKEMUX_CORE_LOG_PIPE_FD";
const MID_LINE: &str = "SIKEMUX_CORE_LOG_MID_LINE";

struct Stamping {
    log: RawFd,
    pipe: RawFd,
    /// Held while lines are copied, so handing over never cuts one that was read off.
    copying: Mutex<Copied>,
}

#[derive(Default)]
pub(crate) struct Copied {
    mid_line: bool,
}

static STAMPING: OnceLock<Stamping> = OnceLock::new();

/// Starts timing the log's lines, when stderr is a log file or this core took over from one
/// that was. A terminal or a pipe someone else reads is left as it is.
pub(crate) fn start() {
    let handed = (env_fd(LOG_FD)).zip(env_fd(PIPE_FD));
    let mid_line = std::env::var_os(MID_LINE).is_some();
    for name in [LOG_FD, PIPE_FD, MID_LINE] {
        std::env::remove_var(name);
    }
    let (log, pipe) = match handed {
        Some(fds) => fds,
        None => match take_stderr() {
            Some(fds) => fds,
            None => return,
        },
    };
    set_close_on_exec(log);
    set_close_on_exec(pipe);
    let stamping = Stamping {
        log,
        pipe,
        copying: Mutex::new(Copied { mid_line }),
    };
    if STAMPING.set(stamping).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("sikemux-log".into())
        .spawn(copy);
}

/// What the next core needs to carry on, given to `command` before it replaces this one: the
/// guard stops lines being copied until the exec, and the descriptors must stay open over it.
pub(crate) fn hand_over(
    command: &mut Command,
) -> Option<(MutexGuard<'static, Copied>, [RawFd; 2])> {
    let stamping = STAMPING.get()?;
    let copied = stamping
        .copying
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    command
        .env(LOG_FD, stamping.log.to_string())
        .env(PIPE_FD, stamping.pipe.to_string());
    if copied.mid_line {
        command.env(MID_LINE, "1");
    }
    Some((copied, [stamping.log, stamping.pipe]))
}

fn env_fd(name: &str) -> Option<RawFd> {
    let fd: RawFd = std::env::var(name).ok()?.parse().ok()?;
    // SAFETY: F_GETFD only reads the descriptor's flags.
    (unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0).then_some(fd)
}

/// Keeps the log file stderr names and puts a pipe in its place, answering both.
fn take_stderr() -> Option<(RawFd, RawFd)> {
    // SAFETY: fstat fills `stat` for descriptor 2; the calls after it only duplicate,
    // create and close descriptors this function owns, and checked each answer.
    unsafe {
        let mut stat = std::mem::zeroed::<libc::stat>();
        if libc::fstat(libc::STDERR_FILENO, &mut stat) != 0
            || stat.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return None;
        }
        let log = libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if log < 0 {
            return None;
        }
        let mut ends = [0; 2];
        if libc::pipe(ends.as_mut_ptr()) != 0 {
            libc::close(log);
            return None;
        }
        let [read, write] = ends;
        let flags = libc::fcntl(read, libc::F_GETFL);
        libc::fcntl(read, libc::F_SETFL, flags | libc::O_NONBLOCK);
        if libc::dup2(write, libc::STDERR_FILENO) < 0 {
            libc::close(read);
            libc::close(write);
            libc::close(log);
            return None;
        }
        libc::close(write);
        Some((log, read))
    }
}

fn set_close_on_exec(fd: RawFd) {
    // SAFETY: F_GETFD and F_SETFD read and set one descriptor's flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Copies what reaches the pipe into the log, the time in front of each line, until the pipe
/// closes. Waiting happens unlocked; reading and writing happen under the lock.
fn copy() {
    let Some(stamping) = STAMPING.get() else {
        return;
    };
    let mut buffer = vec![0u8; 16 * 1024];
    let mut out = Vec::with_capacity(buffer.len() + 256);
    loop {
        let mut wait = libc::pollfd {
            fd: stamping.pipe,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `wait` is one valid pollfd.
        if unsafe { libc::poll(&mut wait, 1, -1) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        let mut copied = stamping
            .copying
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            // SAFETY: `buffer` has room for as many bytes as asked for.
            let read =
                unsafe { libc::read(stamping.pipe, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read == 0 {
                return;
            }
            if read < 0 {
                break;
            }
            out.clear();
            for line in buffer[..read as usize].split_inclusive(|byte| *byte == b'\n') {
                if !copied.mid_line {
                    out.extend_from_slice(stamp().as_bytes());
                }
                out.extend_from_slice(line);
                copied.mid_line = line.last() != Some(&b'\n');
            }
            write_all(stamping.log, &out);
        }
    }
}

fn write_all(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for its length.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written <= 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        bytes = &bytes[written as usize..];
    }
}

/// The time now in this computer's zone: "2026-11-11 14:02:03.512 ".
fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as libc::time_t;
    // SAFETY: localtime_r fills `parts` from `seconds` and keeps no pointer to either.
    let parts = unsafe {
        let mut parts = std::mem::zeroed::<libc::tm>();
        libc::localtime_r(&seconds, &mut parts);
        parts
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} ",
        parts.tm_year + 1900,
        parts.tm_mon + 1,
        parts.tm_mday,
        parts.tm_hour,
        parts.tm_min,
        parts.tm_sec,
        now.subsec_millis()
    )
}
