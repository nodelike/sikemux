use serde::Serialize;

use super::{git_ok, open_repo, run_blocking};

#[derive(Debug, PartialEq)]
enum PullRequestHost {
    GitHub,
    Bitbucket,
    GitLab,
}

fn remote_host_and_path(remote_url: &str) -> Option<(String, String)> {
    if let Ok(parsed) = url::Url::parse(remote_url) {
        if !matches!(
            parsed.scheme(),
            "https" | "http" | "ssh" | "git" | "git+ssh"
        ) {
            return None;
        }
        return Some((parsed.host_str()?.to_string(), parsed.path().to_string()));
    }
    let (user_host, path) = remote_url.split_once(':')?;
    if user_host.contains('/') {
        return None;
    }
    let host = user_host
        .rsplit_once('@')
        .map_or(user_host, |(_, host)| host);
    Some((host.to_string(), path.to_string()))
}

fn is_plain_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// gitlab.com, or a company's own GitLab, which goes by a `gitlab.` name.
fn is_gitlab(host: &str) -> bool {
    host == "gitlab.com" || host.starts_with("gitlab.")
}

fn pull_request_url(remote_url: &str, branch: &str) -> Result<String, String> {
    let unsupported = || format!("unsupported remote: {remote_url}");
    let (host, path) = remote_host_and_path(remote_url.trim()).ok_or_else(unsupported)?;
    let host = host.to_ascii_lowercase();
    let provider = match host.as_str() {
        "github.com" => PullRequestHost::GitHub,
        "bitbucket.org" => PullRequestHost::Bitbucket,
        gitlab if is_gitlab(gitlab) => PullRequestHost::GitLab,
        _ => return Err(unsupported()),
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    // A GitLab project can sit in groups inside groups; the other hosts have one owner.
    let shaped = match provider {
        PullRequestHost::GitLab => parts.len() >= 2,
        _ => parts.len() == 2,
    };
    if !shaped || !parts.iter().all(|part| is_plain_path_segment(part)) {
        return Err(unsupported());
    }

    let mut url = url::Url::parse(&match provider {
        PullRequestHost::GitHub => "https://github.com/".to_string(),
        PullRequestHost::Bitbucket => "https://bitbucket.org/".to_string(),
        PullRequestHost::GitLab => format!("https://{host}/"),
    })
    .map_err(|e| e.to_string())?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "cannot build pull request url".to_string())?;
        segments.clear().extend(&parts);
        match provider {
            PullRequestHost::GitHub => {
                segments.push("compare").extend(branch.split('/'));
            }
            PullRequestHost::Bitbucket => {
                segments.extend(["pull-requests", "new"]);
            }
            PullRequestHost::GitLab => {
                segments.extend(["-", "merge_requests", "new"]);
            }
        }
    }
    match provider {
        PullRequestHost::GitHub => url.query_pairs_mut().append_pair("expand", "1"),
        PullRequestHost::Bitbucket => url.query_pairs_mut().append_pair("source", branch),
        PullRequestHost::GitLab => url
            .query_pairs_mut()
            .append_pair("merge_request[source_branch]", branch),
    };
    Ok(url.into())
}

#[tauri::command]
pub async fn pr_open(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
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

        let url = pull_request_url(&remote_url, &branch)?;
        open::that_detached(&url).map_err(|e| e.to_string())?;
        Ok(url)
    })
    .await
}

#[derive(Serialize, Clone)]
pub struct GitRemote {
    pub name: String,
    /// Fetch URL — the one we display + use for cloning context.
    pub url: String,
}

/// `git remote -v` shape: `<name>\t<url> (fetch|push)` per line. We keep
/// only the fetch URL since that's authoritative for branch listing.
#[tauri::command]
pub async fn git_remotes(repo: String) -> Result<Vec<GitRemote>, String> {
    run_blocking(move || -> Result<Vec<GitRemote>, String> {
        let out = git_ok(&repo, &["remote", "-v"])?;
        let mut seen: std::collections::HashMap<String, String> = Default::default();
        for line in out.lines() {
            // Format: "origin\tgit@github.com:foo/bar.git (fetch)"
            let mut parts = line.split('\t');
            let name = parts.next().unwrap_or("").trim().to_string();
            let rest = parts.next().unwrap_or("");
            if name.is_empty() || rest.is_empty() {
                continue;
            }
            // Only keep fetch URLs.
            let is_fetch = rest.trim_end().ends_with("(fetch)");
            if !is_fetch {
                continue;
            }
            let url = rest
                .rsplit_once(' ')
                .map(|(url, _)| url)
                .unwrap_or("")
                .trim()
                .to_string();
            if !url.is_empty() {
                seen.insert(name, url);
            }
        }
        let mut list: Vec<GitRemote> = seen
            .into_iter()
            .map(|(name, url)| GitRemote { name, url })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    })
    .await
}

/// The URL of every remote of the repository `path` is in; none outside one.
pub fn remote_urls(path: &str) -> Vec<String> {
    let Ok(repo) = open_repo(path) else {
        return Vec::new();
    };
    let Ok(names) = repo.remotes() else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|name| name.ok().flatten())
        .filter_map(|name| {
            let remote = repo.find_remote(name).ok()?;
            remote.url().ok().map(str::to_owned)
        })
        .map(|url| with_real_ssh_host(&url, ssh_hostname))
        .collect()
}

/// A remote such as `git@github-work:org/repo` names a host alias from
/// ~/.ssh/config; this swaps in the host it stands for, so a plugin can tell
/// which service the remote lives on.
fn with_real_ssh_host(remote_url: &str, resolve: impl Fn(&str) -> Option<String>) -> String {
    if let Ok(mut parsed) = url::Url::parse(remote_url) {
        if !matches!(parsed.scheme(), "ssh" | "git+ssh") {
            return remote_url.to_owned();
        }
        let Some(real) = parsed.host_str().and_then(&resolve) else {
            return remote_url.to_owned();
        };
        return match parsed.set_host(Some(&real)) {
            Ok(()) => parsed.into(),
            Err(_) => remote_url.to_owned(),
        };
    }
    let Some((user_host, path)) = remote_url.split_once(':') else {
        return remote_url.to_owned();
    };
    if user_host.contains('/') {
        return remote_url.to_owned();
    }
    let (user, host) = match user_host.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, user_host),
    };
    let Some(real) = resolve(host) else {
        return remote_url.to_owned();
    };
    match user {
        Some(user) => format!("{user}@{real}:{path}"),
        None => format!("{real}:{path}"),
    }
}

/// `ssh -G` prints the settings ssh would use for a host without connecting.
fn ssh_hostname(host: &str) -> Option<String> {
    if host.starts_with('-') {
        return None;
    }
    let output = sikemux_process::user_environment::command("ssh")
        .args(["-G", host])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(str::to_owned)
        .filter(|real| real != host)
}

#[tauri::command]
pub async fn git_remote_add(repo: String, name: String, url: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "add", "--end-of-options", &name, &url])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_remote_remove(repo: String, name: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "remove", "--end-of-options", &name])?;
        Ok(())
    })
    .await
}

#[tauri::command]
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

#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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

#[derive(Serialize, Clone)]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
    use super::*;

    #[test]
    fn an_ssh_host_alias_is_replaced_by_the_host_it_stands_for() {
        let resolve = |host: &str| (host == "github-work").then(|| "github.com".to_owned());
        assert_eq!(
            with_real_ssh_host("git@github-work:org/repo.git", resolve),
            "git@github.com:org/repo.git"
        );
        assert_eq!(
            with_real_ssh_host("ssh://git@github-work/org/repo.git", resolve),
            "ssh://git@github.com/org/repo.git"
        );
        for untouched in [
            "git@bitbucket.org:team/repo.git",
            "https://github-work/org/repo.git",
            "/local/path/repo.git",
        ] {
            assert_eq!(with_real_ssh_host(untouched, resolve), untouched);
        }
    }

    #[test]
    fn remote_urls_are_read_from_anywhere_inside_the_repository() {
        let repo = tempfile::tempdir().expect("tempdir");
        let git = git2::Repository::init(repo.path()).expect("init");
        git.remote("origin", "git@github.com:nodelike/sikemux.git")
            .expect("origin");
        git.remote("mirror", "https://bitbucket.org/team/sikemux.git")
            .expect("mirror");
        let inside = repo.path().join("src");
        std::fs::create_dir(&inside).expect("subdirectory");

        let mut urls = remote_urls(&inside.to_string_lossy());
        urls.sort();
        assert_eq!(
            urls,
            [
                "git@github.com:nodelike/sikemux.git",
                "https://bitbucket.org/team/sikemux.git"
            ]
        );
        let outside = tempfile::tempdir().expect("tempdir");
        assert!(remote_urls(&outside.path().to_string_lossy()).is_empty());
    }

    #[test]
    fn a_fetched_ref_part_cannot_smuggle_an_option_or_a_second_refspec() {
        for good in ["origin", "pull/12/head", "pr-12", "feat/thing"] {
            assert!(super::plain_ref_part(good), "{good}");
        }
        for bad in ["", "-u", "a:b", "a b", "a..b", "a~1", "a^", "*"] {
            assert!(!super::plain_ref_part(bad), "{bad}");
        }
    }

    #[test]
    fn pull_request_urls_only_point_at_known_hosts() {
        let github = "https://github.com/nodelike/sikemux/compare/feat/x?expand=1";
        for remote in [
            "git@github.com:nodelike/sikemux.git",
            "https://github.com/nodelike/sikemux.git",
            "https://token@github.com/nodelike/sikemux",
            "ssh://git@github.com/nodelike/sikemux.git",
        ] {
            assert_eq!(pull_request_url(remote, "feat/x").as_deref(), Ok(github));
        }
        assert_eq!(
            pull_request_url("git@bitbucket.org:team/app.git", "feat/x").as_deref(),
            Ok("https://bitbucket.org/team/app/pull-requests/new?source=feat%2Fx")
        );
        assert_eq!(
            pull_request_url("git@gitlab.com:team/app.git", "feat/x").as_deref(),
            Ok("https://gitlab.com/team/app/-/merge_requests/new?merge_request%5Bsource_branch%5D=feat%2Fx")
        );
        assert_eq!(
            pull_request_url("git@gitlab.acme.dev:platform/payments/billing-api.git", "fix").as_deref(),
            Ok("https://gitlab.acme.dev/platform/payments/billing-api/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix")
        );
        assert_eq!(
            pull_request_url("git@github.com:o/r.git", "a#b?c d").as_deref(),
            Ok("https://github.com/o/r/compare/a%23b%3Fc%20d?expand=1")
        );
        for remote in [
            "https://github.com.evil.test/o/r.git",
            "https://evil.test/github.com/o/r.git",
            "git@evil.test:github.com/r.git",
            "file:///tmp/github.com/o/r",
            "javascript:alert(1)//github.com/o/r",
            "/tmp/github.com/o/r",
            "https://github.com/o/r/extra",
            "https://github.com/../r",
            "https://gitlab.com/only",
            "https://evilgitlab.com/o/r.git",
            "https://gitlab.com/o/../r",
        ] {
            assert!(pull_request_url(remote, "main").is_err(), "{remote}");
        }
    }
}
