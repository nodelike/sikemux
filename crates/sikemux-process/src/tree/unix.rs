pub(super) fn spawn_tree(
    command: &mut crate::command::Command,
) -> std::io::Result<crate::command::Child> {
    command.spawn()
}

pub(super) fn terminate_tree(pid: u32, force: bool) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    if pid == 0 {
        return;
    }
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: `kill` takes only integers. pid is non-zero, so this never becomes
    // kill(0), which would signal our own process group.
    unsafe {
        libc::kill(-pid, signal);
        libc::kill(pid, signal);
    }
}
