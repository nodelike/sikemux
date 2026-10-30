// Bitbucket Cloud: the pull requests and pipelines of whichever repository
// Sikemux has open, answered in the shapes the Git pane reads from every host.
//
//   config    — which way this is signed in, and the Keychain entry behind it
//   oauth     — signing in through the browser, and keeping the token fresh
//   client    — the HTTP client, the credential, and Bitbucket's error shapes
//   auth      — signing in and out, and who the app is talking to Bitbucket as
//   repo      — a git remote turned into workspace and repository
//   pulls     — pull requests, their files, commits and history, and merging one
//   review    — what reviewers asked of a pull request, and answering it
//   issues    — the issue tracker, tags as releases, and the inbox of pull requests waiting
//   pipelines — pipelines as runs, their steps as jobs, and starting one
//   watch     — following a pipeline while it is going
//   images    — avatars, handed to the window as data: addresses

mod auth;
mod client;
mod config;
mod error;
mod images;
mod issues;
mod oauth;
mod pipelines;
mod pulls;
mod repo;
mod review;

use std::path::Path;
use std::sync::Arc;

use serde::de::IgnoredAny;
use serde_json::Value;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::asks::{
    AccountQuery, Answers, CommitRef, CommitsQuery, Dispatch, ExcerptQuery, FeedbackQuery,
    FilesQuery, ImageRef, InboxQuery, IssueQuery, IssueRef, JobRef, Merge, MineQuery, NewComment,
    NewIssue, NewPull, NewReview, NotificationRef, PullEdit, PullQuery, PullRef, RemoteQuery,
    RepoRef, Rerun, RunQuery, RunRef, SetIssueState, SetPullState, ThreadOf, ThreadRef,
    WorkflowRef,
};
use sikemux_plugin_api::codehost::{
    self, work, ChangedFile, CodeHost, Comment, CommitAuthor, Issue, IssuePage, JobLog, LogExcerpt,
    Notification, Pull, PullCommit, Release, RepoListing, Resolved, Review, ReviewFeedback,
    RunDetail, RunPage, RunTiming, TimelineItem, Workflow, WorkflowFile,
};
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::BitbucketResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Bitbucket {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Bitbucket {
    manifest: Manifest,
}

fn resolve(query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| repo.host == repo::HOST),
        slug: found.as_ref().map(repo::slug),
        repo: found,
    }
}

/// The status of the account that just signed in.
async fn signed_in(
    data_dir: &Path,
    outcome: BitbucketResult<String>,
) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether the repository has a remote on Bitbucket, and an account is signed in.
fn works_in(data_dir: &Path, remotes: &[String]) -> bool {
    let on_bitbucket = remotes
        .iter()
        .filter_map(|remote| repo::from_remote(remote))
        .any(|repo| repo.host == repo::HOST);
    on_bitbucket && config::load(data_dir).is_ok_and(|config| !config.accounts.is_empty())
}

/// Which account a call is for; with none named, the default one.
fn account_of(input: &Value) -> Option<String> {
    input
        .get("account")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
}

impl Plugin for Bitbucket {
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
        Box::pin(as_account(account, dispatch(self, ctx, method, input)))
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        let account = account_of(&input);
        Box::pin(as_account(account, dispatch_stream(ctx, method, sink)))
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(works_in(ctx.data_dir(), remotes)) })
    }
}

async fn dispatch(
    host: &Bitbucket,
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
        "signInWithToken" => {
            signed_in(
                data_dir,
                auth::sign_in_with_token(data_dir, params(input)?).await,
            )
            .await
        }
        "signOut" => answer(auth::sign_out(data_dir)).await,
        // Agents read a pull request's builds beside it, which the shared answer has no room for.
        "pull" => answer(pulls::get(data_dir, params(input)?)).await,
        _ => codehost::dispatch(host, data_dir, method, input).await,
    }
}

impl CodeHost for Bitbucket {
    fn resolve_remote<'a>(
        &'a self,
        _dir: &'a Path,
        ask: RemoteQuery,
    ) -> PluginFuture<'a, Resolved> {
        Box::pin(async move { Ok(resolve(ask)) })
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
        work(repo::branches(dir, ask))
    }

    fn image<'a>(&'a self, dir: &'a Path, ask: ImageRef) -> PluginFuture<'a, String> {
        work(images::image(dir, ask))
    }

    fn workflows<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<Workflow>> {
        work(pipelines::workflows(dir, ask))
    }

    fn workflow_file<'a>(
        &'a self,
        dir: &'a Path,
        ask: WorkflowRef,
    ) -> PluginFuture<'a, WorkflowFile> {
        work(pipelines::workflow_file(dir, ask))
    }

    fn dispatch_workflow<'a>(&'a self, dir: &'a Path, ask: Dispatch) -> PluginFuture<'a, ()> {
        work(pipelines::dispatch(dir, ask))
    }

    fn runs<'a>(&'a self, dir: &'a Path, ask: RunQuery) -> PluginFuture<'a, RunPage> {
        work(pipelines::list(dir, ask))
    }

    fn run<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, RunDetail> {
        work(pipelines::detail(dir, ask))
    }

    fn run_timing<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, RunTiming> {
        work(pipelines::timing(dir, ask))
    }

    fn rerun<'a>(&'a self, dir: &'a Path, ask: Rerun) -> PluginFuture<'a, ()> {
        work(pipelines::rerun(dir, ask))
    }

    fn cancel<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(pipelines::cancel(dir, ask))
    }

    fn job_log<'a>(&'a self, dir: &'a Path, ask: JobRef) -> PluginFuture<'a, JobLog> {
        work(pipelines::log(dir, ask))
    }

    fn job_log_excerpt<'a>(
        &'a self,
        dir: &'a Path,
        ask: ExcerptQuery,
    ) -> PluginFuture<'a, LogExcerpt> {
        work(pipelines::excerpt(dir, ask))
    }

    fn pulls<'a>(&'a self, dir: &'a Path, ask: PullQuery) -> PluginFuture<'a, Vec<Pull>> {
        work(pulls::list(dir, ask))
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

    fn edit_pull<'a>(&'a self, dir: &'a Path, ask: PullEdit) -> PluginFuture<'a, Pull> {
        work(pulls::edit(dir, ask))
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

    /// Bitbucket numbers issues apart from pull requests, so a thread says which it is.
    fn comments<'a>(&'a self, dir: &'a Path, ask: ThreadRef) -> PluginFuture<'a, Vec<Comment>> {
        let ThreadRef { repo, number, of } = ask;
        match of {
            ThreadOf::Issue => work(issues::comments(dir, IssueRef { repo, number })),
            ThreadOf::Pull => work(pulls::comments(dir, PullRef { repo, number })),
        }
    }

    fn timeline<'a>(
        &'a self,
        dir: &'a Path,
        ask: ThreadRef,
    ) -> PluginFuture<'a, Vec<TimelineItem>> {
        let ThreadRef { repo, number, of } = ask;
        match of {
            ThreadOf::Issue => work(issues::timeline(dir, IssueRef { repo, number })),
            ThreadOf::Pull => work(pulls::timeline(dir, PullRef { repo, number })),
        }
    }

    fn add_comment<'a>(&'a self, dir: &'a Path, ask: NewComment) -> PluginFuture<'a, ()> {
        let NewComment {
            thread: ThreadRef { repo, number, of },
            body,
        } = ask;
        work(async move {
            match of {
                ThreadOf::Issue => {
                    issues::add_comment(dir, &IssueRef { repo, number }, &body).await
                }
                ThreadOf::Pull => pulls::add_comment(dir, &PullRef { repo, number }, &body).await,
            }
        })
    }

    fn releases<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<Release>> {
        work(issues::releases(dir, ask))
    }

    fn inbox<'a>(&'a self, dir: &'a Path, ask: InboxQuery) -> PluginFuture<'a, Vec<Notification>> {
        work(issues::inbox(dir, ask))
    }

    fn mark_read<'a>(&'a self, dir: &'a Path, ask: NotificationRef) -> PluginFuture<'a, ()> {
        work(async move { issues::mark_read(dir, ask) })
    }

    fn mark_all_read<'a>(&'a self, dir: &'a Path, _ask: IgnoredAny) -> PluginFuture<'a, ()> {
        work(issues::mark_all_read(dir))
    }
}

async fn dispatch_stream(
    ctx: &PluginContext,
    method: &str,
    sink: StreamSink,
) -> Result<(), PluginError> {
    let data_dir = ctx.data_dir();
    match method {
        "signInWithBrowser" => {
            let id = auth::sign_in_with_browser(data_dir, &sink).await?;
            sink.send(reply(as_account(Some(id), auth::status(data_dir)).await)?)
        }
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
            "sikemux.bitbucket"
        );
    }

    #[tokio::test]
    async fn reading_a_remote_never_needs_the_network() {
        let plugin = plugin().expect("plugin loads");
        let ctx = PluginContext::new(std::env::temp_dir().join("sikemux-bb-remote"));
        let resolved = plugin
            .call(
                &ctx,
                "resolveRemote",
                json!({ "url": "git@bitbucket.org:swishx/api-docs.git" }),
            )
            .await
            .expect("resolves");
        assert_eq!(resolved["slug"], "swishx/api-docs");
        assert_eq!(resolved["sameHost"], true);

        let elsewhere = plugin
            .call(
                &ctx,
                "resolveRemote",
                json!({ "url": "git@github.com:nodelike/sikemux.git" }),
            )
            .await
            .expect("resolves");
        assert_eq!(elsewhere["sameHost"], false);
    }

    #[test]
    fn an_agent_is_offered_bitbucket_only_when_signed_in_with_a_remote_there() {
        let dir = std::env::temp_dir().join(format!("sikemux-bb-offer-{}", std::process::id()));
        let remote = ["https://bitbucket.org/swishx/api-docs.git".to_string()];
        assert!(!works_in(&dir, &remote));

        let mut config = config::BitbucketConfig::default();
        config.upsert(config::Account {
            id: "abc".into(),
            login: "someone".into(),
            display_name: None,
            avatar_url: None,
            method: config::Method::Token,
            email: None,
        });
        sikemux_plugin_api::store::save(&dir, &config).expect("saves");
        assert!(works_in(&dir, &remote));
        assert!(!works_in(
            &dir,
            &["git@github.com:nodelike/sikemux.git".to_string()]
        ));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out() {
        let plugin = plugin().expect("plugin loads");
        let dir = std::env::temp_dir().join(format!("sikemux-bb-status-{}", std::process::id()));
        let ctx = PluginContext::new(dir.clone());
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(status["configured"], false);
        assert_eq!(status["ok"], false);
        let error = plugin
            .call(&ctx, "pulls", json!({ "owner": "a", "name": "b" }))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, sikemux_plugin_api::Category::Unconfigured);
        std::fs::remove_dir_all(dir).ok();
    }
}
