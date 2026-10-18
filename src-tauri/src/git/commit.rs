use serde_json::json;
use tauri::State;

use super::{current_branch_name, default_remote, has_upstream, run_blocking, run_git};
use crate::agent_names::call;
use crate::pty::PtyManager;

/// Commits what is staged with `message`. The core runs the commit, so that
/// its footer names the agents whose lines it holds once the hooks are done.
pub(super) async fn commit_staged(
    manager: &PtyManager,
    repo: String,
    message: String,
) -> Result<String, String> {
    let client = manager.client().await.map_err(|error| error.to_string())?;
    let said = call(
        &client,
        "commitStaged".to_owned(),
        json!({ "dir": repo, "message": message }),
    )
    .await
    .map_err(|error| error.to_string())?;
    crate::activity::record_commit(&repo);
    Ok(said.as_str().map(str::to_owned).unwrap_or_default())
}

#[tauri::command]
pub async fn git_commit(
    manager: State<'_, PtyManager>,
    repo: String,
    message: String,
) -> Result<String, String> {
    commit_staged(&manager, repo, message).await
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
