//! Which checkouts Sikemux made, and what would be lost with one. Only its own are ever taken
//! away, and only while every ignored file in them can be made again.

use std::path::{Path, PathBuf};

use super::git_folder;
use super::warm::is_cargo_output;
use crate::git::Git;

/// The file in a checkout's record in the main checkout that says Sikemux made it.
const MADE_HERE: &str = "sikemux-made";
/// Folders that tools fill and fill again.
const REBUILT_FOLDERS: [&str; 12] = [
    "node_modules",
    "dist",
    "build",
    ".build",
    ".next",
    ".astro",
    ".turbo",
    ".expo",
    ".gradle",
    "Pods",
    "DerivedData",
    "__pycache__",
];
/// What a folder holds at its top when a tool can fill it again: a cache, a Python environment.
const REBUILT_MARKS: [&str; 2] = ["CACHEDIR.TAG", "pyvenv.cfg"];
const REBUILT_FILES: [&str; 2] = [".DS_Store", "next-env.d.ts"];
const BUILD_INFO: &str = ".tsbuildinfo";
const GENERATED: &str = ".generated.";

pub fn mark(tree: &Path) -> Result<(), String> {
    let record = git_folder(tree).ok_or("git keeps no record of the new worktree")?;
    std::fs::write(record.join(MADE_HERE), "")
        .map_err(|error| format!("could not mark the worktree as Sikemux's: {error}"))
}

/// Whether Sikemux made the checkout at `tree`, which is still there.
pub fn made_here(tree: &Path) -> bool {
    tree.join(".git").is_file()
        && git_folder(tree).is_some_and(|record| record.join(MADE_HERE).is_file())
}

/// A file Git ignores in `tree` that would be lost with it: neither build output nor the same as
/// the main checkout's.
pub fn only_here(main: &Path, tree: &Path) -> Result<Option<PathBuf>, String> {
    let listed = Git::slow(tree).text(&[
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ])?;
    for path in listed.split('\0').filter(|path| !path.is_empty()) {
        if let Some(lost) = lost_with(main, tree, Path::new(path))
            .map_err(|error| format!("{path} could not be read: {error}"))?
        {
            return Ok(Some(lost));
        }
    }
    Ok(None)
}

fn lost_with(main: &Path, tree: &Path, inside: &Path) -> std::io::Result<Option<PathBuf>> {
    let path = tree.join(inside);
    let meta = path.symlink_metadata()?;
    if rebuilt(inside, meta.is_dir()) {
        return Ok(None);
    }
    if meta.is_file() {
        let same = same_file(&path, &main.join(inside))?;
        return Ok((!same).then(|| inside.to_owned()));
    }
    if !meta.is_dir()
        || is_cargo_output(&path)
        || REBUILT_MARKS.iter().any(|mark| path.join(mark).is_file())
    {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&path)? {
        if let Some(lost) = lost_with(main, tree, &inside.join(entry?.file_name()))? {
            return Ok(Some(lost));
        }
    }
    Ok(None)
}

fn rebuilt(path: &Path, folder: bool) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if folder {
        return REBUILT_FOLDERS.contains(&&*name);
    }
    REBUILT_FILES.contains(&&*name) || name.ends_with(BUILD_INFO) || name.contains(GENERATED)
}

fn same_file(one: &Path, other: &Path) -> std::io::Result<bool> {
    match other.metadata() {
        Ok(meta) if meta.is_file() && meta.len() == one.metadata()?.len() => {
            Ok(std::fs::read(one)? == std::fs::read(other)?)
        }
        _ => Ok(false),
    }
}
