// Bitbucket Cloud: the pull requests and pipelines of whichever repository
// Sikemux has open, answered in the shapes every code host answers in.

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

use serde_json::Value;
use sikemux_plugin_api::account::{as_account, named_in};
use sikemux_plugin_api::codehost::asks::{
    AccountQuery, Answers, CommitRef, CommitsQuery, Dispatch, ExcerptQuery, FeedbackQuery,
    FilesQuery, ImageRef, InboxQuery, IssueQuery, IssueRef, JobRef, Merge, MineQuery, NewComment,
    NewIssue, NewPull, NewReview, PullEdit, PullQuery, PullRef, RemoteQuery, RepoRef, Rerun,
    RunQuery, RunRef, SetIssueState, SetPullState, ThreadOf, ThreadRef, WorkflowRef,
};
use sikemux_plugin_api::codehost::{
    self, work, ChangedFile, CodeHost, Comment, CommitAuthor, Issue, IssuePage, JobLog, LogExcerpt,
    Notification, Pull, PullCommit, Release, RepoListing, Resolved, Review, ReviewFeedback,
    RunDetail, RunPage, RunTiming, TimelineItem, Workflow, WorkflowFile,
};
use sikemux_plugin_api::host_work;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{
    answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::BitbucketResult;

sikemux_plugin_api::plugin!(Bitbucket);

fn resolve(query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| repo.host == repo::HOST),
        slug: found.as_ref().map(repo::slug),
        repo: found,
    }
}

async fn signed_in(
    data_dir: &Path,
    outcome: BitbucketResult<String>,
) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether an account is signed in. Its tools name the repository they act on, so they work
/// from any folder, and a person who connected Bitbucket expects their agents to have them.
fn has_account(data_dir: &Path) -> bool {
    store::has_account::<config::BitbucketConfig>(data_dir)
}

impl Plugin for Bitbucket {
    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        let account = named_in(&input);
        Box::pin(as_account(account, dispatch(self, ctx, method, input)))
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        let account = named_in(&input);
        Box::pin(as_account(account, dispatch_stream(ctx, method, sink)))
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(has_account(ctx.data_dir())) })
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
        "setDefaultAccount" => {
            answer(store::set_default::<
                config::BitbucketConfig,
                error::Bitbucket,
            >(data_dir, params(input)?))
            .await
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

    host_work! {
        inbox(dir, _ask: InboxQuery) -> Vec<Notification> = issues::inbox(dir);
        account_for(dir, ask: AccountQuery) -> Option<String> = auth::account_for(dir, ask.repo);
        my_repos(dir, ask: MineQuery) -> Vec<RepoListing> = repo::mine(dir, ask.limit);
        branches(dir, ask: RepoRef) -> Vec<String> = repo::branches(dir, ask);
        image(dir, ask: ImageRef) -> String = images::image(dir, ask);
        workflows(dir, ask: RepoRef) -> Vec<Workflow> = pipelines::workflows(dir, ask);
        workflow_file(dir, ask: WorkflowRef) -> WorkflowFile = pipelines::workflow_file(dir, ask);
        dispatch_workflow(dir, ask: Dispatch) -> () = pipelines::dispatch(dir, ask);
        runs(dir, ask: RunQuery) -> RunPage = pipelines::list(dir, ask);
        run(dir, ask: RunRef) -> RunDetail = pipelines::detail(dir, ask);
        run_timing(dir, ask: RunRef) -> RunTiming = pipelines::timing(dir, ask);
        rerun(dir, ask: Rerun) -> () = pipelines::rerun(dir, ask);
        cancel(dir, ask: RunRef) -> () = pipelines::cancel(dir, ask);
        job_log(dir, ask: JobRef) -> JobLog = pipelines::log(dir, ask);
        job_log_excerpt(dir, ask: ExcerptQuery) -> LogExcerpt = pipelines::excerpt(dir, ask);
        pulls(dir, ask: PullQuery) -> Vec<Pull> = pulls::list(dir, ask);
        pull_files(dir, ask: FilesQuery) -> Vec<ChangedFile> = pulls::files(dir, ask);
        pull_commits(dir, ask: PullRef) -> Vec<PullCommit> = pulls::commits(dir, ask);
        pull_reviews(dir, ask: PullRef) -> Vec<Review> = pulls::reviews(dir, ask);
        commit_pulls(dir, ask: CommitRef) -> Vec<Pull> = pulls::of_commit(dir, ask);
        commit_authors(dir, ask: CommitsQuery) -> Vec<CommitAuthor> = pulls::commit_authors(dir, ask);
        merge_pull(dir, ask: Merge) -> () = pulls::merge(dir, ask);
        create_pull(dir, ask: NewPull) -> Pull = pulls::create(dir, ask);
        edit_pull(dir, ask: PullEdit) -> Pull = pulls::edit(dir, ask);
        set_pull_state(dir, ask: SetPullState) -> () = pulls::set_state(dir, ask);
        review_pull(dir, ask: NewReview) -> () = pulls::review(dir, ask);
        review_feedback(dir, ask: FeedbackQuery) -> Vec<ReviewFeedback> = review::feedback(dir, ask);
        answer_review(dir, ask: Answers) -> () = review::answer(dir, ask);
        issues(dir, ask: IssueQuery) -> IssuePage = issues::list(dir, ask);
        issue(dir, ask: IssueRef) -> Issue = issues::get(dir, ask);
        create_issue(dir, ask: NewIssue) -> Issue = issues::create(dir, ask);
        set_issue_state(dir, ask: SetIssueState) -> () = issues::set_state(dir, ask);
        releases(dir, ask: RepoRef) -> Vec<Release> = issues::releases(dir, ask);
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
    use sikemux_plugin_api::store::Accounts;

    #[tokio::test]
    async fn reading_a_remote_never_needs_the_network() {
        let plugin = plugin();
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
    fn an_agent_is_offered_bitbucket_whenever_signed_in() {
        let dir = std::env::temp_dir().join(format!("sikemux-bb-offer-{}", std::process::id()));
        assert!(!has_account(&dir));

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
        assert!(has_account(&dir));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out() {
        let plugin = plugin();
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
