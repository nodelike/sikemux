use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
};
use windows_sys::Win32::System::LibraryLoader::{
    SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

/// Loads libraries named without a folder only from Windows' own and this program's, never from
/// PATH or the current folder, which the person's other programs can write to.
pub fn pin_library_search() -> io::Result<()> {
    // SAFETY: takes plain flags and changes only where later loads look.
    let pinned = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR)
    };
    if pinned == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Every library loaded into this process, by the path it was loaded from.
pub fn loaded_libraries() -> io::Result<Vec<PathBuf>> {
    // SAFETY: takes plain values; a snapshot of this process's own libraries needs no rights.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, GetCurrentProcessId()) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the snapshot is open and owned here alone.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let mut libraries = Vec::new();
    // SAFETY: the snapshot is open and `entry` is sized as the call requires.
    let mut more = unsafe { Module32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        let path = &entry.szExePath;
        let length = path
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(path.len());
        libraries.push(PathBuf::from(OsString::from_wide(&path[..length])));
        // SAFETY: as for the first entry.
        more = unsafe { Module32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(libraries)
}
