use std::ffi::OsStr;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A link's target in the bytes a commit holds it as.
pub fn link_target_bytes(target: &Path) -> Vec<u8> {
    target.as_os_str().as_bytes().to_vec()
}

pub fn write_link(target: &[u8], link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(OsStr::from_bytes(target), link)
}

pub fn is_executable(meta: &Metadata) -> bool {
    meta.permissions().mode() & 0o111 != 0
}

/// Turns the execute bits on or off, as git does, leaving the rest of the permissions alone.
pub fn set_executable(file: &Path, meta: &Metadata, executable: bool) -> io::Result<()> {
    let now = meta.permissions().mode();
    let wanted = match executable {
        true => now | (now & 0o444) >> 2,
        false => now & !0o111,
    };
    if wanted == now {
        return Ok(());
    }
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(wanted))
}

pub fn open_private_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// The person's login shell, for commands written for their own terminal.
pub fn login_shell() -> Result<std::process::Command, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    Ok(sikemux_process::user_environment::agent_command_in_own_cgroup(shell))
}

pub fn real_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
