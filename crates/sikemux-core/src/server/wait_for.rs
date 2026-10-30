//! `events_wait` on a file or a process: waits on work started outside
//! Sikemux's tasks, such as a background job writing its output to a file, a
//! log, or a process, so an agent need not sleep and look again.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::cli::protocol::HarnessRequest;

const DEFAULT_WAIT_MS: u64 = 30_000;
const MAX_WAIT_MS: u64 = 30_000;
/// How long a file must stay unchanged to count as finished.
const QUIET: Duration = Duration::from_secs(2);
/// A file watch can miss a write, so the file is looked at this often too.
const LOOK_AGAIN: Duration = Duration::from_secs(1);
const PROCESS_POLL: Duration = Duration::from_millis(250);
const TAIL_BYTES: u64 = 2000;

/// Whether an `events_wait` call waits on a file or a process rather than
/// on events.
pub(crate) fn asked(params: &Value) -> bool {
    params.get("file").is_some() || params.get("pid").is_some()
}

pub(crate) async fn call(request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    if params.get("cursor").is_some() {
        return Err("pass a cursor for task or UI events, or a file or pid, not both".into());
    }
    let file = params
        .get("file")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let pid = params
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| i32::try_from(pid).ok());
    if text.is_some() && file.is_none() {
        return Err("text is looked for in a file, so pass the file too".into());
    }
    if file.as_deref().is_some_and(|file| !file.is_absolute()) {
        return Err("file must be an absolute path".into());
    }
    let limit = Duration::from_millis(
        params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_WAIT_MS)
            .min(MAX_WAIT_MS),
    );
    let started = Instant::now();
    let deadline = started + limit;
    let changed = Arc::new(Notify::new());
    let _watch = file
        .as_deref()
        .and_then(|file| watch(file, changed.clone()));
    let mut scan = file.map(|file| Scan::new(file, text));
    loop {
        let mut failing = Vec::new();
        let mut wake = deadline.min(Instant::now() + LOOK_AGAIN);
        if let Some(scan) = &mut scan {
            match scan.look() {
                Ok(Looked::Met) => {}
                Ok(Looked::QuietAt(at)) => {
                    failing.push(format!(
                        "{} changed under {}s ago",
                        scan.shown(),
                        QUIET.as_secs()
                    ));
                    wake = wake.min(at);
                }
                Ok(Looked::Missing) => failing.push(format!("{} does not exist yet", scan.shown())),
                Ok(Looked::Lacks) => failing.push(format!(
                    "{} does not hold {:?} yet",
                    scan.shown(),
                    scan.text.as_deref().unwrap_or_default()
                )),
                Err(error) => return Err(error),
            }
        }
        if let Some(pid) = pid {
            if running(pid) {
                failing.push(format!("process {pid} is still running"));
                wake = wake.min(Instant::now() + PROCESS_POLL);
            }
        }
        let waited = started.elapsed().as_millis() as u64;
        if failing.is_empty() || Instant::now() >= deadline {
            let mut answer = json!({ "met": failing.is_empty(), "waitedMs": waited });
            if !failing.is_empty() {
                answer["failing"] = json!(failing);
            }
            if let Some(tail) = scan.as_ref().and_then(Scan::tail) {
                answer["tail"] = json!(tail);
            }
            return Ok(answer);
        }
        tokio::select! {
            () = changed.notified() => {}
            () = tokio::time::sleep_until(wake.into()) => {}
        }
    }
}

/// Watches the folder holding `file`, which may not exist yet.
fn watch(file: &Path, changed: Arc<Notify>) -> Option<RecommendedWatcher> {
    let folder = file.parent()?.to_owned();
    let name = file.file_name()?.to_owned();
    let mut watcher =
        ::notify::recommended_watcher(move |event: ::notify::Result<::notify::Event>| {
            if event.is_ok_and(|event| {
                event
                    .paths
                    .iter()
                    .any(|path| path.file_name() == Some(&name))
            }) {
                changed.notify_one();
            }
        })
        .ok()?;
    watcher.watch(&folder, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}

fn running(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists; nothing is sent.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

enum Looked {
    Met,
    Missing,
    Lacks,
    /// Still changing; quiet enough once this passes without a change.
    QuietAt(Instant),
}

/// Reads a file as it grows: only what was added since the last look, kept
/// short of the text's length so a match across two reads is still found.
struct Scan {
    file: PathBuf,
    text: Option<String>,
    read_to: u64,
    carry: Vec<u8>,
    last: Option<(u64, std::time::SystemTime)>,
    changed_at: Instant,
}

impl Scan {
    fn new(file: PathBuf, text: Option<String>) -> Self {
        Self {
            file,
            text,
            read_to: 0,
            carry: Vec::new(),
            last: None,
            changed_at: Instant::now(),
        }
    }

    fn shown(&self) -> String {
        self.file.display().to_string()
    }

    fn look(&mut self) -> Result<Looked, String> {
        let Ok(meta) = std::fs::metadata(&self.file) else {
            return Ok(Looked::Missing);
        };
        let stamp = (meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH));
        if self.last != Some(stamp) {
            let age = match self.last {
                None => stamp.1.elapsed().unwrap_or_default(),
                Some(_) => Duration::ZERO,
            };
            self.last = Some(stamp);
            self.changed_at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
        }
        let Some(text) = self.text.clone() else {
            let quiet_at = self.changed_at + QUIET;
            return Ok(match Instant::now() >= quiet_at {
                true => Looked::Met,
                false => Looked::QuietAt(quiet_at),
            });
        };
        if meta.len() < self.read_to {
            self.read_to = 0;
            self.carry.clear();
        }
        let mut opened = std::fs::File::open(&self.file).map_err(|error| error.to_string())?;
        opened
            .seek(SeekFrom::Start(self.read_to))
            .map_err(|error| error.to_string())?;
        let mut added = Vec::new();
        opened
            .read_to_end(&mut added)
            .map_err(|error| error.to_string())?;
        self.read_to += added.len() as u64;
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(&added);
        let wanted = text.as_bytes();
        if window.windows(wanted.len()).any(|part| part == wanted) {
            return Ok(Looked::Met);
        }
        let keep = window.len().saturating_sub(wanted.len().saturating_sub(1));
        self.carry = window.split_off(keep);
        Ok(Looked::Lacks)
    }

    /// The end of the file, from the start of a line.
    fn tail(&self) -> Option<String> {
        let mut opened = std::fs::File::open(&self.file).ok()?;
        let length = opened.metadata().ok()?.len();
        opened
            .seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES)))
            .ok()?;
        let mut end = Vec::new();
        opened.read_to_end(&mut end).ok()?;
        let end = String::from_utf8_lossy(&end);
        let end = match length > TAIL_BYTES {
            true => end.split_once('\n').map_or(&*end, |(_, rest)| rest),
            false => &end,
        };
        Some(end.to_owned()).filter(|end| !end.is_empty())
    }
}
