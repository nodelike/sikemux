// Sentry: a project's issues with the stack trace of the latest event, resolving,
// ignoring, assigning and commenting, and recent releases with their health.
//
//   config   — where Sentry is, the organisation and project in use, and the token in the Keychain
//   client   — the REST client, the next page, and Sentry's errors in words
//   auth     — signing in with a personal token, and choosing the organisation and project
//   issues   — the list, one issue with its latest event, changes and comments, and a brief for agents
//   releases — recent releases with new issues, deploys and crash-free rates

mod auth;
mod client;
mod config;
mod error;
mod issues;
mod releases;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
};

use crate::error::SentryResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Sentry {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Sentry {
    manifest: Manifest,
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = SentryResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Sentry {
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
                    auth::sign_in(data_dir, params(input)?).await?;
                    reply(auth::status(data_dir).await)
                }
                "signOut" => answer(auth::sign_out(data_dir)).await,
                "organizations" => answer(auth::organizations(data_dir)).await,
                "choose" => answer(auth::choose(data_dir, params(input)?)).await,
                "projects" => answer(auth::projects(data_dir, params(input)?)).await,
                "members" => answer(auth::members(data_dir, params(input)?)).await,
                "environments" => answer(auth::environments(data_dir, params(input)?)).await,
                "issues" => answer(issues::list(data_dir, params(input)?)).await,
                "issue" => answer(issues::detail(data_dir, params(input)?)).await,
                "brief" => answer(issues::brief_of(data_dir, params(input)?)).await,
                "change" => answer(issues::change(data_dir, params(input)?)).await,
                "comment" => answer(issues::comment(data_dir, params(input)?)).await,
                "releases" => answer(releases::list(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Sentry is not tied to a repository, so its tools are offered wherever a sign-in is kept.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(config::load(ctx.data_dir()).is_signed_in()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-sentry-{name}-{}", std::process::id())),
        )
    }

    #[test]
    fn its_manifest_parses_with_the_five_tools() {
        let plugin = plugin().expect("manifest parses");
        let names: Vec<&str> = plugin
            .manifest()
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "sentry_issues",
                "sentry_issue",
                "sentry_update",
                "sentry_comment",
                "sentry_releases"
            ]
        );
    }

    #[tokio::test]
    async fn nothing_signed_in_reads_as_signed_out_and_offers_no_tools() {
        let plugin = plugin().expect("plugin loads");
        let ctx = scratch("status");
        let status = plugin
            .call(&ctx, "status", Value::Null)
            .await
            .expect("status");
        assert_eq!(
            (status["configured"].as_bool(), status["ok"].as_bool()),
            (Some(false), Some(false))
        );
        let error = plugin
            .call(&ctx, "issues", json!({}))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, "unconfigured");
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await.expect("answers"));
    }

    #[tokio::test]
    async fn an_organisation_token_is_turned_away_before_any_request() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(
                &scratch("org-token"),
                "signIn",
                json!({ "token": "sntrys_abc" }),
            )
            .await
            .expect_err("not a personal token");
        assert_eq!(error.category, "bad-params");
        assert!(
            error.message.contains("Personal Tokens"),
            "{}",
            error.message
        );
    }
}
