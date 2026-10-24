//! The island over the MacBook notch. The `sikemux-notch` helper draws it and
//! reads the core's device view; the core starts it once a window opens and
//! stops it when the person turns it off. It keeps running after the window
//! closes, as the core does.

use super::Core;

/// Starts or stops the helper, as the person's Notch preferences say.
pub(crate) fn follow(core: &Core) {
    #[cfg(target_os = "macos")]
    {
        let Some(socket) = core
            .listening
            .get()
            .map(|listening| listening.config.socket.clone())
        else {
            return;
        };
        let enabled = core.preferences.get().notch.enabled;
        tokio::task::spawn_blocking(move || {
            if let Err(error) = mac::configure(&socket, enabled) {
                eprintln!("sikemux core: the notch did not start: {error}");
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = core;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::server::local_peer::{BUNDLED_NOTCH, NOTCH_EXECUTABLE};

    const DEV_HELPER: &str = "Sikemux Notch Dev.app/Contents/MacOS/sikemux-notch";

    /// Set once this core has started the helper, which replaces any helper
    /// an earlier core left behind.
    static STARTED: AtomicBool = AtomicBool::new(false);

    fn dev() -> bool {
        cfg!(debug_assertions)
    }

    fn state_dir(socket: &Path) -> PathBuf {
        socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir)
    }

    fn state_file(state_dir: &Path, extension: &str) -> PathBuf {
        state_dir.join(if dev() {
            format!("notch.dev.{extension}")
        } else {
            format!("notch.{extension}")
        })
    }

    /// The helper inside its own app, which it must run from: the window server
    /// plays a background process's trackpad haptics only for a real app. A
    /// release carries it in the app bundle's Helpers; a dev build beside the
    /// core's binary. `SIKEMUX_NOTCH_EXECUTABLE` names another.
    fn helper(core: &Path) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("SIKEMUX_NOTCH_EXECUTABLE") {
            return Some(PathBuf::from(path));
        }
        let path = match sikemux_process::paths::app_bundle_of_core(core) {
            Some(bundle) => bundle.join(BUNDLED_NOTCH).join(NOTCH_EXECUTABLE),
            None if dev() => core.parent()?.join(DEV_HELPER),
            None => return None,
        };
        path.is_file().then_some(path)
    }

    /// The pid of the helper holding this build's lock, or None when none runs.
    fn running(state_dir: &Path) -> Option<libc::pid_t> {
        let path = state_file(state_dir, "lock");
        let file = std::fs::File::open(&path).ok()?;
        use std::os::fd::AsRawFd;
        // SAFETY: flock only reads the descriptor of a file this function owns.
        let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0;
        if free {
            // SAFETY: as above; releases the lock just taken.
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
            return None;
        }
        std::fs::read_to_string(&path)
            .ok()?
            .trim()
            .parse()
            .ok()
            .filter(|pid| *pid > 0)
    }

    fn stop(state_dir: &Path) {
        if let Some(pid) = running(state_dir) {
            // SAFETY: kill sends a signal to the process holding the helper's lock.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        STARTED.store(false, Ordering::Release);
    }

    fn start(socket: &Path, state_dir: &Path) -> std::io::Result<()> {
        let core = std::env::current_exe()?;
        let helper =
            helper(&core).ok_or_else(|| std::io::Error::other("this build has no notch helper"))?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(state_file(state_dir, "log"))?;
        let mut command = sikemux_process::user_environment::command(&helper);
        command
            .arg("--socket")
            .arg(socket)
            .arg("--protocol-version")
            .arg(crate::protocol::PROTOCOL_VERSION.to_string())
            .arg("--state-dir")
            .arg(state_dir)
            .arg("--core-binary")
            .arg(&core);
        if let Some(bundle) = sikemux_process::paths::app_bundle_of_core(&core) {
            command.arg("--app").arg(bundle);
        }
        if dev() {
            command
                .arg("--dev")
                .arg("--app-pid")
                .arg(std::process::id().to_string());
        }
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        // SAFETY: the hook runs in the forked child before exec and calls only
        // fork, setsid and _exit, which are async-signal-safe. The helper is never
        // the core's child, so it outlives the core and is never left a zombie.
        unsafe {
            command.pre_exec(|| match libc::fork() {
                -1 => Err(std::io::Error::last_os_error()),
                0 => {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                }
                _ => libc::_exit(0),
            });
        }
        command.spawn()?.wait()?;
        STARTED.store(true, Ordering::Release);
        Ok(())
    }

    /// The helper is SwiftUI for macOS 14, while the core runs on older systems.
    fn supported() -> bool {
        crate::usage::os_version()
            .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
            .is_some_and(|major| major >= 14)
    }

    pub(super) fn configure(socket: &Path, enabled: bool) -> std::io::Result<()> {
        if !supported() {
            return Ok(());
        }
        let state_dir = state_dir(socket);
        if !enabled {
            stop(&state_dir);
            return Ok(());
        }
        if !STARTED.load(Ordering::Acquire) || running(&state_dir).is_none() {
            start(socket, &state_dir)?;
        }
        Ok(())
    }
}
