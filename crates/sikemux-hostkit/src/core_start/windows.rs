use std::time::Duration;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

/// A core has no console or window to be asked through, so it is forced at once.
pub(super) const STOPS: [(fn(u32), Duration); 1] = [(terminate, Duration::from_secs(2))];

fn terminate(pid: u32) {
    // SAFETY: the handle is checked before use and closed after; ending a process by id is
    // what the call is for.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return;
        }
        TerminateProcess(process, 1);
        CloseHandle(process);
    }
}
