use git2::{BranchType, Repository};
use serde::Serialize;

use super::{git_ok, open_repo, remote_names, run_blocking};

#[derive(Serialize, Clone)]
pub struct GitBranch {
    name: String,
    current: bool,
    upstream: Option<String>,
}

pub(super) fn read_branches(repo: &Repository) -> Result<Vec<GitBranch>, String> {
    // Lazygit-style ordering: current branch always at top, then everything
    // else sorted by most-recently-committed-on (so branches you've actually
    // touched recently float up over stale `main` / `master` copies).
    struct Row {
        branch: GitBranch,
        committed_at: i64,
        is_current: bool,
    }
    let mut rows: Vec<Row> = Vec::new();
    let iter = repo
        .branches(Some(BranchType::Local))
        .map_err(|e| e.message().to_string())?;
    for b in iter {
        let (branch, _) = match b {
            Ok(p) => p,
            Err(_) => continue,
        };
        let name = match branch.name() {
            Ok(Some(n)) => n.to_string(),
            _ => continue,
        };
        let upstream = branch
            .upstream()
            .ok()
            .and_then(|up| up.name().ok().flatten().map(String::from));
        let is_current = branch.is_head();
        // Tip-of-branch commit time; 0 if we can't resolve (won't push it
        // above a real branch — the sort prefers larger timestamps).
        let committed_at = branch
            .get()
            .peel_to_commit()
            .map(|c| c.time().seconds())
            .unwrap_or(0);
        rows.push(Row {
            branch: GitBranch {
                name,
                current: is_current,
                upstream,
            },
            committed_at,
            is_current,
        });
    }
    rows.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current) // current = true sorts first
            .then(b.committed_at.cmp(&a.committed_at)) // newer first
            .then(a.branch.name.cmp(&b.branch.name)) // tie-break alphabetical
    });
    Ok(rows.into_iter().map(|r| r.branch).collect())
}

#[tauri::command]
pub async fn git_branches(repo: String) -> Result<Vec<GitBranch>, String> {
    run_blocking(move || read_branches(&open_repo(&repo)?)).await
}

#[tauri::command]
pub async fn git_checkout(repo: String, branch: String) -> Result<(), String> {
    // git2 checkout is fiddly with working-tree handling — shell out.
    run_blocking(move || git_ok(&repo, &["checkout", "--end-of-options", &branch]).map(|_| ()))
        .await
}

fn local_branch_exists(repo: &str, branch: &str) -> bool {
    git_ok(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

fn remote_branch_exists(repo: &str, remote: &str, branch: &str) -> bool {
    git_ok(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{branch}"),
        ],
    )
    .is_ok()
}

fn normalize_branch_input(repo: &str, raw: &str) -> Result<(Option<String>, String), String> {
    let mut b = raw.trim().trim_start_matches("refs/heads/").to_string();
    if let Some(rest) = b.strip_prefix("refs/remotes/") {
        b = rest.to_string();
    }
    if b.is_empty() || b == "—" || b.eq_ignore_ascii_case("n/a") {
        return Err("No deployed branch to checkout for this environment.".into());
    }
    let remotes = remote_names(repo).unwrap_or_default();
    if let Some((maybe_remote, rest)) = b.split_once('/') {
        if remotes.iter().any(|r| r == maybe_remote) {
            return Ok((Some(maybe_remote.to_string()), rest.to_string()));
        }
    }
    Ok((None, b))
}

fn find_remote_branch(
    repo: &str,
    preferred: Option<&str>,
    branch: &str,
) -> Result<Option<String>, String> {
    let remotes = remote_names(repo)?;
    if let Some(r) = preferred {
        return Ok(remote_branch_exists(repo, r, branch).then(|| r.to_string()));
    }
    if remotes.iter().any(|r| r == "origin") && remote_branch_exists(repo, "origin", branch) {
        return Ok(Some("origin".into()));
    }
    let matches: Vec<String> = remotes
        .into_iter()
        .filter(|r| remote_branch_exists(repo, r, branch))
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => Err(format!(
            "Branch {branch} exists on multiple remotes ({}). Checkout from the remotes panel to choose one.",
            many.join(", ")
        )),
    }
}

#[tauri::command]
pub async fn git_checkout_smart(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let (preferred_remote, local) = normalize_branch_input(&repo, &branch)?;
        if local_branch_exists(&repo, &local) {
            git_ok(&repo, &["checkout", "--end-of-options", &local])?;
            return Ok(format!("checked out {local}"));
        }

        let mut remote = find_remote_branch(&repo, preferred_remote.as_deref(), &local)?;
        if remote.is_none() {
            match preferred_remote.as_deref() {
                Some(r) => {
                    let _ = git_ok(&repo, &["fetch", "--prune", "--end-of-options", r]);
                }
                None => {
                    let _ = git_ok(&repo, &["fetch", "--all", "--prune"]);
                }
            }
            remote = find_remote_branch(&repo, preferred_remote.as_deref(), &local)?;
        }
        let Some(remote) = remote else {
            return Err(format!(
                "Branch {branch} was not found locally or on any remote after fetch."
            ));
        };
        let full_ref = format!("{remote}/{local}");
        git_ok(
            &repo,
            &[
                "checkout",
                "-b",
                &local,
                "--track",
                "--end-of-options",
                &full_ref,
            ],
        )?;
        Ok(format!("checked out {local} tracking {full_ref}"))
    })
    .await
}

/// Create a new branch starting at `start_point` (default HEAD) and check it
/// out. Mirrors `git checkout -b name [start_point]` — the usual "branch
/// from where I am right now" flow.
#[tauri::command]
pub async fn git_branch_create(
    repo: String,
    name: String,
    start_point: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        let mut args: Vec<&str> = vec!["checkout", "-b", trimmed];
        if let Some(sp) = start_point.as_deref() {
            if !sp.is_empty() {
                args.extend(["--end-of-options", sp]);
            }
        }
        git_ok(&repo, &args).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_branch_delete(repo: String, name: String, force: bool) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        let flag = if force { "-D" } else { "-d" };
        git_ok(&repo, &["branch", flag, "--end-of-options", trimmed]).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_branch_rename(
    repo: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let old_trimmed = old_name.trim();
        let new_trimmed = new_name.trim();
        if old_trimmed.is_empty() || new_trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(
            &repo,
            &["branch", "-m", "--end-of-options", old_trimmed, new_trimmed],
        )
        .map(|_| ())
    })
    .await
}

/// Merge `branch` into the current HEAD with a merge commit (--no-ff so the
/// branch topology stays visible — common lazygit / Tower convention).
/// Returns the merge command output; conflict text comes back as the Err
/// for the caller to surface.
#[tauri::command]
pub async fn git_merge(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let trimmed = branch.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(&repo, &["merge", "--no-ff", "--end-of-options", trimmed])
    })
    .await
}

#[tauri::command]
pub async fn git_merge_squash(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let trimmed = branch.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(&repo, &["merge", "--squash", "--end-of-options", trimmed])
    })
    .await
}

#[tauri::command]
pub async fn git_reset(repo: String, rev: String, mode: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = rev.trim();
        if trimmed.is_empty() {
            return Err("revision is empty".into());
        }
        let flag = match mode.as_str() {
            "soft" => "--soft",
            "mixed" => "--mixed",
            "hard" => "--hard",
            other => return Err(format!("unknown reset mode: {other}")),
        };
        git_ok(&repo, &["reset", flag, "--end-of-options", trimmed]).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_revert(repo: String, rev: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = rev.trim();
        if trimmed.is_empty() {
            return Err("revision is empty".into());
        }
        git_ok(&repo, &["revert", "--no-edit", "--end-of-options", trimmed]).map(|_| ())
    })
    .await
}
