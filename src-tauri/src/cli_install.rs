use std::env;
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CliInstallState {
    Unavailable,
    NotInstalled,
    Installed,
    Outdated,
    Conflict,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallStatus {
    state: CliInstallState,
    install_dir: String,
    cli_path: String,
    editor_path: String,
    executable: Option<String>,
    path_configured: bool,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationState {
    Missing,
    Installed,
    Outdated,
    Conflict,
}

#[tauri::command]
pub fn cli_install_status() -> CliInstallStatus {
    status_for(crate::cli_server::cli_executable_path())
}

#[tauri::command]
pub fn cli_install() -> AppResult<CliInstallStatus> {
    let executable = crate::cli_server::cli_executable_path().ok_or_else(|| {
        AppError::State("the packaged Sikemux CLI is unavailable in this build".into())
    })?;
    install_for(&executable)?;
    Ok(status_for(Some(executable)))
}

fn status_for(executable: Option<PathBuf>) -> CliInstallStatus {
    let install_dir = install_dir().unwrap_or_default();
    let (cli_path, editor_path) = destination_paths(&install_dir);
    let path_configured = path_contains(&install_dir);
    let Some(executable) = executable.filter(|path| path.is_file()) else {
        return make_status(
            CliInstallState::Unavailable,
            install_dir,
            cli_path,
            editor_path,
            None,
            path_configured,
            "The CLI is included in packaged builds; it is unavailable from this development executable.",
        );
    };

    let cli_state = destination_state(&cli_path, &executable);
    let editor_state = destination_state(&editor_path, &executable);
    let install_state = if cli_state == DestinationState::Conflict
        || editor_state == DestinationState::Conflict
    {
        CliInstallState::Conflict
    } else if cli_state == DestinationState::Installed
        && editor_state == DestinationState::Installed
    {
        CliInstallState::Installed
    } else if cli_state == DestinationState::Outdated || editor_state == DestinationState::Outdated
    {
        CliInstallState::Outdated
    } else {
        CliInstallState::NotInstalled
    };
    let message = match install_state {
        CliInstallState::Installed if path_configured => {
            "Ready. Run `sikemux <path>` or use Sikemux as `$EDITOR`."
        }
        CliInstallState::Installed => {
            "Installed. Add the shown directory to PATH, then restart your shell."
        }
        CliInstallState::Outdated => {
            "A previous managed CLI is installed and can be updated safely."
        }
        CliInstallState::Conflict => {
            "An unrelated file already uses one of these names. It was left untouched."
        }
        CliInstallState::NotInstalled => {
            "Install two small launchers that connect your shell to this Sikemux app."
        }
        CliInstallState::Unavailable => unreachable!(),
    };
    make_status(
        install_state,
        install_dir,
        cli_path,
        editor_path,
        Some(executable),
        path_configured,
        message,
    )
}

fn make_status(
    state: CliInstallState,
    install_dir: PathBuf,
    cli_path: PathBuf,
    editor_path: PathBuf,
    executable: Option<PathBuf>,
    path_configured: bool,
    message: &str,
) -> CliInstallStatus {
    CliInstallStatus {
        state,
        install_dir: install_dir.to_string_lossy().into_owned(),
        cli_path: cli_path.to_string_lossy().into_owned(),
        editor_path: editor_path.to_string_lossy().into_owned(),
        executable: executable.map(|path| path.to_string_lossy().into_owned()),
        path_configured,
        message: message.into(),
    }
}

fn install_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA").map(|value| PathBuf::from(value).join("Sikemux/bin"))
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME").map(|value| PathBuf::from(value).join(".local/bin"))
    }
}

fn destination_paths(install_dir: &Path) -> (PathBuf, PathBuf) {
    #[cfg(windows)]
    {
        (
            install_dir.join("sikemux.exe"),
            install_dir.join("sikemux-editor.exe"),
        )
    }
    #[cfg(not(windows))]
    {
        (
            install_dir.join("sikemux"),
            install_dir.join("sikemux-editor"),
        )
    }
}

fn path_contains(directory: &Path) -> bool {
    env::var_os("PATH")
        .is_some_and(|value| env::split_paths(&value).any(|candidate| candidate == directory))
}

#[cfg(unix)]
fn destination_state(destination: &Path, executable: &Path) -> DestinationState {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DestinationState::Missing,
        Err(_) => return DestinationState::Conflict,
    };
    if !metadata.file_type().is_symlink() {
        return DestinationState::Conflict;
    }
    let Ok(target) = fs::read_link(destination) else {
        return DestinationState::Conflict;
    };
    let target = if target.is_absolute() {
        target
    } else {
        destination
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(target)
    };
    if paths_match(&target, executable) {
        DestinationState::Installed
    } else {
        DestinationState::Conflict
    }
}

#[cfg(unix)]
fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

#[cfg(unix)]
fn install_for(executable: &Path) -> AppResult<()> {
    let directory = install_dir().ok_or_else(|| AppError::State("no home directory".into()))?;
    install_unix_into(executable, &directory)
}

#[cfg(unix)]
fn install_unix_into(executable: &Path, directory: &Path) -> AppResult<()> {
    use std::os::unix::fs::symlink;

    let (cli_path, editor_path) = destination_paths(directory);
    for destination in [&cli_path, &editor_path] {
        if destination_state(destination, executable) == DestinationState::Conflict {
            return Err(AppError::State(format!(
                "refusing to replace unrelated path: {}",
                destination.display()
            )));
        }
    }
    fs::create_dir_all(directory)?;

    let mut created = Vec::new();
    for destination in [&cli_path, &editor_path] {
        if destination_state(destination, executable) == DestinationState::Installed {
            continue;
        }
        if let Err(error) = symlink(executable, destination) {
            for path in created {
                let _ = fs::remove_file(path);
            }
            return Err(error.into());
        }
        created.push(destination);
    }
    Ok(())
}

#[cfg(windows)]
const WINDOWS_MARKER: &[u8] = b"sikemux-cli-managed-v1\n";

#[cfg(windows)]
const WINDOWS_APP_POINTER: &str = ".sikemux-app-executable";

#[cfg(windows)]
fn windows_marker(directory: &Path) -> PathBuf {
    directory.join(".sikemux-cli-managed")
}

#[cfg(windows)]
fn windows_directory_managed(directory: &Path) -> bool {
    fs::read(windows_marker(directory)).ok().as_deref() == Some(WINDOWS_MARKER)
}

#[cfg(windows)]
fn destination_state(destination: &Path, executable: &Path) -> DestinationState {
    if !destination.exists() {
        return DestinationState::Missing;
    }
    let Some(directory) = destination.parent() else {
        return DestinationState::Conflict;
    };
    if !windows_directory_managed(directory) {
        return DestinationState::Conflict;
    }
    match (fs::read(destination), fs::read(executable)) {
        (Ok(installed), Ok(current)) if installed == current => DestinationState::Installed,
        (Ok(_), Ok(_)) => DestinationState::Outdated,
        _ => DestinationState::Conflict,
    }
}

#[cfg(windows)]
fn install_for(executable: &Path) -> AppResult<()> {
    let directory =
        install_dir().ok_or_else(|| AppError::State("LOCALAPPDATA is unavailable".into()))?;
    let (cli_path, editor_path) = destination_paths(&directory);
    let marker = windows_marker(&directory);
    let app_pointer = directory.join(WINDOWS_APP_POINTER);
    let app_executable = executable
        .parent()
        .map(|parent| parent.join("sikemux.exe"))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            AppError::State("the packaged Sikemux application executable is unavailable".into())
        })?;
    let managed = windows_directory_managed(&directory);
    if !managed
        && ([&cli_path, &editor_path, &marker, &app_pointer]
            .iter()
            .any(|path| path.exists()))
    {
        return Err(AppError::State(format!(
            "refusing to replace files in unmanaged directory: {}",
            directory.display()
        )));
    }
    fs::create_dir_all(&directory)?;
    if !managed {
        fs::write(&marker, WINDOWS_MARKER)?;
    }
    fs::write(&app_pointer, app_executable.to_string_lossy().as_bytes())?;
    for destination in [&cli_path, &editor_path] {
        let temporary = destination.with_extension("exe.sikemux-new");
        fs::copy(executable, &temporary)?;
        if destination.exists() {
            fs::remove_file(destination)?;
        }
        fs::rename(temporary, destination)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_matching_symlinks_as_installed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("sikemux-editor");
        fs::write(&executable, b"cli").unwrap();
        let destination = directory.path().join("sikemux");
        assert_eq!(
            destination_state(&destination, &executable),
            DestinationState::Missing
        );
        symlink(&executable, &destination).unwrap();
        assert_eq!(
            destination_state(&destination, &executable),
            DestinationState::Installed
        );
        fs::remove_file(&destination).unwrap();
        fs::write(&destination, b"user file").unwrap();
        assert_eq!(
            destination_state(&destination, &executable),
            DestinationState::Conflict
        );
    }

    #[test]
    fn install_is_idempotent_and_refuses_collisions() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("sikemux-editor-bin");
        fs::write(&executable, b"cli").unwrap();
        let install = temporary.path().join("bin");
        let (cli_path, editor_path) = destination_paths(&install);

        install_unix_into(&executable, &install).unwrap();
        install_unix_into(&executable, &install).unwrap();
        assert_eq!(
            destination_state(&cli_path, &executable),
            DestinationState::Installed
        );
        assert_eq!(
            destination_state(&editor_path, &executable),
            DestinationState::Installed
        );

        fs::remove_file(&cli_path).unwrap();
        fs::write(&cli_path, b"user owned").unwrap();
        assert!(install_unix_into(&executable, &install).is_err());
        assert_eq!(fs::read(&cli_path).unwrap(), b"user owned");
    }
}
