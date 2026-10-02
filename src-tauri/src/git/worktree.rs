use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use super::{git_ok, open_repo, run_blocking};

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GitWorktree {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    reference: Option<String>,
    detached: bool,
    locked: bool,
    lock_reason: Option<String>,
    prunable: bool,
    prune_reason: Option<String>,
    bare: bool,
    current: bool,
    is_main: bool,
}

fn worktree_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("worktree path is empty".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("worktree path must be absolute".into());
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("worktree path must not contain '.' or '..' components".into());
    }
    if path.parent().is_none() {
        return Err("the filesystem root cannot be used as a worktree".into());
    }
    Ok(path)
}

fn same_worktree_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Resolve every existing path component, then append the still-missing tail.
/// This catches a target whose lexical parent is harmless but whose nearest
/// existing parent is a symlink into a protected Git/worktree directory.
fn resolved_path_for_containment(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("cannot resolve worktree target {}", path.display()))?;
        missing.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| format!("cannot resolve worktree target {}", path.display()))?;
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|error| format!("resolve worktree target {}: {error}", path.display()))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn validate_worktree_target(
    repo: &str,
    path: &Path,
    worktrees: &[GitWorktree],
) -> Result<(), String> {
    let resolved_target = resolved_path_for_containment(path)?;
    let common_dir = git_ok(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = resolved_path_for_containment(Path::new(common_dir.trim()))?;
    if resolved_target.starts_with(&common_dir) {
        return Err("the target path cannot be inside Git's common directory".into());
    }
    for worktree in worktrees {
        let root = resolved_path_for_containment(Path::new(&worktree.path))?;
        if resolved_target.starts_with(&root) {
            return Err(format!(
                "the target path cannot be inside registered worktree {}",
                worktree.path
            ));
        }
    }
    Ok(())
}

fn current_worktree_path(repo: &str) -> Option<PathBuf> {
    git_ok(
        repo,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )
    .ok()
    .map(|path| PathBuf::from(path.trim()))
    .or_else(|| {
        git_ok(repo, &["rev-parse", "--path-format=absolute", "--git-dir"])
            .ok()
            .map(|path| PathBuf::from(path.trim()))
    })
}

fn read_worktrees(repo: &str) -> Result<Vec<GitWorktree>, String> {
    // `-z` makes paths and optional reason strings unambiguous. Git emits a
    // double NUL between records; every other field is a single NUL token.
    let output = git_ok(repo, &["worktree", "list", "--porcelain", "-z"])?;
    let current = current_worktree_path(repo);
    let mut worktrees = Vec::new();
    let mut fields: Vec<&str> = Vec::new();

    let finish = |fields: &mut Vec<&str>, worktrees: &mut Vec<GitWorktree>| {
        if fields.is_empty() {
            return;
        }
        let mut path = None;
        let mut head = None;
        let mut reference = None;
        let mut detached = false;
        let mut bare = false;
        let mut lock_reason = None;
        let mut prune_reason = None;
        for field in fields.drain(..) {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(value.to_string());
            } else if let Some(value) = field.strip_prefix("HEAD ") {
                head = Some(value.to_string());
            } else if let Some(value) = field.strip_prefix("branch ") {
                reference = Some(value.to_string());
            } else if field == "detached" {
                detached = true;
            } else if field == "bare" {
                bare = true;
            } else if field == "locked" {
                lock_reason = Some(String::new());
            } else if let Some(value) = field.strip_prefix("locked ") {
                lock_reason = Some(value.to_string());
            } else if field == "prunable" {
                prune_reason = Some(String::new());
            } else if let Some(value) = field.strip_prefix("prunable ") {
                prune_reason = Some(value.to_string());
            }
        }
        let Some(path) = path else {
            return;
        };
        let branch = reference
            .as_deref()
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .map(ToOwned::to_owned);
        let path_buf = PathBuf::from(&path);
        let current = current
            .as_deref()
            .is_some_and(|candidate| same_worktree_path(candidate, &path_buf));
        worktrees.push(GitWorktree {
            path,
            head,
            branch,
            reference,
            detached,
            locked: lock_reason.is_some(),
            lock_reason: lock_reason.filter(|reason| !reason.is_empty()),
            prunable: prune_reason.is_some(),
            prune_reason: prune_reason.filter(|reason| !reason.is_empty()),
            bare,
            current,
            // `git worktree list` guarantees the primary worktree first.
            is_main: worktrees.is_empty(),
        });
    };

    for field in output.split('\0') {
        if field.is_empty() {
            finish(&mut fields, &mut worktrees);
        } else {
            fields.push(field);
        }
    }
    finish(&mut fields, &mut worktrees);
    if worktrees.is_empty() {
        Err("git returned no worktrees for this repository".into())
    } else {
        Ok(worktrees)
    }
}

fn validate_branch_name(repo: &str, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("worktree branch name is empty".into());
    }
    if branch.starts_with('-') {
        return Err("worktree branch name cannot start with '-'".into());
    }
    git_ok(repo, &["check-ref-format", "--branch", branch])?;
    Ok(branch.to_string())
}

fn validate_start_point(repo: &str, start_point: Option<String>) -> Result<Option<String>, String> {
    let Some(start_point) = start_point else {
        return Ok(None);
    };
    let start_point = start_point.trim();
    if start_point.is_empty() {
        return Ok(None);
    }
    if start_point.starts_with('-') {
        return Err("worktree start point cannot start with '-'".into());
    }
    let commit = format!("{start_point}^{{commit}}");
    git_ok(repo, &["rev-parse", "--verify", &commit])?;
    Ok(Some(start_point.to_string()))
}

fn create_worktree(
    repo: &str,
    path: &str,
    branch: &str,
    create_branch: bool,
    start_point: Option<String>,
) -> Result<GitWorktree, String> {
    // Discover up front so errors for non-repositories are returned before
    // any target-path or branch processing.
    open_repo(repo)?;
    let path = worktree_path(path)?;
    let path_arg = path.to_string_lossy().into_owned();
    let branch = validate_branch_name(repo, branch)?;
    let start_point = validate_start_point(repo, start_point)?;
    if !create_branch && start_point.is_some() {
        return Err("a start point is only valid when creating a new branch".into());
    }
    let worktrees = read_worktrees(repo)?;
    validate_worktree_target(repo, &path, &worktrees)?;
    let parent = path
        .parent()
        .ok_or_else(|| "worktree path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create worktree parent {}: {error}", parent.display()))?;

    let mut args = vec!["worktree", "add"];
    if create_branch {
        args.extend(["-b", branch.as_str()]);
    }
    args.extend(["--", path_arg.as_str()]);
    if let Some(start_point) = start_point.as_deref() {
        args.push(start_point);
    } else if !create_branch {
        args.push(branch.as_str());
    }
    git_ok(repo, &args)?;

    read_worktrees(repo)?
        .into_iter()
        .find(|worktree| same_worktree_path(Path::new(&worktree.path), &path))
        .ok_or_else(|| "worktree was created but could not be found in Git's registry".into())
}

fn remove_worktree(repo: &str, path: &str, force: bool) -> Result<GitWorktree, String> {
    open_repo(repo)?;
    let path = worktree_path(path)?;
    let worktree = read_worktrees(repo)?
        .into_iter()
        .find(|worktree| same_worktree_path(Path::new(&worktree.path), &path))
        .ok_or_else(|| "the target path is not a registered worktree".to_string())?;
    if worktree.is_main {
        return Err("the main worktree cannot be removed".into());
    }
    if worktree.bare {
        return Err("a bare repository cannot be removed as a linked worktree".into());
    }

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", worktree.path.as_str()]);
    git_ok(repo, &args)?;
    Ok(worktree)
}

#[tauri::command]
pub async fn git_worktree_list(repo: String) -> Result<Vec<GitWorktree>, String> {
    run_blocking(move || read_worktrees(&repo)).await
}

#[tauri::command]
pub async fn git_worktree_create(
    repo: String,
    path: String,
    branch: String,
    create_branch: bool,
    start_point: Option<String>,
) -> Result<GitWorktree, String> {
    run_blocking(move || create_worktree(&repo, &path, &branch, create_branch, start_point)).await
}

#[tauri::command]
pub async fn git_worktree_remove(
    repo: String,
    path: String,
    force: bool,
) -> Result<GitWorktree, String> {
    run_blocking(move || remove_worktree(&repo, &path, force)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{commit_base, git, init_repo};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn worktree_create_list_and_remove_new_branch() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent
            .path()
            .join("missing")
            .join("project")
            .join("feature-worktree");

        let created = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "feature/worktrees",
            true,
            Some("HEAD".into()),
        )
        .expect("create worktree");

        assert_eq!(created.branch.as_deref(), Some("feature/worktrees"));
        assert_eq!(
            created.reference.as_deref(),
            Some("refs/heads/feature/worktrees")
        );
        assert!(!created.is_main);
        assert!(!created.current);
        assert!(target.join("f.txt").is_file());
        let listed = read_worktrees(td.path().to_str().expect("repo path")).expect("list");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].is_main);
        assert!(listed[0].current);

        let removed = remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            false,
        )
        .expect("remove worktree");
        assert_eq!(removed.branch.as_deref(), Some("feature/worktrees"));
        assert!(!target.exists());
        assert_eq!(
            read_worktrees(td.path().to_str().expect("repo path"))
                .expect("list after removal")
                .len(),
            1
        );
    }

    #[test]
    fn worktree_create_accepts_a_folder_beside_the_repository() {
        let td = init_repo();
        commit_base(td.path());
        let repo = td.path().to_str().expect("repo path");
        let name = td.path().file_name().expect("repo name").to_string_lossy();
        let target = td
            .path()
            .with_file_name(format!("{name}.worktrees"))
            .join("fix-flaky-pty-test");

        let created = create_worktree(
            repo,
            target.to_str().expect("target path"),
            "sikemux/fix-flaky-pty-test",
            true,
            Some("HEAD".into()),
        )
        .expect("a sibling folder is outside the repository");
        assert_eq!(
            created.branch.as_deref(),
            Some("sikemux/fix-flaky-pty-test")
        );
        assert!(target.join("f.txt").is_file());

        remove_worktree(repo, target.to_str().expect("target path"), false).expect("remove");
        fs::remove_dir_all(target.parent().expect("worktrees folder")).expect("clean up");
    }

    #[test]
    fn worktree_uses_existing_branch_and_tracks_current_context() {
        let td = init_repo();
        commit_base(td.path());
        git(td.path(), &["branch", "existing"]);
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("existing-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "existing",
            false,
            None,
        )
        .expect("create existing branch worktree");

        let listed = read_worktrees(target.to_str().expect("target path")).expect("linked list");
        assert_eq!(listed.len(), 2);
        assert!(!listed[0].current);
        assert!(listed[1].current);
        assert_eq!(listed[1].branch.as_deref(), Some("existing"));
    }

    #[test]
    fn worktree_remove_refuses_main_and_requires_force_for_dirty_tree() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("dirty-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "dirty",
            true,
            None,
        )
        .expect("create worktree");

        let main_error = remove_worktree(
            target.to_str().expect("linked repo path"),
            td.path().to_str().expect("main path"),
            true,
        )
        .expect_err("main worktree must be protected from every linked worktree");
        assert!(main_error.contains("main worktree"), "{main_error}");

        fs::write(target.join("dirty.txt"), "uncommitted\n").expect("dirty worktree");
        let dirty_error = remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            false,
        )
        .expect_err("dirty worktree needs force");
        assert!(
            dirty_error.contains("modified or untracked"),
            "{dirty_error}"
        );
        remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            true,
        )
        .expect("force remove dirty worktree");
    }

    #[test]
    fn worktree_list_reports_lock_reason() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("locked-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "locked",
            true,
            None,
        )
        .expect("create worktree");
        git(
            td.path(),
            &[
                "worktree",
                "lock",
                "--reason",
                "agent session active",
                target.to_str().expect("target path"),
            ],
        );

        let listed = read_worktrees(td.path().to_str().expect("repo path")).expect("list");
        let locked = listed.iter().find(|item| !item.is_main).expect("linked");
        assert!(locked.locked);
        assert_eq!(locked.lock_reason.as_deref(), Some("agent session active"));
    }

    #[test]
    fn worktree_create_rejects_ambiguous_paths_and_option_like_refs() {
        let td = init_repo();
        commit_base(td.path());

        let relative = create_worktree(
            td.path().to_str().expect("repo path"),
            "relative/worktree",
            "feature",
            true,
            None,
        )
        .expect_err("relative targets must be rejected");
        assert!(relative.contains("must be absolute"), "{relative}");

        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("malicious-worktree");
        let branch = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "--force",
            true,
            None,
        )
        .expect_err("option-like branches must be rejected");
        assert!(branch.contains("cannot start with '-'"), "{branch}");

        let start = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "safe-feature",
            true,
            Some("--help".into()),
        )
        .expect_err("option-like revisions must be rejected");
        assert!(start.contains("cannot start with '-'"), "{start}");
        assert!(!target.exists());
    }

    #[test]
    fn worktree_create_rejects_protected_and_nested_targets_before_creating_dirs() {
        let td = init_repo();
        commit_base(td.path());

        let nested = td.path().join("nested/worktree");
        let nested_error = create_worktree(
            td.path().to_str().expect("repo"),
            nested.to_str().expect("nested target"),
            "nested-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("target inside a registered worktree must fail");
        assert!(
            nested_error.contains("inside registered worktree"),
            "{nested_error}"
        );
        assert!(!td.path().join("nested").exists());

        let common = td.path().join(".git/sikemux-test/worktree");
        let common_error = create_worktree(
            td.path().to_str().expect("repo"),
            common.to_str().expect("common-dir target"),
            "common-dir-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("target inside common Git dir must fail");
        assert!(common_error.contains("common directory"), "{common_error}");
        assert!(!td.path().join(".git/sikemux-test").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_create_rejects_symlink_parent_escape_before_creating_dirs() {
        use std::os::unix::fs::symlink;

        let td = init_repo();
        commit_base(td.path());
        let outside = tempdir().expect("outside");
        let alias = outside.path().join("apparently-external");
        symlink(td.path(), &alias).expect("symlink into worktree");
        let target = alias.join("escaped/worktree");

        let error = create_worktree(
            td.path().to_str().expect("repo"),
            target.to_str().expect("symlink target"),
            "symlink-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("symlink parent into registered worktree must fail");
        assert!(error.contains("inside registered worktree"), "{error}");
        assert!(!td.path().join("escaped").exists());
    }
}
