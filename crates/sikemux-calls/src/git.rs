use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct PullRequestUrlParams {
    pub repo: String,
}

/// Each `git.*` method with what it answers and its parameters by the names clients send them
/// under, the first of them the repository it works in.
macro_rules! methods {
    ($($name:literal $marker:ident -> $answer:ty { $($arg:ident: $ty:ty),* $(,)? }),* $(,)?) => {
        /// The methods as a client in Rust calls them: each struct is what the method takes.
        pub mod call {
            use serde::{Deserialize, Serialize};
            use sikemux_git::{blame, branches, log, remote, revisions, stash, status, worktree};

            $(
                #[derive(Clone, Serialize, Deserialize)]
                #[serde(rename_all = "camelCase")]
                pub struct $marker { $(pub $arg: $ty),* }

                impl crate::Call for $marker {
                    const METHOD: &'static str = $name;
                    type Params = $marker;
                    type Answer = $answer;
                }
            )*
        }
    };
}

methods!(
    "git.status" Status -> status::GitStatus { repo: String },
    "git.discoverRepos" DiscoverRepos -> Vec<status::DiscoveredRepo> { root: String },
    "git.projectRepo" ProjectRepo -> status::ProjectRepo { project: String },
    "git.overview" Overview -> log::GitOverview { repo: String },
    "git.log" Log -> Vec<log::GitCommit> { repo: String },
    "git.commitsNaming" CommitsNaming -> Vec<log::GitCommit> { repo: String, key: String },
    "git.diff" Diff -> String { repo: String, path: String, staged: bool },
    "git.stage" Stage -> () { repo: String, path: String },
    "git.unstage" Unstage -> () { repo: String, path: String },
    "git.stagePaths" StagePaths -> () { repo: String, paths: Vec<String> },
    "git.unstagePaths" UnstagePaths -> () { repo: String, paths: Vec<String> },
    "git.stageAll" StageAll -> () { repo: String },
    "git.unstageAll" UnstageAll -> () { repo: String },
    "git.discardFiles" DiscardFiles -> () { repo: String, paths: Vec<String>, mode: String },
    "git.branches" Branches -> Vec<branches::GitBranch> { repo: String },
    "git.checkout" Checkout -> () { repo: String, branch: String },
    "git.checkoutSmart" CheckoutSmart -> String { repo: String, branch: String },
    "git.branchCreate" BranchCreate -> () { repo: String, name: String, start_point: Option<String> },
    "git.branchDelete" BranchDelete -> () { repo: String, name: String, force: bool },
    "git.branchRename" BranchRename -> () { repo: String, old_name: String, new_name: String },
    "git.merge" Merge -> String { repo: String, branch: String },
    "git.mergeSquash" MergeSquash -> String { repo: String, branch: String },
    "git.reset" Reset -> () { repo: String, rev: String, mode: String },
    "git.revert" Revert -> () { repo: String, rev: String },
    "git.worktrees" Worktrees -> Vec<worktree::GitWorktree> { repo: String },
    "git.worktreeCreate" WorktreeCreate -> worktree::GitWorktree { repo: String, path: String, branch: String, create_branch: bool, start_point: Option<String> },
    "git.worktreeRemove" WorktreeRemove -> worktree::GitWorktree { repo: String, path: String, force: bool },
    "git.show" Show -> String { repo: String, rev: String },
    "git.fileAt" FileAt -> String { repo: String, rev: String, path: String },
    "git.fileDiff" FileDiff -> Vec<sikemux_git::diff::DiffRow> { repo: String, path: String, base_rev: String, head_rev: Option<String>, full: bool },
    "git.commitFiles" CommitFiles -> Vec<String> { repo: String, rev: String },
    "git.compare" Compare -> revisions::GitCompare { repo: String, base: String, head: String },
    "git.blame" Blame -> blame::GitBlame { repo: String, path: String, contents: Option<String> },
    "git.push" Push -> String { repo: String },
    "git.pull" Pull -> String { repo: String },
    "git.aiMessage" AiMessage -> String { repo: String, provider: Option<String>, model: Option<String> },
    "git.stashList" StashList -> Vec<stash::GitStash> { repo: String },
    "git.stashPush" StashPush -> () { repo: String, message: Option<String>, mode: String },
    "git.stashApply" StashApply -> () { repo: String, refname: String, sha: String },
    "git.stashPop" StashPop -> () { repo: String, refname: String, sha: String },
    "git.stashDrop" StashDrop -> () { repo: String, refname: String, sha: String },
    "git.stashBranch" StashBranch -> () { repo: String, refname: String, sha: String, name: String },
    "git.stashRename" StashRename -> () { repo: String, refname: String, sha: String, new_message: String },
    "git.remotes" Remotes -> Vec<remote::GitRemote> { repo: String },
    "git.remoteAdd" RemoteAdd -> () { repo: String, name: String, url: String },
    "git.remoteRemove" RemoteRemove -> () { repo: String, name: String },
    "git.remoteRename" RemoteRename -> () { repo: String, old_name: String, new_name: String },
    "git.remoteSetUrl" RemoteSetUrl -> () { repo: String, name: String, url: String },
    "git.fetch" Fetch -> String { repo: String, remote: Option<String> },
    "git.fetchRef" FetchRef -> String { repo: String, remote: String, source: String, branch: String },
    "git.remoteBranches" RemoteBranches -> Vec<remote::GitRemoteBranch> { repo: String, remote: String },
    "git.checkoutRemoteBranch" CheckoutRemoteBranch -> () { repo: String, remote: String, branch: String, local_name: Option<String> },
    "git.deleteRemoteBranch" DeleteRemoteBranch -> () { repo: String, remote: String, branch: String },
    "git.setUpstream" SetUpstream -> () { repo: String, branch: String, upstream: Option<String> },
);
