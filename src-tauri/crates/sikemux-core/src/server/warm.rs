//! A new checkout's build and package folders start as copy-on-write clones
//! of the main checkout's, so its first build redoes only what differs. A
//! clone costs no disk until one side changes a file, and goes when the
//! checkout does. Where there is no cheap clone, installed packages are linked
//! to the main checkout's instead.

use std::path::{Path, PathBuf};

use super::board::git::git;
use super::sandbox::included;

#[cfg(target_os = "macos")]
use clone::bring;

/// What marks a folder that would keep pointing back into the main checkout
/// once cloned: a Python environment's scripts name its interpreter there, and
/// a CMake build its sources.
const ROOTED_IN_PLACE: [&str; 2] = ["pyvenv.cfg", "CMakeCache.txt"];
/// Cargo writes this at the top of every output folder it builds in.
const CARGO_OUTPUT: &str = ".rustc_info.json";
const NODE_MODULES: &str = "node_modules";
const LOCKFILES: [&str; 5] = [
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];

/// [`seed`] on a thread of its own, so the checkout is handed over at once.
pub(crate) fn seed_in_background(main: &Path, tree: &Path) {
    let (main, tree) = (main.to_path_buf(), tree.to_path_buf());
    let _ = std::thread::Builder::new()
        .name("sikemux-warm".into())
        .spawn(move || seed(&main, &tree));
}

/// Clones into `tree` the folders Git ignores in `main` that a new checkout
/// can build from: Cargo's output, installed packages `tree` has the same
/// lockfile for, and the folders `.worktreeinclude` lists.
pub(crate) fn seed(main: &Path, tree: &Path) {
    let Ok(listed) = git(
        main,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    ) else {
        return;
    };
    let ignored: Vec<PathBuf> = listed
        .split('\0')
        .filter_map(|path| path.strip_suffix('/'))
        .map(PathBuf::from)
        .collect();
    for folder in chosen(main, tree, &ignored, &included(main)) {
        if let Err(error) = bring(&main.join(&folder), &tree.join(&folder)) {
            eprintln!(
                "sikemux core: {} was not brought into the worktree: {error}",
                folder.display()
            );
        }
    }
}

fn chosen(main: &Path, tree: &Path, ignored: &[PathBuf], listed: &[PathBuf]) -> Vec<PathBuf> {
    let opted_in = listed.iter().filter(|folder| {
        main.join(folder).is_dir() && ignored.iter().any(|above| folder.starts_with(above))
    });
    let mut folders: Vec<PathBuf> = ignored
        .iter()
        .filter(|folder| builds_from(main, tree, folder))
        .chain(opted_in)
        .filter(|folder| !rooted_in_place(&main.join(folder)))
        .cloned()
        .collect();
    folders.sort();
    folders.dedup_by(|inner, outer| inner.starts_with(outer));
    folders
}

fn builds_from(main: &Path, tree: &Path, folder: &Path) -> bool {
    if main.join(folder).join(CARGO_OUTPUT).is_file() {
        return true;
    }
    folder.file_name() == Some(NODE_MODULES.as_ref())
        && folder.parent().is_some_and(|project| {
            main.join(project).join("package.json").is_file() && same_lockfile(main, tree, project)
        })
}

/// Whether `folder` resolves its packages from the same lockfile in `main` and
/// `tree`: the nearest one in it or a folder above it. A checkout of a branch
/// with other packages would otherwise run against `main`'s.
fn same_lockfile(main: &Path, tree: &Path, folder: &Path) -> bool {
    let lockfile = folder.ancestors().find_map(|dir| {
        LOCKFILES
            .iter()
            .map(|name| dir.join(name))
            .find(|path| main.join(path).is_file())
    });
    lockfile.is_none_or(|path| {
        matches!(
            (std::fs::read(main.join(&path)), std::fs::read(tree.join(&path))),
            (Ok(ours), Ok(theirs)) if ours == theirs
        )
    })
}

fn rooted_in_place(folder: &Path) -> bool {
    std::iter::once(folder.to_path_buf())
        .chain(subfolders(folder))
        .any(|dir| ROOTED_IN_PLACE.iter().any(|mark| dir.join(mark).is_file()))
}

fn subfolders(folder: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
}

/// Links installed packages to the main checkout's, since copying them would
/// be slow and fill the disk.
#[cfg(not(target_os = "macos"))]
fn bring(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.file_name() != Some(NODE_MODULES.as_ref())
        || to.symlink_metadata().is_ok()
        || !to.parent().is_some_and(Path::is_dir)
    {
        return Ok(());
    }
    sikemux_process::os::symlink(from, to)
}

#[cfg(target_os = "macos")]
mod clone {
    use std::ffi::CString;
    use std::fs::File;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use rustix::fs::FlockOperation;

    use super::{subfolders, CARGO_OUTPUT};

    /// Clones the folder `from` to `to` under a name of its own first, so a build
    /// that starts in the new checkout before the clone is done keeps its folder.
    pub(super) fn bring(from: &Path, to: &Path) -> io::Result<()> {
        let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else {
            return Ok(());
        };
        let source = from.symlink_metadata()?;
        if !source.is_dir()
            || to.symlink_metadata().is_ok()
            || !parent
                .metadata()
                .is_ok_and(|parent| parent.dev() == source.dev())
        {
            return Ok(());
        }
        let staged = parent.join(format!(".{}.cloning", name.to_string_lossy()));
        let cloned = {
            let Some(_held) = hold_builds(from) else {
                return Ok(());
            };
            clone_folder(from, &staged)
        };
        let placed = cloned.and_then(|()| rename_new(&staged, to));
        if placed.is_err() {
            let _ = std::fs::remove_dir_all(&staged);
        }
        match placed {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            placed => placed,
        }
    }

    /// Locks each Cargo build in `folder` against writing while it is cloned, or
    /// `None` while one is running. Cloning one mid-build could pair a finished
    /// build's record with the files it replaced, which Cargo would then trust.
    fn hold_builds(folder: &Path) -> Option<Vec<File>> {
        if !folder.join(CARGO_OUTPUT).is_file() {
            return Some(Vec::new());
        }
        let mut held = Vec::new();
        for profile in subfolders(folder).flat_map(|dir| {
            let inner: Vec<PathBuf> = subfolders(&dir).collect();
            std::iter::once(dir).chain(inner)
        }) {
            let Ok(lock) = File::open(profile.join(".cargo-lock")) else {
                continue;
            };
            rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive).ok()?;
            held.push(lock);
        }
        Some(held)
    }

    fn clone_folder(from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (c_path(from)?, c_path(to)?);
        // SAFETY: both are NUL-terminated paths that live through the call.
        if unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (c_path(from)?, c_path(to)?);
        // SAFETY: both are NUL-terminated paths that live through the call.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_whose_lockfile_differs_from_main_gets_no_installed_packages() {
        let main = tempfile::tempdir().unwrap();
        let same = tempfile::tempdir().unwrap();
        let moved = tempfile::tempdir().unwrap();
        for dir in [main.path(), same.path(), moved.path()] {
            std::fs::write(dir.join("package.json"), "{}").unwrap();
        }
        std::fs::create_dir(main.path().join("node_modules")).unwrap();
        std::fs::write(main.path().join("pnpm-lock.yaml"), "a: 1\n").unwrap();
        std::fs::write(same.path().join("pnpm-lock.yaml"), "a: 1\n").unwrap();
        std::fs::write(moved.path().join("pnpm-lock.yaml"), "a: 2\n").unwrap();
        let ignored = [PathBuf::from("node_modules")];
        assert_eq!(
            chosen(main.path(), same.path(), &ignored, &[]),
            ignored.to_vec()
        );
        assert!(chosen(main.path(), moved.path(), &ignored, &[]).is_empty());
    }
}
