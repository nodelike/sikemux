//! Stopping a process together with everything it started.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// Starts `command` as the head of a tree that [`terminate_tree`] stops as one: in a process
/// group of its own on Unix, in a job on Windows. Every child the core may later stop starts
/// here.
pub fn spawn_tree(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let child = spawn_hearing_ctrl_c(|| command.spawn())?;
    own_tree(child.id());
    Ok(child)
}

/// Runs `spawn`, whose child then hears Ctrl-C. On Windows a child inherits whether this
/// process ignores Ctrl-C, and this process does once it has asked a tree to stop, so a
/// terminal's shell starts here too. Unix has no such inheritance.
pub fn spawn_hearing_ctrl_c<T>(spawn: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    return platform::spawn_hearing_ctrl_c(spawn);
    #[cfg(unix)]
    spawn()
}

/// Makes `pid`, just started by another library, the head of a tree as [`spawn_tree`] does,
/// such as a terminal's shell, which leads a session of its own on Unix, so this does nothing
/// there.
pub fn own_tree(pid: u32) {
    platform::own_tree(pid);
}

/// Asks the process `pid` and everything it started to stop, or makes them when `force`.
/// The process must not be reaped yet, so its pid still belongs to it. On Unix it must lead
/// its own group; on Windows the tree is the job [`own_tree`] made, else every process
/// descended from it. Asking is SIGTERM on Unix and Ctrl-C into the tree's console on
/// Windows, which a program reads the way it reads the key.
pub fn terminate_tree(pid: u32, force: bool) {
    platform::terminate_tree(pid, force);
}
