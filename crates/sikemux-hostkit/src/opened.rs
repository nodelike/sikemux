//! Which project a path opened from outside a shell belongs to.

use std::path::{Path, PathBuf};

use sikemux_base::cli::protocol::CliTargetKind;

/// The project a path opened from outside a shell belongs to: its repository,
/// or else the folder itself, or the folder a file sits in.
pub fn project_root(path: &Path, kind: CliTargetKind) -> PathBuf {
    git_root(path, kind).unwrap_or_else(|| enclosing_folder(path, kind).to_path_buf())
}

pub fn git_root(path: &Path, kind: CliTargetKind) -> Option<PathBuf> {
    sikemux_process::files::repo_root(enclosing_folder(path, kind)).map(Path::to_path_buf)
}

pub fn enclosing_folder(path: &Path, kind: CliTargetKind) -> &Path {
    if kind == CliTargetKind::Directory {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}
