//! Puts the time at the start of every line the core writes to its log. The log is whatever
//! file launchd or systemd opened as the core's stderr; the core puts a pipe in its place and
//! a thread copies each line across with the time in front. Whenever nothing would read the
//! pipe, as before an update replaces the process or once the thread stops, stderr goes back
//! to the file, so a write to it never waits.

use std::os::fd::RawFd;
use std::sync::Mutex;

struct Stamping {
    log: RawFd,
    pipe: RawFd,
    mid_line: bool,
}

static STAMPING: Mutex<Option<Stamping>> = Mutex::new(None);

fn stamping() -> std::sync::MutexGuard<'static, Option<Stamping>> {
    STAMPING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Starts timing the log's lines when stderr is a log file. A terminal or a pipe someone else
/// reads is left as it is.
pub fn start() {
    let mut held = stamping();
    if held.is_some() {
        return;
    }
    let Some((log, pipe)) = take_stderr() else {
        return;
    };
    *held = Some(Stamping {
        log,
        pipe,
        mid_line: false,
    });
    drop(held);
    let started = std::thread::Builder::new()
        .name("sikemux-log".into())
        .spawn(move || copy(pipe));
    if started.is_err() {
        give_back(&mut stamping());
    }
    flush_on_panic();
}

/// Copies whatever waits in the pipe into the log now, as before the process ends.
pub fn flush() {
    let mut held = stamping();
    if let Some(stamping) = held.as_mut() {
        if drain(stamping).is_err() {
            give_back(&mut held);
        }
    }
}

/// Copies what waits, then puts stderr back on the log file, for a process about to be
/// replaced by one that may not read the pipe. [`start`] takes it up again if it is not.
pub fn before_exec() {
    let mut held = stamping();
    if let Some(stamping) = held.as_mut() {
        let _ = drain(stamping);
    }
    give_back(&mut held);
}

fn flush_on_panic() {
    static HOOKED: std::sync::Once = std::sync::Once::new();
    HOOKED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            flush();
        }));
    });
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
        let status = libc::fcntl(read, libc::F_GETFL);
        libc::fcntl(read, libc::F_SETFL, status | libc::O_NONBLOCK);
        let flags = libc::fcntl(read, libc::F_GETFD);
        libc::fcntl(read, libc::F_SETFD, flags | libc::FD_CLOEXEC);
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

/// Points stderr at the log file again and lets the pipe go.
fn give_back(held: &mut Option<Stamping>) {
    let Some(stamping) = held.take() else {
        return;
    };
    // SAFETY: both descriptors are this module's own; dup2 only repoints descriptor 2.
    unsafe {
        libc::dup2(stamping.log, libc::STDERR_FILENO);
        libc::close(stamping.pipe);
        libc::close(stamping.log);
    }
}

/// Copies what reaches `pipe` into the log, the time in front of each line, until stderr is
/// given back. Waiting happens unlocked; reading and writing happen under the lock.
fn copy(pipe: RawFd) {
    loop {
        let mut wait = libc::pollfd {
            fd: pipe,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `wait` is one valid pollfd.
        let polled = unsafe { libc::poll(&mut wait, 1, -1) };
        let interrupted =
            polled < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
        if interrupted {
            continue;
        }
        let mut held = stamping();
        let Some(stamping) = held.as_mut().filter(|stamping| stamping.pipe == pipe) else {
            return;
        };
        if polled < 0 || wait.revents & libc::POLLNVAL != 0 || drain(stamping).is_err() {
            give_back(&mut held);
            return;
        }
    }
}

/// Copies all the pipe holds now into the log; an error once the pipe cannot be read.
fn drain(stamping: &mut Stamping) -> Result<(), ()> {
    let mut buffer = [0u8; 16 * 1024];
    let mut out = Vec::with_capacity(buffer.len() + 256);
    loop {
        // SAFETY: `buffer` has room for as many bytes as asked for.
        let read = unsafe { libc::read(stamping.pipe, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read == 0 {
            return Err(());
        }
        if read < 0 {
            return match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => Ok(()),
                _ => Err(()),
            };
        }
        out.clear();
        for line in buffer[..read as usize].split_inclusive(|byte| *byte == b'\n') {
            if !stamping.mid_line {
                out.extend_from_slice(stamp().as_bytes());
            }
            out.extend_from_slice(line);
            stamping.mid_line = line.last() != Some(&b'\n');
        }
        write_all(stamping.log, &out);
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
