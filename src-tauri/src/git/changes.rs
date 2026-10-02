use std::path::Path;

use git2::{DiffFormat, DiffLineType, DiffOptions};

use super::{git_ok, open_repo, path_in_head, path_in_index, run_blocking, run_git};

fn write_diff_to_string(diff: &git2::Diff) -> Result<String, String> {
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_d, _h, line| {
        match line.origin_value() {
            DiffLineType::Context => out.push(' '),
            DiffLineType::Addition => out.push('+'),
            DiffLineType::Deletion => out.push('-'),
            DiffLineType::Binary => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[binary file changed]\n");
                return true;
            }
            DiffLineType::FileHeader | DiffLineType::HunkHeader => {}
            _ => {}
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.message().to_string())?;
    Ok(out)
}

fn git_diff_sync(repo: String, path: String, staged: bool) -> Result<String, String> {
    let r = open_repo(&repo)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(&path).context_lines(3);

    if staged {
        let head_tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = r
            .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
            .map_err(|e| e.message().to_string())?;
        return write_diff_to_string(&diff);
    }

    let diff = r
        .diff_index_to_workdir(None, Some(&mut opts))
        .map_err(|e| e.message().to_string())?;
    let s = write_diff_to_string(&diff)?;
    if !s.trim().is_empty() {
        return Ok(s);
    }

    let mut untracked = DiffOptions::new();
    untracked
        .pathspec(&path)
        .context_lines(3)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .include_ignored(true)
        .recurse_ignored_dirs(true);
    let diff = r
        .diff_index_to_workdir(None, Some(&mut untracked))
        .map_err(|e| e.message().to_string())?;
    write_diff_to_string(&diff)
}

#[tauri::command]
pub async fn git_diff(repo: String, path: String, staged: bool) -> Result<String, String> {
    run_blocking(move || git_diff_sync(repo, path, staged)).await
}

fn git_stage_sync(repo: String, path: String) -> Result<(), String> {
    let r = open_repo(&repo)?;
    let mut index = r.index().map_err(|e| e.message().to_string())?;
    let p = Path::new(&path);
    // If the file is gone, stage the deletion; else add the worktree content.
    let abs = Path::new(&repo).join(p);
    if !abs.exists() {
        index.remove_path(p).map_err(|e| e.message().to_string())?;
    } else {
        index.add_path(p).map_err(|e| e.message().to_string())?;
    }
    index.write().map_err(|e| e.message().to_string())
}

fn git_unstage_sync(repo: String, path: String) -> Result<(), String> {
    let r = open_repo(&repo)?;
    let head_commit = r.head().and_then(|h| h.peel_to_commit()).ok();
    let result = if let Some(head) = head_commit {
        r.reset_default(Some(head.as_object()), [&path])
            .map_err(|e| e.message().to_string())
    } else {
        // Pre-first-commit — remove from index.
        let mut idx = r.index().map_err(|e| e.message().to_string())?;
        idx.remove_path(Path::new(&path))
            .map_err(|e| e.message().to_string())?;
        idx.write().map_err(|e| e.message().to_string())
    };
    result
}

#[tauri::command]
pub async fn git_stage(repo: String, path: String) -> Result<(), String> {
    run_blocking(move || git_stage_sync(repo, path)).await
}

#[tauri::command]
pub async fn git_unstage(repo: String, path: String) -> Result<(), String> {
    run_blocking(move || git_unstage_sync(repo, path)).await
}

/// Paths per `git` invocation. Selecting a range in the git pane can name
/// thousands of files, and an argument list that long is rejected by the OS.
const GIT_PATH_BATCH: usize = 128;

fn git_over_paths(repo: &str, leading: &[&str], paths: &[String]) -> Result<(), String> {
    for chunk in paths.chunks(GIT_PATH_BATCH) {
        let mut args: Vec<&str> = leading.to_vec();
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        git_ok(repo, &args)?;
    }
    Ok(())
}

/// Which of `paths` the given listing command reports back. `-z` because a
/// file name may contain a newline.
fn paths_listed_by(
    repo: &str,
    leading: &[&str],
    paths: &[String],
) -> std::collections::HashSet<String> {
    let mut found = std::collections::HashSet::new();
    for chunk in paths.chunks(GIT_PATH_BATCH) {
        let mut args: Vec<&str> = leading.to_vec();
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        if let Ok((true, out, _)) = run_git(repo, &args) {
            found.extend(
                out.split('\0')
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
            );
        }
    }
    found
}

fn unstage_paths_with_git(repo: &str, paths: &[String]) -> Result<(), String> {
    git_over_paths(repo, &["restore", "--staged"], paths)
        .or_else(|_| git_over_paths(repo, &["reset", "HEAD"], paths))
        .or_else(|_| git_over_paths(repo, &["rm", "--cached", "--ignore-unmatch"], paths))
}

/// Stage a whole selection against one open repository and one index write.
/// Staging file by file re-opened the repo and rewrote the index per row.
#[tauri::command]
pub async fn git_stage_paths(repo: String, paths: Vec<String>) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let r = open_repo(&repo)?;
        let mut index = r.index().map_err(|e| e.message().to_string())?;
        for path in &paths {
            let relative = Path::new(path);
            // A file that is gone stages as a deletion.
            if Path::new(&repo).join(relative).exists() {
                index
                    .add_path(relative)
                    .map_err(|e| e.message().to_string())?;
            } else {
                index
                    .remove_path(relative)
                    .map_err(|e| e.message().to_string())?;
            }
        }
        index.write().map_err(|e| e.message().to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_unstage_paths(repo: String, paths: Vec<String>) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let r = open_repo(&repo)?;
        let head_commit = r.head().and_then(|h| h.peel_to_commit()).ok();
        match head_commit {
            Some(head) => r
                .reset_default(Some(head.as_object()), paths.iter().map(String::as_str))
                .map_err(|e| e.message().to_string()),
            None => {
                // Pre-first-commit — remove from the index.
                let mut index = r.index().map_err(|e| e.message().to_string())?;
                for path in &paths {
                    index
                        .remove_path(Path::new(path))
                        .map_err(|e| e.message().to_string())?;
                }
                index.write().map_err(|e| e.message().to_string())
            }
        }
    })
    .await
}

/// Discard a whole selection. Modes match `git_discard_file`; the paths are
/// grouped so each kind of git call happens once rather than once per file.
#[tauri::command]
pub async fn git_discard_files(
    repo: String,
    paths: Vec<String>,
    mode: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let restore_or_clean = |known: std::collections::HashSet<String>| -> Result<(), String> {
            let (restore, clean): (Vec<String>, Vec<String>) =
                paths.iter().cloned().partition(|path| known.contains(path));
            git_over_paths(&repo, &["restore", "--worktree"], &restore)?;
            git_over_paths(&repo, &["clean", "-f"], &clean)
        };
        match mode.as_str() {
            "staged" => unstage_paths_with_git(&repo, &paths),
            "unstaged" => restore_or_clean(paths_listed_by(&repo, &["ls-files", "-z"], &paths)),
            "all" => {
                let _ = unstage_paths_with_git(&repo, &paths);
                restore_or_clean(paths_listed_by(
                    &repo,
                    &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
                    &paths,
                ))
            }
            other => Err(format!("unknown discard mode: {other}")),
        }
    })
    .await
}

#[tauri::command]
pub async fn git_stage_all(repo: String) -> Result<(), String> {
    run_blocking(move || {
        let r = open_repo(&repo)?;
        let mut idx = r.index().map_err(|e| e.message().to_string())?;
        idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .map_err(|e| e.message().to_string())?;
        // Also stage deletions.
        idx.update_all(["*"], None)
            .map_err(|e| e.message().to_string())?;
        idx.write().map_err(|e| e.message().to_string())
    })
    .await
}

/// Reset every staged change back to HEAD — lazygit-style "unstage all".
/// Used by the `a` toggle in the files panel when everything is already
/// staged. Shells out to `git reset` because libgit2's mixed-reset path
/// is fiddlier than spawning the canonical command.
#[tauri::command]
pub async fn git_unstage_all(repo: String) -> Result<(), String> {
    run_blocking(move || git_ok(&repo, &["reset", "HEAD", "--"]).map(|_| ())).await
}

/// Discard changes to a single file. `mode`:
///   - "unstaged"       → revert working tree to match the index
///                        (= `git restore --worktree <path>`). Staged changes are
///                        preserved.
///   - "staged"         → unstage but leave the worktree alone
///                        (= `git restore --staged <path>`).
///   - "all"            → discard staged AND unstaged changes: first
///                        unstage, then restore. For untracked files this
///                        deletes the file (= `git clean -f <path>`).
///
/// For new (untracked) files, "unstaged" and "all" both remove the file
/// since there's no index or HEAD version to restore from.
#[tauri::command]
pub async fn git_discard_file(repo: String, path: String, mode: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        match mode.as_str() {
            "staged" => {
                git_ok(&repo, &["restore", "--staged", "--", &path])
                    .or_else(|_| git_ok(&repo, &["reset", "HEAD", "--", &path]))
                    .or_else(|_| {
                        git_ok(&repo, &["rm", "--cached", "--ignore-unmatch", "--", &path])
                    })?;
            }
            "unstaged" => {
                if path_in_index(&repo, &path) {
                    // Restore the worktree from the index, preserving staged
                    // content. `checkout HEAD -- path` would also wipe staged edits.
                    git_ok(&repo, &["restore", "--worktree", "--", &path])?;
                } else {
                    git_ok(&repo, &["clean", "-f", "--", &path])?;
                }
            }
            "all" => {
                let _ = git_ok(&repo, &["restore", "--staged", "--", &path])
                    .or_else(|_| git_ok(&repo, &["reset", "HEAD", "--", &path]));
                if path_in_head(&repo, &path) {
                    git_ok(&repo, &["restore", "--worktree", "--", &path])?;
                } else {
                    git_ok(&repo, &["clean", "-f", "--", &path])?;
                }
            }
            other => return Err(format!("unknown discard mode: {other}")),
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{commit_base, git, init_repo, repo_arg};
    use std::fs;

    #[tokio::test]
    async fn discard_unstaged_preserves_staged_changes() {
        let td = init_repo();
        commit_base(td.path());

        fs::write(td.path().join("f.txt"), "staged\n").expect("write staged");
        git(td.path(), &["add", "f.txt"]);
        fs::write(td.path().join("f.txt"), "unstaged\n").expect("write unstaged");

        git_discard_file(repo_arg(td.path()), "f.txt".into(), "unstaged".into())
            .await
            .expect("discard unstaged");

        assert_eq!(
            fs::read_to_string(td.path().join("f.txt")).expect("read worktree"),
            "staged\n"
        );
        assert_eq!(git(td.path(), &["show", ":f.txt"]), "staged\n");
    }

    #[tokio::test]
    async fn batched_staging_and_unstaging_covers_every_path_and_deletions() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("a.txt"), "a\n").expect("write a");
        fs::write(td.path().join("b.txt"), "b\n").expect("write b");
        fs::remove_file(td.path().join("f.txt")).expect("remove base");

        git_stage_paths(
            repo_arg(td.path()),
            vec!["a.txt".into(), "b.txt".into(), "f.txt".into()],
        )
        .await
        .expect("stage batch");

        let staged = git(td.path(), &["diff", "--cached", "--name-status"]);
        assert!(staged.contains("A\ta.txt"), "{staged}");
        assert!(staged.contains("A\tb.txt"), "{staged}");
        assert!(staged.contains("D\tf.txt"), "{staged}");

        git_unstage_paths(repo_arg(td.path()), vec!["a.txt".into(), "f.txt".into()])
            .await
            .expect("unstage batch");

        let staged = git(td.path(), &["diff", "--cached", "--name-status"]);
        assert_eq!(staged.trim(), "A\tb.txt");
    }

    #[tokio::test]
    async fn batched_discard_handles_tracked_and_untracked_together() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("f.txt"), "edited\n").expect("edit tracked");
        fs::write(td.path().join("new.txt"), "new\n").expect("write untracked");
        git(td.path(), &["add", "new.txt"]);

        git_discard_files(
            repo_arg(td.path()),
            vec!["f.txt".into(), "new.txt".into()],
            "all".into(),
        )
        .await
        .expect("discard batch");

        assert_eq!(
            fs::read_to_string(td.path().join("f.txt")).expect("read tracked"),
            "base\n"
        );
        assert!(!td.path().join("new.txt").exists());
        assert_eq!(git(td.path(), &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn discard_all_removes_staged_new_file() {
        let td = init_repo();
        commit_base(td.path());

        fs::write(td.path().join("new.txt"), "new\n").expect("write new");
        git(td.path(), &["add", "new.txt"]);

        git_discard_file(repo_arg(td.path()), "new.txt".into(), "all".into())
            .await
            .expect("discard all");

        assert!(!td.path().join("new.txt").exists());
        assert_eq!(git(td.path(), &["status", "--porcelain"]), "");
    }
}
