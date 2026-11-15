use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};

pub const TERMINATE: i32 = libc::SIGTERM;
pub const KILL: i32 = libc::SIGKILL;

pub fn signal_named(name: &str) -> Option<i32> {
    Some(match name {
        "TERM" => libc::SIGTERM,
        "KILL" => libc::SIGKILL,
        "INT" => libc::SIGINT,
        "HUP" => libc::SIGHUP,
        "QUIT" => libc::SIGQUIT,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        _ => return None,
    })
}

pub fn own_group(command: &mut Command) {
    command.process_group(0);
}

/// The child, or its whole process group when it leads one.
pub struct Target(libc::pid_t);

impl Target {
    pub fn of(child: &Child, foreground: bool) -> io::Result<Self> {
        let pid = child.id() as libc::pid_t;
        Ok(Self(if foreground { pid } else { -pid }))
    }

    pub fn signal(&self, signal: i32) {
        // SAFETY: kill only sends a signal to the child or its group.
        unsafe { libc::kill(self.0, signal) };
    }
}

pub fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}
