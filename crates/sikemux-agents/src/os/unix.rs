use sikemux_process::command::Command;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;

pub const PATH_LIST_SEPARATOR: char = ':';

pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

pub fn is_link(path: &Path) -> bool {
    path.is_symlink()
}

pub fn is_hard_link_of(_path: &Path, _program: &Path) -> bool {
    false
}

pub fn make_private_folder(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

pub fn make_private_file(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

pub fn make_executable(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

pub fn is_executable(meta: &fs::Metadata) -> bool {
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

pub fn parent_id() -> u32 {
    std::os::unix::process::parent_id()
}

/// Becomes `command`, returning only when it could not be started.
pub fn exec(command: &mut Command) -> io::Error {
    command.exec()
}

/// A process killed by a signal reports 128 plus the signal, as shells do.
pub fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or_default())
}
