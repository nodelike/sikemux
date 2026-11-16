use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_VM_READ};

use crate::core::il_name;
use crate::sd::current_integrity;
use crate::util::report;

/// Spawned by a High core with a Medium, admin-denied token. It confirms it
/// landed at Medium and that the core's process guard still refuses it a read
/// handle on itself.
pub fn run() {
    report(
        "child",
        "child_il",
        il_name(current_integrity().unwrap_or(0)),
    );

    let pid = unsafe { GetCurrentProcessId() };
    let h = unsafe { OpenProcess(PROCESS_VM_READ, 0, pid) };
    report(
        "child",
        "child_self_open",
        if h.is_null() { "denied" } else { "allowed" },
    );
    if !h.is_null() {
        unsafe { CloseHandle(h) };
    }

    let wrote = std::env::var("LOCALAPPDATA")
        .ok()
        .map(|l| std::path::Path::new(&l).join("SikemuxSpike/child.txt"))
        .map(|p| std::fs::write(p, "child").is_ok())
        .unwrap_or(false);
    report(
        "child",
        "child_user_write",
        if wrote { "allowed" } else { "denied" },
    );
}
