//! What a code host's plugin answers, as one trait, and the one place its
//! method names are read. A host leaves out what it cannot do, which then
//! answers as a method it does not have.

use std::future::Future;
use std::path::Path;

use serde_json::Value;

use super::asks::*;
use super::*;
use crate::{params, reply, PluginError, PluginFuture, PluginResult};

/// Hands back what `work` finds, its error said as a plugin's.
pub fn work<'a, T, E: Into<PluginError>>(
    work: impl Future<Output = Result<T, E>> + Send + 'a,
) -> PluginFuture<'a, T> {
    Box::pin(async move { work.await.map_err(Into::into) })
}

/// `CodeHost` methods that only run one piece of the plugin's work, each written
/// `method(dir, ask: Ask) -> Answer = work;`.
#[macro_export]
macro_rules! host_work {
    ($($method:ident($dir:ident, $ask:ident: $asks:ty) -> $answer:ty = $work:expr;)*) => {
        $(
            fn $method<'a>(
                &'a self,
                $dir: &'a std::path::Path,
                $ask: $asks,
            ) -> $crate::PluginFuture<'a, $answer> {
                $crate::codehost::work($work)
            }
        )*
    };
}

macro_rules! code_host {
    ($($(#[$doc:meta])* $method:ident $name:literal $($read:ident)? ($ask:ty) -> $answer:ty;)*) => {
        /// A code host as Sikemux reads it. Each method takes the folder only
        /// this plugin writes to and what it is asked.
        pub trait CodeHost: Send + Sync {
            $(
                $(#[$doc])*
                fn $method<'a>(&'a self, _dir: &'a Path, _ask: $ask) -> PluginFuture<'a, $answer> {
                    Box::pin(async { Err(PluginError::unknown_method($name)) })
                }
            )*
        }

        pub async fn dispatch(
            host: &dyn CodeHost,
            dir: &Path,
            method: &str,
            input: Value,
        ) -> PluginResult<Value> {
            match method {
                $($name => reply(host.$method(dir, params(input)?).await?),)*
                _ => Err(PluginError::unknown_method(method)),
            }
        }

        pub fn reads(method: &str) -> bool {
            match method {
                $($name => code_host!(@read $($read)?),)*
                _ => false,
            }
        }

        pub fn writes(method: &str) -> bool {
            match method {
                $($name => !code_host!(@read $($read)?),)*
                _ => false,
            }
        }
    };
    (@read read) => { true };
    (@read) => { false };
}

code_host! {
    /// What a git remote points at, and whether it is on the server this plugin serves.
    resolve_remote "resolveRemote" read (RemoteQuery) -> Resolved;
    /// The account that reaches a repository, the default one first.
    account_for "accountFor" read (AccountQuery) -> Option<String>;
    my_repos "myRepos" read (MineQuery) -> Vec<RepoListing>;
    branches "branches" read (RepoRef) -> Vec<String>;
    /// An avatar or picture the host serves, as a `data:` address.
    image "image" read (ImageRef) -> String;

    workflows "workflows" read (RepoRef) -> Vec<Workflow>;
    workflow_file "workflowFile" read (WorkflowRef) -> WorkflowFile;
    workflow_file_at "workflowFileAt" read (FileAt) -> WorkflowFile;
    dispatch_workflow "dispatch" (Dispatch) -> ();
    runs "runs" read (RunQuery) -> RunPage;
    run "run" read (RunRef) -> RunDetail;
    run_attempt "runAttempt" read (AttemptRef) -> RunDetail;
    run_timing "runTiming" read (RunRef) -> RunTiming;
    rerun "rerun" (Rerun) -> ();
    rerun_job "rerunJob" (RerunJob) -> ();
    cancel "cancel" (RunRef) -> ();
    delete_run "deleteRun" (RunRef) -> ();
    delete_run_logs "deleteRunLogs" (RunRef) -> ();
    job_log "jobLog" read (JobRef) -> JobLog;
    job_log_excerpt "jobLogExcerpt" read (ExcerptQuery) -> LogExcerpt;
    annotations "annotations" read (CheckRunRef) -> Vec<Annotation>;
    job_summary "jobSummary" read (CheckRunRef) -> Option<JobSummary>;
    checks "checks" read (ChecksQuery) -> Vec<Check>;
    artifacts "artifacts" read (RunRef) -> Vec<Artifact>;
    pending_approvals "pendingApprovals" read (RunRef) -> Vec<PendingApproval>;
    review_deployment "reviewDeployment" (DeploymentReview) -> ();

    pulls "pulls" read (PullQuery) -> Vec<Pull>;
    pull_counts "pullCounts" read (RepoRef) -> PullCounts;
    issue_counts "issueCounts" read (RepoRef) -> PullCounts;
    pull "pull" read (PullRef) -> Pull;
    pull_files "pullFiles" read (FilesQuery) -> Vec<ChangedFile>;
    pull_commits "pullCommits" read (PullRef) -> Vec<PullCommit>;
    pull_reviews "pullReviews" read (PullRef) -> Vec<Review>;
    /// The pull requests a commit came in through, the merged one first.
    commit_pulls "commitPulls" read (CommitRef) -> Vec<Pull>;
    commit_authors "commitAuthors" read (CommitsQuery) -> Vec<CommitAuthor>;
    merge_pull "mergePull" (Merge) -> ();
    create_pull "createPull" (NewPull) -> Pull;
    edit_pull "editPull" (PullEdit) -> Pull;
    /// Keeps a picture beside the repository's pull requests, for a description to show.
    attach_picture "attachPicture" (PictureFile) -> Attached;
    set_pull_state "setPullState" (SetPullState) -> ();
    review_pull "reviewPull" (NewReview) -> ();
    review_feedback "reviewFeedback" read (FeedbackQuery) -> Vec<ReviewFeedback>;
    answer_review "answerReview" (Answers) -> ();

    issues "issues" read (IssueQuery) -> IssuePage;
    issue "issue" read (IssueRef) -> Issue;
    create_issue "createIssue" (NewIssue) -> Issue;
    set_issue_state "setIssueState" (SetIssueState) -> ();
    comments "comments" read (ThreadRef) -> Vec<Comment>;
    timeline "timeline" read (ThreadRef) -> Vec<TimelineItem>;
    add_comment "addComment" (NewComment) -> ();

    releases "releases" read (RepoRef) -> Vec<Release>;
    inbox "inbox" read (InboxQuery) -> Vec<Notification>;
}
