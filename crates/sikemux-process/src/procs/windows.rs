use windows_sys::Win32::Foundation::{CloseHandle, FALSE, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

struct Entry {
    pid: u32,
    parent: u32,
    name: String,
}

/// Every process, from one snapshot.
fn entries() -> Vec<Entry> {
    // SAFETY: takes plain values; the result is a handle we own, or INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entries = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the snapshot is open and `entry` has its size set, as both calls require.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        entries.push(Entry {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..length]),
        });
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is open and owned here.
    unsafe { CloseHandle(snapshot) };
    entries
}

fn entry(pid: u32) -> Option<Entry> {
    entries().into_iter().find(|entry| entry.pid == pid)
}

pub(super) fn footprint(_pid: u32) -> Option<u64> {
    None
}

/// When `pid` was created, in 100 ns ticks since 1601.
pub(super) fn started(pid: u32) -> Option<u64> {
    // SAFETY: OpenProcess takes plain values and returns a handle we own, or null.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
    if process.is_null() {
        return None;
    }
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: the handle is open with PROCESS_QUERY_LIMITED_INFORMATION and every
    // pointer is valid for a write.
    let read = unsafe { GetProcessTimes(process, created, exited, kernel, user) } != 0;
    // SAFETY: the handle is open and owned here.
    unsafe { CloseHandle(process) };
    let created = times[0];
    read.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn parent(pid: u32) -> Option<(u32, u64)> {
    Some((entry(pid)?.parent, started(pid)?))
}

/// The program's file name without `.exe`, as Unix names a process.
pub(super) fn name(pid: u32) -> Option<String> {
    let name = entry(pid)?.name;
    Some(match name.len().checked_sub(4) {
        Some(stem) if name[stem..].eq_ignore_ascii_case(".exe") => name[..stem].to_string(),
        _ => name,
    })
}

/// Every process with its parent and start; one this user may not look at started at 0.
pub(super) fn processes() -> Vec<(u32, u32, u64)> {
    entries()
        .into_iter()
        .map(|entry| (entry.pid, entry.parent, started(entry.pid).unwrap_or(0)))
        .collect()
}

pub(super) fn environment_value(_pid: u32, _key: &str) -> Option<String> {
    None
}

pub(super) fn usages() -> Vec<super::Usage> {
    Vec::new()
}

pub(super) fn command_line(_pid: u32) -> Option<Vec<String>> {
    None
}

pub(super) fn places() -> Option<Vec<super::Place>> {
    None
}
