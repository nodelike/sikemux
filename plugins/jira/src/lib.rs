// Jira Cloud: tools for the person's agents to read and change issues.

mod adf;
mod auth;
mod client;
mod config;
mod error;
mod issues;
mod oauth;

use std::path::Path;

use serde_json::Value;
use sikemux_plugin_api::store;
use sikemux_plugin_api::{
    answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::error::JiraResult;

sikemux_plugin_api::plugin!(Jira);

async fn signed_in(data_dir: &Path, outcome: JiraResult<()>) -> Result<Value, PluginError> {
    outcome?;
    reply(auth::status(data_dir).await)
}

/// An answer as an agent reads it: without its empty fields.
fn for_agent(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, for_agent(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(for_agent).collect()),
        other => other,
    }
}

impl Plugin for Jira {
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
                "agentSearch" => answer(issues::search(data_dir, params(input)?))
                    .await
                    .map(for_agent),
                "agentIssue" => answer(issues::agent_issue(data_dir, params(input)?))
                    .await
                    .map(for_agent),
                "comment" => answer(issues::comment(data_dir, params(input)?)).await,
                "transition" => answer(issues::transition(data_dir, params(input)?)).await,
                "create" => answer(issues::create(data_dir, params(input)?)).await,
                "edit" => answer(issues::edit(data_dir, params(input)?)).await,
                "link" => answer(issues::link(data_dir, params(input)?)).await,
                "worklog" => answer(issues::worklog(data_dir, params(input)?)).await,
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
        Box::pin(async move { Ok(store::has_account::<config::JiraConfig>(ctx.data_dir())) })
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
        let jira = plugin();
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
