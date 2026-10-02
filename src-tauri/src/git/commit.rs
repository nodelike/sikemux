use super::{
    current_branch_name, default_remote, git_command, has_upstream, run_blocking,
    run_command_with_timeout, run_git, GIT_COMMAND_TIMEOUT,
};

pub(super) fn commit_with_message(repo: &str, message: &str) -> Result<String, String> {
    let mut command = git_command(repo);
    command.args(["commit", "-F", "-"]);
    let out =
        run_command_with_timeout(&mut command, Some(message.as_bytes()), GIT_COMMAND_TIMEOUT)?;
    if out.status.success() {
        crate::activity::record_commit(repo);
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[tauri::command]
pub async fn git_commit(repo: String, message: String) -> Result<String, String> {
    run_blocking(move || commit_with_message(&repo, &message)).await
}

#[tauri::command]
pub async fn git_push(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let branch = current_branch_name(&repo)?;
        if !has_upstream(&repo) {
            let remote = default_remote(&repo)?;
            let (ok, so, se) = run_git(
                &repo,
                &[
                    "push",
                    "--set-upstream",
                    "--end-of-options",
                    &remote,
                    &branch,
                ],
            )?;
            return if ok {
                let out = format!("{so}{se}").trim().to_string();
                Ok(if out.is_empty() {
                    format!("published {branch} → {remote}/{branch}")
                } else {
                    format!("published {branch} → {remote}/{branch}\n{out}")
                })
            } else {
                Err(if se.trim().is_empty() { so } else { se })
            };
        }

        let (ok, so, se) = run_git(&repo, &["push"])?;
        if ok {
            return Ok(format!("{so}{se}").trim().to_string());
        }
        // Race-proof fallback: if upstream disappeared between the preflight
        // check and push, publish with -u instead of dumping raw Git advice.
        if se.contains("has no upstream branch") || se.contains("--set-upstream") {
            let remote = default_remote(&repo)?;
            let (ok2, so2, se2) = run_git(
                &repo,
                &[
                    "push",
                    "--set-upstream",
                    "--end-of-options",
                    &remote,
                    &branch,
                ],
            )?;
            return if ok2 {
                Ok(format!(
                    "published {branch} → {remote}/{branch}\n{}",
                    format!("{so2}{se2}").trim()
                ))
            } else {
                Err(if se2.trim().is_empty() { so2 } else { se2 })
            };
        }
        Err(if se.trim().is_empty() { so } else { se })
    })
    .await
}

fn looks_like_ff_only_divergence(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("not possible to fast-forward")
        || s.contains("divergent branches")
        || s.contains("need to specify how to reconcile")
        || s.contains("fatal: not possible to fast-forward")
}

#[tauri::command]
pub async fn git_pull(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        if !has_upstream(&repo) {
            return Err("No upstream configured for this branch — publish it or set an upstream from the remotes panel first.".into());
        }
        let (ok, so, se) = run_git(&repo, &["pull", "--ff-only"])?;
        if ok {
            return Ok(format!("{so}{se}").trim().to_string());
        }
        let err = format!("{so}{se}");
        if !looks_like_ff_only_divergence(&err) {
            return Err(err.trim().to_string());
        }

        // Git 2.27+ asks users to configure pull.rebase for divergent pulls.
        // Do the app-level sane default instead: rebase with autostash, without
        // mutating the user's global config.
        let (ok2, so2, se2) = run_git(&repo, &["pull", "--rebase", "--autostash"])?;
        if ok2 {
            let out = format!("{so2}{se2}").trim().to_string();
            Ok(if out.is_empty() { "rebased onto upstream".into() } else { format!("rebased onto upstream\n{out}") })
        } else {
            Err(format!(
                "Fast-forward was not possible, and rebase needs attention. Resolve in the git pane or terminal, then continue the rebase.\n\n{}",
                format!("{so2}{se2}").trim()
            ))
        }
    })
    .await
}
