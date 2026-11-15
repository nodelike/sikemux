use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// A link's target in the bytes a commit holds it as, with git's forward slashes.
pub fn link_target_bytes(target: &Path) -> Vec<u8> {
    target.to_string_lossy().replace('\\', "/").into_bytes()
}

/// Windows lets only Developer Mode or an administrator make symbolic links, so without that
/// the link is written as a file holding its target, as git does there.
pub fn write_link(target: &[u8], link: &Path) -> io::Result<()> {
    let named = PathBuf::from(String::from_utf8_lossy(target).replace('/', "\\"));
    let pointed = match link.parent().map(|parent| parent.join(&named)) {
        Some(resolved) if resolved.is_dir() => std::os::windows::fs::symlink_dir(&named, link),
        _ => std::os::windows::fs::symlink_file(&named, link),
    };
    match pointed {
        Err(error) if error.raw_os_error() == Some(PRIVILEGE_NOT_HELD) => {
            std::fs::write(link, target)
        }
        other => other,
    }
}

const PRIVILEGE_NOT_HELD: i32 = 1314;

/// Windows files carry no execute bit; git there keeps a file's mode from the commit.
pub fn is_executable(_meta: &Metadata) -> bool {
    false
}

pub fn set_executable(_file: &Path, _meta: &Metadata, _executable: bool) -> io::Result<()> {
    Ok(())
}

pub fn open_private_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}
