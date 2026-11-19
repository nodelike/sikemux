use std::os::windows::io::FromRawHandle;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileType, ReadFile, WriteFile, FILE_TYPE_DISK};
use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// A handle kept as a number, so the copying thread can own one.
pub type Handle = usize;

pub const CHUNK: usize = 16 * 1024;

pub enum Read {
    Bytes(usize),
    Empty,
    Closed,
}

fn raw(handle: Handle) -> HANDLE {
    handle as HANDLE
}

fn duplicate(handle: HANDLE) -> Option<HANDLE> {
    let mut copy = null_mut();
    // SAFETY: duplicates a handle this process holds into a new one it owns.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut copy,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    (duplicated != 0).then_some(copy)
}

/// Keeps the log file stderr names and puts a pipe in its place, answering both.
pub fn take_stderr() -> Option<(Handle, Handle)> {
    // SAFETY: each call is checked, and only handles this function made are closed.
    unsafe {
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        if stderr.is_null()
            || stderr == INVALID_HANDLE_VALUE
            || GetFileType(stderr) != FILE_TYPE_DISK
        {
            return None;
        }
        let log = duplicate(stderr)?;
        let (mut read, mut write) = (null_mut(), null_mut());
        if CreatePipe(&mut read, &mut write, std::ptr::null(), 0) == 0 {
            CloseHandle(log);
            return None;
        }
        if SetStdHandle(STD_ERROR_HANDLE, write) == 0 {
            CloseHandle(read);
            CloseHandle(write);
            CloseHandle(log);
            return None;
        }
        Some((log as Handle, read as Handle))
    }
}

/// The pipe's reading end stays open for the copying thread, which may still be waiting on
/// it; closing the writing end is what wakes it to let go. Once given back, it does nothing.
pub fn give_back(log: Handle, _pipe: Handle) {
    // SAFETY: the writing end is the one take_stderr put in place, and only it is closed.
    unsafe {
        let write = GetStdHandle(STD_ERROR_HANDLE);
        if write == raw(log) {
            return;
        }
        SetStdHandle(STD_ERROR_HANDLE, raw(log));
        CloseHandle(write);
    }
}

pub fn release(pipe: Handle) {
    // SAFETY: the copying thread owns the reading end once stderr is given back.
    unsafe { CloseHandle(raw(pipe)) };
}

pub fn copy_of(log: Option<Handle>) -> Option<std::fs::File> {
    // SAFETY: GetStdHandle only reads this process's standard handle.
    let handle = log.map_or_else(|| unsafe { GetStdHandle(STD_ERROR_HANDLE) }, raw);
    let copy = duplicate(handle)?;
    // SAFETY: `copy` is a new handle nothing else owns.
    Some(unsafe { std::fs::File::from_raw_handle(copy) })
}

/// Waits for the pipe's next byte, which it returns, as a pipe on Windows cannot be waited on
/// without reading.
pub fn wait(pipe: Handle) -> Result<Vec<u8>, ()> {
    let mut byte = [0u8; 1];
    let mut read = 0;
    // SAFETY: `byte` has room for the one byte asked for.
    let ok = unsafe { ReadFile(raw(pipe), byte.as_mut_ptr(), 1, &mut read, null_mut()) };
    if ok == 0 || read == 0 {
        return Err(());
    }
    Ok(byte.to_vec())
}

pub fn read(pipe: Handle, buffer: &mut [u8]) -> Read {
    let mut waiting = 0;
    // SAFETY: PeekNamedPipe only reports how many bytes wait, into `waiting`.
    let peeked = unsafe {
        PeekNamedPipe(
            raw(pipe),
            null_mut(),
            0,
            null_mut(),
            &mut waiting,
            null_mut(),
        )
    };
    if peeked == 0 {
        return Read::Closed;
    }
    if waiting == 0 {
        return Read::Empty;
    }
    let asked = (waiting as usize).min(buffer.len()) as u32;
    let mut read = 0;
    // SAFETY: `buffer` has room for the `asked` bytes, which are already waiting.
    let ok = unsafe { ReadFile(raw(pipe), buffer.as_mut_ptr(), asked, &mut read, null_mut()) };
    if ok == 0 || read == 0 {
        return Read::Closed;
    }
    Read::Bytes(read as usize)
}

pub fn write_all(log: Handle, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let mut written = 0;
        let asked = bytes.len().min(u32::MAX as usize) as u32;
        // SAFETY: `bytes` is valid for `asked` bytes.
        let ok = unsafe { WriteFile(raw(log), bytes.as_ptr(), asked, &mut written, null_mut()) };
        if ok == 0 || written == 0 {
            return;
        }
        bytes = bytes.get(written as usize..).unwrap_or_default();
    }
}
