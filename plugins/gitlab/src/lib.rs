// GitLab, on gitlab.com or a company's own server: the merge requests,
// pipelines, issues and releases of whichever project Sikemux has open,
// answered in the shapes the Git pane reads from every host.
//
//   config    — the accounts signed in, each on its server, and the Keychain entry behind it
//   client    — the HTTP client, the token, GitLab's paging and error shapes
//   auth      — signing in and out, and who the app is talking to GitLab as
//   repo      — a git remote turned into group and project
//   pulls     — merge requests, their changes, commits and approvals, and merging one
//   review    — what reviewers asked of a merge request, and answering it
//   notes     — the comments and events on a merge request or issue
//   issues    — issues, releases, and the To-Do list as the inbox
//   images    — avatars, handed to the window as data: addresses
//   pipelines — pipelines as runs, their jobs by stage, logs, and starting one
//   watch     — following a pipeline while it is going

mod auth;
mod client;
mod config;
mod error;
mod images;
mod issues;
mod notes;
mod pipelines;
mod pulls;
mod repo;
mod review;

use std::path::Path;
use std::sync::Arc;

use serde::de::IgnoredAny;
use serde_json::Value;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::asks::*;
use sikemux_plugin_api::codehost::{self, work, CodeHost};
use sikemux_plugin_api::codehost::{
    ChangedFile, Comment, Issue, IssuePage, JobLog, LogExcerpt, Notification, Pull, PullCommit,
    Release, RepoListing, Resolved, Review, ReviewFeedback, RunDetail, RunPage, RunTiming,
    TimelineItem, Workflow, WorkflowFile,
};
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
};

use crate::error::GitlabResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Gitlab {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Gitlab {
    manifest: Manifest,
}

/// A remote is GitLab's when it is on gitlab.com or on a server an account is signed in to.
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

/// The status of the account that just signed in.
async fn signed_in(data_dir: &Path, outcome: GitlabResult<String>) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether the repository has a remote on a GitLab server an account is signed in to.
fn works_in(data_dir: &Path, remotes: &[String]) -> bool {
    let config = config::load(data_dir).unwrap_or_default();
    remotes
        .iter()
        .filter_map(|remote| repo::from_remote(remote))
        .any(|repo| config.account_on(&repo.host).is_some())
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

impl Plugin for Gitlab {
    fn manifest(&self) -> &Manifest {
        &self.manifest
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
        remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(works_in(ctx.data_dir(), remotes)) })
    }
}

async fn dispatch(
    host: &Gitlab,
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
        _ => codehost::dispatch(host, data_dir, method, input).await,
    }
}

impl CodeHost for Gitlab {
    fn resolve_remote<'a>(&'a self, dir: &'a Path, ask: RemoteQuery) -> PluginFuture<'a, Resolved> {
        Box::pin(async move { Ok(resolve(dir, ask)) })
    }

    fn account_for<'a>(
        &'a self,
        dir: &'a Path,
        ask: AccountQuery,
    ) -> PluginFuture<'a, Option<String>> {
        work(auth::account_for(dir, ask))
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

    fn rerun_job<'a>(&'a self, dir: &'a Path, ask: RerunJob) -> PluginFuture<'a, ()> {
        work(pipelines::rerun_job(dir, ask))
    }

    fn cancel<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(pipelines::cancel(dir, ask))
    }

    fn delete_run<'a>(&'a self, dir: &'a Path, ask: RunRef) -> PluginFuture<'a, ()> {
        work(pipelines::delete(dir, ask))
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

    fn pull<'a>(&'a self, dir: &'a Path, ask: PullRef) -> PluginFuture<'a, Pull> {
        work(pulls::get(dir, ask))
    }

    fn commit_pulls<'a>(&'a self, dir: &'a Path, ask: CommitRef) -> PluginFuture<'a, Vec<Pull>> {
        work(pulls::of_commit(dir, ask))
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

    fn timeline<'a>(
        &'a self,
        dir: &'a Path,
        ask: ThreadRef,
    ) -> PluginFuture<'a, Vec<TimelineItem>> {
        work(notes::timeline(dir, ask))
    }

    fn comments<'a>(&'a self, dir: &'a Path, ask: ThreadRef) -> PluginFuture<'a, Vec<Comment>> {
        work(notes::comments(dir, ask))
    }

    fn add_comment<'a>(&'a self, dir: &'a Path, ask: NewComment) -> PluginFuture<'a, ()> {
        work(notes::add_comment(dir, ask))
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

    fn releases<'a>(&'a self, dir: &'a Path, ask: RepoRef) -> PluginFuture<'a, Vec<Release>> {
        work(issues::releases(dir, ask))
    }

    fn inbox<'a>(&'a self, dir: &'a Path, ask: InboxQuery) -> PluginFuture<'a, Vec<Notification>> {
        work(issues::inbox(dir, ask))
    }

    fn mark_read<'a>(&'a self, dir: &'a Path, ask: NotificationRef) -> PluginFuture<'a, ()> {
        work(issues::mark_read(dir, ask))
    }

    fn mark_all_read<'a>(&'a self, dir: &'a Path, _ask: IgnoredAny) -> PluginFuture<'a, ()> {
        work(issues::mark_all_read(dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sikemux-gl-{name}-{}", std::process::id()))
    }

    fn signed_in_to(dir: &Path, host: &str) {
        let mut config = config::GitlabConfig::default();
        config.upsert(config::Account {
            id: config::Account::id_for(host, 1),
            host: host.into(),
            login: "someone".into(),
            display_name: None,
            avatar_url: None,
        });
        sikemux_plugin_api::store::save(dir, &config).expect("saves");
    }

    #[test]
    fn its_manifest_parses() {
        assert_eq!(
            plugin().expect("manifest parses").manifest().id,
            "sikemux.gitlab"
        );
    }

    #[tokio::test]
    async fn gitlab_com_is_claimed_and_a_company_server_once_signed_in_to() {
        let plugin = plugin().expect("plugin loads");
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
        let com = claimed("git@gitlab.com:swishx/api-docs.git").await;
        assert_eq!(com["slug"], "swishx/api-docs");
        assert_eq!(com["sameHost"], true);
        assert_eq!(
            claimed("git@gitlab.acme.dev:platform/api.git").await["sameHost"],
            false
        );
        assert_eq!(
            claimed("git@github.com:nodelike/sikemux.git").await["sameHost"],
            false
        );

        signed_in_to(&dir, "gitlab.acme.dev");
        assert_eq!(
            claimed("git@gitlab.acme.dev:platform/api.git").await["sameHost"],
            true
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_agent_is_offered_gitlab_only_when_signed_in_to_the_remotes_server() {
        let dir = temp("offer");
        let remote = ["https://gitlab.acme.dev/platform/api.git".to_string()];
        assert!(!works_in(&dir, &remote));
        signed_in_to(&dir, "gitlab.acme.dev");
        assert!(works_in(&dir, &remote));
        assert!(!works_in(&dir, &["git@gitlab.com:x/y.git".to_string()]));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_server_on_its_own_port_matches_its_remote() {
        let dir = temp("port");
        signed_in_to(&dir, "git.acme.dev:8443");
        let remote = ["ssh://git@git.acme.dev:2222/platform/api.git".to_string()];
        assert!(works_in(&dir, &remote));
        let resolved = resolve(
            &dir,
            RemoteQuery {
                url: remote[0].clone(),
            },
        );
        assert!(resolved.same_host);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_call_naming_no_account_goes_to_the_first_on_its_server() {
        let dir = temp("account-of");
        let mut config = config::GitlabConfig::default();
        for (host, id) in [("gitlab.com", 1), ("git.acme.dev:8443", 2)] {
            config.upsert(config::Account {
                id: config::Account::id_for(host, id),
                host: host.into(),
                login: "someone".into(),
                display_name: None,
                avatar_url: None,
            });
        }
        sikemux_plugin_api::store::save(&dir, &config).expect("saves");
        assert_eq!(
            account_of(&dir, &json!({ "host": "git.acme.dev" })).as_deref(),
            Some("git.acme.dev:8443#2")
        );
        assert_eq!(
            account_of(
                &dir,
                &json!({ "host": "git.acme.dev", "account": "gitlab.com#1" })
            )
            .as_deref(),
            Some("gitlab.com#1")
        );
        assert_eq!(
            account_of(&dir, &json!({ "host": "gitlab.other.dev" })),
            None
        );
        assert_eq!(account_of(&dir, &json!({})), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out() {
        let plugin = plugin().expect("plugin loads");
        let dir = temp("status");
        let ctx = PluginContext::new(dir.clone());
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(status["configured"], false);
        assert_eq!(status["ok"], false);
        let error = plugin
            .call(&ctx, "branches", json!({ "owner": "a", "name": "b" }))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, sikemux_plugin_api::Category::Unconfigured);
        std::fs::remove_dir_all(dir).ok();
    }
}
