// GitHub: the runs, pull requests, issues, releases and notifications of
// whichever repository Sikemux has open.
//
//   config    — which GitHub this is, and where the token comes from
//   client    — the HTTP client, size limits, and GitHub's error shapes
//   auth      — signing in, signing out, and who the token belongs to
//   repo      — a git remote turned into owner and repository
//   workflows — the workflows a repository has, and starting one by hand
//   runs      — runs, the jobs in one, and re-running or stopping them
//   logs      — a job's log, and what GitHub flagged in it
//   artifacts — what a run left behind, and putting one on disk
//   approvals — a run held at an environment until somebody signs it off
//   pulls     — pull requests, the files they touch, and merging one
//   review    — what reviewers asked of a pull request, and answering it
//   issues    — issues, and closing or reopening one
//   releases  — releases, their notes and the files hung off them
//   inbox     — the notifications GitHub would otherwise email
//   common    — the shapes every part of the API repeats
//   images    — avatars and pictures, handed to the window as data: addresses
//   watch     — following a run while it is going

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
mod pulls;
mod releases;
mod repo;
mod review;
mod runs;
mod workflows;

use std::path::Path;
use std::sync::Arc;

use serde::de::IgnoredAny;
use serde_json::Value;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::asks::*;
use sikemux_plugin_api::codehost::{self, work, CodeHost};
use sikemux_plugin_api::codehost::{
    Annotation, Artifact, ChangedFile, Check, Comment, CommitAuthor, Issue, IssuePage, JobLog,
    JobSummary, LogExcerpt, Notification, PendingApproval, Pull, PullCommit, PullCounts, Release,
    RepoListing, Resolved, Review, ReviewFeedback, RunDetail, RunPage, RunTiming, TimelineItem,
    Workflow, WorkflowFile,
};
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::GithubResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Github {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Github {
    manifest: Manifest,
}

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

/// Which account a call is for; with none named, the default one.
fn account_of(input: &Value) -> Option<String> {
    input
        .get("account")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
}

impl Plugin for Github {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        let account = account_of(&input);
        Box::pin(as_account(account, self.dispatch(ctx, method, input)))
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        let account = account_of(&input);
        Box::pin(as_account(
            account,
            dispatch_stream(ctx, method, input, sink),
        ))
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
            "accounts" => reply(auth::accounts(data_dir)?),
            "setDefaultAccount" => answer(auth::set_default(data_dir, params(input)?)).await,
            "rateLimit" => {
                let id = config::load(data_dir)?
                    .account(chosen().as_deref())
                    .map(|account| account.id.clone())
                    .unwrap_or_default();
                reply(client::LIMITS.budget(&id))
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

    fn account_for<'a>(
        &'a self,
        dir: &'a Path,
        ask: AccountQuery,
    ) -> PluginFuture<'a, Option<String>> {
        work(auth::account_for(dir, ask.repo))
    }

    fn my_repos<'a>(&'a self, dir: &'a Path, ask: MineQuery) -> PluginFuture<'a, Vec<RepoListing>> {
        work(repo::mine(dir, ask.limit))
    }

    fn branches<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<String>> {
        work(workflows::branches(dir, ask))
    }

    fn image<'a>(&'a self, dir: &'a Path, ask: ImageRef) -> PluginFuture<'a, String> {
        work(images::image(dir, ask))
    }

    fn workflows<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<Workflow>> {
        work(workflows::list(dir, ask))
    }

    fn workflow_file<'a>(
        &'a self,
        dir: &'a Path,
        ask: WorkflowRef,
    ) -> PluginFuture<'a, WorkflowFile> {
        work(workflows::file(dir, ask))
    }

    fn workflow_file_at<'a>(
        &'a self,
        dir: &'a Path,
        ask: FileAt,
    ) -> PluginFuture<'a, WorkflowFile> {
        work(workflows::file_at(dir, ask))
    }

    fn dispatch_workflow<'a>(&'a self, dir: &'a Path, ask: Dispatch) -> PluginFuture<'a, ()> {
        work(workflows::dispatch(dir, ask))
    }

    fn runs<'a>(&'a self, dir: &'a Path, ask: RunQuery) -> PluginFuture<'a, RunPage> {
        work(runs::list(dir, ask))
    }

    fn run<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, RunDetail> {
        work(runs::detail(dir, ask))
    }

    fn run_attempt<'a>(&'a self, dir: &'a Path, ask: AttemptRef) -> PluginFuture<'a, RunDetail> {
        work(runs::attempt(dir, ask))
    }

    fn run_timing<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, RunTiming> {
        work(runs::timing(dir, ask))
    }

    fn rerun<'a>(&'a self, dir: &'a Path, ask: Rerun) -> PluginFuture<'a, ()> {
        work(runs::rerun(dir, ask))
    }

    fn rerun_job<'a>(&'a self, dir: &'a Path, ask: RerunJob) -> PluginFuture<'a, ()> {
        work(runs::rerun_job(dir, ask))
    }

    fn cancel<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(runs::cancel(dir, ask))
    }

    fn delete_run<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(runs::delete(dir, ask))
    }

    fn delete_run_logs<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(runs::delete_logs(dir, ask))
    }

    fn job_log<'a>(&'a self, dir: &'a Path, ask: JobRef) -> PluginFuture<'a, JobLog> {
        work(logs::job(dir, ask))
    }

    fn job_log_excerpt<'a>(
        &'a self,
        dir: &'a Path,
        ask: ExcerptQuery,
    ) -> PluginFuture<'a, LogExcerpt> {
        work(logs::excerpt(dir, ask))
    }

    fn annotations<'a>(
        &'a self,
        dir: &'a Path,
        ask: CheckRunRef,
    ) -> PluginFuture<'a, Vec<Annotation>> {
        work(annotations::list(dir, ask))
    }

    fn job_summary<'a>(
        &'a self,
        dir: &'a Path,
        ask: CheckRunRef,
    ) -> PluginFuture<'a, Option<JobSummary>> {
        work(annotations::summary(dir, ask))
    }

    fn checks<'a>(&'a self, dir: &'a Path, ask: ChecksQuery) -> PluginFuture<'a, Vec<Check>> {
        work(checks::list(dir, ask))
    }

    fn artifacts<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, Vec<Artifact>> {
        work(artifacts::list(dir, ask))
    }

    fn pending_approvals<'a>(
        &'a self,
        dir: &'a Path,
        ask: RunRef,
    ) -> PluginFuture<'a, Vec<PendingApproval>> {
        work(approvals::pending(dir, ask))
    }

    fn review_deployment<'a>(
        &'a self,
        dir: &'a Path,
        ask: DeploymentReview,
    ) -> PluginFuture<'a, ()> {
        work(approvals::review(dir, ask))
    }

    fn pulls<'a>(&'a self, dir: &'a Path, ask: PullQuery) -> PluginFuture<'a, Vec<Pull>> {
        work(pulls::list(dir, ask))
    }

    fn pull_counts<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, PullCounts> {
        work(pulls::counts(dir, ask))
    }

    fn issue_counts<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, PullCounts> {
        work(pulls::issue_counts(dir, ask))
    }

    fn pull<'a>(&'a self, dir: &'a Path, ask: PullRef) -> PluginFuture<'a, Pull> {
        work(pulls::get(dir, ask))
    }

    fn pull_files<'a>(
        &'a self,
        dir: &'a Path,
        ask: FilesQuery,
    ) -> PluginFuture<'a, Vec<ChangedFile>> {
        work(pulls::files(dir, ask))
    }

    fn pull_commits<'a>(
        &'a self,
        dir: &'a Path,
        ask: PullRef,
    ) -> PluginFuture<'a, Vec<PullCommit>> {
        work(pulls::commits(dir, ask))
    }

    fn pull_reviews<'a>(&'a self, dir: &'a Path, ask: PullRef) -> PluginFuture<'a, Vec<Review>> {
        work(pulls::reviews(dir, ask))
    }

    fn commit_pulls<'a>(&'a self, dir: &'a Path, ask: CommitRef) -> PluginFuture<'a, Vec<Pull>> {
        work(pulls::of_commit(dir, ask))
    }

    fn commit_authors<'a>(
        &'a self,
        dir: &'a Path,
        ask: CommitsQuery,
    ) -> PluginFuture<'a, Vec<CommitAuthor>> {
        work(pulls::commit_authors(dir, ask))
    }

    fn merge_pull<'a>(&'a self, dir: &'a Path, ask: Merge) -> PluginFuture<'a, ()> {
        work(pulls::merge(dir, ask))
    }

    fn create_pull<'a>(&'a self, dir: &'a Path, ask: NewPull) -> PluginFuture<'a, Pull> {
        work(pulls::create(dir, ask))
    }

    fn set_pull_state<'a>(&'a self, dir: &'a Path, ask: SetPullState) -> PluginFuture<'a, ()> {
        work(pulls::set_state(dir, ask))
    }

    fn review_pull<'a>(&'a self, dir: &'a Path, ask: NewReview) -> PluginFuture<'a, ()> {
        work(pulls::review(dir, ask))
    }

    fn review_feedback<'a>(
        &'a self,
        dir: &'a Path,
        ask: FeedbackQuery,
    ) -> PluginFuture<'a, Vec<ReviewFeedback>> {
        work(review::feedback(dir, ask))
    }

    fn answer_review<'a>(&'a self, dir: &'a Path, ask: Answers) -> PluginFuture<'a, ()> {
        work(review::answer(dir, ask))
    }

    fn issues<'a>(&'a self, dir: &'a Path, ask: IssueQuery) -> PluginFuture<'a, IssuePage> {
        work(issues::list(dir, ask))
    }

    fn issue<'a>(&'a self, dir: &'a Path, ask: IssueRef) -> PluginFuture<'a, Issue> {
        work(issues::get(dir, ask))
    }

    fn create_issue<'a>(&'a self, dir: &'a Path, ask: NewIssue) -> PluginFuture<'a, Issue> {
        work(issues::create(dir, ask))
    }

    fn set_issue_state<'a>(&'a self, dir: &'a Path, ask: SetIssueState) -> PluginFuture<'a, ()> {
        work(issues::set_state(dir, ask))
    }

    fn comments<'a>(&'a self, dir: &'a Path, ask: ThreadRef) -> PluginFuture<'a, Vec<Comment>> {
        work(common::comments(dir, ask))
    }

    fn timeline<'a>(
        &'a self,
        dir: &'a Path,
        ask: ThreadRef,
    ) -> PluginFuture<'a, Vec<TimelineItem>> {
        work(common::timeline(dir, ask))
    }

    fn add_comment<'a>(&'a self, dir: &'a Path, ask: NewComment) -> PluginFuture<'a, ()> {
        work(common::add_comment(dir, ask))
    }

    fn releases<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<Release>> {
        work(releases::list(dir, ask))
    }

    fn inbox<'a>(&'a self, dir: &'a Path, ask: InboxQuery) -> PluginFuture<'a, Vec<Notification>> {
        work(inbox::list(dir, ask))
    }

    fn mark_read<'a>(&'a self, dir: &'a Path, ask: NotificationRef) -> PluginFuture<'a, ()> {
        work(inbox::mark_read(dir, ask))
    }

    fn mark_all_read<'a>(&'a self, dir: &'a Path, _ask: IgnoredAny) -> PluginFuture<'a, ()> {
        work(inbox::mark_all_read(dir))
    }
}

async fn dispatch_stream(
    ctx: &PluginContext,
    method: &str,
    input: Value,
    sink: StreamSink,
) -> Result<(), PluginError> {
    let data_dir = ctx.data_dir();
    match method {
        "downloadArtifact" => artifacts::download(data_dir, params(input)?, &sink).await,
        "downloadAsset" => releases::download(data_dir, params(input)?, &sink).await,
        _ => Err(PluginError::unknown_method(method)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn its_manifest_parses() {
        assert_eq!(
            plugin().expect("manifest parses").manifest().id,
            "sikemux.github"
        );
    }

    #[tokio::test]
    async fn reading_a_remote_never_needs_the_network() {
        let plugin = plugin().expect("plugin loads");
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
        let plugin = plugin().expect("plugin loads");
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

    /// `GHA_LIVE_ARTIFACT=owner/repo:artifact_id GH_TOKEN=… cargo test -p sikemux-plugin-github -- --ignored`
    /// with an artifact over 16 MiB, which used to fail. The token is read from
    /// the environment, so the Keychain is never touched.
    #[tokio::test]
    #[ignore]
    async fn downloads_an_artifact_larger_than_an_api_answer() {
        let (Some(target), Some(_)) = (env("GHA_LIVE_ARTIFACT"), env("GH_TOKEN")) else {
            return;
        };
        let (slug, id) = target
            .split_once(':')
            .expect("GHA_LIVE_ARTIFACT is owner/repo:id");
        let (owner, name) = slug.split_once('/').expect("owner/repo");
        let dir = std::env::temp_dir().join(format!("sikemux-gha-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::env::set_var("HOME", &dir);
        let ctx = PluginContext::new(dir.clone());
        let plugin = plugin().expect("plugin loads");
        plugin
            .call(&ctx, "signIn", json!({ "host": "github.com" }))
            .await
            .expect("sign in with the environment's token");
        let ticks = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = Arc::clone(&ticks);
        let sink = StreamSink::new(move |tick| kept.lock().map(|mut all| all.push(tick)).is_ok());
        plugin
            .stream(
                &ctx,
                "downloadArtifact",
                json!({ "owner": owner, "name": name, "artifactId": id.parse::<u64>().expect("id"), "fileName": "live" }),
                sink,
            )
            .await
            .expect("download");
        let last = ticks
            .lock()
            .expect("ticks")
            .last()
            .cloned()
            .expect("a tick");
        let saved = &last["saved"];
        let bytes = saved["bytes"].as_u64().expect("bytes");
        assert!(bytes > 16 * 1024 * 1024, "{bytes}");
        let path = saved["path"].as_str().expect("path");
        assert_eq!(std::fs::metadata(path).expect("saved").len(), bytes);
        std::fs::remove_dir_all(&dir).ok();
    }
}
