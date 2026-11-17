use std::time::Duration;

/// Asked to by signal first, then forced, each with the time it gets to exit.
pub(super) const STOPS: [(fn(u32), Duration); 2] = [
    (terminate, Duration::from_secs(5)),
    (kill, Duration::from_secs(2)),
];

fn terminate(pid: u32) {
    signal(pid, libc::SIGTERM);
}

fn kill(pid: u32) {
    signal(pid, libc::SIGKILL);
}

fn signal(pid: u32, signal: i32) {
    let Ok(id) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: kill only takes integers; the pid is a single process above
    // init, so this never signals a group or every process.
    unsafe {
        libc::kill(id, signal);
    }
}
