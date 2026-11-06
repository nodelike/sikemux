//! The git panel's methods, `git.*`, for every client, and the watch on each
//! project that tells them all when to read again.

use crate::server::changed::What;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use super::codehost::repo as codehost;
use super::connection::blocking;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;
use sikemux_view::workspace::{ProjectCodeHost, ProjectGit, ProjectPull, PullChecks};

use super::changed::Topic;
use crate::git::{
    ai, blame, branches, changes, commit, log, remote, revisions, stash, status, watch, worktree,
};

use super::access::{Needs, Peer, Program};
use super::calls::Row;
use super::{Core, CoreResult};

/// Each method's [`Row`], its parameters by the names clients send them under, the first of
/// them the repository it works in, and the git function it runs, `core` before it for one
/// that runs with the core as well.
macro_rules! methods {
    ($($name:literal $marker:ident -> $answer:ty, $needs:ident $(, $flag:ident)* $(; $with:ident)? => $run:path { $repo:ident: String $(, $arg:ident: $ty:ty)* $(,)? }),* $(,)?) => {
        /// The methods as a client in Rust calls them: each struct is what the method takes.
        pub mod call {
            #[allow(unused_imports)]
            use super::*;
            use serde::Serialize;

            $(
                #[derive(Clone, Serialize, Deserialize)]
                #[serde(rename_all = "camelCase")]
                pub struct $marker { pub $repo: String $(, pub $arg: $ty)* }

                impl crate::server::call::Call for $marker {
                    const METHOD: &'static str = $name;
                    type Params = $marker;
                    type Answer = $answer;
                }
            )*
        }

        pub(super) fn row(method: &str) -> Option<Row> {
            match method {
                $($name => Some(Row::of(Needs::$needs) $(.$flag())*),)*
                _ => None,
            }
        }

        pub(crate) async fn call(core: &Core, peer: &Peer, method: &str, params: Value) -> CoreResult<Value> {
            match method {
                $($name => {
                    let call::$marker { $repo $(, $arg)* } = super::calls::parse(params)?;
                    confine(core, peer, &$repo).await?;
                    let answer: $answer = methods!(@call $run, core, ($($with)?), $repo $(, $arg)*).await?;
                    Ok(serde_json::to_value(answer)?)
                })*
                _ => Err(crate::server::CoreError::unknown_method(method)),
            }
        }
    };
    (@call $run:path, $core:ident, (core), $($arg:ident),*) => { $run($core, $($arg),*) };
    (@call $run:path, $core:ident, (), $($arg:ident),*) => { $run($($arg),*) };
}

/// Whether `method` only reads, which any program, a mod among them, may do.
pub(super) fn reads(method: &str) -> bool {
    row(method).is_some_and(|row| matches!(row.program, Program::Anyone))
}

/// Refuses a device a repository outside the workspace's projects.
async fn confine(core: &Core, peer: &Peer, repo: &str) -> CoreResult<()> {
    if !matches!(peer, Peer::Device { .. }) {
        return Ok(());
    }
    let (asked, projects) = (PathBuf::from(repo), core.workspaces.project_paths());
    if !blocking(move || Ok(of_a_project(&asked, &projects))).await? {
        return Err("a device may only look at the workspace's projects".into());
    }
    Ok(())
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

#[derive(Serialize, Deserialize)]
pub struct PullRequestUrlParams {
    pub repo: String,
}

/// Where to open a pull request for the repository's branch, which the code host layer says.
pub(super) async fn pull_request_url(
    core: &Core,
    peer: &Peer,
    params: PullRequestUrlParams,
) -> CoreResult<String> {
    confine(core, peer, &params.repo).await?;
    let (origin, branch) = remote::origin_and_branch(params.repo).await?;
    Ok(codehost::compare_url(core, &origin, &branch).await?)
}

/// The repository of `project`, preferring the one the person picked for it.
async fn project_repo(core: &Core, project: String) -> Result<status::ProjectRepo, String> {
    let chosen = core
        .preferences
        .get()
        .default_git_repos
        .get(&project)
        .cloned();
    status::git_project_repo(project, chosen).await
}

/// Removes a worktree no project or agent here still works in.
async fn remove_worktree(
    core: &Core,
    repo: String,
    path: String,
    force: bool,
) -> Result<worktree::GitWorktree, String> {
    if lived_in(core, Path::new(&path)) {
        return Err("close the worktree's Sikemux project and agents before removing it".into());
    }
    worktree::git_worktree_remove(repo, path, force).await
}

methods!(
    "git.status" Status -> status::GitStatus, Watch, any_program => status::git_status { repo: String },
    "git.discoverRepos" DiscoverRepos -> Vec<status::DiscoveredRepo>, Watch, any_program => status::git_discover_repos { root: String },
    "git.projectRepo" ProjectRepo -> status::ProjectRepo, Watch, any_program; core => project_repo { project: String },
    "git.overview" Overview -> log::GitOverview, Watch, any_program => log::git_overview { repo: String },
    "git.log" Log -> Vec<log::GitCommit>, Watch, any_program => log::git_log { repo: String },
    "git.commitsNaming" CommitsNaming -> Vec<log::GitCommit>, Watch, any_program => log::git_commits_naming { repo: String, key: String },
    "git.diff" Diff -> String, Watch, any_program => changes::git_diff { repo: String, path: String, staged: bool },
    "git.stage" Stage -> (), Full => changes::git_stage { repo: String, path: String },
    "git.unstage" Unstage -> (), Full => changes::git_unstage { repo: String, path: String },
    "git.stagePaths" StagePaths -> (), Full, counted => changes::git_stage_paths { repo: String, paths: Vec<String> },
    "git.unstagePaths" UnstagePaths -> (), Full => changes::git_unstage_paths { repo: String, paths: Vec<String> },
    "git.stageAll" StageAll -> (), Full => changes::git_stage_all { repo: String },
    "git.unstageAll" UnstageAll -> (), Full => changes::git_unstage_all { repo: String },
    "git.discardFiles" DiscardFiles -> (), Full, counted => changes::git_discard_files { repo: String, paths: Vec<String>, mode: String },
    "git.branches" Branches -> Vec<branches::GitBranch>, Watch, any_program => branches::git_branches { repo: String },
    "git.checkout" Checkout -> (), Full => branches::git_checkout { repo: String, branch: String },
    "git.checkoutSmart" CheckoutSmart -> String, Full, counted => branches::git_checkout_smart { repo: String, branch: String },
    "git.branchCreate" BranchCreate -> (), Full, counted => branches::git_branch_create { repo: String, name: String, start_point: Option<String> },
    "git.branchDelete" BranchDelete -> (), Full => branches::git_branch_delete { repo: String, name: String, force: bool },
    "git.branchRename" BranchRename -> (), Full => branches::git_branch_rename { repo: String, old_name: String, new_name: String },
    "git.merge" Merge -> String, Full, counted => branches::git_merge { repo: String, branch: String },
    "git.mergeSquash" MergeSquash -> String, Full => branches::git_merge_squash { repo: String, branch: String },
    "git.reset" Reset -> (), Full => branches::git_reset { repo: String, rev: String, mode: String },
    "git.revert" Revert -> (), Full => branches::git_revert { repo: String, rev: String },
    "git.worktrees" Worktrees -> Vec<worktree::GitWorktree>, Watch, any_program => worktree::git_worktree_list { repo: String },
    "git.worktreeCreate" WorktreeCreate -> worktree::GitWorktree, Full => worktree::git_worktree_create { repo: String, path: String, branch: String, create_branch: bool, start_point: Option<String> },
    "git.worktreeRemove" WorktreeRemove -> worktree::GitWorktree, Full; core => remove_worktree { repo: String, path: String, force: bool },
    "git.show" Show -> String, Watch, any_program => revisions::git_show { repo: String, rev: String },
    "git.fileAt" FileAt -> String, Watch, any_program => revisions::git_file_at { repo: String, rev: String, path: String },
    "git.fileDiff" FileDiff -> Vec<crate::git::diff::DiffRow>, Watch, any_program => revisions::git_file_diff { repo: String, path: String, base_rev: String, head_rev: Option<String>, full: bool },
    "git.commitFiles" CommitFiles -> Vec<String>, Watch, any_program => revisions::git_commit_files { repo: String, rev: String },
    "git.compare" Compare -> revisions::GitCompare, Watch, any_program, counted => revisions::git_compare { repo: String, base: String, head: String },
    "git.blame" Blame -> blame::GitBlame, Watch, any_program, counted => blame::git_blame { repo: String, path: String, contents: Option<String> },
    "git.push" Push -> String, Owner, counted => commit::git_push { repo: String },
    "git.pull" Pull -> String, Full, counted => commit::git_pull { repo: String },
    "git.aiMessage" AiMessage -> String, Full, counted => ai::git_ai_message { repo: String, provider: Option<String>, model: Option<String> },
    "git.stashList" StashList -> Vec<stash::GitStash>, Watch, any_program => stash::git_stash_list { repo: String },
    "git.stashPush" StashPush -> (), Full, counted => stash::git_stash_push { repo: String, message: Option<String>, mode: String },
    "git.stashApply" StashApply -> (), Full => stash::git_stash_apply { repo: String, refname: String, sha: String },
    "git.stashPop" StashPop -> (), Full => stash::git_stash_pop { repo: String, refname: String, sha: String },
    "git.stashDrop" StashDrop -> (), Full => stash::git_stash_drop { repo: String, refname: String, sha: String },
    "git.stashBranch" StashBranch -> (), Full => stash::git_stash_branch { repo: String, refname: String, sha: String, name: String },
    "git.stashRename" StashRename -> (), Full => stash::git_stash_rename { repo: String, refname: String, sha: String, new_message: String },
    "git.remotes" Remotes -> Vec<remote::GitRemote>, Watch, any_program => remote::git_remotes { repo: String },
    "git.remoteAdd" RemoteAdd -> (), Full => remote::git_remote_add { repo: String, name: String, url: String },
    "git.remoteRemove" RemoteRemove -> (), Full => remote::git_remote_remove { repo: String, name: String },
    "git.remoteRename" RemoteRename -> (), Full => remote::git_remote_rename { repo: String, old_name: String, new_name: String },
    "git.remoteSetUrl" RemoteSetUrl -> (), Full => remote::git_remote_set_url { repo: String, name: String, url: String },
    "git.fetch" Fetch -> String, Full => remote::git_fetch { repo: String, remote: Option<String> },
    "git.fetchRef" FetchRef -> String, Full => remote::git_fetch_ref { repo: String, remote: String, source: String, branch: String },
    "git.remoteBranches" RemoteBranches -> Vec<remote::GitRemoteBranch>, Watch, any_program => remote::git_remote_branches { repo: String, remote: String },
    "git.checkoutRemoteBranch" CheckoutRemoteBranch -> (), Full => remote::git_checkout_remote_branch { repo: String, remote: String, branch: String, local_name: Option<String> },
    "git.deleteRemoteBranch" DeleteRemoteBranch -> (), Full => remote::git_delete_remote_branch { repo: String, remote: String, branch: String },
    "git.setUpstream" SetUpstream -> (), Full => remote::git_set_upstream { repo: String, branch: String, upstream: Option<String> },
);

/// Whether `asked` lies inside one of the projects, or is a worktree of one
/// of their repositories.
fn of_a_project(asked: &Path, projects: &[PathBuf]) -> bool {
    let Ok(asked) = asked.canonicalize() else {
        return false;
    };
    let shared = |path: &Path| crate::git::main_checkout(path);
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
            let mut chips = core.git.chips.locked();
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
            core.changed(What::View);
        }
    });
}

/// What the core keeps of its projects' git: their watches, branch chips and
/// code hosts, each by project folder.
#[derive(Default)]
pub(crate) struct Projects {
    watches: crate::git::watch::Watches,
    chips: Mutex<HashMap<String, ProjectGit>>,
    hosted: Mutex<HashMap<String, Hosted>>,
    /// The projects whose code hosts were asked again lately, and whether their refs moved
    /// since, so a burst of commits or staging asks them once.
    refreshes: Mutex<HashMap<String, bool>>,
}

impl Projects {
    /// The project at `folder` as clients are shown it: its code host, and its branch chip
    /// with the pull request that branch is in.
    pub(crate) fn shown(&self, folder: &str) -> (Option<ProjectCodeHost>, Option<ProjectGit>) {
        let hosts = self.hosted.locked();
        let hosted = hosts.get(folder);
        let chip = self.chips.locked().get(folder).cloned().map(|mut chip| {
            if let Some(hosted) =
                hosted.filter(|hosted| hosted.branch.as_deref() == Some(chip.branch.as_str()))
            {
                chip.pull = hosted.pull.open().cloned();
                chip.needs_pull = hosted.pull == BranchPull::Wanted;
            }
            chip
        });
        (hosted.and_then(|hosted| hosted.shown.clone()), chip)
    }
}

/// A project's repository on its code host, and the pull request its branch
/// was in when last read.
#[derive(Clone, PartialEq)]
struct Hosted {
    shown: Option<ProjectCodeHost>,
    branch: Option<String>,
    pull: BranchPull,
    /// The repository as the CI watch names it, once an account reaches it.
    watched: Option<super::codehost::ci::Shown>,
}

/// The pull request a branch is in, as its code host said.
#[derive(Clone, PartialEq)]
enum BranchPull {
    /// None, or none could be read; a usual base needs none.
    None,
    /// None, though the branch is not a usual base, so it wants one.
    Wanted,
    Open(ProjectPull),
}

impl BranchPull {
    fn open(&self) -> Option<&ProjectPull> {
        match self {
            Self::Open(pull) => Some(pull),
            Self::None | Self::Wanted => None,
        }
    }
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
                    let origin = crate::git::origin(&folder);
                    Ok((origin, super::codehost::repo::picks(&core, &folder)))
                })
                .await
            };
            let Ok((Some(origin), picks)) = found else {
                read.push((folder, None));
                continue;
            };
            let shown = codehost::shown(&core, &origin, &picks).await;
            let branch = crate::git::status::git_chip(folder.clone())
                .await
                .ok()
                .map(|chip| chip.branch);
            let (pull, watched) = match (&shown, &branch) {
                (Some(_), Some(branch)) => match codehost::find(&core, &origin, &picks).await {
                    Ok(repo) => {
                        let pull = match codehost::branch_pull(&repo, branch).await {
                            Ok(Some(pull)) => BranchPull::Open(pull),
                            Ok(None) if !codehost::is_usual_base(branch) => BranchPull::Wanted,
                            Ok(None) | Err(_) => BranchPull::None,
                        };
                        (pull, Some(repo.watched()))
                    }
                    Err(_) => (BranchPull::None, None),
                },
                _ => (BranchPull::None, None),
            };
            let hosted = Hosted {
                shown,
                branch,
                pull,
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
            let mut hosts = core.git.hosted.locked();
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
            core.changed(What::View);
        }
    });
}

/// The repositories whose pull request checks are still running, which the
/// CI watch keeps checking until they settle.
pub(crate) fn pending_pulls(core: &Core) -> Vec<super::codehost::ci::Shown> {
    core.git
        .hosted
        .locked()
        .values()
        .filter(|hosted| {
            hosted
                .pull
                .open()
                .is_some_and(|pull| pull.checks == Some(PullChecks::Pending))
        })
        .filter_map(|hosted| hosted.watched.clone())
        .collect()
}

/// The CI of `repo` moved, so the pull requests of the projects on it are
/// read again.
pub(crate) fn ci_moved(core: &Arc<Core>, repo: &super::codehost::ci::Shown) {
    let folders: Vec<String> = core
        .git
        .hosted
        .locked()
        .iter()
        .filter(|(_, hosted)| hosted.watched.as_ref() == Some(repo))
        .map(|(folder, _)| folder.clone())
        .collect();
    if !folders.is_empty() {
        read_hosts(core, folders);
    }
}

/// The soonest a project's code host is asked again after its refs or index moved.
const HOST_REFRESH_GAP: Duration = Duration::from_secs(5);

/// Asks the projects' code hosts where their branches and lists stand now, at once for a
/// project not asked lately, else once more when `HOST_REFRESH_GAP` is up, however often the
/// refs moved meanwhile.
fn refresh_hosts(core: &Arc<Core>, projects: &[String]) {
    let fresh: Vec<String> = {
        let mut refreshes = core.git.refreshes.locked();
        projects
            .iter()
            .filter(|project| match refreshes.get_mut(*project) {
                Some(again) => {
                    *again = true;
                    false
                }
                None => {
                    refreshes.insert((*project).clone(), false);
                    true
                }
            })
            .cloned()
            .collect()
    };
    for project in fresh {
        refresh_host_now(core, project);
    }
}

fn refresh_host_now(core: &Arc<Core>, project: String) {
    read_hosts(core, vec![project.clone()]);
    super::codehost::refs_moved(core, std::slice::from_ref(&project));
    core.checks.refs_moved(&project);
    let weak = Arc::downgrade(core);
    tokio::spawn(async move {
        tokio::time::sleep(HOST_REFRESH_GAP).await;
        let Some(core) = weak.upgrade() else { return };
        let again = {
            let mut refreshes = core.git.refreshes.locked();
            match refreshes.get_mut(&project) {
                Some(again) if *again => {
                    *again = false;
                    true
                }
                _ => {
                    refreshes.remove(&project);
                    false
                }
            }
        };
        if again {
            refresh_host_now(&core, project);
        }
    });
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
            refresh_hosts(&core, projects);
        }
        for project in projects {
            super::project_files::moved(project, paths.as_deref());
            core.changed(What::ReadIn(Topic::Git, Some(project), paths.clone()));
            if state_moved {
                core.collisions.head_moved(&core, project);
            }
        }
    });
    tokio::task::spawn_blocking(move || {
        core.git
            .watches
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
        core.workspaces.set_identities(identities);
    });
}
