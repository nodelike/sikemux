// GitHub: the runs, pull requests, issues, releases and notifications of
// whichever repository Sikemux has open.

mod annotations;
mod approvals;
mod artifacts;
mod auth;
mod checks;
mod client;
mod common;
mod config;
mod error;
mod images;
mod inbox;
mod issues;
mod logs;
mod pictures;
mod pulls;
mod releases;
mod repo;
mod review;
mod runs;
mod workflows;

use std::path::Path;

use serde_json::Value;
use sikemux_plugin_api::account::{as_account, named_in};
use sikemux_plugin_api::codehost::asks::*;
use sikemux_plugin_api::codehost::{self, CodeHost};
use sikemux_plugin_api::codehost::{
    Annotation, Artifact, Attached, ChangedFile, Check, Comment, CommitAuthor, Issue, IssuePage,
    JobLog, JobSummary, LogExcerpt, Notification, PendingApproval, Pull, PullCommit, PullCounts,
    Release, RepoListing, Resolved, Review, ReviewFeedback, RunDetail, RunPage, RunTiming,
    TimelineItem, Workflow, WorkflowFile,
};
use sikemux_plugin_api::host_work;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

use crate::error::GithubResult;

sikemux_plugin_api::plugin!(Github);

/// What a git remote points at, and whether it is the GitHub this is signed in
/// to. A repository on another host is still described, so the view can say so
/// rather than showing nothing.
fn resolve(data_dir: &Path, query: RemoteQuery) -> Resolved {
    let config = config::load(data_dir).unwrap_or_default();
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| config.serves(&repo.host)),
        slug: found.as_ref().map(repo::slug),
        repo: found,
    }
}

/// Everything the sign-in screen needs about the account that just signed in.
async fn signed_in(data_dir: &Path, outcome: GithubResult<String>) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

impl Plugin for Github {
    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        let account = named_in(&input);
        Box::pin(as_account(account, self.dispatch(ctx, method, input)))
    }
}

impl Github {
    async fn dispatch(
        &self,
        ctx: &PluginContext,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginError> {
        let data_dir = ctx.data_dir();
        match method {
            "status" => reply(auth::status(data_dir).await),
            "setDefaultAccount" => {
                answer(store::set_default::<config::GithubConfig, error::Github>(
                    data_dir,
                    params(input)?,
                ))
                .await
            }
            "signIn" => signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await,
            "signOut" => answer(auth::sign_out(data_dir)).await,
            _ => codehost::dispatch(self, data_dir, method, input).await,
        }
    }
}

impl CodeHost for Github {
    fn resolve_remote<'a>(&'a self, dir: &'a Path, ask: RemoteQuery) -> PluginFuture<'a, Resolved> {
        Box::pin(async move { Ok(resolve(dir, ask)) })
    }

    host_work! {
        inbox(dir, ask: InboxQuery) -> Vec<Notification> = inbox::list(dir, ask);
        account_for(dir, ask: AccountQuery) -> Option<String> = auth::account_for(dir, ask.repo);
        my_repos(dir, ask: MineQuery) -> Vec<RepoListing> = repo::mine(dir, ask.limit);
        branches(dir, ask: RepoRef) -> Vec<String> = workflows::branches(dir, ask);
        image(dir, ask: ImageRef) -> String = images::image(dir, ask);
        workflows(dir, ask: RepoRef) -> Vec<Workflow> = workflows::list(dir, ask);
        workflow_file(dir, ask: WorkflowRef) -> WorkflowFile = workflows::file(dir, ask);
        workflow_file_at(dir, ask: FileAt) -> WorkflowFile = workflows::file_at(dir, ask);
        dispatch_workflow(dir, ask: Dispatch) -> () = workflows::dispatch(dir, ask);
        runs(dir, ask: RunQuery) -> RunPage = runs::list(dir, ask);
        run(dir, ask: RunRef) -> RunDetail = runs::detail(dir, ask);
        run_attempt(dir, ask: AttemptRef) -> RunDetail = runs::attempt(dir, ask);
        run_timing(dir, ask: RunRef) -> RunTiming = runs::timing(dir, ask);
        rerun(dir, ask: Rerun) -> () = runs::rerun(dir, ask);
        rerun_job(dir, ask: RerunJob) -> () = runs::rerun_job(dir, ask);
        cancel(dir, ask: RunRef) -> () = runs::cancel(dir, ask);
        delete_run(dir, ask: RunRef) -> () = runs::delete(dir, ask);
        delete_run_logs(dir, ask: RunRef) -> () = runs::delete_logs(dir, ask);
        job_log(dir, ask: JobRef) -> JobLog = logs::job(dir, ask);
        job_log_excerpt(dir, ask: ExcerptQuery) -> LogExcerpt = logs::excerpt(dir, ask);
        annotations(dir, ask: CheckRunRef) -> Vec<Annotation> = annotations::list(dir, ask);
        job_summary(dir, ask: CheckRunRef) -> Option<JobSummary> = annotations::summary(dir, ask);
        checks(dir, ask: ChecksQuery) -> Vec<Check> = checks::list(dir, ask);
        artifacts(dir, ask: RunRef) -> Vec<Artifact> = artifacts::list(dir, ask);
        pending_approvals(dir, ask: RunRef) -> Vec<PendingApproval> = approvals::pending(dir, ask);
        review_deployment(dir, ask: DeploymentReview) -> () = approvals::review(dir, ask);
        pulls(dir, ask: PullQuery) -> Vec<Pull> = pulls::list(dir, ask);
        pull_counts(dir, ask: RepoRef) -> PullCounts = pulls::counts(dir, ask);
        issue_counts(dir, ask: RepoRef) -> PullCounts = pulls::issue_counts(dir, ask);
        pull(dir, ask: PullRef) -> Pull = pulls::get(dir, ask);
        pull_files(dir, ask: FilesQuery) -> Vec<ChangedFile> = pulls::files(dir, ask);
        pull_commits(dir, ask: PullRef) -> Vec<PullCommit> = pulls::commits(dir, ask);
        pull_reviews(dir, ask: PullRef) -> Vec<Review> = pulls::reviews(dir, ask);
        commit_pulls(dir, ask: CommitRef) -> Vec<Pull> = pulls::of_commit(dir, ask);
        commit_authors(dir, ask: CommitsQuery) -> Vec<CommitAuthor> = pulls::commit_authors(dir, ask);
        merge_pull(dir, ask: Merge) -> () = pulls::merge(dir, ask);
        create_pull(dir, ask: NewPull) -> Pull = pulls::create(dir, ask);
        edit_pull(dir, ask: PullEdit) -> Pull = pulls::edit(dir, ask);
        attach_picture(dir, ask: PictureFile) -> Attached = pictures::attach(dir, ask);
        set_pull_state(dir, ask: SetPullState) -> () = pulls::set_state(dir, ask);
        review_pull(dir, ask: NewReview) -> () = pulls::review(dir, ask);
        review_feedback(dir, ask: FeedbackQuery) -> Vec<ReviewFeedback> = review::feedback(dir, ask);
        answer_review(dir, ask: Answers) -> () = review::answer(dir, ask);
        issues(dir, ask: IssueQuery) -> IssuePage = issues::list(dir, ask);
        issue(dir, ask: IssueRef) -> Issue = issues::get(dir, ask);
        create_issue(dir, ask: NewIssue) -> Issue = issues::create(dir, ask);
        set_issue_state(dir, ask: SetIssueState) -> () = issues::set_state(dir, ask);
        comments(dir, ask: ThreadRef) -> Vec<Comment> = common::comments(dir, ask);
        timeline(dir, ask: ThreadRef) -> Vec<TimelineItem> = common::timeline(dir, ask);
        add_comment(dir, ask: NewComment) -> () = common::add_comment(dir, ask);
        releases(dir, ask: RepoRef) -> Vec<Release> = releases::list(dir, ask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn reading_a_remote_never_needs_the_network() {
        let plugin = plugin();
        let ctx = PluginContext::new(std::env::temp_dir().join("sikemux-gha-remote"));
        let resolved = plugin
            .call(
                &ctx,
                "resolveRemote",
                json!({ "url": "git@github.com:nodelike/sikemux.git" }),
            )
            .await
            .expect("resolves");
        assert_eq!(resolved["slug"], "nodelike/sikemux");
        assert_eq!(resolved["sameHost"], true);

        let elsewhere = plugin
            .call(
                &ctx,
                "resolveRemote",
                json!({ "url": "git@gitlab.com:team/thing.git" }),
            )
            .await
            .expect("resolves");
        assert_eq!(elsewhere["sameHost"], false);
        assert_eq!(elsewhere["slug"], "team/thing");

        let nothing = plugin
            .call(&ctx, "resolveRemote", json!({ "url": "/srv/local.git" }))
            .await
            .expect("resolves");
        assert_eq!(nothing["repo"], Value::Null);
    }
}

/// Runs against a real GitHub only when asked:
/// `GHA_LIVE_REPO=owner/repo GH_TOKEN=… cargo test -p sikemux-plugin-github -- --ignored`
/// The token is read from the environment, so the Keychain is never touched.
#[cfg(test)]
mod live {
    use super::*;
    use serde_json::json;

    fn env(name: &str) -> Option<String> {
        // The app installs this at start-up; a test has to do it for itself.
        let _ = rustls::crypto::ring::default_provider().install_default();
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    #[tokio::test]
    #[ignore]
    async fn reads_workflows_runs_and_a_log() {
        let (Some(slug), Some(_)) = (env("GHA_LIVE_REPO"), env("GH_TOKEN")) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("sikemux-gha-live-{}", std::process::id()));
        let ctx = PluginContext::new(dir.clone());
        let plugin = plugin();
        let (owner, name) = slug.split_once('/').expect("GHA_LIVE_REPO is owner/repo");

        let status = plugin
            .call(&ctx, "signIn", json!({ "host": "github.com" }))
            .await
            .expect("sign in with the environment's token");
        assert_eq!(status["ok"], true, "{status}");
        assert_eq!(status["tokenSource"], "environment", "{status}");

        let target = json!({ "owner": owner, "name": name });
        let workflows = plugin
            .call(&ctx, "workflows", target.clone())
            .await
            .expect("workflows");
        assert!(workflows.as_array().is_some_and(|rows| !rows.is_empty()));

        let page = plugin
            .call(
                &ctx,
                "runs",
                json!({ "owner": owner, "name": name, "perPage": 5 }),
            )
            .await
            .expect("runs");
        let rows = page["runs"].as_array().expect("run rows");
        assert!(rows.len() <= 5);
        let Some(run_id) = rows.first().and_then(|row| row["id"].as_str()) else {
            return;
        };
        let run_id: u64 = run_id.parse().expect("a run id is a number");

        let detail = plugin
            .call(
                &ctx,
                "run",
                json!({ "owner": owner, "name": name, "runId": run_id }),
            )
            .await
            .expect("run detail");
        assert_eq!(detail["run"]["id"], run_id.to_string());

        if let Some(job_id) = detail["jobs"]
            .as_array()
            .and_then(|jobs| jobs.first())
            .and_then(|job| job["id"].as_str())
            .and_then(|id| id.parse::<u64>().ok())
        {
            let log = plugin
                .call(
                    &ctx,
                    "jobLog",
                    json!({ "owner": owner, "name": name, "jobId": job_id }),
                )
                .await
                .expect("job log");
            assert!(log["expired"].is_boolean());
        }

        plugin
            .call(&ctx, "signOut", Value::Null)
            .await
            .expect("sign out");
        std::fs::remove_dir_all(dir).ok();
    }
}
