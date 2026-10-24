use serde::{Deserialize, Serialize};

use super::{git_ok, open_repo, run_blocking};

#[derive(Serialize, Deserialize, Clone)]
pub struct GitStash {
    /// Reflog index (0 = top). We treat this as the stable id within a
    /// single session, but it shifts whenever the user drops/pops, so
    /// the UI re-reads after every mutation.
    pub index: usize,
    /// Stash commit id. This is the stable guard used before apply/pop/drop so
    /// external stash-list edits cannot make a stale UI row target another stash.
    pub sha: String,
    /// `stash@{N}` — the symbolic ref form, useful for `git stash apply <ref>`.
    pub refname: String,
    /// Branch name the stash was created from.
    pub branch: String,
    /// Free-form message (usually `WIP on <branch>: <sha> <subject>`).
    pub message: String,
}

pub async fn git_stash_list(repo: String) -> Result<Vec<GitStash>, String> {
    run_blocking(move || -> Result<Vec<GitStash>, String> {
        let repo = open_repo(&repo)?;
        let Ok(reflog) = repo.reflog("refs/stash") else {
            return Ok(Vec::new());
        };
        Ok(reflog
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let message =
                    String::from_utf8_lossy(entry.message_bytes().unwrap_or_default()).into_owned();
                GitStash {
                    index,
                    sha: entry.id_new().to_string(),
                    refname: format!("stash@{{{index}}}"),
                    branch: parse_stash_branch(&message),
                    message,
                }
            })
            .collect())
    })
    .await
}

fn parse_stash_branch(message: &str) -> String {
    // `WIP on foo: 1234abc subject` or `On foo: custom message`
    let stripped = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))
        .unwrap_or(message);
    stripped.split(':').next().unwrap_or("").trim().to_string()
}

fn resolve_stash_ref(repo: &str, refname: &str, expected_sha: &str) -> Result<String, String> {
    let cur_sha = git_ok(
        repo,
        &["rev-parse", "--verify", "--end-of-options", refname],
    )
    .unwrap_or_default()
    .trim()
    .to_string();
    if !expected_sha.is_empty() && cur_sha == expected_sha {
        return Ok(refname.to_string());
    }

    let out = git_ok(repo, &["stash", "list", "--format=%H%x09%gd"])?;
    for line in out.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        if sha == expected_sha && !name.is_empty() {
            return Ok(name.to_string());
        }
    }

    if expected_sha.is_empty() && !cur_sha.is_empty() {
        return Ok(refname.to_string());
    }
    Err(format!(
        "stash {refname} changed or no longer exists; refresh the stash list"
    ))
}

/// Create a new stash. `mode`:
/// - "all" (default) → `git stash push -u` (includes untracked).
/// - "staged" → `git stash push --staged`.
/// - "unstaged" → `git stash push --keep-index`, the closest single command
///   to leaving only the unstaged work in the stash.
pub async fn git_stash_push(
    repo: String,
    message: Option<String>,
    mode: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let mut args: Vec<String> = vec!["stash".into(), "push".into()];
        match mode.as_str() {
            "all" => {
                args.push("-u".into());
            }
            "staged" => {
                args.push("--staged".into());
            }
            "unstaged" => {
                args.push("--keep-index".into());
            }
            other => return Err(format!("unknown stash mode: {other}")),
        }
        if let Some(m) = message {
            if !m.trim().is_empty() {
                args.push("-m".into());
                args.push(m);
            }
        }
        let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
        git_ok(&repo, &str_args)?;
        Ok(())
    })
    .await
}

pub async fn git_stash_apply(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "apply", "--end-of-options", &r])?;
        Ok(())
    })
    .await
}

pub async fn git_stash_pop(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "pop", "--end-of-options", &r])?;
        Ok(())
    })
    .await
}

pub async fn git_stash_drop(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "drop", "--end-of-options", &r])?;
        Ok(())
    })
    .await
}

pub async fn git_stash_branch(
    repo: String,
    refname: String,
    sha: String,
    name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "branch", "--end-of-options", &name, &r])?;
        Ok(())
    })
    .await
}

pub async fn git_stash_rename(
    repo: String,
    refname: String,
    sha: String,
    new_message: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        // No native `git stash rename`. Store the replacement first, then remove
        // the now-shifted original. A failure can leave a harmless duplicate but
        // can never remove the only ordinary stash reference.
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        // Grab the underlying commit SHA for the stash so we can re-store.
        let sha = git_ok(&repo, &["rev-parse", "--verify", "--end-of-options", &r])?.trim().to_string();
        if sha.is_empty() {
            return Err(format!("could not resolve {r}"));
        }
        git_ok(&repo, &["stash", "store", "-m", &new_message, "--", &sha])?;
        let original_index = r
            .strip_prefix("stash@{")
            .and_then(|value| value.strip_suffix('}'))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                format!(
                    "unexpected stash reference {r}; replacement was stored but the original was kept"
                )
            })?;
        let shifted_original = format!("stash@{{{}}}", original_index + 1);
        let shifted_sha = git_ok(
            &repo,
            &["rev-parse", "--verify", "--end-of-options", &shifted_original],
        )?
            .trim()
            .to_string();
        if shifted_sha != sha {
            return Err(
                "stash list changed during rename; replacement was stored and the original was kept"
                    .to_string(),
            );
        }
        git_ok(
            &repo,
            &["stash", "drop", "--end-of-options", &shifted_original],
        )?;
        Ok(())
    })
    .await
}
