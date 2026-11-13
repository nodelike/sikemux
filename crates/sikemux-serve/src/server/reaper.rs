//! Waits for the children an updated core takes over from the one it replaced. The new core
//! knows only the terminals and chats it was handed; anything else the old one started, such
//! as a helper still finishing, would stay a zombie once it exits, since nothing waits for it.

use std::time::Duration;

const LOOK_EVERY: Duration = Duration::from_secs(60);

/// The process's children now, with when each started, taken before it starts any of its own.
pub fn children_now() -> Vec<(u32, u64)> {
    let own = std::process::id();
    sikemux_process::procs::processes()
        .into_iter()
        .filter(|(_, parent, _)| *parent == own)
        .map(|(pid, _, started)| (pid, started))
        .collect()
}

/// Reaps each of `children` once it has stayed a zombie for a whole look, which a child
/// whose exit something else waits for never does.
pub fn watch(children: Vec<(u32, u64)>) {
    if children.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("sikemux-reaper".into())
        .spawn(move || {
            let mut left: Vec<(u32, u64, bool)> = children
                .into_iter()
                .map(|(pid, started)| (pid, started, false))
                .collect();
            while !left.is_empty() {
                std::thread::sleep(LOOK_EVERY);
                left.retain_mut(|(pid, started, seen)| {
                    if sikemux_process::procs::started(*pid) != Some(*started) {
                        return false;
                    }
                    if !is_zombie(*pid) {
                        return true;
                    }
                    if !*seen {
                        *seen = true;
                        return true;
                    }
                    let mut status = 0;
                    // SAFETY: waitpid with WNOHANG only collects this exited child's status.
                    unsafe { libc::waitpid(*pid as libc::pid_t, &mut status, libc::WNOHANG) };
                    eprintln!(
                        "sikemux core: reaped process {pid}, left by the core before the update"
                    );
                    false
                });
            }
        });
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The state follows the parenthesised command name, which may itself hold spaces.
    stat.rsplit_once(") ")
        .is_some_and(|(_, rest)| rest.starts_with('Z'))
}

/// Only Linux hosts have been seen keeping one.
#[cfg(not(target_os = "linux"))]
fn is_zombie(_pid: u32) -> bool {
    false
}
