//! Where the `sikemux` CLI lives and where its endpoint file is published.

use std::fs;
use std::path::{Path, PathBuf};

/// The endpoint file the core publishes for this app. See
/// [`sikemux_core::cli::endpoint::default_endpoint_path`] for why
/// `SIKEMUX_CLI_ENDPOINT` is not read here.
pub fn cli_endpoint_path() -> Option<PathBuf> {
    sikemux_core::cli::endpoint::default_endpoint_path()
}

/// The sidecar bundled beside this app. `SIKEMUX_BIN_PATH` is not read here:
/// every Sikemux terminal sets it, so an app started from one would run another
/// build's sidecar. Tests point the app elsewhere with `SIKEMUX_SIDECAR_PATH`.
pub fn cli_executable_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SIKEMUX_SIDECAR_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let current = std::env::current_exe().ok()?;
    let filename = if cfg!(windows) {
        "sikemux-editor.exe"
    } else {
        "sikemux-editor"
    };
    let sibling = current.parent()?.join(filename);
    sibling.is_file().then_some(sibling)
}

static CLI_ON_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Agents shell out to `sikemux`, but the packaged CLI is named
/// `sikemux-editor`, so a `sikemux` link to it goes first on PATH for every
/// process this app launches. Call once at startup.
pub fn link_cli_for_children() {
    let Some(executable) = cli_executable_path() else {
        return;
    };
    let Some(directory) = crate::state::state_path().and_then(|path| {
        Some(path.parent()?.join(if cfg!(debug_assertions) {
            "bin-dev"
        } else {
            "bin"
        }))
    }) else {
        return;
    };
    if let Ok(link) = link_cli(&executable, &directory) {
        let _ = CLI_ON_PATH.set(link);
    }
}

pub fn cli_link_directory() -> Option<&'static Path> {
    CLI_ON_PATH.get()?.parent()
}

pub fn cli_command_path() -> Option<PathBuf> {
    CLI_ON_PATH.get().cloned().or_else(cli_executable_path)
}

fn link_cli(executable: &Path, directory: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let link = directory.join(if cfg!(windows) {
        "sikemux.exe"
    } else {
        "sikemux"
    });
    #[cfg(unix)]
    {
        if fs::read_link(&link).is_ok_and(|target| target == executable) {
            return Ok(link);
        }
        let staging = directory.join(format!(".sikemux-{}", std::process::id()));
        let _ = fs::remove_file(&staging);
        std::os::unix::fs::symlink(executable, &staging)?;
        fs::rename(&staging, &link)?;
    }
    #[cfg(windows)]
    fs::copy(executable, &link)?;
    Ok(link)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliRuntimeInfo {
    endpoint: String,
    executable: Option<String>,
}

#[tauri::command]
pub fn cli_runtime_info() -> CliRuntimeInfo {
    CliRuntimeInfo {
        endpoint: cli_endpoint_path()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        executable: cli_executable_path().map(|path| path.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_cli_is_linked_as_sikemux_and_relinked_when_it_moves() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first/sikemux-editor");
        let second = root.path().join("second/sikemux-editor");
        let directory = root.path().join("bin");
        let link = link_cli(&first, &directory).unwrap();
        assert_eq!(link, directory.join("sikemux"));
        assert_eq!(fs::read_link(&link).unwrap(), first);
        assert_eq!(link_cli(&first, &directory).unwrap(), link);
        link_cli(&second, &directory).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), second);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }

    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_second_app_started_from_our_terminal_keeps_its_own_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|error| error.into_inner());
        // What a terminal we spawned carries, so a Sikemux launched there
        // inherits it. Publishing to it would steal our agents' endpoint.
        std::env::set_var("SIKEMUX_CLI_ENDPOINT", "/tmp/sikemux-other-instance.json");
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT_PUBLISH");

        let published = cli_endpoint_path().expect("an endpoint path");

        assert_ne!(published, Path::new("/tmp/sikemux-other-instance.json"));
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT");
    }

    #[test]
    fn the_publish_override_is_honoured() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|error| error.into_inner());
        std::env::set_var(
            "SIKEMUX_CLI_ENDPOINT_PUBLISH",
            "/tmp/sikemux-published-here.json",
        );

        let published = cli_endpoint_path().expect("an endpoint path");

        assert_eq!(published, Path::new("/tmp/sikemux-published-here.json"));
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT_PUBLISH");
    }
}
