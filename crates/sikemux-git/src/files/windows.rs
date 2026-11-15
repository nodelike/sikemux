use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

pub(super) fn stamp(meta: &fs::Metadata) -> String {
    format!(
        "{}-{}-{}",
        meta.file_size(),
        meta.last_write_time(),
        meta.creation_time()
    )
}

/// Files on Windows are never named pipes, so a plain open cannot hang.
pub(super) fn open_for_reading(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

/// Without MOVEFILE_REPLACE_EXISTING, Windows refuses to move onto an existing entry.
pub(super) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain([0]).collect() };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
