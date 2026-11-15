//! The command runs in a job of its own, which holds every process it starts, and ending the
//! job ends them all. With `--foreground` only the command itself ends.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle};
use std::process::{Child, Command, ExitStatus};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::TerminateProcess;

pub const TERMINATE: i32 = 15;
pub const KILL: i32 = 9;

pub fn signal_named(name: &str) -> Option<i32> {
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "KILL" => 9,
        "USR1" => 10,
        "USR2" => 12,
        "TERM" => 15,
        _ => return None,
    })
}

pub fn own_group(_command: &mut Command) {}

pub enum Target {
    Process(OwnedHandle),
    Job(HANDLE),
}

// SAFETY: a job handle may be used from any thread.
unsafe impl Send for Target {}

impl Target {
    pub fn of(child: &Child, foreground: bool) -> io::Result<Self> {
        if foreground {
            return Ok(Self::Process(child.as_handle().try_clone_to_owned()?));
        }
        let process = child.as_raw_handle();
        // SAFETY: with no name or attributes the call only makes a job.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `job` was just made and `process` is the child's open handle.
        if unsafe { AssignProcessToJobObject(job, process) } == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: `job` is closed once.
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        Ok(Self::Job(job))
    }

    pub fn signal(&self, signal: i32) {
        let code = 128 + signal as u32;
        // SAFETY: both handles are this target's own and open until it drops.
        unsafe {
            match self {
                Self::Process(process) => TerminateProcess(process.as_raw_handle(), code),
                Self::Job(job) => TerminateJobObject(*job, code),
            };
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Self::Job(job) = self {
            // SAFETY: the job was made in `of` and is closed once.
            unsafe { CloseHandle(*job) };
        }
    }
}

pub fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
