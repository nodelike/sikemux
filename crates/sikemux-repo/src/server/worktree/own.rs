//! Which checkouts Sikemux made, and what would be lost with one. Only its own are ever taken
//! away, and only while every ignored file in them can be made again.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::git_folder;
use super::warm::is_cargo_output;
use crate::git::Git;

const MADE_HERE: &str = "sikemux-made";
const REBUILT_FOLDERS: [&str; 40] = [
    "target",
    "node_modules",
    "dist",
    "build",
    ".build",
    "out",
    "_build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".vercel",
    ".astro",
    ".turbo",
    ".expo",
    ".output",
    ".cache",
    ".parcel-cache",
    ".vite",
    "coverage",
    ".nyc_output",
    ".gradle",
    "Pods",
    "Carthage",
    "DerivedData",
    "xcuserdata",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "venv",
    "cmake-build-debug",
    "cmake-build-release",
    ".pnpm-store",
    ".yarn",
    ".npm",
    ".pnpm",
    ".bun",
    ".deno",
];
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
        let same = same_file(&path, meta.len(), &main.join(inside))?;
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

#[inline(never)]
fn same_file(one: &Path, one_len: u64, other: &Path) -> std::io::Result<bool> {
    let Ok(other_meta) = other.metadata() else {
        return Ok(false);
    };
    if !other_meta.is_file() || other_meta.len() != one_len {
        return Ok(false);
    }
    let mut first = File::open(one)?;
    let mut second = File::open(other)?;
    let mut first_buf = [0u8; 16 * 1024];
    let mut second_buf = [0u8; 16 * 1024];
    loop {
        let a = read_chunk(&mut first, &mut first_buf)?;
        let b = read_chunk(&mut second, &mut second_buf)?;
        if a != b || first_buf.get(..a) != second_buf.get(..b) {
            return Ok(false);
        }
        if a == 0 {
            return Ok(true);
        }
    }
}

fn read_chunk(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while let Some(rest) = buf.get_mut(filled..) {
        if rest.is_empty() {
            break;
        }
        match file.read(rest) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}
