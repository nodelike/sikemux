use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Windows lets only Developer Mode or an administrator make symbolic links, so a folder falls
/// back to a junction and a file to a hard link, which anyone may make.
pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    let folder = target.is_dir();
    let linked = match folder {
        true => std::os::windows::fs::symlink_dir(target, link),
        false => std::os::windows::fs::symlink_file(target, link),
    };
    match linked {
        Err(error) if error.raw_os_error() == Some(PRIVILEGE_NOT_HELD) => match folder {
            true => junction(target, link),
            false => fs::hard_link(target, link),
        },
        other => other,
    }
}

const PRIVILEGE_NOT_HELD: i32 = 1314;

pub const PATH_LIST_SEPARATOR: char = ';';

/// Where [`symlink`] fell back to a hard link, the link is a plain file.
pub fn is_link(path: &Path) -> bool {
    path.is_symlink() || path.is_file()
}

/// A hard link shares its program's size and times; a copy gets a creation time of its own.
pub fn is_hard_link_of(path: &Path, program: &Path) -> bool {
    let (Ok(link), Ok(program)) = (fs::metadata(path), fs::metadata(program)) else {
        return false;
    };
    link.len() == program.len()
        && link.modified().ok() == program.modified().ok()
        && link.created().ok() == program.created().ok()
}

fn junction(target: &Path, link: &Path) -> io::Result<()> {
    let status = sikemux_process::user_environment::command("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    match status.success() {
        true => Ok(()),
        false => Err(io::Error::other(format!(
            "mklink /J {} {} failed",
            link.display(),
            target.display()
        ))),
    }
}

/// A profile folder on Windows is already its user's alone.
pub fn make_private_folder(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn make_private_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Windows has no executable bit: what runs is decided by the file's extension.
pub fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn is_executable(meta: &fs::Metadata) -> bool {
    meta.is_file()
}

pub fn parent_id() -> u32 {
    sikemux_process::procs::parent(std::process::id()).map_or(0, |(parent, _)| parent)
}

/// Windows cannot replace a running program, so this runs `command` to the end and leaves with
/// its exit code, returning only when it could not be started.
pub fn exec(command: &mut Command) -> io::Error {
    match command.status() {
        Ok(status) => std::process::exit(exit_code(status)),
        Err(error) => error,
    }
}

pub fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
