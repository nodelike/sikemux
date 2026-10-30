//! The project picker's scan of the folders the person keeps projects in:
//! a root is a project itself when it is a git repo or set to index itself,
//! and below it, within its depth, each git repo is one. The walk never goes
//! into a repo or a dot folder.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_process::paths::expand_home;

pub(crate) use crate::protocol::ProjectRoot;
use crate::protocol::MAX_PROJECT_DEPTH;

const MAX_PROJECT_ROOTS: usize = 32;
const MAX_VISITED_DIRECTORIES: usize = 50_000;
const MAX_DISCOVERED_PROJECTS: usize = 4_096;

#[derive(Serialize, Deserialize, PartialEq)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
}

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

fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// What is at a path as the person typed it, `~` and all.
#[derive(Serialize)]
pub(crate) struct Resolved {
    path: String,
    kind: Option<&'static str>,
}

#[derive(Deserialize)]
pub(crate) struct ResolveParams {
    path: String,
}

pub(crate) fn resolve(params: ResolveParams) -> Resolved {
    let path = expand_home(&params.path);
    let kind = fs::metadata(&path)
        .ok()
        .map(|meta| if meta.is_dir() { "dir" } else { "file" });
    Resolved {
        path: path.to_string_lossy().into_owned(),
        kind,
    }
}

// DFS within `root` up to `remaining` levels deep. Within the walk, only
// git repos are emitted; repos are also terminal — we never descend into
// one.
fn walk(
    root: &Path,
    depth: i64,
    out: &mut Vec<ProjectEntry>,
    seen_projects: &mut HashSet<String>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let mut pending = VecDeque::from([(root.to_path_buf(), depth)]);
    while let Some((dir, remaining)) = pending.pop_front() {
        if remaining <= 0 {
            continue;
        }
        let Ok(canonical) = fs::canonicalize(&dir) else {
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
            let Ok(canonical) = fs::canonicalize(&p) else {
                continue;
            };
            if !canonical.is_dir() {
                continue;
            }
            if is_repo(&canonical) {
                let path = canonical.to_string_lossy().into_owned();
                if seen_projects.insert(path.clone()) {
                    if out.len() >= MAX_DISCOVERED_PROJECTS {
                        return Err("project discovery found too many repositories; narrow the roots or depth".into());
                    }
                    out.push(ProjectEntry {
                        name: relative_name(root, &p),
                        path,
                    });
                }
                continue;
            }
            pending.push_back((p, remaining - 1));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct ScanParams {
    roots: Vec<ProjectRoot>,
}

/// Runs on a blocking thread.
pub(crate) fn scan(params: ScanParams) -> Result<Vec<ProjectEntry>, String> {
    scan_project_roots_sync(params.roots)
}

fn scan_project_roots_sync(roots: Vec<ProjectRoot>) -> Result<Vec<ProjectEntry>, String> {
    if roots.len() > MAX_PROJECT_ROOTS {
        return Err("too many project discovery roots".into());
    }
    let mut out: Vec<ProjectEntry> = Vec::new();
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
                out.push(ProjectEntry {
                    name: name_of(&root),
                    path: root_path,
                });
            }
        }
        if is_repo(&root) {
            let root_path = root.to_string_lossy().into_owned();
            if seen.insert(root_path.clone()) {
                out.push(ProjectEntry {
                    name: name_of(&root),
                    path: root_path,
                });
            }
            // If the root itself is a git repo, don't enumerate its
            // insides — it's already the project.
            continue;
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
    fn frontend_self_index_field_includes_a_non_git_root() {
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
}
