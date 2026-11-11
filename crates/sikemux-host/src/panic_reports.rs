//! Writes a report when a Sikemux process panics, which `sikemux doctor` reads back and Sentry
//! is sent. Release builds abort on a panic, and nobody reads an app's stderr, so without this
//! the message and the line that panicked are lost with the process.

use std::fs;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

use sentry::protocol::DebugImage;
use serde::{Deserialize, Serialize};

pub const FILE_PREFIX: &str = "panic-";
/// A process that panics on every start would otherwise fill the folder.
const KEPT_REPORTS: usize = 20;
const MAX_FRAMES: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanicReport {
    pub process: String,
    pub version: String,
    pub commit: String,
    pub captured_at: String,
    pub captured_at_ms: u64,
    pub thread: String,
    pub message: String,
    pub location: String,
    /// Return addresses, innermost first. A release's symbols ship apart from it, so the
    /// release's debug files name them, matched through `images`.
    pub frames: Vec<String>,
    /// Where each loaded image sat and which build it was.
    pub images: Vec<DebugImage>,
    /// Written while the person shared usage, and not yet sent to Sentry.
    #[serde(default)]
    pub pending: bool,
}

/// Records every later panic in this process to the autopsy folder, hands it to Sentry, then
/// lets it go on as before.
pub fn record_panics(process: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let written = write_report(process, info);
        previous(info);
        if let Some((file, report)) = written {
            crate::crash_reports::panicked(&file, &report);
        }
    }));
}

fn write_report(process: &str, info: &PanicHookInfo<'_>) -> Option<(PathBuf, PanicReport)> {
    let directory = crate::hang_reports::autopsy_dir()?;
    let captured_at_ms = sikemux_process::clock::epoch_ms();
    let report = PanicReport {
        process: process.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        commit: env!("SIKEMUX_BUILD_COMMIT").to_owned(),
        captured_at: sikemux_core::accounts::live::rfc3339(captured_at_ms),
        captured_at_ms,
        thread: std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned(),
        message: message(info),
        location: info
            .location()
            .map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()))
            .unwrap_or_default(),
        frames: frames(),
        images: sentry::integrations::debug_images::debug_images(),
        pending: crate::crash_reports::wanted(),
    };
    let file = directory.join(format!(
        "{FILE_PREFIX}{captured_at_ms}-{process}-{}.json",
        std::process::id()
    ));
    fs::create_dir_all(&directory).ok()?;
    save(&file, &report);
    prune(&directory);
    Some((file, report))
}

pub fn save(file: &Path, report: &PanicReport) {
    if let Ok(json) = serde_json::to_vec_pretty(report) {
        let _ = fs::write(file, json);
    }
}

fn prune(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_report(path))
        .collect::<Vec<_>>();
    // Names start with the time in milliseconds, so they sort oldest first.
    files.sort();
    for old in files.iter().rev().skip(KEPT_REPORTS) {
        let _ = fs::remove_file(old);
    }
}

fn is_report(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(FILE_PREFIX) && name.ends_with(".json"))
}

fn message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_owned())
}

#[cfg(unix)]
fn frames() -> Vec<String> {
    let mut addresses = [std::ptr::null_mut(); MAX_FRAMES];
    // SAFETY: backtrace writes at most the given count of pointers into the buffer.
    let count = unsafe { libc::backtrace(addresses.as_mut_ptr(), addresses.len() as libc::c_int) };
    addresses
        .iter()
        .take(count.max(0) as usize)
        .map(|address| format!("{:#x}", *address as usize))
        .collect()
}

#[cfg(not(unix))]
fn frames() -> Vec<String> {
    let mut frames = Vec::new();
    backtrace::trace(|frame| {
        frames.push(format!("{:#x}", frame.ip() as usize));
        frames.len() < MAX_FRAMES
    });
    frames
}

/// Reads every panic report in `directory`, newest first.
pub fn list_reports(directory: &Path) -> Vec<(PathBuf, PanicReport)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut reports = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_report(path))
        .filter_map(|path| {
            let report = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
            Some((path, report))
        })
        .collect::<Vec<(PathBuf, PanicReport)>>();
    reports.sort_by_key(|(_, report)| std::cmp::Reverse(report.captured_at_ms));
    reports
}
