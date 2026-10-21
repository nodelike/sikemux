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
mod watch;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::Resolved;
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

#[derive(Deserialize)]
struct RemoteQuery {
    url: String,
}

fn resolve(query: RemoteQuery) -> Resolved {
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| repo.host == repo::HOST),
        slug: found.as_ref().map(repo::slug),
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
    reply(as_account(Some(id), auth::status(data_dir)).await)
}

/// Whether the repository has a remote on Bitbucket, and an account is signed in.
fn works_in(data_dir: &std::path::Path, remotes: &[String]) -> bool {
    let on_bitbucket = remotes
        .iter()
        .filter_map(|remote| repo::from_remote(remote))
        .any(|repo| repo.host == repo::HOST);
    on_bitbucket && config::load(data_dir).is_ok_and(|config| !config.accounts.is_empty())
}

/// Whether a thread call is about an issue; Bitbucket numbers issues apart from pull requests.
fn is_issue(input: &Value) -> bool {
    input.get("of").and_then(Value::as_str) == Some("issue")
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
        Box::pin(as_account(account, dispatch(ctx, method, input)))
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

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(works_in(ctx.data_dir(), remotes)) })
    }
}

async fn dispatch(ctx: &PluginContext, method: &str, input: Value) -> Result<Value, PluginError> {
    let data_dir = ctx.data_dir();
    match method {
        "status" => reply(auth::status(data_dir).await),
        "accounts" => reply(auth::accounts(data_dir)?),
        "setDefaultAccount" => answer(auth::set_default(data_dir, params(input)?)).await,
        "accountFor" => answer(auth::account_for(data_dir, params(input)?)).await,
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

        "resolveRemote" => reply(resolve(params(input)?)),
        "myRepos" => {
            let query: MineQuery = params(input)?;
            answer(repo::mine(data_dir, query.limit)).await
        }
        "branches" => answer(repo::branches(data_dir, params(input)?)).await,
        "image" => answer(images::image(data_dir, params(input)?)).await,

        "workflows" => answer(pipelines::workflows(data_dir, params(input)?)).await,
        "workflowFile" => answer(pipelines::workflow_file(data_dir, params(input)?)).await,
        "dispatch" => answer(pipelines::dispatch(data_dir, params(input)?)).await,
        "runs" => answer(pipelines::list(data_dir, params(input)?)).await,
        "run" => answer(pipelines::detail(data_dir, params(input)?)).await,
        "runTiming" => answer(pipelines::timing(data_dir, params(input)?)).await,
        "rerun" => answer(pipelines::rerun(data_dir, params(input)?)).await,
        "cancel" => answer(pipelines::cancel(data_dir, params(input)?)).await,
        "jobLog" => answer(pipelines::log(data_dir, params(input)?)).await,
        "jobLogExcerpt" => answer(pipelines::excerpt(data_dir, params(input)?)).await,

        "pulls" => answer(pulls::list(data_dir, params(input)?)).await,
        "pull" => answer(pulls::get(data_dir, params(input)?)).await,
        "pullFiles" => answer(pulls::files(data_dir, params(input)?)).await,
        "pullCommits" => answer(pulls::commits(data_dir, params(input)?)).await,
        "commitPulls" => answer(pulls::of_commit(data_dir, params(input)?)).await,
        "commitAuthors" => answer(pulls::commit_authors(data_dir, params(input)?)).await,
        "pullReviews" => answer(pulls::reviews(data_dir, params(input)?)).await,
        "timeline" if is_issue(&input) => answer(issues::timeline(data_dir, params(input)?)).await,
        "comments" if is_issue(&input) => answer(issues::comments(data_dir, params(input)?)).await,
        "addComment" if is_issue(&input) => {
            answer(issues::add_comment(data_dir, params(input)?)).await
        }
        "timeline" => answer(pulls::timeline(data_dir, params(input)?)).await,
        "comments" => answer(pulls::comments(data_dir, params(input)?)).await,
        "addComment" => answer(pulls::add_comment(data_dir, params(input)?)).await,

        "issues" => answer(issues::list(data_dir, params(input)?)).await,
        "issue" => answer(issues::get(data_dir, params(input)?)).await,
        "createIssue" => answer(issues::create(data_dir, params(input)?)).await,
        "setIssueState" => answer(issues::set_state(data_dir, params(input)?)).await,
        "releases" => answer(issues::releases(data_dir, params(input)?)).await,
        "inbox" => answer(issues::inbox(data_dir, params(input)?)).await,
        "markRead" => reply(issues::mark_read(data_dir, params(input)?)?),
        "markAllRead" => answer(issues::mark_all_read(data_dir)).await,
        "mergePull" => answer(pulls::merge(data_dir, params(input)?)).await,
        "createPull" => answer(pulls::create(data_dir, params(input)?)).await,
        "editPull" => answer(pulls::edit(data_dir, params(input)?)).await,
        "setPullState" => answer(pulls::set_state(data_dir, params(input)?)).await,
        "reviewPull" => answer(pulls::review(data_dir, params(input)?)).await,
        "reviewFeedback" => answer(review::feedback(data_dir, params(input)?)).await,
        "answerReview" => answer(review::answer(data_dir, params(input)?)).await,

        _ => Err(PluginError::unknown_method(method)),
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
        "signInWithBrowser" => {
            let id = auth::sign_in_with_browser(data_dir, &sink).await?;
            sink.send(reply(as_account(Some(id), auth::status(data_dir)).await)?)
        }
        "watchRun" => watch::run(data_dir, params(input)?, sink).await,
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
        assert_eq!(error.category, "unconfigured");
        std::fs::remove_dir_all(dir).ok();
    }
}
