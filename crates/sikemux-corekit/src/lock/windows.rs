use std::fs::File;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    LockFileEx, UnlockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
use windows_sys::Win32::System::IO::OVERLAPPED;

// Windows locks bar reading the bytes they cover, so the lock sits on the last byte a file
// can address, leaving the holder's pid at the start readable.
fn last_byte() -> OVERLAPPED {
    // SAFETY: OVERLAPPED is plain integers and a handle, for which all zeroes is valid.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.Anonymous.Anonymous.Offset = u32::MAX;
    overlapped.Anonymous.Anonymous.OffsetHigh = u32::MAX;
    overlapped
}

fn lock(file: &File, flags: u32) -> bool {
    let mut overlapped = last_byte();
    // SAFETY: the handle stays open for the call and `overlapped` outlives it.
    unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut overlapped) != 0 }
}

pub fn hold(file: &File) -> bool {
    lock(file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)
}

pub fn held_elsewhere(file: &File) -> bool {
    if !lock(file, LOCKFILE_FAIL_IMMEDIATELY) {
        return true;
    }
    let mut overlapped = last_byte();
    // SAFETY: the handle stays open for the call and `overlapped` outlives it.
    unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    false
}
