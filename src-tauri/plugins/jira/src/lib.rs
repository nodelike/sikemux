// Jira Cloud: the issues a person is working on, and tools for their agents.
//
//   config  — the sites signed in here, and their sign-ins in the Keychain
//   client  — the HTTP client, size limits, and Jira's error shape
//   oauth   — signing in through the browser with an Atlassian account
//   auth    — signing in with an API token or the browser, and the status of each
//   adf     — Jira's document format to markdown and back
//   issues  — search, one issue, comments, transitions, assignment, new issues, worklogs, filters

mod adf;
mod auth;
mod boards;
mod client;
mod config;
mod error;
mod issues;
mod oauth;

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::JiraResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Jira {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Jira {
    manifest: Manifest,
}

async fn signed_in(data_dir: &Path, outcome: JiraResult<()>) -> Result<Value, PluginError> {
    outcome?;
    reply(auth::status(data_dir).await)
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = JiraResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Jira {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "status" => reply(auth::status(data_dir).await),
                "signIn" => {
                    signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await
                }
                "signOut" => answer(auth::sign_out(data_dir, params(input)?)).await,
                "search" => answer(issues::search(data_dir, params(input)?)).await,
                "issue" => answer(issues::issue(data_dir, params(input)?)).await,
                "comment" => answer(issues::comment(data_dir, params(input)?)).await,
                "transition" => answer(issues::transition(data_dir, params(input)?)).await,
                "assign" => answer(issues::assign(data_dir, params(input)?)).await,
                "assignable" => answer(issues::assignable(data_dir, params(input)?)).await,
                "setTask" => answer(issues::set_task(data_dir, params(input)?)).await,
                "create" => answer(issues::create(data_dir, params(input)?)).await,
                "worklog" => answer(issues::worklog(data_dir, params(input)?)).await,
                "filters" => answer(issues::filters(data_dir, params(input)?)).await,
                "projects" => answer(issues::projects(data_dir, params(input)?)).await,
                "boards" => answer(boards::boards(data_dir, params(input)?)).await,
                "board" => answer(boards::board(data_dir, params(input)?)).await,
                "moveIssue" => answer(boards::move_issue(data_dir, params(input)?)).await,
                "keys" => reply(issues::keys_in(&params::<issues::KeysRequest>(input)?.text)),
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        _input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "signInWithBrowser" => {
                    auth::sign_in_with_browser(data_dir, &sink).await?;
                    sink.send(reply(auth::status(data_dir).await)?)
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Jira is not tied to a repository, so its tools are offered wherever a site is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(!config::load(ctx.data_dir()).sites.is_empty()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-jira-{name}-{}", std::process::id())),
        )
    }

    #[test]
    fn its_manifest_parses_with_the_six_tools() {
        let plugin = plugin().map_err(|error| error.to_string());
        let names = plugin
            .as_ref()
            .map(|plugin| {
                plugin
                    .manifest()
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            [
                "jira_search",
                "jira_issue",
                "jira_comment",
                "jira_transition",
                "jira_create",
                "jira_worklog"
            ]
        );
    }

    #[tokio::test]
    async fn with_no_site_signed_in_it_offers_no_tools_and_says_so() {
        let ctx = scratch("signed-out");
        let Ok(jira) = plugin() else { return };
        assert_eq!(jira.offers_agent_tools(&ctx, &[]).await.ok(), Some(false));
        let status = jira
            .call(&ctx, "status", Value::Null)
            .await
            .unwrap_or_default();
        assert_eq!(status.get("configured"), Some(&Value::Bool(false)));
        let unknown = jira.call(&ctx, "nope", Value::Null).await;
        assert!(unknown.is_err());
    }
}
