// Vercel: an account's projects and deployments, each deployment's build log, and
// building, promoting and stopping them, in a pane and as agent tools.
//
//   config      — the account signed in here, its tokens in the Keychain, and the team chosen
//   client      — the REST client, the token, the team a call is for, and Vercel's errors in words
//   oauth       — installing the Sikemux integration from the browser, and removing it
//   auth        — signing in with the browser or an access token, choosing a team, and the account's status
//   projects    — projects, their domains, and the names of their environment variables
//   deploy      — deploying a folder on this Mac, sending only the files Vercel lacks
//   deployments — listing and reading deployments, build logs, redeploy, promote and cancel
//   domains     — adding a domain, the DNS records it needs, verifying, routing and removing it
//   firewall    — the firewall's rules and IP blocks, and Attack Challenge Mode
//   env         — adding, changing and removing environment variables; values only go to Vercel
//   protection  — whether deployments ask for a sign-in or password, and a link past it
//   runtime     — a project's recent requests and function logs, newest first
//   settings    — a project's framework, commands, root directory and Node version, read and changed

mod auth;
mod client;
mod config;
mod deploy;
mod deployments;
mod domains;
mod env;
mod error;
mod firewall;
mod oauth;
mod projects;
mod protection;
mod runtime;
mod settings;

use std::sync::Arc;

use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Vercel {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Vercel {
    manifest: Manifest,
}

impl Plugin for Vercel {
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
                "setTeam" => answer(auth::set_team(data_dir, params(input)?)).await,
                "projects" => answer(projects::projects(data_dir, params(input)?)).await,
                "domains" => answer(projects::domains(data_dir, params(input)?)).await,
                "env" => answer(projects::env(data_dir, params(input)?)).await,
                "deployments" => answer(deployments::deployments(data_dir, params(input)?)).await,
                "deployment" => answer(deployments::detail(data_dir, params(input)?)).await,
                "logs" => answer(deployments::logs(data_dir, params(input)?)).await,
                "redeploy" => answer(deployments::redeploy(data_dir, params(input)?)).await,
                "promote" => answer(deployments::promote(data_dir, params(input)?)).await,
                "cancel" => answer(deployments::cancel(data_dir, params(input)?)).await,
                "domain" => answer(domains::detail(data_dir, params(input)?)).await,
                "addDomain" => answer(domains::add(data_dir, params(input)?)).await,
                "updateDomain" => answer(domains::update(data_dir, params(input)?)).await,
                "verifyDomain" => answer(domains::verify(data_dir, params(input)?)).await,
                "removeDomain" => answer(domains::remove(data_dir, params(input)?)).await,
                "changeDomain" => answer(domains::change(data_dir, input)).await,
                "runtimeLogs" => answer(runtime::recent(data_dir, params(input)?)).await,
                "setEnv" => answer(env::set(data_dir, params(input)?)).await,
                "removeEnv" => answer(env::remove(data_dir, params(input)?)).await,
                "changeEnv" => answer(env::change(data_dir, input)).await,
                "firewall" => answer(firewall::firewall(data_dir, params(input)?)).await,
                "setAttackMode" => {
                    answer(firewall::set_attack_mode(data_dir, params(input)?)).await
                }
                "protection" => answer(protection::protection(data_dir, params(input)?)).await,
                "shareLink" => answer(protection::share(data_dir, params(input)?)).await,
                "crons" => answer(settings::crons(data_dir, params(input)?)).await,
                "settings" => answer(settings::settings(data_dir, params(input)?)).await,
                "updateSettings" => answer(settings::update(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            match method {
                "deployFolder" => deploy::deploy(data_dir, input, &sink).await,
                "signInWithBrowser" => {
                    auth::sign_in_with_browser(data_dir, &sink).await?;
                    sink.send(reply(auth::status(data_dir).await)?)
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    /// Vercel is not tied to a repository, so its tools are offered wherever an account is signed in.
    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(config::load(ctx.data_dir())?.account.is_some()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        PluginContext::new(
            std::env::temp_dir().join(format!("sikemux-vercel-{name}-{}", std::process::id())),
        )
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
            .call(&ctx, "projects", json!({}))
            .await
            .expect_err("signed out");
        assert_eq!(error.category, "unconfigured");
        assert!(!plugin.offers_agent_tools(&ctx, &[]).await.expect("answers"));
    }

    #[tokio::test]
    async fn an_empty_token_is_turned_away_before_any_request() {
        let plugin = plugin().expect("plugin loads");
        let error = plugin
            .call(&scratch("empty-token"), "signIn", json!({ "token": "  " }))
            .await
            .expect_err("no token");
        assert_eq!(error.category, "bad-params");
    }
}
