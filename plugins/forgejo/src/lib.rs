// Forgejo and Gitea, on Codeberg, gitea.com or a company's own server: the pull
// requests, Actions runs, issues and releases of whichever repository Sikemux
// has open, answered in the shapes every code host answers in. Forgejo began as
// a fork of Gitea and keeps its API, so one plugin serves both.

mod actions;
mod auth;
mod client;
mod config;
mod error;
mod images;
mod issues;
mod notes;
mod pulls;
mod repo;

use std::path::Path;

use serde_json::Value;
use sikemux_plugin_api::account::as_account;
use sikemux_plugin_api::codehost::asks::*;
use sikemux_plugin_api::codehost::{self, CodeHost};
use sikemux_plugin_api::codehost::{
    ChangedFile, Comment, Issue, IssuePage, JobLog, LogExcerpt, Notification, Pull, PullCommit,
    Release, RepoListing, Resolved, Review, RunDetail, RunPage, TimelineItem,
};
use sikemux_plugin_api::host_work;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

use crate::error::ForgejoResult;

sikemux_plugin_api::plugin!(Forgejo);

/// A remote is Forgejo's when it is on Codeberg, gitea.com or a server an account is signed in to.
fn resolve(data_dir: &Path, query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    let hosts = config::load(data_dir).unwrap_or_default().hosts();
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| {
            hosts
                .iter()
                .any(|host| config::same_server(host, &repo.host))
        }),
        slug: found.as_ref().map(repo::slug),
        repo: found,
    }
}

async fn signed_in(data_dir: &Path, outcome: ForgejoResult<String>) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether an account is signed in. Its tools name the repository they act on,
/// so they work from any folder.
fn has_account(data_dir: &Path) -> bool {
    config::load(data_dir).is_ok_and(|config| !config.accounts.is_empty())
}

/// Which account a call is for: the one it names, or else the first on the
/// server it names, or with neither, the default one.
fn account_of(data_dir: &Path, input: &Value) -> Option<String> {
    let text = |field: &str| {
        input
            .get(field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    if let Some(account) = text("account") {
        return Some(account.to_string());
    }
    config::load(data_dir)
        .ok()?
        .account_on(text("host")?)
        .map(|account| account.id)
}

impl Plugin for Forgejo {
    fn vault(&self) -> Option<&'static dyn sikemux_plugin_api::vault::Book> {
        Some(&config::BOOK)
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        let account = account_of(ctx.data_dir(), &input);
        Box::pin(as_account(account, dispatch(self, ctx, method, input)))
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
    host: &Forgejo,
    ctx: &PluginContext,
    method: &str,
    input: Value,
) -> Result<Value, PluginError> {
    let data_dir = ctx.data_dir();
    match method {
        "status" => reply(auth::status(data_dir).await),
        "setDefaultAccount" => {
            answer(store::set_default::<config::ForgejoConfig, error::Forgejo>(
                data_dir,
                params(input)?,
            ))
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
        _ => codehost::dispatch(host, data_dir, method, input).await,
    }
}

impl CodeHost for Forgejo {
    fn resolve_remote<'a>(&'a self, dir: &'a Path, ask: RemoteQuery) -> PluginFuture<'a, Resolved> {
        Box::pin(async move { Ok(resolve(dir, ask)) })
    }

    host_work! {
        inbox(dir, ask: InboxQuery) -> Vec<Notification> = issues::inbox(dir, ask);
        account_for(dir, ask: AccountQuery) -> Option<String> = auth::account_for(dir, ask);
        my_repos(dir, ask: MineQuery) -> Vec<RepoListing> = repo::mine(dir, ask.limit);
        branches(dir, ask: RepoRef) -> Vec<String> = repo::branches(dir, ask);
        image(dir, ask: ImageRef) -> String = images::image(dir, ask);
        runs(dir, ask: RunQuery) -> RunPage = actions::list(dir, ask);
        run(dir, ask: RunRef) -> RunDetail = actions::detail(dir, ask);
        rerun(dir, ask: Rerun) -> () = actions::rerun(dir, ask);
        rerun_job(dir, ask: RerunJob) -> () = actions::rerun_job(dir, ask);
        cancel(dir, ask: RunRef) -> () = actions::cancel(dir, ask);
        delete_run(dir, ask: RunRef) -> () = actions::delete(dir, ask);
        job_log(dir, ask: JobRef) -> JobLog = actions::log(dir, ask);
        job_log_excerpt(dir, ask: ExcerptQuery) -> LogExcerpt = actions::excerpt(dir, ask);
        pulls(dir, ask: PullQuery) -> Vec<Pull> = pulls::list(dir, ask);
        pull(dir, ask: PullRef) -> Pull = pulls::get(dir, ask);
        commit_pulls(dir, ask: CommitRef) -> Vec<Pull> = pulls::of_commit(dir, ask);
        pull_files(dir, ask: FilesQuery) -> Vec<ChangedFile> = pulls::files(dir, ask);
        pull_commits(dir, ask: PullRef) -> Vec<PullCommit> = pulls::commits(dir, ask);
        pull_reviews(dir, ask: PullRef) -> Vec<Review> = pulls::reviews(dir, ask);
        merge_pull(dir, ask: Merge) -> () = pulls::merge(dir, ask);
        create_pull(dir, ask: NewPull) -> Pull = pulls::create(dir, ask);
        edit_pull(dir, ask: PullEdit) -> Pull = pulls::edit(dir, ask);
        set_pull_state(dir, ask: SetPullState) -> () = pulls::set_state(dir, ask);
        review_pull(dir, ask: NewReview) -> () = pulls::review(dir, ask);
        timeline(dir, ask: ThreadRef) -> Vec<TimelineItem> = notes::timeline(dir, ask);
        comments(dir, ask: ThreadRef) -> Vec<Comment> = notes::comments(dir, ask);
        add_comment(dir, ask: NewComment) -> () = notes::add_comment(dir, ask);
        issues(dir, ask: IssueQuery) -> IssuePage = issues::list(dir, ask);
        issue(dir, ask: IssueRef) -> Issue = issues::get(dir, ask);
        create_issue(dir, ask: NewIssue) -> Issue = issues::create(dir, ask);
        set_issue_state(dir, ask: SetIssueState) -> () = issues::set_state(dir, ask);
        releases(dir, ask: RepoRef) -> Vec<Release> = issues::releases(dir, ask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sikemux_plugin_api::store::Accounts;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sikemux-fj-{name}-{}", std::process::id()))
    }

    #[tokio::test]
    async fn the_public_servers_are_claimed_and_a_company_one_once_signed_in_to() {
        let plugin = plugin();
        let dir = temp("remote");
        let ctx = PluginContext::new(dir.clone());
        let claimed = |url: &'static str| {
            let plugin = plugin.clone();
            let ctx = &ctx;
            async move {
                plugin
                    .call(ctx, "resolveRemote", json!({ "url": url }))
                    .await
                    .expect("resolves")
            }
        };
        let codeberg = claimed("git@codeberg.org:forgejo/runner.git").await;
        assert_eq!(codeberg["slug"], "forgejo/runner");
        assert_eq!(codeberg["sameHost"], true);
        assert_eq!(
            claimed("https://gitea.com/gitea/tea.git").await["sameHost"],
            true
        );
        assert_eq!(
            claimed("git@git.acme.dev:platform/api.git").await["sameHost"],
            false
        );
        let mut config = config::ForgejoConfig::default();
        config.upsert(config::Account {
            id: config::Account::id_for("git.acme.dev", 1),
            host: "git.acme.dev".into(),
            login: "someone".into(),
            display_name: None,
            avatar_url: None,
        });
        store::save(&dir, &config).expect("saves");
        assert_eq!(
            claimed("ssh://git@git.acme.dev:2222/platform/api.git").await["sameHost"],
            true
        );
        assert!(has_account(&dir));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out() {
        let plugin = plugin();
        let dir = temp("status");
        let ctx = PluginContext::new(dir.clone());
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(status["configured"], false);
        let error = plugin
            .call(&ctx, "branches", json!({ "owner": "a", "name": "b" }))
            .await
            .expect_err("signed out");
        assert_eq!(
            error.category,
            sikemux_plugin_api::FailureCategory::Unconfigured
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
