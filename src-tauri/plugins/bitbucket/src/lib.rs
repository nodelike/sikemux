// Bitbucket Cloud: the pull requests and pipelines of whichever repository
// Sikemux has open, answered in the shapes the Git pane reads from every host.
//
//   config    — which way this is signed in, and the Keychain entry behind it
//   oauth     — signing in through the browser, and keeping the token fresh
//   client    — the HTTP client, the credential, and Bitbucket's error shapes
//   auth      — signing in and out, and who the app is talking to Bitbucket as
//   repo      — a git remote turned into workspace and repository
//   pulls     — pull requests, their files, commits and history, and merging one
//   pipelines — pipelines as runs, their steps as jobs, and starting one
//   watch     — following a pipeline while it is going
//   ratelimit — holding requests back once Bitbucket refuses for too many
//   images    — avatars, handed to the window as data: addresses

mod auth;
mod client;
mod config;
mod error;
mod images;
mod oauth;
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

use crate::error::BitbucketResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Bitbucket {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Bitbucket {
    manifest: Manifest,
}

fn answer<'a, I, T, F>(
    input: Value,
    work: impl FnOnce(I) -> F + Send + 'a,
) -> PluginFuture<'a, Value>
where
    I: serde::de::DeserializeOwned + Send + 'a,
    T: Serialize,
    F: std::future::Future<Output = BitbucketResult<T>> + Send + 'a,
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

fn resolve(query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| repo.host == repo::HOST),
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
    outcome: BitbucketResult<String>,
) -> Result<Value, PluginError> {
    let id = outcome?;
    reply(client::as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether the repository has a remote on Bitbucket, and an account is signed in.
fn works_in(data_dir: &std::path::Path, remotes: &[String]) -> bool {
    let on_bitbucket = remotes
        .iter()
        .filter_map(|remote| repo::from_remote(remote))
        .any(|repo| repo.host == repo::HOST);
    on_bitbucket && !config::load(data_dir).accounts.is_empty()
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
        Box::pin(client::as_account(account, dispatch(ctx, method, input)))
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        let account = account_of(&input);
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
            let status = auth::status(data_dir).await;
            reply(ratelimit::budget(
                status.account.as_deref().unwrap_or_default(),
            ))
        }),
        "signInWithToken" => Box::pin(async move {
            signed_in(
                data_dir,
                auth::sign_in_with_token(data_dir, params(input)?).await,
            )
            .await
        }),
        "signOut" => Box::pin(async move { reply(auth::sign_out(data_dir).await?) }),

        "resolveRemote" => Box::pin(async move { reply(resolve(params(input)?)) }),
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
        "cancel" => answer(input, move |q| pipelines::cancel(data_dir, q)),
        "jobLog" => answer(input, move |q| pipelines::log(data_dir, q)),
        "jobLogExcerpt" => answer(input, move |q| pipelines::excerpt(data_dir, q)),

        "pulls" => answer(input, move |q| pulls::list(data_dir, q)),
        "pull" => answer(input, move |q| pulls::get(data_dir, q)),
        "pullFiles" => answer(input, move |q| pulls::files(data_dir, q)),
        "pullCommits" => answer(input, move |q| pulls::commits(data_dir, q)),
        "commitAuthors" => answer(input, move |q| pulls::commit_authors(data_dir, q)),
        "pullReviews" => answer(input, move |q| pulls::reviews(data_dir, q)),
        "timeline" => answer(input, move |q| pulls::timeline(data_dir, q)),
        "comments" => answer(input, move |q| pulls::comments(data_dir, q)),
        "addComment" => answer(input, move |q| pulls::add_comment(data_dir, q)),
        "mergePull" => answer(input, move |q| pulls::merge(data_dir, q)),
        "createPull" => answer(input, move |q| pulls::create(data_dir, q)),
        "setPullState" => answer(input, move |q| pulls::set_state(data_dir, q)),
        "reviewPull" => answer(input, move |q| pulls::review(data_dir, q)),

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
        "signInWithBrowser" => Box::pin(async move {
            let id = auth::sign_in_with_browser(data_dir, &sink).await?;
            sink.send(reply(
                client::as_account(Some(id), auth::status(data_dir)).await,
            )?)
        }),
        "watchRun" => Box::pin(async move { watch::run(data_dir, params(input)?, sink).await }),
        _ => Box::pin(async move { Err(PluginError::unknown_method(method)) }),
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
    async fn an_unknown_method_is_refused() {
        let plugin = plugin().expect("plugin loads");
        let ctx = PluginContext::new(std::env::temp_dir().join("sikemux-bb-unknown"));
        let error = plugin
            .call(&ctx, "nonsense", Value::Null)
            .await
            .expect_err("refused");
        assert_eq!(error.category, "unknown-method");
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
        config::save(&dir, &config).expect("saves");
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
        assert_eq!(error.category, "unconfigured");
        std::fs::remove_dir_all(dir).ok();
    }
}
