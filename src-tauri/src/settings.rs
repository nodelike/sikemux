// Backend bits for the settings page:
//   - scan_project_roots: a root emits itself when it is a git repo, or
//     when `self_index` is set — that flag replaces the separate list of
//     pinned projects. Subdirectories within the configured `depth` are emitted
//     ONLY if they are git repos — and the walk does not descend INTO a
//     git repo (its inner src/, vendor/, etc. shouldn't pollute the
//     picker). Dotfile dirs are skipped at every level.
//   - expand_path: resolves `~` against $HOME on the Rust side.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

const MAX_PROJECT_ROOTS: usize = 32;
const MAX_PROJECT_DEPTH: i64 = 8;
const MAX_VISITED_DIRECTORIES: usize = 50_000;
const MAX_DISCOVERED_PROJECTS: usize = 4_096;

#[derive(Serialize)]
pub struct ProjectEntry {
    name: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRoot {
    path: String,
    #[serde(default = "default_depth")]
    depth: i64,
    /// Emit the root itself as a project, whether or not it is a git repo.
    /// This is what a "pinned project" used to be: a root with depth 0 and
    /// this set is exactly the old behaviour.
    #[serde(default)]
    self_index: bool,
}

fn default_depth() -> i64 {
    1
}

fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
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

#[tauri::command]
pub fn expand_path(path: String) -> String {
    expand(&path).to_string_lossy().into_owned()
}

#[tauri::command]
pub fn is_directory(path: String) -> bool {
    expand(&path).is_dir()
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
) -> AppResult<()> {
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
            return Err(AppError::BadArg(
                "project discovery visited too many directories; narrow the roots or depth",
            ));
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
                        return Err(AppError::BadArg("project discovery found too many repositories; narrow the roots or depth"));
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

#[tauri::command]
pub async fn scan_project_roots(roots: Vec<ProjectRoot>) -> AppResult<Vec<ProjectEntry>> {
    tauri::async_runtime::spawn_blocking(move || scan_project_roots_sync(roots))
        .await
        .map_err(|error| AppError::Other(format!("scan_project_roots join: {error}")))?
}

fn scan_project_roots_sync(roots: Vec<ProjectRoot>) -> AppResult<Vec<ProjectEntry>> {
    if roots.len() > MAX_PROJECT_ROOTS {
        return Err(AppError::BadArg("too many project discovery roots"));
    }
    let mut out: Vec<ProjectEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    for r in roots {
        let root = expand(&r.path);
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
            r.depth.clamp(0, MAX_PROJECT_DEPTH),
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
