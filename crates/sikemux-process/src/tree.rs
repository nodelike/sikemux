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
pub fn spawn_tree(command: &mut crate::command::Command) -> std::io::Result<crate::command::Child> {
    #[cfg(unix)]
    command.process_group(0);
    spawn_hearing_ctrl_c(|| platform::spawn_tree(command))
}

/// Runs `spawn`, whose child then hears Ctrl-C. On Windows a child inherits whether this
/// process ignores Ctrl-C, and this process does once it has asked a tree to stop. Unix has
/// no such inheritance.
fn spawn_hearing_ctrl_c<T>(spawn: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    return platform::spawn_hearing_ctrl_c(spawn);
    #[cfg(unix)]
    spawn()
}

/// Asks the process `pid` and everything it started to stop, or makes them when `force`.
/// The process must not be reaped yet, so its pid still belongs to it. On Unix it must lead
/// its own group; on Windows the tree is the job [`spawn_tree`] made, else every process
/// descended from it. Asking is SIGTERM on Unix and Ctrl-C into the tree's console on
/// Windows, which a program reads the way it reads the key.
pub fn terminate_tree(pid: u32, force: bool) {
    platform::terminate_tree(pid, force);
}
