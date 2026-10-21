//! The island over the MacBook notch. The `sikemux-notch` helper draws it and
//! reads the core's device view; the app gives it the core's binary, whose
//! `start` runs the core as the app would. It keeps running after the app
//! quits, as the core does.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::pty::PtyManager;

/// The Notch section of the settings, named in `src/notch/settings.json`.
/// The app only starts or stops the helper; the helper reads the rest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NotchSettings {
    enabled: bool,
    #[serde(flatten)]
    helper: serde_json::Map<String, serde_json::Value>,
}

/// Writes the settings the helper reads, then starts or stops it to match.
#[tauri::command]
pub async fn notch_configure(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    settings: NotchSettings,
) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    return mac::configure(&app, &manager, &settings);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, manager, settings);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tauri::{AppHandle, Manager};

    use super::NotchSettings;
    use crate::error::{AppError, AppResult};
    use crate::login_item::CoreLaunch;
    use crate::pty::PtyManager;

    /// Set once this run of the app has started the helper, which replaces any
    /// helper an earlier run left behind.
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

    fn settings_path(state_dir: &Path) -> PathBuf {
        state_dir.join(if dev() {
            "notch.dev.json"
        } else {
            "notch.json"
        })
    }

    fn lock_path(state_dir: &Path) -> PathBuf {
        state_dir.join(if dev() {
            "notch.dev.lock"
        } else {
            "notch.lock"
        })
    }

    /// The helper built beside the app, or the one `SIKEMUX_NOTCH_EXECUTABLE` names.
    /// The helper inside its own app, which it must run from: the window server
    /// plays a background process's trackpad haptics only for a real app. A
    /// release carries it in Contents/Helpers; a dev build beside its binary.
    fn helper() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("SIKEMUX_NOTCH_EXECUTABLE") {
            return Some(PathBuf::from(path));
        }
        let macos = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let executable = |app: PathBuf| app.join("Contents/MacOS/sikemux-notch");
        let candidates = if dev() {
            vec![executable(macos.join("Sikemux Notch Dev.app"))]
        } else {
            vec![executable(
                macos.parent()?.join("Helpers/Sikemux Notch.app"),
            )]
        };
        candidates.into_iter().find(|path| path.is_file())
    }

    /// The app bundle, which the helper opens when the window it asks for is closed.
    fn bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe.parent()?.parent()?.parent()?;
        (bundle.extension()? == "app").then(|| bundle.to_path_buf())
    }

    /// The pid of the helper holding this build's lock, or None when none runs.
    fn running(state_dir: &Path) -> Option<libc::pid_t> {
        let path = lock_path(state_dir);
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

    fn start(app: &AppHandle, launch: &CoreLaunch) -> AppResult<()> {
        let helper =
            helper().ok_or_else(|| AppError::Other("this build has no notch helper".into()))?;
        let state_dir = state_dir(&launch.socket);
        let log_path = app
            .path()
            .app_log_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("notch.log");
        if let Some(directory) = log_path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)?;
        let mut command = sikemux_process::user_environment::command(&helper);
        command
            .arg("--socket")
            .arg(&launch.socket)
            .arg("--protocol-version")
            .arg(sikemux_core::protocol::PROTOCOL_VERSION.to_string())
            .arg("--settings")
            .arg(settings_path(&state_dir))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--core-binary")
            .arg(&launch.binary);
        if let Some(bundle) = bundle() {
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
        // this app's child, so it outlives the app and is never left a zombie.
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

    /// Writes the settings the helper reads, then starts or stops it to match.
    /// The helper is SwiftUI for macOS 14, while the app runs on older systems.
    fn supported() -> bool {
        objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
            >= 14
    }

    pub(super) fn configure(
        app: &AppHandle,
        manager: &PtyManager,
        settings: &NotchSettings,
    ) -> AppResult<()> {
        if !supported() {
            return Ok(());
        }
        let launch = manager
            .core_launch()
            .ok_or_else(|| AppError::Other("the terminal core is not configured yet".into()))?;
        let state_dir = state_dir(&launch.socket);
        sikemux_process::files::write_atomic(
            &settings_path(&state_dir),
            &serde_json::to_vec_pretty(settings)?,
            false,
        )?;
        if !settings.enabled {
            stop(&state_dir);
            return Ok(());
        }
        if !STARTED.load(Ordering::Acquire) || running(&state_dir).is_none() {
            start(app, &launch)?;
        }
        Ok(())
    }
}
