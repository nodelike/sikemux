//! The git panel's methods, `git.*`, for every client, and the watch on each
//! project that tells them all when to read again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use git2::Repository;

use super::board::codehost;
use super::connection::blocking;
use crate::protocol::{ProjectCodeHost, ProjectPull, PullChecks};
use serde::Deserialize;
use serde_json::Value;
use sikemux_process::sync::Locked;

use super::changed::Topic;
use crate::git::{
    ai, blame, branches, changes, commit, log, remote, revisions, stash, status, watch, worktree,
};

use super::access::{Needs, Peer};
use super::{Core, CoreError, CoreResult};

/// What a client must be allowed to call `method`.
pub(crate) fn needs(method: &str) -> Needs {
    if reads(method) {
        Needs::Watch
    } else if matches!(method, "git.push") {
        Needs::Person
    } else {
        Needs::Full
    }
}

/// Whether only Sikemux's own window or notch may call `method` from this
/// host; another program here may only read.
pub(crate) fn for_person(method: &str) -> bool {
    !reads(method)
}

/// Each method's parameters by the names clients send them under, the git
/// function it runs, and `read` for one that only reads.
macro_rules! methods {
    ($($name:literal $($read:ident)? => $run:path { $($arg:ident: $ty:ty),* $(,)? }),* $(,)?) => {
        fn reads(method: &str) -> bool {
            match method {
                $($name => methods!(@read $($read)?),)*
                _ => false,
            }
        }

        async fn run(method: &str, params: Value) -> CoreResult<Value> {
            match method {
                $($name => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Params { $($arg: $ty),* }
                    let Params { $($arg),* } = serde_json::from_value(params).map_err(unfit)?;
                    Ok(serde_json::to_value($run($($arg),*).await?)?)
                })*
                _ => Err(format!("this host does not know {method}").into()),
            }
        }
    };
    (@read read) => { true };
    (@read) => { false };
}

/// Whether a project or an agent here works inside `path`.
fn lived_in(core: &Core, path: &Path) -> bool {
    let projects = core.workspaces.project_paths();
    projects.iter().any(|project| project.starts_with(path))
        || core
            .workspaces
            .agent_ids()
            .into_iter()
            .filter_map(|id| core.workspaces.agent(&id))
            .any(|agent| agent.cwd.starts_with(path))
}

pub(crate) async fn call(
    core: &Core,
    peer: &Peer,
    method: &str,
    params: Value,
) -> CoreResult<Value> {
    if matches!(peer, Peer::Device { .. }) {
        let asked = ["repo", "root", "project"]
            .into_iter()
            .find_map(|key| params.get(key).and_then(Value::as_str))
            .map(PathBuf::from)
            .ok_or("the call names no repository")?;
        let projects = core.workspaces.project_paths();
        if !super::connection::blocking(move || Ok(of_a_project(&asked, &projects))).await? {
            return Err("a device may only look at the workspace's projects".into());
        }
    }
    if method == "git.worktreeRemove" {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        if path.is_some_and(|path| lived_in(core, &path)) {
            return Err(
                "close the worktree's Sikemux project and agents before removing it".into(),
            );
        }
    }
    let mut params = params;
    if method == "git.projectRepo" {
        let chosen = params
            .get("project")
            .and_then(Value::as_str)
            .and_then(|project| {
                core.preferences
                    .get()
                    .default_git_repos
                    .get(project)
                    .cloned()
            });
        params["chosen"] = serde_json::json!(chosen);
    }
    run(method, params).await
}

methods!(
    "git.status" read => status::git_status { repo: String },
    "git.discoverRepos" read => status::git_discover_repos { root: String },
    "git.projectRepo" read => status::git_project_repo { project: String, chosen: Option<String> },
    "git.overview" read => log::git_overview { repo: String },
    "git.log" read => log::git_log { repo: String },
    "git.diff" read => changes::git_diff { repo: String, path: String, staged: bool },
    "git.stage" => changes::git_stage { repo: String, path: String },
    "git.unstage" => changes::git_unstage { repo: String, path: String },
    "git.stagePaths" => changes::git_stage_paths { repo: String, paths: Vec<String> },
    "git.unstagePaths" => changes::git_unstage_paths { repo: String, paths: Vec<String> },
    "git.stageAll" => changes::git_stage_all { repo: String },
    "git.unstageAll" => changes::git_unstage_all { repo: String },
    "git.discardFiles" => changes::git_discard_files { repo: String, paths: Vec<String>, mode: String },
    "git.branches" read => branches::git_branches { repo: String },
    "git.checkout" => branches::git_checkout { repo: String, branch: String },
    "git.checkoutSmart" => branches::git_checkout_smart { repo: String, branch: String },
    "git.branchCreate" => branches::git_branch_create { repo: String, name: String, start_point: Option<String> },
    "git.branchDelete" => branches::git_branch_delete { repo: String, name: String, force: bool },
    "git.branchRename" => branches::git_branch_rename { repo: String, old_name: String, new_name: String },
    "git.merge" => branches::git_merge { repo: String, branch: String },
    "git.mergeSquash" => branches::git_merge_squash { repo: String, branch: String },
    "git.reset" => branches::git_reset { repo: String, rev: String, mode: String },
    "git.revert" => branches::git_revert { repo: String, rev: String },
    "git.worktrees" read => worktree::git_worktree_list { repo: String },
    "git.worktreeCreate" => worktree::git_worktree_create { repo: String, path: String, branch: String, create_branch: bool, start_point: Option<String> },
    "git.worktreeRemove" => worktree::git_worktree_remove { repo: String, path: String, force: bool },
    "git.show" read => revisions::git_show { repo: String, rev: String },
    "git.fileAt" read => revisions::git_file_at { repo: String, rev: String, path: String },
    "git.fileDiff" read => revisions::git_file_diff { repo: String, path: String, base_rev: String, head_rev: Option<String>, full: bool },
    "git.commitFiles" read => revisions::git_commit_files { repo: String, rev: String },
    "git.compare" read => revisions::git_compare { repo: String, base: String, head: String },
    "git.blame" read => blame::git_blame { repo: String, path: String, contents: Option<String> },
    "git.push" => commit::git_push { repo: String },
    "git.pull" => commit::git_pull { repo: String },
    "git.pullRequestUrl" read => remote::pull_request_url { repo: String },
    "git.aiMessage" => ai::git_ai_message { repo: String, provider: Option<String>, model: Option<String> },
    "git.stashList" read => stash::git_stash_list { repo: String },
    "git.stashPush" => stash::git_stash_push { repo: String, message: Option<String>, mode: String },
    "git.stashApply" => stash::git_stash_apply { repo: String, refname: String, sha: String },
    "git.stashPop" => stash::git_stash_pop { repo: String, refname: String, sha: String },
    "git.stashDrop" => stash::git_stash_drop { repo: String, refname: String, sha: String },
    "git.stashBranch" => stash::git_stash_branch { repo: String, refname: String, sha: String, name: String },
    "git.stashRename" => stash::git_stash_rename { repo: String, refname: String, sha: String, new_message: String },
    "git.remotes" read => remote::git_remotes { repo: String },
    "git.remoteAdd" => remote::git_remote_add { repo: String, name: String, url: String },
    "git.remoteRemove" => remote::git_remote_remove { repo: String, name: String },
    "git.remoteRename" => remote::git_remote_rename { repo: String, old_name: String, new_name: String },
    "git.remoteSetUrl" => remote::git_remote_set_url { repo: String, name: String, url: String },
    "git.fetch" => remote::git_fetch { repo: String, remote: Option<String> },
    "git.fetchRef" => remote::git_fetch_ref { repo: String, remote: String, source: String, branch: String },
    "git.remoteBranches" read => remote::git_remote_branches { repo: String, remote: String },
    "git.checkoutRemoteBranch" => remote::git_checkout_remote_branch { repo: String, remote: String, branch: String, local_name: Option<String> },
    "git.deleteRemoteBranch" => remote::git_delete_remote_branch { repo: String, remote: String, branch: String },
    "git.setUpstream" => remote::git_set_upstream { repo: String, branch: String, upstream: Option<String> },
);

/// Whether `asked` lies inside one of the projects, or is a worktree of one
/// of their repositories.
fn of_a_project(asked: &Path, projects: &[PathBuf]) -> bool {
    let Ok(asked) = asked.canonicalize() else {
        return false;
    };
    let shared = |path: &Path| {
        Repository::discover(path)
            .ok()
            .and_then(|repo| repo.commondir().canonicalize().ok())
    };
    let asked_shared = shared(&asked);
    projects.iter().any(|project| {
        let Ok(project) = project.canonicalize() else {
            return false;
        };
        asked.starts_with(&project)
            || asked_shared
                .as_ref()
                .is_some_and(|common| shared(&project).as_ref() == Some(common))
    })
}

fn unfit(error: serde_json::Error) -> CoreError {
    format!("the call's params do not fit: {error}").into()
}

/// Reads the branch chips of the projects at `folders` again, forgetting
/// those of folders that are no longer projects, and sends devices the view
/// when one changed.
fn read_chips(core: &Arc<Core>, folders: Vec<String>) {
    let core = core.clone();
    tokio::spawn(async move {
        let mut read = Vec::with_capacity(folders.len());
        for folder in folders {
            let chip = crate::git::status::git_chip(folder.clone()).await.ok();
            read.push((folder, chip));
        }
        let projects: HashSet<String> = core
            .workspaces
            .project_paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let changed = {
            let mut chips = core.git_chips.locked();
            let before = chips.len();
            chips.retain(|folder, _| projects.contains(folder));
            let mut changed = chips.len() != before;
            for (folder, chip) in read {
                changed |= match chip.filter(|_| projects.contains(&folder)) {
                    Some(chip) => chips.insert(folder, chip.clone()) != Some(chip),
                    None => chips.remove(&folder).is_some(),
                };
            }
            changed
        };
        if changed {
            core.view_changed();
        }
    });
}

/// A project's repository on its code host, and the pull request its branch
/// was in when last read.
#[derive(Clone, PartialEq)]
pub(crate) struct Hosted {
    pub shown: Option<ProjectCodeHost>,
    pub branch: Option<String>,
    pub pull: Option<ProjectPull>,
    /// The branch has no open pull request and is not a usual base.
    pub needs_pull: bool,
    /// The repository as the CI watch names it, once an account reaches it.
    pub watched: Option<super::ci::Shown>,
}

/// Reads again where the projects at `folders` are on their code hosts and
/// the pull requests their branches are in, and sends devices the view when
/// either changed. Runs when a project's branch or refs move, its CI moves,
/// or the person picks another repository for it.
pub(crate) fn read_hosts(core: &Arc<Core>, folders: Vec<String>) {
    let core = core.clone();
    tokio::spawn(async move {
        let mut read = Vec::with_capacity(folders.len());
        for folder in folders {
            let found = {
                let folder = PathBuf::from(&folder);
                let core = core.clone();
                blocking(move || {
                    let origin = super::board::git::origin(&folder);
                    Ok((origin, super::board::codehost::picks(&core, &folder)))
                })
                .await
            };
            let Ok((Some(origin), picks)) = found else {
                read.push((folder, None));
                continue;
            };
            let shown = codehost::shown(&origin, &picks).await;
            let branch = crate::git::status::git_chip(folder.clone())
                .await
                .ok()
                .map(|chip| chip.branch);
            let (pull, watched) = match (&shown, &branch) {
                (Some(_), Some(branch)) => match codehost::find(&origin, &picks).await {
                    Ok(repo) => (
                        codehost::branch_pull(&repo, branch).await.ok(),
                        Some(repo.watched()),
                    ),
                    Err(_) => (None, None),
                },
                _ => (None, None),
            };
            let needs_pull = matches!(pull, Some(None))
                && branch
                    .as_deref()
                    .is_some_and(|branch| !codehost::is_usual_base(branch));
            let hosted = Hosted {
                shown,
                branch,
                pull: pull.flatten(),
                needs_pull,
                watched,
            };
            read.push((folder, Some(hosted)));
        }
        let projects: HashSet<String> = core
            .workspaces
            .project_paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let changed = {
            let mut hosts = core.code_hosts.locked();
            let before = hosts.len();
            hosts.retain(|folder, _| projects.contains(folder));
            let mut changed = hosts.len() != before;
            for (folder, hosted) in read {
                changed |= match hosted.filter(|_| projects.contains(&folder)) {
                    Some(hosted) => hosts.insert(folder, hosted.clone()) != Some(hosted),
                    None => hosts.remove(&folder).is_some(),
                };
            }
            changed
        };
        if changed {
            core.view_changed();
        }
    });
}

/// The repositories whose pull request checks are still running, which the
/// CI watch keeps checking until they settle.
pub(crate) fn pending_pulls(core: &Core) -> Vec<super::ci::Shown> {
    core.code_hosts
        .locked()
        .values()
        .filter(|hosted| {
            hosted
                .pull
                .as_ref()
                .is_some_and(|pull| pull.checks == Some(PullChecks::Pending))
        })
        .filter_map(|hosted| hosted.watched.clone())
        .collect()
}

/// The CI of `repo` moved, so the pull requests of the projects on it are
/// read again.
pub(crate) fn ci_moved(core: &Arc<Core>, repo: &super::ci::Shown) {
    let folders: Vec<String> = core
        .code_hosts
        .locked()
        .iter()
        .filter(|(_, hosted)| hosted.watched.as_ref() == Some(repo))
        .map(|(folder, _)| folder.clone())
        .collect();
    if !folders.is_empty() {
        read_hosts(core, folders);
    }
}

/// Watches the workspace's projects as they are now, telling every client
/// when one moves.
pub(crate) fn follow_projects(core: &Core) {
    let Some(core) = core.me.upgrade() else {
        return;
    };
    let weak: Weak<Core> = core.me.clone();
    let changed: watch::Changed = Arc::new(move |projects, paths| {
        let Some(core) = weak.upgrade() else { return };
        let state_moved = paths
            .as_ref()
            .is_none_or(|paths| paths.iter().any(|path| path == watch::GIT_STATE));
        read_chips(&core, projects.to_vec());
        if state_moved {
            read_hosts(&core, projects.to_vec());
            super::codehost::refs_moved(&core, projects);
        }
        for project in projects {
            super::project_files::moved(project, paths.as_deref());
            core.changes
                .changed(Topic::Git, Some(project), paths.clone());
            if state_moved {
                core.collisions.head_moved(&core, project);
            }
        }
    });
    tokio::task::spawn_blocking(move || {
        core.git_watches
            .follow(|| core.workspaces.project_paths(), &changed);
        let identities = core
            .workspaces
            .project_paths()
            .into_iter()
            .map(|path| {
                let identity = crate::git::remote::project_identity(&path.to_string_lossy());
                (path, identity)
            })
            .collect();
        if core.workspaces.set_identities(identities) {
            core.view_changed();
        }
    });
}
