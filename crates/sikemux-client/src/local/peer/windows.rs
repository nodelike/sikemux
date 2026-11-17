use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::local::Connection;

/// The pipe names its client by pid alone, so this keeps when the connection was taken up: a
/// process started after that has only been given the pid of one that left.
pub struct Peer {
    pid: u32,
    connected: u64,
}

fn ticks(time: FILETIME) -> u64 {
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}

impl Peer {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// A process cannot run another program on Windows, so this is what connected.
    pub fn program(&self) -> Option<PathBuf> {
        // SAFETY: OpenProcess takes plain values; the handle is closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, self.pid) };
        if process.is_null() {
            return None;
        }
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: `process` is open, and the call writes four FILETIMEs.
        let timed = unsafe { GetProcessTimes(process, created, exited, kernel, user) } != 0;
        let mut path = vec![0u16; 32 * 1024];
        let mut length = path.len() as u32;
        // SAFETY: `path` is writable for `length` characters, and the call writes at most
        // that many and the length it wrote.
        let named = unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut length)
        } != 0;
        // SAFETY: `process` was opened above and is closed once.
        unsafe { CloseHandle(process) };
        if !timed || ticks(times[0]) > self.connected || !named {
            return None;
        }
        path.truncate(length as usize);
        Some(PathBuf::from(OsString::from_wide(&path)))
    }
}

pub fn peer(connection: &Connection) -> Option<Peer> {
    let mut pid = 0u32;
    // SAFETY: the pipe's handle is open while `connection` lives, and the call writes one u32.
    let read = unsafe { GetNamedPipeClientProcessId(connection.as_raw_handle(), &mut pid) };
    let mut now = FILETIME::default();
    // SAFETY: the call writes one FILETIME.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    (read != 0 && pid != 0).then(|| Peer {
        pid,
        connected: ticks(now),
    })
}
