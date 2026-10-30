//! What a code host's plugin answers, as one trait, and the one place its
//! method names are read. A host leaves out what it cannot do, which then
//! answers as a method it does not have.

use std::future::Future;
use std::path::Path;

use serde::de::IgnoredAny;
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

macro_rules! code_host {
    ($($(#[$doc:meta])* $method:ident $name:literal ($ask:ty) -> $answer:ty;)*) => {
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

        /// Answers the code host method `method`.
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
    };
}

code_host! {
    /// What a git remote points at, and whether it is on the server this plugin serves.
    resolve_remote "resolveRemote" (RemoteQuery) -> Resolved;
    /// The account that reaches a repository, the default one first.
    account_for "accountFor" (AccountQuery) -> Option<String>;
    my_repos "myRepos" (MineQuery) -> Vec<RepoListing>;
    branches "branches" (RepoRef) -> Vec<String>;
    /// An avatar or picture the host serves, as a `data:` address.
    image "image" (ImageRef) -> String;

    workflows "workflows" (RepoRef) -> Vec<Workflow>;
    workflow_file "workflowFile" (WorkflowRef) -> WorkflowFile;
    workflow_file_at "workflowFileAt" (FileAt) -> WorkflowFile;
    dispatch_workflow "dispatch" (Dispatch) -> ();
    runs "runs" (RunQuery) -> RunPage;
    run "run" (RunRef) -> RunDetail;
    run_attempt "runAttempt" (AttemptRef) -> RunDetail;
    run_timing "runTiming" (RunRef) -> RunTiming;
    rerun "rerun" (Rerun) -> ();
    rerun_job "rerunJob" (RerunJob) -> ();
    cancel "cancel" (RunRef) -> ();
    delete_run "deleteRun" (RunRef) -> ();
    delete_run_logs "deleteRunLogs" (RunRef) -> ();
    job_log "jobLog" (JobRef) -> JobLog;
    job_log_excerpt "jobLogExcerpt" (ExcerptQuery) -> LogExcerpt;
    annotations "annotations" (CheckRunRef) -> Vec<Annotation>;
    job_summary "jobSummary" (CheckRunRef) -> Option<JobSummary>;
    checks "checks" (ChecksQuery) -> Vec<Check>;
    artifacts "artifacts" (RunRef) -> Vec<Artifact>;
    pending_approvals "pendingApprovals" (RunRef) -> Vec<PendingApproval>;
    review_deployment "reviewDeployment" (DeploymentReview) -> ();

    pulls "pulls" (PullQuery) -> Vec<Pull>;
    pull_counts "pullCounts" (RepoRef) -> PullCounts;
    issue_counts "issueCounts" (RepoRef) -> PullCounts;
    pull "pull" (PullRef) -> Pull;
    pull_files "pullFiles" (FilesQuery) -> Vec<ChangedFile>;
    pull_commits "pullCommits" (PullRef) -> Vec<PullCommit>;
    pull_reviews "pullReviews" (PullRef) -> Vec<Review>;
    /// The pull requests a commit came in through, the merged one first.
    commit_pulls "commitPulls" (CommitRef) -> Vec<Pull>;
    commit_authors "commitAuthors" (CommitsQuery) -> Vec<CommitAuthor>;
    merge_pull "mergePull" (Merge) -> ();
    create_pull "createPull" (NewPull) -> Pull;
    edit_pull "editPull" (PullEdit) -> Pull;
    set_pull_state "setPullState" (SetPullState) -> ();
    review_pull "reviewPull" (NewReview) -> ();
    review_feedback "reviewFeedback" (FeedbackQuery) -> Vec<ReviewFeedback>;
    answer_review "answerReview" (Answers) -> ();

    issues "issues" (IssueQuery) -> IssuePage;
    issue "issue" (IssueRef) -> Issue;
    create_issue "createIssue" (NewIssue) -> Issue;
    set_issue_state "setIssueState" (SetIssueState) -> ();
    comments "comments" (ThreadRef) -> Vec<Comment>;
    timeline "timeline" (ThreadRef) -> Vec<TimelineItem>;
    add_comment "addComment" (NewComment) -> ();

    releases "releases" (RepoRef) -> Vec<Release>;
    inbox "inbox" (InboxQuery) -> Vec<Notification>;
    mark_read "markRead" (NotificationRef) -> ();
    mark_all_read "markAllRead" (IgnoredAny) -> ();
}
