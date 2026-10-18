//! Secrets in files only the person can read, one per service and account,
//! under `~/.config/sikemux/secrets`.

use std::io::ErrorKind;
use std::path::PathBuf;

use crate::{KeychainError, KeychainResult};

fn failed(error: impl std::fmt::Display) -> KeychainError {
    KeychainError::Failed(format!("the saved secrets could not be reached: {error}"))
}

fn root() -> KeychainResult<PathBuf> {
    sikemux_process::paths::config_dir()
        .map(|dir| dir.join("secrets"))
        .ok_or_else(|| failed("there is no home folder"))
}

pub(crate) fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    match std::fs::read_to_string(root()?.join(service).join(account)) {
        Ok(secret) => Ok(Some(secret.trim().to_owned()).filter(|secret| !secret.is_empty())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed(error)),
    }
}

pub(crate) fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let root = root()?;
    let folder = root.join(service);
    std::fs::create_dir_all(&folder).map_err(failed)?;
    #[cfg(unix)]
    for dir in [&root, &folder] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
    }
    sikemux_process::files::write_atomic(&folder.join(account), secret.as_bytes(), true)
        .map_err(failed)
}

pub(crate) fn delete(service: &str, account: &str) -> KeychainResult<()> {
    match std::fs::remove_file(root()?.join(service).join(account)) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(failed(error)),
        _ => Ok(()),
    }
}
