//! The `sikemux` command for the person's shell: a link in their own bin
//! folder to the core's executable, which Settings offers to put there.

use std::env;
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::connection::blocking;
use super::{CoreError, CoreResult};

type Installed<T> = Result<T, String>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CliInstallState {
    Unavailable,
    NotInstalled,
    Installed,
    #[cfg(windows)]
    Outdated,
    Conflict,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliInstallStatus {
    state: CliInstallState,
    install_dir: String,
    cli_path: String,
    executable: Option<String>,
    path_configured: bool,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationState {
    Missing,
    Installed,
    #[cfg(windows)]
    Outdated,
    Conflict,
}

/// The executable the command links to: this core's own, which is the CLI.
/// Tests point it elsewhere with `SIKEMUX_SIDECAR_PATH`.
fn cli_executable() -> Option<PathBuf> {
    if let Some(path) = sikemux_process::paths::env_path("SIKEMUX_SIDECAR_PATH") {
        return path.is_file().then_some(path);
    }
    let name = if cfg!(windows) {
        "sikemux-host.exe"
    } else {
        "sikemux-host"
    };
    env::current_exe()
        .ok()
        .filter(|path| path.file_name().is_some_and(|file| file == name))
}

pub(crate) async fn status() -> CoreResult<CliInstallStatus> {
    blocking(|| Ok(status_for(cli_executable()))).await
}

pub(crate) async fn install() -> CoreResult<CliInstallStatus> {
    blocking(|| {
        let executable =
            cli_executable().ok_or("this build of Sikemux has no packaged command to install")?;
        install_for(&executable).map_err(CoreError::from)?;
        Ok(status_for(Some(executable)))
    })
    .await
}

fn status_for(executable: Option<PathBuf>) -> CliInstallStatus {
    let install_dir = install_dir().unwrap_or_default();
    let cli_path = destination_path(&install_dir);
    let path_configured = path_contains(&install_dir);
    let Some(executable) = executable.filter(|path| path.is_file()) else {
        return make_status(
            CliInstallState::Unavailable,
            install_dir,
            cli_path,
            None,
            path_configured,
            "The CLI is included in packaged builds; it is unavailable from this development executable.",
        );
    };

    let install_state = match destination_state(&cli_path, &executable) {
        DestinationState::Conflict => CliInstallState::Conflict,
        DestinationState::Installed => CliInstallState::Installed,
        #[cfg(windows)]
        DestinationState::Outdated => CliInstallState::Outdated,
        DestinationState::Missing => CliInstallState::NotInstalled,
    };
    let message = match install_state {
        CliInstallState::Installed if path_configured => "Ready. Run `sikemux <path>`.",
        CliInstallState::Installed => {
            "Installed. Add the shown directory to PATH, then restart your shell."
        }
        #[cfg(windows)]
        CliInstallState::Outdated => {
            "A previous managed CLI is installed and can be updated safely."
        }
        CliInstallState::Conflict => {
            "An unrelated file already uses this name. It was left untouched."
        }
        CliInstallState::NotInstalled => {
            "Install a small launcher that connects your shell to this Sikemux app."
        }
        CliInstallState::Unavailable => unreachable!(),
    };
    make_status(
        install_state,
        install_dir,
        cli_path,
        Some(executable),
        path_configured,
        message,
    )
}

fn make_status(
    state: CliInstallState,
    install_dir: PathBuf,
    cli_path: PathBuf,
    executable: Option<PathBuf>,
    path_configured: bool,
    message: &str,
) -> CliInstallStatus {
    CliInstallStatus {
        state,
        install_dir: install_dir.to_string_lossy().into_owned(),
        cli_path: cli_path.to_string_lossy().into_owned(),
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

fn destination_path(install_dir: &Path) -> PathBuf {
    install_dir.join(if cfg!(windows) {
        "sikemux.exe"
    } else {
        "sikemux"
    })
}

fn path_contains(directory: &Path) -> bool {
    sikemux_process::user_environment::var_os("PATH")
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
fn install_for(executable: &Path) -> Installed<()> {
    let directory = install_dir().ok_or("no home directory")?;
    install_unix_into(executable, &directory)
}

#[cfg(unix)]
fn install_unix_into(executable: &Path, directory: &Path) -> Installed<()> {
    use std::os::unix::fs::symlink;

    let cli_path = destination_path(directory);
    match destination_state(&cli_path, executable) {
        DestinationState::Installed => return Ok(()),
        DestinationState::Conflict => {
            return Err(format!(
                "refusing to replace unrelated path: {}",
                cli_path.display()
            ))
        }
        DestinationState::Missing => {}
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    symlink(executable, &cli_path).map_err(|error| error.to_string())?;
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
fn install_for(executable: &Path) -> Installed<()> {
    let directory = install_dir().ok_or("LOCALAPPDATA is unavailable")?;
    let cli_path = destination_path(&directory);
    let marker = windows_marker(&directory);
    let app_pointer = directory.join(WINDOWS_APP_POINTER);
    let app_executable = executable
        .parent()
        .map(|parent| parent.join("sikemux.exe"))
        .filter(|path| path.is_file())
        .ok_or("the packaged Sikemux application executable is unavailable")?;
    let managed = windows_directory_managed(&directory);
    if !managed
        && ([&cli_path, &marker, &app_pointer]
            .iter()
            .any(|path| path.exists()))
    {
        return Err(format!(
            "refusing to replace files in unmanaged directory: {}",
            directory.display()
        ));
    }
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    if !managed {
        fs::write(&marker, WINDOWS_MARKER).map_err(|error| error.to_string())?;
    }
    fs::write(&app_pointer, app_executable.to_string_lossy().as_bytes())
        .map_err(|error| error.to_string())?;
    let temporary = cli_path.with_extension("exe.sikemux-new");
    fs::copy(executable, &temporary).map_err(|error| error.to_string())?;
    if cli_path.exists() {
        fs::remove_file(&cli_path).map_err(|error| error.to_string())?;
    }
    fs::rename(temporary, &cli_path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_matching_symlinks_as_installed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("sikemux-host");
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
        let executable = temporary.path().join("sikemux-host-bin");
        fs::write(&executable, b"cli").unwrap();
        let install = temporary.path().join("bin");
        let cli_path = destination_path(&install);

        install_unix_into(&executable, &install).unwrap();
        install_unix_into(&executable, &install).unwrap();
        assert_eq!(
            destination_state(&cli_path, &executable),
            DestinationState::Installed
        );

        fs::remove_file(&cli_path).unwrap();
        fs::write(&cli_path, b"user owned").unwrap();
        assert!(install_unix_into(&executable, &install).is_err());
        assert_eq!(fs::read(&cli_path).unwrap(), b"user owned");
    }
}
