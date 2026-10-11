use std::time::Duration;

use portable_pty::Child;

// Grace window between the SIGTERM and the SIGKILL backstop in `drain`.
// Long enough for a foreground program (editor, agent, build) to catch
// SIGTERM and flush; short enough that app quit / update-relaunch doesn't
// feel laggy. One shared window, not per-PTY — see `drain`.
pub const DRAIN_GRACE: Duration = Duration::from_millis(250);

pub fn child_process_id(child: &mut Box<dyn Child + Send + Sync>) -> Option<u32> {
    child.process_id()
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int) {
    if pid == 0 {
        return;
    }
    // Negative pid targets the process group. portable_pty/forkpty makes the
    // shell the session/group leader on Unix; if that assumption ever fails,
    // Child::kill below still targets the direct child as a fallback.
    // SAFETY: kill only takes integers and touches no memory. pid is non-zero,
    // so this never becomes kill(0), which would signal our own process group.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(unix)]
pub fn terminate_process_tree(pid: u32, force: bool) {
    signal_process_group(pid, if force { libc::SIGKILL } else { libc::SIGTERM });
}

#[cfg(windows)]
pub fn terminate_process_tree(pid: u32, force: bool) {
    // ConPTY's portable child handle terminates only the direct shell. Use
    // taskkill's tree mode so foreground commands and agent subprocesses do
    // not survive an app close. Child::kill remains the fallback below.
    let mut command = sikemux_process::user_environment::command("taskkill");
    let pid = pid.to_string();
    command.args(["/PID", &pid, "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.status();
}

pub fn kill_and_reap_child(
    child: &mut Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
) -> Option<portable_pty::ExitStatus> {
    let _ = child.kill();
    if let Some(pid) = pid {
        terminate_process_tree(pid, true);
    }
    child.wait().ok()
}

pub fn terminate_and_reap_child(
    child: &mut Box<dyn Child + Send + Sync>,
    force_process_tree_after_grace: bool,
) -> Option<portable_pty::ExitStatus> {
    // For a task, do not reap an exited shell until after its process group
    // receives the force backstop: a descendant may still retain the PTY and
    // ignore SIGTERM. Non-task callers preserve the historical fast path.
    if !force_process_tree_after_grace {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
    }
    let pid = child_process_id(child);
    if let Some(pid) = pid {
        terminate_process_tree(pid, false);
    }
    std::thread::sleep(DRAIN_GRACE);
    if force_process_tree_after_grace {
        if let Some(pid) = pid {
            terminate_process_tree(pid, true);
        }
    }
    if let Ok(Some(status)) = child.try_wait() {
        return Some(status);
    }
    kill_and_reap_child(child, pid)
}

/// Owns a freshly-spawned child until the fully-initialized `Pty` takes it.
/// Child handles do not kill on drop, so every fallible setup step after spawn
/// must be guarded explicitly.
pub struct SpawnedChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl SpawnedChildGuard {
    pub fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self(Some(child))
    }

    pub fn into_inner(mut self) -> Box<dyn Child + Send + Sync> {
        self.0.take().expect("spawned child guard already empty")
    }
}

impl Drop for SpawnedChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let pid = child_process_id(&mut child);
            let _ = kill_and_reap_child(&mut child, pid);
        }
    }
}

/// Whether `shell` holds the terminal alone, so it is waiting at its prompt
/// rather than running a command.
#[cfg(unix)]
pub fn shell_at_prompt(terminal: std::os::fd::RawFd, shell: u32) -> bool {
    // SAFETY: tcgetpgrp only reads the terminal's foreground process group.
    let foreground = unsafe { libc::tcgetpgrp(terminal) };
    foreground > 0
        && foreground as u32 == shell
        && process_group_members(shell).is_some_and(|members| members == 1)
}

#[cfg(windows)]
pub fn shell_at_prompt(_terminal: i32, _shell: u32) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn process_group_members(group: u32) -> Option<usize> {
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = [0 as libc::pid_t; 64];
    // SAFETY: the buffer is valid for its full byte length.
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            group,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(&pids) as libc::c_int,
        )
    };
    if bytes < 0 {
        return None;
    }
    let listed = bytes as usize / std::mem::size_of::<libc::pid_t>();
    Some(pids[..listed].iter().filter(|&&pid| pid > 0).count())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_group_members(group: u32) -> Option<usize> {
    let mut members = 0;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // The command name in parentheses may hold spaces, so fields count from the last `)`.
        let pgrp = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(2))
            .and_then(|field| field.parse::<u32>().ok());
        if pgrp == Some(group) {
            members += 1;
        }
    }
    Some(members)
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    fn at_prompt_after(args: &[&str]) -> bool {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("open pty");
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(args);
        let mut child = pair.slave.spawn_command(command).expect("spawn shell");
        std::thread::sleep(Duration::from_millis(300));
        let pid = child.process_id().expect("shell pid");
        let terminal = pair.master.as_raw_fd().expect("master fd");
        let at_prompt = super::shell_at_prompt(terminal, pid);
        drop(pair);
        let _ = super::kill_and_reap_child(&mut child, Some(pid));
        at_prompt
    }

    #[test]
    fn an_interactive_shell_waiting_for_input_is_at_its_prompt() {
        assert!(at_prompt_after(&["-i"]));
    }

    #[test]
    fn a_shell_running_a_command_is_not_at_its_prompt() {
        assert!(!at_prompt_after(&["-c", "sleep 5; true"]));
    }
}
