use serde::{Deserialize, Serialize};

use super::{default_remote, git_ok, open_repo, run_blocking, run_git};

/// A remote's address with its scheme, user and `.git` left out, so the same
/// repository reads the same however it was cloned.
pub fn normalized_remote(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = match rest.split_once('@') {
        Some((user, host)) if !user.contains('/') => host,
        _ => rest,
    };
    let rest = match rest.split_once(':') {
        Some((host, path)) if !path.starts_with(|c: char| c.is_ascii_digit()) => {
            format!("{host}/{path}")
        }
        _ => rest.to_owned(),
    };
    rest.to_lowercase()
}

/// What a project is known by on every host (ADR 0017): its remote,
/// normalised, else its repository's first commit, which every copy shares.
/// A folder outside git has none.
pub fn project_identity(path: &str) -> Option<String> {
    let remote = default_remote(path)
        .and_then(|remote| git_ok(path, &["remote", "get-url", &remote]))
        .ok()
        .filter(|url| !url.trim().is_empty());
    if let Some(url) = remote {
        return Some(normalized_remote(&url));
    }
    git_ok(path, &["rev-list", "--max-parents=0", "HEAD"])
        .ok()?
        .lines()
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .min()
        .map(str::to_owned)
}

/// The repository's origin and its current branch, which a new pull request
/// comes from.
pub async fn origin_and_branch(repo: String) -> Result<(String, String), String> {
    run_blocking(move || -> Result<(String, String), String> {
        let r = open_repo(&repo)?;
        let remote_url = r
            .find_remote("origin")
            .map_err(|e| e.message().to_string())?
            .url()
            .map_err(|e| e.message().to_string())?
            .to_string();

        let branch = r
            .head()
            .ok()
            .and_then(|h| h.shorthand().ok().map(String::from))
            .ok_or("no current branch (detached HEAD?)")?;

        Ok((remote_url.trim().to_owned(), branch))
    })
    .await
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct GitRemote {
    pub name: String,
    /// Fetch URL — the one we display + use for cloning context.
    pub url: String,
}

/// Each remote with its fetch URL, by name.
pub async fn git_remotes(repo: String) -> Result<Vec<GitRemote>, String> {
    run_blocking(move || -> Result<Vec<GitRemote>, String> {
        let repo = open_repo(&repo)?;
        let names = repo.remotes().map_err(|e| e.message().to_string())?;
        let mut list: Vec<GitRemote> = names
            .iter()
            .flatten()
            .flatten()
            .filter_map(|name| {
                let remote = repo.find_remote(name).ok()?;
                let url = remote.url().ok()?.trim();
                (!url.is_empty()).then(|| GitRemote {
                    name: name.to_string(),
                    url: url.to_string(),
                })
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    })
    .await
}

pub async fn git_remote_add(repo: String, name: String, url: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "add", "--end-of-options", &name, &url])?;
        Ok(())
    })
    .await
}

pub async fn git_remote_remove(repo: String, name: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "remove", "--end-of-options", &name])?;
        Ok(())
    })
    .await
}

pub async fn git_remote_rename(
    repo: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(
            &repo,
            &["remote", "rename", "--end-of-options", &old_name, &new_name],
        )?;
        Ok(())
    })
    .await
}

pub async fn git_remote_set_url(repo: String, name: String, url: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(
            &repo,
            &["remote", "set-url", "--end-of-options", &name, &url],
        )?;
        Ok(())
    })
    .await
}

/// Fetch a single remote when `remote` is set, otherwise `--all`. Always
/// passes `--prune` so stale remote-tracking branches get reaped — this
/// matches lazygit's default and avoids the "branch shows up after it was
/// deleted upstream" trap.
pub async fn git_fetch(repo: String, remote: Option<String>) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let out = match remote {
            Some(r) if !r.is_empty() => {
                git_ok(&repo, &["fetch", "--prune", "--end-of-options", &r])?
            }
            _ => git_ok(&repo, &["fetch", "--all", "--prune"])?,
        };
        Ok(out)
    })
    .await
}

/// Fetches one ref from a remote into a local branch, such as a pull request
/// from a fork, which exists only as `pull/N/head` on the remote it was opened on.
pub async fn git_fetch_ref(
    repo: String,
    remote: String,
    source: String,
    branch: String,
) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        for part in [&remote, &source, &branch] {
            if !plain_ref_part(part) {
                return Err(format!("{part:?} cannot be fetched"));
            }
        }
        let spec = format!("+{source}:refs/heads/{branch}");
        git_ok(&repo, &["fetch", "--end-of-options", &remote, &spec])
    })
    .await
}

/// The files commit `head` and the remote's branch `base` both changed in ways git cannot
/// merge, worked out without touching the checkout. `head` is fetched first when this
/// clone lacks it, as a pull request's head on the host may be.
pub async fn git_conflicts(
    repo: String,
    base: String,
    head: String,
) -> Result<Vec<String>, String> {
    run_blocking(move || -> Result<Vec<String>, String> {
        for part in [&base, &head] {
            if !plain_ref_part(part) {
                return Err(format!("{part:?} cannot be merged"));
            }
        }
        let remote = default_remote(&repo)?;
        let commit = format!("{head}^{{commit}}");
        if git_ok(&repo, &["cat-file", "-e", &commit]).is_err() {
            git_ok(
                &repo,
                &[
                    "fetch",
                    "--no-tags",
                    "--quiet",
                    "--end-of-options",
                    &remote,
                    &head,
                ],
            )?;
        }
        let base = format!("refs/remotes/{remote}/{base}");
        let (merged, out, err) = run_git(
            &repo,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                &base,
                &head,
            ],
        )?;
        if merged {
            return Ok(Vec::new());
        }
        let mut files: Vec<String> = out
            .lines()
            .skip(1)
            .take_while(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if files.is_empty() {
            return Err(err.trim().to_owned());
        }
        files.dedup();
        Ok(files)
    })
    .await
}

/// A remote, ref or branch name that cannot be read as an option or a second refspec.
fn plain_ref_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.contains(':')
        && !part.contains("..")
        && !part
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^?*[\\".contains(c))
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct GitRemoteBranch {
    /// Branch name WITHOUT the remote prefix (`main` not `origin/main`).
    pub name: String,
    /// Full ref form (`origin/main`) — what `git checkout --track` expects.
    pub full_ref: String,
    /// True if this is the symbolic HEAD pointer for the remote (e.g.
    /// `origin/HEAD -> origin/main`). UI shows it differently and skips
    /// it from most ops.
    pub is_head_pointer: bool,
    /// Local branch currently tracking this remote ref, if any.
    pub tracked_by: Option<String>,
    /// Tip subject line for the branch (best-effort).
    pub subject: Option<String>,
}

/// List branches under `refs/remotes/<remote>/`. We use `for-each-ref` so
/// we can extract the upstream-of mapping + the tip subject in a single
/// command instead of fanning out N `log -1` calls.
pub async fn git_remote_branches(
    repo: String,
    remote: String,
) -> Result<Vec<GitRemoteBranch>, String> {
    run_blocking(move || -> Result<Vec<GitRemoteBranch>, String> {
        let prefix = format!("refs/remotes/{remote}/");
        let format = "%(refname:short)%09%(symref)%09%(subject)";
        let out = git_ok(
            &repo,
            &[
                "for-each-ref",
                "--sort=refname",
                &format!("--format={format}"),
                &prefix,
            ],
        )?;

        // Build the local-branch → upstream map once so we can annotate each
        // remote branch with its tracking local. The cheap form:
        // `git for-each-ref refs/heads --format='%(refname:short)\t%(upstream:short)'`.
        let mut upstreams: std::collections::HashMap<String, String> = Default::default();
        if let Ok(locals) = git_ok(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname:short)\t%(upstream:short)",
                "refs/heads/",
            ],
        ) {
            for line in locals.lines() {
                let mut p = line.splitn(2, '\t');
                let local = p.next().unwrap_or("").to_string();
                let upstream = p.next().unwrap_or("").trim().to_string();
                if !upstream.is_empty() {
                    upstreams.insert(upstream, local);
                }
            }
        }

        let mut list = Vec::new();
        for line in out.lines() {
            let mut p = line.splitn(3, '\t');
            let full_ref = p.next().unwrap_or("").to_string();
            let symref = p.next().unwrap_or("").trim().to_string();
            let subject = p.next().unwrap_or("").to_string();
            if full_ref.is_empty() {
                continue;
            }
            let name = full_ref
                .strip_prefix(&format!("{remote}/"))
                .unwrap_or(&full_ref)
                .to_string();
            let is_head_pointer = !symref.is_empty() || name == "HEAD";
            list.push(GitRemoteBranch {
                name,
                full_ref: full_ref.clone(),
                is_head_pointer,
                tracked_by: upstreams.get(&full_ref).cloned(),
                subject: if subject.is_empty() {
                    None
                } else {
                    Some(subject)
                },
            });
        }
        Ok(list)
    })
    .await
}

/// Check out a remote branch into a new local tracking branch. If
/// `local_name` is omitted, uses the remote branch's leaf name (so
/// `origin/feat/foo` → local `feat/foo`).
pub async fn git_checkout_remote_branch(
    repo: String,
    remote: String,
    branch: String,
    local_name: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let full_ref = format!("{remote}/{branch}");
        let local = local_name.unwrap_or_else(|| branch.clone());
        // If the local already exists, just `checkout <local>`; otherwise
        // create-and-track.
        let exists = git_ok(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{local}"),
            ],
        )
        .is_ok();
        if exists {
            git_ok(&repo, &["checkout", "--end-of-options", &local])?;
        } else {
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
        }
        Ok(())
    })
    .await
}

/// Delete a remote branch by pushing the empty ref. Strict form
/// `git push <remote> --delete <branch>` — the one lazygit invokes.
pub async fn git_delete_remote_branch(
    repo: String,
    remote: String,
    branch: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(
            &repo,
            &["push", "--delete", "--end-of-options", &remote, &branch],
        )?;
        Ok(())
    })
    .await
}

/// Point a local branch's upstream at the given remote ref. Pass `null` /
/// empty `upstream` to clear the upstream entirely (matches lazygit's
/// "unset upstream" flow).
pub async fn git_set_upstream(
    repo: String,
    branch: String,
    upstream: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        match upstream {
            Some(u) if !u.is_empty() => {
                git_ok(
                    &repo,
                    &[
                        "branch",
                        &format!("--set-upstream-to={u}"),
                        "--end-of-options",
                        &branch,
                    ],
                )?;
            }
            _ => {
                git_ok(
                    &repo,
                    &["branch", "--unset-upstream", "--end-of-options", &branch],
                )?;
            }
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_fetched_ref_part_cannot_smuggle_an_option_or_a_second_refspec() {
        for good in ["origin", "pull/12/head", "pr-12", "feat/thing"] {
            assert!(super::plain_ref_part(good), "{good}");
        }
        for bad in ["", "-u", "a:b", "a b", "a..b", "a~1", "a^", "*"] {
            assert!(!super::plain_ref_part(bad), "{bad}");
        }
    }
}
