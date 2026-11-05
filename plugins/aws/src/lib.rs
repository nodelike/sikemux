// AWS: read-only aws_* tools for agents, on the profiles in ~/.aws/config.
// Every API call shells out to the `aws` CLI so the heavy SDK crates stay out
// of the binary; the CLI reuses the user's existing SSO token cache and
// ~/.aws/config resolution rules (sso_session refs, source_profile chains,
// region precedence) for free.

mod agent;
mod agent_profile;
mod auth;
mod common;
mod ec2;
mod ecs;
mod elb;
mod error;
mod logs;
mod metrics;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

sikemux_plugin_api::plugin!(Aws);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileParams {
    profile: String,
}

/// The profile the plugin's sign-in is checked as, or the one agents start on.
#[derive(Serialize, Deserialize)]
struct Account {
    profile: Option<String>,
}

impl Plugin for Aws {
    fn agent_environment(&self, ctx: &PluginContext, project: &Path) -> BTreeMap<String, String> {
        agent_profile::environment(ctx, project)
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            match method {
                "profiles" => reply(auth::profiles().await),
                "status" => reply(auth::status(ctx).await),
                "setDefaultAccount" => {
                    let Account { profile } = params(input)?;
                    reply(Account {
                        profile: agent_profile::pick_account(ctx, profile)?,
                    })
                }
                "agentProfilePicks" => reply(agent_profile::picks(ctx, None)),
                "pickAgentProfile" => {
                    let Account { profile } = params(input)?;
                    reply(agent_profile::pick(ctx, None, profile)?)
                }
                _ => match agent::call(ctx, method, input).await {
                    Some(answer) => answer,
                    None => Err(PluginError::unknown_method(method)),
                },
            }
        })
    }

    fn stream<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            match method {
                "signIn" => {
                    let ProfileParams { profile } = params(input)?;
                    sink.send(reply(auth::sso_login(profile).await)?)
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn offers_agent_tools<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async {
            Ok(tokio::task::spawn_blocking(auth::aws_in_use)
                .await
                .unwrap_or(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_manifest_parses() {
        let plugin = plugin().expect("manifest parses");
        assert_eq!(plugin.manifest().id, "sikemux.aws");
    }
}
