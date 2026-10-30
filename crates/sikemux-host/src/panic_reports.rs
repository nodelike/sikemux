//! Writes a report when a Sikemux process panics, and reads them back for `sikemux doctor`.
//! Release builds abort on a panic, and nobody reads an app's stderr, so without this the
//! message and the line that panicked are lost with the process.

use std::fs;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_PREFIX: &str = "panic-";
/// A process that panics on every start would otherwise fill the folder.
const KEPT_REPORTS: usize = 20;

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
    /// Where the executable was loaded, which `scripts/symbolicate.sh` needs to turn the
    /// frames into lines with the release's dSYM. Only macOS records it.
    #[serde(default)]
    pub load_address: Option<String>,
    /// The executable's build UUID, which its dSYM must match.
    #[serde(default)]
    pub uuid: Option<String>,
    pub frames: Vec<String>,
}

/// Records every later panic in this process to the autopsy folder, then lets it go on
/// as before.
pub fn record_panics(process: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_report(process, info);
        previous(info);
    }));
}

fn write_report(process: &str, info: &PanicHookInfo<'_>) {
    let Some(directory) = crate::hang_reports::autopsy_dir() else {
        return;
    };
    let captured_at_ms = sikemux_process::clock::epoch_ms();
    let (load_address, uuid, frames) = frames();
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
        load_address,
        uuid,
        frames,
    };
    let Ok(json) = serde_json::to_vec_pretty(&report) else {
        return;
    };
    let file = directory.join(format!(
        "{FILE_PREFIX}{captured_at_ms}-{process}-{}.json",
        std::process::id()
    ));
    let _ = fs::create_dir_all(&directory).and_then(|()| fs::write(file, json));
    prune(&directory);
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

/// A release's symbols ship apart from it, so on macOS the frames are raw addresses for
/// the dSYM to name; elsewhere the standard backtrace names what it can.
#[cfg(target_os = "macos")]
fn frames() -> (Option<String>, Option<String>, Vec<String>) {
    extern "C" {
        fn _dyld_get_image_header(image_index: u32) -> *const u8;
    }
    let mut addresses = [std::ptr::null_mut(); 128];
    // SAFETY: backtrace writes at most the given count of pointers into the buffer.
    let count = unsafe { libc::backtrace(addresses.as_mut_ptr(), addresses.len() as libc::c_int) };
    // SAFETY: image 0 is the main executable, which stays loaded for the process's life.
    let header = unsafe { _dyld_get_image_header(0) };
    let frames = addresses[..count.max(0) as usize]
        .iter()
        .map(|address| format!("{:#x}", *address as usize))
        .collect();
    // SAFETY: dyld hands back the executable's mapped 64-bit Mach-O header.
    let uuid = unsafe { executable_uuid(header) };
    (Some(format!("{:#x}", header as usize)), uuid, frames)
}

/// Reads the build UUID from the load commands after a mapped 64-bit Mach-O header.
#[cfg(target_os = "macos")]
unsafe fn executable_uuid(header: *const u8) -> Option<String> {
    const HEADER_SIZE: usize = 32;
    const LC_UUID: u32 = 0x1b;
    let read = |at: usize| u32::from_ne_bytes(*header.add(at).cast::<[u8; 4]>());
    let mut at = HEADER_SIZE;
    for _ in 0..read(16) {
        let (command, size) = (read(at), read(at + 4) as usize);
        if command == LC_UUID {
            let bytes = *header.add(at + 8).cast::<[u8; 16]>();
            let hex = bytes
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>();
            return Some(format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            ));
        }
        at += size;
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn frames() -> (Option<String>, Option<String>, Vec<String>) {
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    (None, None, backtrace.lines().map(str::to_owned).collect())
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
