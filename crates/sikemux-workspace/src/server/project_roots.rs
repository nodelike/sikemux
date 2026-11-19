//! The project picker's scan of the folders the person keeps projects in:
//! a root is a project itself when it is a git repo or set to index itself,
//! and below it, within its depth, each git repo is one. A linked worktree
//! belongs to its main repo and is never one. The walk never goes into a
//! repo, a worktree or a dot folder.

pub use sikemux_calls::project_roots::{ResolveParams, Resolved, RootEntry, ScanParams};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use sikemux_process::paths::expand_home;

pub use sikemux_view::prefs::ProjectRoot;
use sikemux_view::prefs::MAX_PROJECT_DEPTH;

const MAX_PROJECT_ROOTS: usize = 32;
const MAX_VISITED_DIRECTORIES: usize = 50_000;
const MAX_DISCOVERED_PROJECTS: usize = 4_096;

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn relative_name(root: &Path, p: &Path) -> String {
    let Ok(rel) = p.strip_prefix(root) else {
        return name_of(p);
    };
    if rel.as_os_str().is_empty() {
        return name_of(p);
    }
    let s = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if s.is_empty() {
        name_of(p)
    } else {
        s
    }
}

enum Folder {
    Repo,
    LinkedWorktree,
    Plain,
}

fn folder_kind(dir: &Path) -> Folder {
    let dot_git = dir.join(".git");
    match fs::metadata(&dot_git) {
        Ok(meta) if meta.is_dir() => Folder::Repo,
        Ok(_) if links_a_worktree(dir, &dot_git) => Folder::LinkedWorktree,
        Ok(_) => Folder::Repo,
        Err(_) => Folder::Plain,
    }
}

/// A `.git` file names the folder git keeps for the checkout; only a linked
/// worktree's has a `commondir` pointing back at its main repo.
fn links_a_worktree(dir: &Path, dot_git: &Path) -> bool {
    let Ok(link) = fs::read_to_string(dot_git) else {
        return false;
    };
    link.strip_prefix("gitdir:")
        .is_some_and(|gitdir| dir.join(gitdir.trim()).join("commondir").is_file())
}

pub fn resolve(params: ResolveParams) -> Resolved {
    let typed = expand_home(&params.path);
    let path = sikemux_process::files::real_path(&typed).unwrap_or(typed);
    let kind = match fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => Some("dir"),
        Ok(_) => Some("file"),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Some("denied"),
        Err(_) => None,
    }
    .map(str::to_owned);
    Resolved {
        path: path.to_string_lossy().into_owned(),
        kind,
    }
}

// DFS within `root` up to `remaining` levels deep. Within the walk, only
// git repos are emitted; repos and linked worktrees are terminal — we never
// descend into one.
fn walk(
    root: &Path,
    depth: i64,
    out: &mut Vec<RootEntry>,
    seen_projects: &mut HashSet<String>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let mut pending = VecDeque::from([(root.to_path_buf(), depth)]);
    while let Some((dir, remaining)) = pending.pop_front() {
        if remaining <= 0 {
            continue;
        }
        let Ok(canonical) = sikemux_process::files::real_path(&dir) else {
            continue;
        };
        if !visited.insert(canonical) {
            continue;
        }
        if visited.len() > MAX_VISITED_DIRECTORIES {
            return Err(
                "project discovery visited too many directories; narrow the roots or depth".into(),
            );
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() && !file_type.is_symlink() {
                continue;
            }
            let name = name_of(&p);
            if name.starts_with('.') {
                continue;
            }
            let Ok(canonical) = sikemux_process::files::real_path(&p) else {
                continue;
            };
            if !canonical.is_dir() {
                continue;
            }
            match folder_kind(&canonical) {
                Folder::LinkedWorktree => {}
                Folder::Plain => pending.push_back((p, remaining - 1)),
                Folder::Repo => {
                    let path = canonical.to_string_lossy().into_owned();
                    if seen_projects.insert(path.clone()) {
                        if out.len() >= MAX_DISCOVERED_PROJECTS {
                            return Err("project discovery found too many repositories; narrow the roots or depth".into());
                        }
                        out.push(RootEntry {
                            name: relative_name(root, &p),
                            path,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Runs on a blocking thread.
pub fn scan(params: ScanParams) -> Result<Vec<RootEntry>, String> {
    scan_project_roots_sync(params.roots)
}

fn scan_project_roots_sync(roots: Vec<ProjectRoot>) -> Result<Vec<RootEntry>, String> {
    if roots.len() > MAX_PROJECT_ROOTS {
        return Err("too many project discovery roots".into());
    }
    let mut out: Vec<RootEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    for r in roots {
        let root = expand_home(&r.path);
        if !root.is_dir() {
            continue;
        }
        // A self-indexed root is a project in its own right, git repo or not,
        // and is still scanned for repos beneath it when depth allows.
        if r.self_index {
            let root_path = root.to_string_lossy().into_owned();
            if seen.insert(root_path.clone()) {
                out.push(RootEntry {
                    name: name_of(&root),
                    path: root_path,
                });
            }
        }
        match folder_kind(&root) {
            Folder::LinkedWorktree => continue,
            Folder::Repo => {
                let root_path = root.to_string_lossy().into_owned();
                if seen.insert(root_path.clone()) {
                    out.push(RootEntry {
                        name: name_of(&root),
                        path: root_path,
                    });
                }
                continue;
            }
            Folder::Plain => {}
        }
        walk(
            &root,
            i64::from(r.depth.min(MAX_PROJECT_DEPTH)),
            &mut out,
            &mut seen,
            &mut visited,
        )?;
    }

    out.sort_by_key(|profile| profile.name.to_lowercase());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{scan_project_roots_sync, ProjectRoot};

    #[test]
    fn a_self_indexed_root_is_a_project_though_it_is_not_git() {
        let root = tempfile::tempdir().unwrap();
        let configured: ProjectRoot = serde_json::from_value(serde_json::json!({
            "path": root.path(),
            "depth": 1,
            "selfIndex": true
        }))
        .unwrap();

        let projects = scan_project_roots_sync(vec![configured]).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, root.path().to_string_lossy());
    }

    #[test]
    fn a_linked_worktree_is_not_a_project() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("app");
        let git = |args: &[&str]| {
            crate::git::Git::quick(&repo)
                .trimmed(args)
                .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
        };
        std::fs::create_dir(&repo).unwrap();
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ]);
        git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            "../app.worktrees/feat",
        ]);
        let configured: ProjectRoot = serde_json::from_value(serde_json::json!({
            "path": root.path(),
            "depth": 2,
            "selfIndex": false
        }))
        .unwrap();

        let projects = scan_project_roots_sync(vec![configured]).unwrap();

        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["app"]);
    }
}
