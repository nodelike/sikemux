use git2::{BranchType, Repository};
use serde::Serialize;

use super::branches::{read_branches, GitBranch};
use super::status::{
    cached_walk, overview_cache, read_status, status_cache, store_walk, watched_generation,
    GitStatus,
};
use super::{git_walk_permit, open_repo, run_blocking};

#[derive(Serialize, Clone)]
pub struct GitCommit {
    /// Short, human-facing id (`8b075bd`).
    hash: String,
    /// Full oid — used by the frontend graph to match parents/children.
    full_hash: String,
    /// Full oids of this commit's parents (>1 == a merge).
    parents: Vec<String>,
    author: String,
    /// Stable key for the author colour chip (initials avatar).
    author_email: String,
    date: String,
    pub(super) subject: String,
    /// Ref decorations pointing at this commit: `HEAD`, local branches,
    /// `origin/main`, `tag: v0.1.11`. Rendered as lazygit-style badges.
    refs: Vec<String>,
    /// True when this commit is ahead of the current branch's upstream
    /// (reachable from HEAD but not from `@{u}`) — i.e. not yet pushed.
    /// Drives the unpushed-vs-pushed lane colour in the graph.
    unpushed: bool,
}

#[derive(Serialize, Clone)]
pub struct GitOverview {
    status: GitStatus,
    branches: Vec<GitBranch>,
    log: Vec<GitCommit>,
}

pub(super) fn relative_time(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - secs).max(0);
    if d < 60 {
        return format!("{}s ago", d);
    }
    if d < 3600 {
        return format!("{}m ago", d / 60);
    }
    if d < 86400 {
        return format!("{}h ago", d / 3600);
    }
    if d < 86400 * 30 {
        return format!("{}d ago", d / 86400);
    }
    if d < 86400 * 365 {
        return format!("{}mo ago", d / (86400 * 30));
    }
    format!("{}y ago", d / (86400 * 365))
}

/// Map commit oid → ref decorations (`HEAD`, local branches, remote branches,
/// tags) for the commits the log is about to return, so the graph timeline can
/// render lazygit-style ref badges without N extra git calls.
///
/// Only refs pointing straight at one of those commits are read. Resolving the
/// rest means loading an object per ref — every remote branch and every tag in
/// the repo — to decorate commits nobody is looking at.
fn build_ref_map(
    repo: &Repository,
    commits: &std::collections::HashSet<git2::Oid>,
) -> std::collections::HashMap<git2::Oid, Vec<String>> {
    let mut map: std::collections::HashMap<git2::Oid, Vec<String>> =
        std::collections::HashMap::new();
    // HEAD first so it renders leftmost on its commit.
    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target().filter(|oid| commits.contains(oid)) {
            map.entry(oid).or_default().push("HEAD".to_string());
        }
    }
    for glob in ["refs/heads/*", "refs/remotes/*", "refs/tags/*"] {
        let Ok(refs) = repo.references_glob(glob) else {
            continue;
        };
        for r in refs.flatten() {
            let Some(oid) = r.target().filter(|oid| commits.contains(oid)) else {
                continue;
            };
            let name = match r.shorthand() {
                Ok(n) => n.to_string(),
                Err(_) => continue,
            };
            // `HEAD` is handled above; `origin/HEAD` & friends are symbolic
            // pointers, not real branches — skip the noise.
            if name == "HEAD" || name.ends_with("/HEAD") {
                continue;
            }
            let label = if r.is_tag() {
                format!("tag: {name}")
            } else {
                name
            };
            map.entry(oid).or_default().push(label);
        }
    }
    map
}

/// Set of commit oids that are ahead of the current branch's upstream —
/// reachable from HEAD but not from `@{u}`. Empty when HEAD is detached or
/// the branch has no upstream (nothing to compare against → all "pushed").
fn unpushed_set(repo: &Repository) -> std::collections::HashSet<git2::Oid> {
    let mut set = std::collections::HashSet::new();
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return set,
    };
    let head_oid = match head.target() {
        Some(o) => o,
        None => return set,
    };
    let upstream_oid = head
        .shorthand()
        .ok()
        .and_then(|name| repo.find_branch(name, BranchType::Local).ok())
        .and_then(|b| b.upstream().ok())
        .and_then(|u| u.get().target());
    let upstream_oid = match upstream_oid {
        Some(o) => o,
        None => return set,
    };
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return set,
    };
    if revwalk.push(head_oid).is_err() || revwalk.hide(upstream_oid).is_err() {
        return set;
    }
    for oid in revwalk.flatten() {
        set.insert(oid);
    }
    set
}

/// Two commits made in the same second can come back parent-first, because the
/// time walk has no way to break the tie. Reorder the window so every commit
/// still precedes its own parents, and leave the time order alone otherwise.
pub(super) fn children_before_parents(commits: Vec<git2::Commit<'_>>) -> Vec<git2::Commit<'_>> {
    let position: std::collections::HashMap<git2::Oid, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| (commit.id(), i))
        .collect();
    let mut waiting_on_children = vec![0usize; commits.len()];
    for commit in &commits {
        for parent in commit.parent_ids() {
            if let Some(&i) = position.get(&parent) {
                waiting_on_children[i] += 1;
            }
        }
    }

    let mut taken = vec![false; commits.len()];
    let mut order = Vec::with_capacity(commits.len());
    while order.len() < commits.len() {
        let next = (0..commits.len()).find(|&i| !taken[i] && waiting_on_children[i] == 0);
        let Some(next) = next else { break };
        taken[next] = true;
        for parent in commits[next].parent_ids() {
            if let Some(&i) = position.get(&parent) {
                waiting_on_children[i] -= 1;
            }
        }
        order.push(next);
    }
    order.extend((0..commits.len()).filter(|&i| !taken[i]));

    let mut slots: Vec<Option<git2::Commit<'_>>> = commits.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

fn read_log(repo: &Repository, limit: usize) -> Result<Vec<GitCommit>, String> {
    let mut revwalk = repo.revwalk().map_err(|e| e.message().to_string())?;
    if revwalk.push_head().is_err() {
        return Ok(Vec::new());
    }
    // Newest first by commit time. Adding TOPOLOGICAL makes libgit2 enumerate
    // every reachable commit before it can hand back even the first one.
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.message().to_string())?;
    let mut commits = Vec::with_capacity(limit);
    for oid in revwalk.flatten() {
        if commits.len() >= limit {
            break;
        }
        if let Ok(commit) = repo.find_commit(oid) {
            commits.push(commit);
        }
    }
    Ok(describe_commits(repo, children_before_parents(commits)))
}

pub(super) fn describe_commits(
    repo: &Repository,
    commits: Vec<git2::Commit<'_>>,
) -> Vec<GitCommit> {
    let oids: std::collections::HashSet<git2::Oid> = commits.iter().map(|c| c.id()).collect();
    let ref_map = build_ref_map(repo, &oids);
    let unpushed = unpushed_set(repo);
    let mut out = Vec::with_capacity(commits.len());
    for commit in commits {
        let oid = commit.id();
        let short = commit
            .as_object()
            .short_id()
            .ok()
            .and_then(|b| b.as_str().ok().map(String::from))
            .unwrap_or_else(|| oid.to_string()[..7].to_string());
        out.push(GitCommit {
            hash: short,
            full_hash: oid.to_string(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
            author: commit.author().name().unwrap_or("").to_string(),
            author_email: commit.author().email().unwrap_or("").to_string(),
            date: relative_time(commit.time().seconds()),
            subject: commit.summary().ok().flatten().unwrap_or("").to_string(),
            unpushed: unpushed.contains(&oid),
            refs: ref_map.get(&oid).cloned().unwrap_or_default(),
        });
    }
    out
}

#[tauri::command]
pub async fn git_log(repo: String) -> Result<Vec<GitCommit>, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || read_log(&open_repo(&repo)?, 60)).await
}

#[tauri::command]
pub async fn git_overview(repo: String) -> Result<GitOverview, String> {
    let watched = watched_generation(&repo);
    if let Some((key, generation)) = &watched {
        if let Some(hit) = cached_walk(overview_cache(), key, *generation) {
            return Ok(hit);
        }
    }
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<GitOverview, String> {
        let r = open_repo(&repo)?;
        let overview = GitOverview {
            status: read_status(&r)?,
            branches: read_branches(&r)?,
            log: read_log(&r, 60)?,
        };
        if let Some((key, generation)) = watched {
            store_walk(
                status_cache(),
                key.clone(),
                generation,
                overview.status.clone(),
            );
            store_walk(overview_cache(), key, generation, overview.clone());
        }
        Ok(overview)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{commit_base, git, git_at, init_repo, repo_arg};
    use std::fs;

    /// A branch or tag that decorates a commit outside the window must not cost
    /// anything, and the ones inside it must still be labelled.
    #[test]
    fn ref_badges_cover_the_returned_commits_and_nothing_else() {
        let td = init_repo();
        commit_base(td.path());
        git(td.path(), &["tag", "on-base"]);
        git(td.path(), &["branch", "side"]);
        fs::write(td.path().join("f.txt"), "second\n").expect("write second");
        git(td.path(), &["commit", "-am", "second"]);
        git(td.path(), &["tag", "on-tip"]);

        let repo = open_repo(&repo_arg(td.path())).expect("open repo");
        let log = read_log(&repo, 1).expect("read log");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "second");

        let refs = &log[0].refs;
        assert!(refs.contains(&"HEAD".to_string()), "{refs:?}");
        assert!(refs.contains(&"tag: on-tip".to_string()), "{refs:?}");
        assert!(!refs.contains(&"side".to_string()), "{refs:?}");
        assert!(!refs.contains(&"tag: on-base".to_string()), "{refs:?}");
    }

    /// Commits made in the same second come out of the time walk in whatever
    /// order the heap likes. The log still has to read newest-first.
    #[test]
    fn a_commit_never_follows_its_own_parent_in_the_log() {
        let td = init_repo();
        let stamp = "2026-09-18T13:54:44+05:30";
        for subject in ["base", "second", "third", "fourth"] {
            fs::write(td.path().join("f.txt"), format!("{subject}\n")).expect("write file");
            git(td.path(), &["add", "f.txt"]);
            git_at(td.path(), stamp, &["commit", "-m", subject]);
        }

        let repo = open_repo(&repo_arg(td.path())).expect("open repo");
        let log = read_log(&repo, 10).expect("read log");

        let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["fourth", "third", "second", "base"]);
    }
}
