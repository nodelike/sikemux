use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};

pub const SERVICE_NAME: &str = "SikemuxSpikeSvc";
pub const SVC_PIPE: &str = r"\\.\pipe\sikemux-spike-service";

pub fn install_dir() -> PathBuf {
    PathBuf::from(r"C:\Program Files\SikemuxSpike")
}
pub fn install_exe() -> PathBuf {
    install_dir().join("spike.exe")
}
pub fn data_dir() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\SikemuxSpike")
}
pub fn secrets_dir() -> PathBuf {
    data_dir().join("secrets")
}
pub fn log_path() -> PathBuf {
    data_dir().join("service.log")
}

pub fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

pub fn from_wide_ptr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::ffi::OsString::from_wide(slice).to_string_lossy().into_owned()
}

pub fn errno() -> u32 {
    unsafe { GetLastError() }
}

pub fn is_valid(h: HANDLE) -> bool {
    !h.is_null() && h != INVALID_HANDLE_VALUE
}

/// A HANDLE that can cross a thread boundary. Windows handles are plain kernel
/// handle values, so moving the number between threads is sound.
#[derive(Copy, Clone)]
pub struct SendHandle(pub HANDLE);
unsafe impl Send for SendHandle {}

pub fn as_ptr(h: HANDLE) -> *mut c_void {
    h
}

/// Append one line to the shared log. Every mode writes here; the driver reads
/// the whole file at the end and greps for the keys it expects.
pub fn report(mode: &str, key: &str, value: &str) {
    let line = format!("[{mode}] {key}={value}\n");
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
    eprint!("{line}");
}

pub fn report_err(mode: &str, key: &str, context: &str) {
    report(mode, key, &format!("err({}:{})", context, errno()));
}
