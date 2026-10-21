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
mod watch;
mod workflows;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::Resolved;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteQuery {
    url: String,
}

/// What a git remote points at, and whether it is the GitHub this is signed in
/// to. A repository on another host is still described, so the view can say so
/// rather than showing nothing.
fn resolve(data_dir: &std::path::Path, query: RemoteQuery) -> Resolved {
    let config = config::load(data_dir).unwrap_or_default();
    let found = repo::from_remote(&query.url);
    Resolved {
        same_host: found.as_ref().is_some_and(|repo| config.serves(&repo.host)),
        slug: found.as_ref().map(repo::slug),
        repo: found,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MineQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    50
}

/// Everything the sign-in screen needs about the account that just signed in.
async fn signed_in(
    data_dir: &std::path::Path,
    outcome: GithubResult<String>,
) -> Result<Value, PluginError> {
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
        "signIn" => signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await,
        "signOut" => answer(auth::sign_out(data_dir)).await,

        "resolveRemote" => reply(resolve(data_dir, params(input)?)),
        "myRepos" => {
            let query: MineQuery = params(input)?;
            answer(repo::mine(data_dir, query.limit)).await
        }
        "workflows" => answer(workflows::list(data_dir, params(input)?)).await,
        "branches" => answer(workflows::branches(data_dir, params(input)?)).await,
        "dispatch" => answer(workflows::dispatch(data_dir, params(input)?)).await,

        "runs" => answer(runs::list(data_dir, params(input)?)).await,
        "run" => answer(runs::detail(data_dir, params(input)?)).await,
        "rerun" => answer(runs::rerun(data_dir, params(input)?)).await,
        "rerunJob" => answer(runs::rerun_job(data_dir, params(input)?)).await,
        "cancel" => answer(runs::cancel(data_dir, params(input)?)).await,
        "deleteRunLogs" => answer(runs::delete_logs(data_dir, params(input)?)).await,
        "deleteRun" => answer(runs::delete(data_dir, params(input)?)).await,
        "runAttempt" => answer(runs::attempt(data_dir, params(input)?)).await,

        "jobLog" => answer(logs::job(data_dir, params(input)?)).await,
        "jobLogExcerpt" => answer(logs::excerpt(data_dir, params(input)?)).await,
        "annotations" => answer(annotations::list(data_dir, params(input)?)).await,
        "jobSummary" => answer(annotations::summary(data_dir, params(input)?)).await,
        "runTiming" => answer(runs::timing(data_dir, params(input)?)).await,
        "workflowFile" => answer(workflows::file(data_dir, params(input)?)).await,
        "workflowFileAt" => answer(workflows::file_at(data_dir, params(input)?)).await,

        "artifacts" => answer(artifacts::list(data_dir, params(input)?)).await,

        "pendingApprovals" => answer(approvals::pending(data_dir, params(input)?)).await,
        "reviewDeployment" => answer(approvals::review(data_dir, params(input)?)).await,

        "pulls" => answer(pulls::list(data_dir, params(input)?)).await,
        "pullCounts" => answer(pulls::counts(data_dir, params(input)?)).await,
        "issueCounts" => answer(pulls::issue_counts(data_dir, params(input)?)).await,
        "pull" => answer(pulls::get(data_dir, params(input)?)).await,
        "pullFiles" => answer(pulls::files(data_dir, params(input)?)).await,
        "pullCommits" => answer(pulls::commits(data_dir, params(input)?)).await,
        "commitPulls" => answer(pulls::of_commit(data_dir, params(input)?)).await,
        "commitAuthors" => answer(pulls::commit_authors(data_dir, params(input)?)).await,
        "pullReviews" => answer(pulls::reviews(data_dir, params(input)?)).await,
        "mergePull" => answer(pulls::merge(data_dir, params(input)?)).await,
        "createPull" => answer(pulls::create(data_dir, params(input)?)).await,
        "setPullState" => answer(pulls::set_state(data_dir, params(input)?)).await,
        "reviewPull" => answer(pulls::review(data_dir, params(input)?)).await,
        "reviewFeedback" => answer(review::feedback(data_dir, params(input)?)).await,
        "answerReview" => answer(review::answer(data_dir, params(input)?)).await,

        "issues" => answer(issues::list(data_dir, params(input)?)).await,
        "issue" => answer(issues::get(data_dir, params(input)?)).await,
        "setIssueState" => answer(issues::set_state(data_dir, params(input)?)).await,
        "createIssue" => answer(issues::create(data_dir, params(input)?)).await,

        "addComment" => answer(common::add_comment(data_dir, params(input)?)).await,

        "releases" => answer(releases::list(data_dir, params(input)?)).await,

        "inbox" => answer(inbox::list(data_dir, params(input)?)).await,
        "markRead" => answer(inbox::mark_read(data_dir, params(input)?)).await,
        "comments" => {
            let thread: common::Thread = params(input)?;
            answer(common::comments(data_dir, &thread.repo, thread.number)).await
        }
        "timeline" => {
            let thread: common::Thread = params(input)?;
            answer(common::timeline(data_dir, &thread.repo, thread.number)).await
        }
        "image" => answer(images::image(data_dir, params(input)?)).await,
        "markAllRead" => answer(inbox::mark_all_read(data_dir)).await,

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
        "watchRun" => watch::run(data_dir, params(input)?, sink).await,
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
