// Jira Cloud: the issues a person is working on, and tools for their agents.
//
//   config  — the sites signed in here, and their sign-ins in the Keychain
//   client  — the HTTP client, size limits, and Jira's error shape
//   oauth   — signing in through the browser with an Atlassian account
//   auth    — signing in with an API token or the browser, and the status of each
//   adf     — Jira's document format to markdown and back
//   issues  — search, one issue, comments, transitions, assignment, new and edited issues, links, worklogs, filters

mod adf;
mod auth;
mod boards;
mod client;
mod config;
mod error;
mod images;
mod issues;
mod oauth;
mod views;

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
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

#[derive(serde::Deserialize)]
struct LinkQuery {
    url: String,
}

async fn signed_in(data_dir: &Path, outcome: JiraResult<()>) -> Result<Value, PluginError> {
    outcome?;
    reply(auth::status(data_dir).await)
}

/// An answer as an agent reads it: without the pictures and empty fields
/// only the app's views use.
fn for_agent(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, value)| {
                    !value.is_null()
                        && !matches!(
                            key.as_str(),
                            "avatarUrl" | "authorAvatarUrl" | "statusCategory"
                        )
                })
                .map(|(key, value)| (key, for_agent(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(for_agent).collect()),
        other => other,
    }
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
                "resolveLink" => {
                    let query: LinkQuery = params(input)?;
                    reply(config::issue_link(data_dir, &query.url))
                }
                "signIn" => {
                    signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await
                }
                "signOut" => answer(auth::sign_out(data_dir, params(input)?)).await,
                "search" => answer(issues::search(data_dir, params(input)?)).await,
                "issue" => answer(issues::issue(data_dir, params(input)?)).await,
                "agentSearch" => answer(issues::search(data_dir, params(input)?))
                    .await
                    .map(for_agent),
                "agentIssue" => answer(issues::agent_issue(data_dir, params(input)?))
                    .await
                    .map(for_agent),
                "comment" => answer(issues::comment(data_dir, params(input)?)).await,
                "transition" => answer(issues::transition(data_dir, params(input)?)).await,
                "assign" => answer(issues::assign(data_dir, params(input)?)).await,
                "assignable" => answer(issues::assignable(data_dir, params(input)?)).await,
                "setTask" => answer(issues::set_task(data_dir, params(input)?)).await,
                "create" => answer(issues::create(data_dir, params(input)?)).await,
                "edit" => answer(issues::edit(data_dir, params(input)?)).await,
                "link" => answer(issues::link(data_dir, params(input)?)).await,
                "worklog" => answer(issues::worklog(data_dir, params(input)?)).await,
                "listView" => answer(views::list_view(data_dir, params(input)?)).await,
                "spacesView" => answer(views::spaces_view(data_dir, params(input)?)).await,
                "boardView" => answer(views::board_view(data_dir, params(input)?)).await,
                "issueView" => answer(views::issue_view(data_dir, params(input)?)).await,
                "moveIssue" => answer(boards::move_issue(data_dir, params(input)?)).await,
                "image" => answer(images::image(params(input)?)).await,
                "siteImage" => answer(images::site_image(data_dir, params(input)?)).await,
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
        Box::pin(async move {
            Ok(config::load(ctx.data_dir()).is_ok_and(|config| !config.sites.is_empty()))
        })
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
