//! Secrets in files, one per service and account under a folder only the
//! person can open: `~/.config/sikemux/secrets` for a development build, the
//! folder the try-out's build named, or the folder the guarded store keeps
//! them in.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::{KeychainError, KeychainResult};

fn failed(error: impl std::fmt::Display) -> KeychainError {
    KeychainError::Failed(format!("the saved secrets could not be reached: {error}"))
}

/// The folder the try-out keeps its secrets under, fixed when it is built so
/// nothing at run time can move them.
pub(crate) const TRYOUT_FOLDER: Option<&str> = sikemux_process::paths::TRYOUT_SECRETS;

pub(crate) fn home_root() -> KeychainResult<PathBuf> {
    if let Some(folder) = TRYOUT_FOLDER {
        return tryout_root(Path::new(folder));
    }
    sikemux_process::paths::config_dir()
        .map(|dir| dir.join("secrets"))
        .ok_or_else(|| failed("there is no home folder"))
}

/// The try-out's secrets folder, unless it is or falls inside Sikemux's own
/// folder, whose secrets belong to the person's other builds.
fn tryout_root(folder: &Path) -> KeychainResult<PathBuf> {
    let plain = folder.is_absolute()
        && folder
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    if !plain {
        return Err(failed(
            "the try-out's secrets folder must be a plain full path",
        ));
    }
    let theirs = sikemux_process::paths::home()
        .map(|home| home.join(".config").join("sikemux"))
        .ok_or_else(|| failed("there is no home folder"))?;
    let root = folder.join("secrets");
    if root.starts_with(&theirs) || resolved(&root).starts_with(resolved(&theirs)) {
        return Err(failed(
            "the try-out's secrets folder is inside Sikemux's own folder",
        ));
    }
    Ok(root)
}

/// `path` with its links followed as far as it exists.
fn resolved(path: &Path) -> PathBuf {
    path.ancestors()
        .find_map(|known| {
            let real = known.canonicalize().ok()?;
            Some(real.join(path.strip_prefix(known).ok()?))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

pub(crate) fn read(root: &Path, service: &str, account: &str) -> KeychainResult<Option<String>> {
    match std::fs::read_to_string(root.join(service).join(account)) {
        Ok(secret) => Ok(Some(secret.trim().to_owned()).filter(|secret| !secret.is_empty())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed(error)),
    }
}

pub(crate) fn write(root: &Path, service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let folder = root.join(service);
    std::fs::create_dir_all(&folder).map_err(failed)?;
    #[cfg(unix)]
    for dir in [root, &folder] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
    }
    sikemux_process::files::write_atomic(&folder.join(account), secret.as_bytes(), true)
        .map_err(failed)
}

pub(crate) fn delete(root: &Path, service: &str, account: &str) -> KeychainResult<()> {
    match std::fs::remove_file(root.join(service).join(account)) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(failed(error)),
        _ => Ok(()),
    }
}
