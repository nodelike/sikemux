// GitLab, on gitlab.com or a company's own server: the merge requests,
// pipelines, issues and releases of whichever project Sikemux has open,
// answered in the shapes the Git pane reads from every host.
//
//   config    — the accounts signed in, each on its server, and the Keychain entry behind it
//   client    — the HTTP client, the token, GitLab's paging and error shapes
//   auth      — signing in and out, and who the app is talking to GitLab as
//   repo      — a git remote turned into group and project
//   pulls     — merge requests, their changes, commits and approvals, and merging one
//   notes     — the comments and events on a merge request or issue
//   issues    — issues, releases, and the To-Do list as the inbox
//   images    — avatars, handed to the window as data: addresses
//   pipelines — pipelines as runs, their jobs by stage, logs, and starting one
//   watch     — following a pipeline while it is going
//   ratelimit — holding requests back once GitLab refuses for too many

mod auth;
mod client;
mod config;
mod error;
mod images;
mod issues;
mod notes;
mod pipelines;
mod pulls;
mod ratelimit;
mod repo;
mod watch;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
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

fn answer<'a, I, T, F>(
    input: Value,
    work: impl FnOnce(I) -> F + Send + 'a,
) -> PluginFuture<'a, Value>
where
    I: serde::de::DeserializeOwned + Send + 'a,
    T: Serialize,
    F: std::future::Future<Output = GitlabResult<T>> + Send + 'a,
{
    Box::pin(async move { reply(work(params(input)?).await?) })
}

#[derive(Deserialize)]
struct RemoteQuery {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Resolved {
    repo: Option<repo::Repo>,
    slug: Option<String>,
    same_host: bool,
}

/// A remote is GitLab's when it is on gitlab.com or on a server an account is signed in to.
fn resolve(data_dir: &std::path::Path, query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    let hosts = config::load(data_dir).hosts();
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| {
            hosts
                .iter()
                .any(|host| config::same_server(host, &repo.host))
        }),
        slug: found.as_ref().map(repo::Repo::slug),
        repo: found,
    }
}

#[derive(Deserialize)]
struct MineQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    50
}

/// The status of the account that just signed in.
async fn signed_in(
    data_dir: &std::path::Path,
    outcome: GitlabResult<String>,
) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(client::as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether the repository has a remote on a GitLab server an account is signed in to.
fn works_in(data_dir: &std::path::Path, remotes: &[String]) -> bool {
    let config = config::load(data_dir);
    remotes
        .iter()
        .filter_map(|remote| repo::from_remote(remote))
        .any(|repo| config.account_on(&repo.host).is_some())
}

/// Which account a call is for: the one it names, or else the first on the
/// server it names, or with neither, the default one.
fn account_of(data_dir: &std::path::Path, input: &Value) -> Option<String> {
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
        Box::pin(client::as_account(account, dispatch(ctx, method, input)))
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        let account = account_of(ctx.data_dir(), &input);
        Box::pin(client::as_account(
            account,
            dispatch_stream(ctx, method, input, sink),
        ))
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(works_in(ctx.data_dir(), remotes)) })
    }
}

fn dispatch<'a>(ctx: &'a PluginContext, method: &'a str, input: Value) -> PluginFuture<'a, Value> {
    let data_dir = ctx.data_dir();
    match method {
        "status" => Box::pin(async move { reply(auth::status(data_dir).await) }),
        "accounts" => Box::pin(async move { reply(auth::accounts(data_dir)) }),
        "setDefaultAccount" => answer(input, move |q| auth::set_default(data_dir, q)),
        "accountFor" => answer(input, move |q| auth::account_for(data_dir, q)),
        "rateLimit" => Box::pin(async move {
            let id = config::load(data_dir)
                .account(client::chosen().as_deref())
                .map(|account| account.id.clone())
                .unwrap_or_default();
            reply(ratelimit::budget(&id))
        }),
        "signInWithToken" => Box::pin(async move {
            signed_in(
                data_dir,
                auth::sign_in_with_token(data_dir, params(input)?).await,
            )
            .await
        }),
        "signOut" => Box::pin(async move { reply(auth::sign_out(data_dir).await?) }),

        "resolveRemote" => Box::pin(async move { reply(resolve(data_dir, params(input)?)) }),
        "myRepos" => answer(input, move |query: MineQuery| {
            repo::mine(data_dir, query.limit)
        }),
        "branches" => answer(input, move |q| repo::branches(data_dir, q)),
        "image" => answer(input, move |q| images::image(data_dir, q)),

        "workflows" => answer(input, move |q| pipelines::workflows(data_dir, q)),
        "workflowFile" => answer(input, move |q| pipelines::workflow_file(data_dir, q)),
        "dispatch" => answer(input, move |q| pipelines::dispatch(data_dir, q)),
        "runs" => answer(input, move |q| pipelines::list(data_dir, q)),
        "run" => answer(input, move |q| pipelines::detail(data_dir, q)),
        "runTiming" => answer(input, move |q| pipelines::timing(data_dir, q)),
        "rerun" => answer(input, move |q| pipelines::rerun(data_dir, q)),
        "rerunJob" => answer(input, move |q| pipelines::rerun_job(data_dir, q)),
        "cancel" => answer(input, move |q| pipelines::cancel(data_dir, q)),
        "deleteRun" => answer(input, move |q| pipelines::delete(data_dir, q)),
        "jobLog" => answer(input, move |q| pipelines::log(data_dir, q)),
        "jobLogExcerpt" => answer(input, move |q| pipelines::excerpt(data_dir, q)),

        "pulls" => answer(input, move |q| pulls::list(data_dir, q)),
        "pull" => answer(input, move |q| pulls::get(data_dir, q)),
        "pullFiles" => answer(input, move |q| pulls::files(data_dir, q)),
        "pullCommits" => answer(input, move |q| pulls::commits(data_dir, q)),
        "pullReviews" => answer(input, move |q| pulls::reviews(data_dir, q)),
        "mergePull" => answer(input, move |q| pulls::merge(data_dir, q)),
        "createPull" => answer(input, move |q| pulls::create(data_dir, q)),
        "setPullState" => answer(input, move |q| pulls::set_state(data_dir, q)),
        "reviewPull" => answer(input, move |q| pulls::review(data_dir, q)),
        "timeline" => answer(input, move |q| notes::timeline(data_dir, q)),
        "comments" => answer(input, move |q| notes::comments(data_dir, q)),
        "addComment" => answer(input, move |q| notes::add_comment(data_dir, q)),

        "issues" => answer(input, move |q| issues::list(data_dir, q)),
        "issue" => answer(input, move |q| issues::get(data_dir, q)),
        "createIssue" => answer(input, move |q| issues::create(data_dir, q)),
        "setIssueState" => answer(input, move |q| issues::set_state(data_dir, q)),
        "releases" => answer(input, move |q| issues::releases(data_dir, q)),
        "inbox" => answer(input, move |q| issues::inbox(data_dir, q)),
        "markRead" => answer(input, move |q| issues::mark_read(data_dir, q)),
        "markAllRead" => Box::pin(async move { reply(issues::mark_all_read(data_dir).await?) }),

        _ => Box::pin(async move { Err(PluginError::unknown_method(method)) }),
    }
}

fn dispatch_stream<'a>(
    ctx: &'a PluginContext,
    method: &'a str,
    input: Value,
    sink: StreamSink,
) -> PluginFuture<'a, ()> {
    let data_dir = ctx.data_dir();
    match method {
        "watchRun" => Box::pin(async move { watch::run(data_dir, params(input)?, sink).await }),
        _ => Box::pin(async move { Err(PluginError::unknown_method(method)) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sikemux-gl-{name}-{}", std::process::id()))
    }

    fn signed_in_to(dir: &std::path::Path, host: &str) {
        let mut config = config::GitlabConfig::default();
        config.upsert(config::Account {
            id: config::Account::id_for(host, 1),
            host: host.into(),
            login: "someone".into(),
            display_name: None,
            avatar_url: None,
        });
        config::save(dir, &config).expect("saves");
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
        config::save(&dir, &config).expect("saves");
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
        assert_eq!(error.category, "unconfigured");
        std::fs::remove_dir_all(dir).ok();
    }
}
