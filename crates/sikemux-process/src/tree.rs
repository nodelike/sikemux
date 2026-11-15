//! Stopping a process together with everything it started.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// Makes `pid`, just started, the head of a tree that [`terminate_tree`] stops as one.
/// On Unix the caller gives the process a group of its own instead, with `process_group(0)`
/// or a terminal's session, so this does nothing there. On Windows it puts the process in a
/// job: whatever it starts from then on joins the job too.
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
