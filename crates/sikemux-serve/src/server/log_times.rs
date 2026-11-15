//! Puts the time at the start of every line the core writes to its log. The log is whatever
//! file the service manager opened as the core's stderr; the core puts a pipe in its place and
//! a thread copies each line across with the time in front. Whenever nothing would read the
//! pipe, as before an update replaces the process or once the thread stops, stderr goes back
//! to the file, so a write to it never waits.

use std::sync::Mutex;

use sikemux_corekit::wall_clock::{calendar, Zone};

#[cfg(unix)]
#[path = "log_times/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "log_times/windows.rs"]
mod platform;

use platform::{Handle, Read};

struct Stamping {
    log: Handle,
    pipe: Handle,
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
    let Some((log, pipe)) = platform::take_stderr() else {
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
        if drain(stamping, &[]).is_err() {
            give_back(&mut held);
        }
    }
}

/// Copies what waits, then puts stderr back on the log file, for a process about to be
/// replaced by one that may not read the pipe. [`start`] takes it up again if it is not.
pub fn before_exec() {
    let mut held = stamping();
    if let Some(stamping) = held.as_mut() {
        let _ = drain(stamping, &[]);
    }
    give_back(&mut held);
}

/// The log file stderr ends in, for a process that should keep writing there after this one.
pub fn log_file() -> Option<std::fs::File> {
    let held = stamping();
    platform::copy_of(held.as_ref().map(|stamping| stamping.log))
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

/// Points stderr at the log file again and lets the pipe go.
fn give_back(held: &mut Option<Stamping>) {
    if let Some(stamping) = held.take() {
        platform::give_back(stamping.log, stamping.pipe);
    }
}

/// Copies what reaches `pipe` into the log, the time in front of each line, until stderr is
/// given back. Waiting happens unlocked; reading and writing happen under the lock.
fn copy(pipe: Handle) {
    loop {
        let waited = platform::wait(pipe);
        let mut held = stamping();
        let Some(stamping) = held.as_mut().filter(|stamping| stamping.pipe == pipe) else {
            platform::release(pipe);
            return;
        };
        let copied = waited.and_then(|first| drain(stamping, &first[..]));
        if copied.is_err() {
            give_back(&mut held);
            platform::release(pipe);
            return;
        }
    }
}

/// Copies `first` and all the pipe holds now into the log; an error once the pipe cannot be
/// read.
fn drain(stamping: &mut Stamping, first: &[u8]) -> Result<(), ()> {
    let mut out = Vec::with_capacity(platform::CHUNK + 256);
    stamp_into(stamping, first, &mut out);
    let mut buffer = [0u8; platform::CHUNK];
    loop {
        if !out.is_empty() {
            platform::write_all(stamping.log, &out);
            out.clear();
        }
        match platform::read(stamping.pipe, &mut buffer) {
            Read::Bytes(read) => stamp_into(stamping, &buffer[..read], &mut out),
            Read::Empty => return Ok(()),
            Read::Closed => return Err(()),
        }
    }
}

fn stamp_into(stamping: &mut Stamping, bytes: &[u8], out: &mut Vec<u8>) {
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !stamping.mid_line {
            out.extend_from_slice(stamp().as_bytes());
        }
        out.extend_from_slice(line);
        stamping.mid_line = line.last() != Some(&b'\n');
    }
}

/// The time now in this computer's zone: "2026-11-11 14:02:03.512 ".
fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let Some(parts) = calendar(now.as_millis() as u64, Zone::Host) else {
        return String::new();
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
