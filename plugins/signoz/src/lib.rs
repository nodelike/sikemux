// SigNoz: logs, service health and traces for the projects Sikemux has open.

mod agent;
mod auth;
mod client;
mod config;
mod dashboards;
mod error;
mod fields;
mod filter;
mod logs;
mod query;
mod services;
mod traces;

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::{
    answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture, SignInStatus,
    SignedInAs,
};

use crate::config::AuthMode;
use crate::error::{SignozError, SignozResult};

/// Which SigNoz the plugin is signed in to, and as whom.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Where {
    pub url: String,
    pub auth: AuthMode,
    pub email: String,
    pub key_from_environment: bool,
    pub version: Option<String>,
}

sikemux_plugin_api::plugin!(Signoz);

impl SignedInAs for Where {
    fn signed_in_as(&self) -> Option<String> {
        Some(match self.email.is_empty() {
            true => self.url.clone(),
            false => self.email.clone(),
        })
    }
}

type Status = SignInStatus<Where>;

async fn status(data_dir: &Path) -> Status {
    let config = match auth::load(data_dir).await {
        Ok(config) => config,
        Err(error) => return Status::checked(false, Where::default(), &Err::<(), _>(error)),
    };
    let mut found = Where {
        url: config.url.clone(),
        auth: config.auth,
        email: config.email.clone(),
        key_from_environment: config::env_api_key().is_some(),
        version: None,
    };
    if config.url.is_empty() {
        return Status::checked(false, found, &Err::<(), _>(SignozError::Unconfigured));
    }
    found.version = client::send(
        &auth::Credentials::anonymous(&config.url),
        Method::GET,
        "/api/v1/version",
        None,
    )
    .await
    .ok()
    .and_then(|body| {
        body.get("version")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let probe = query::builder(
        "raw",
        query::window(Some(1)),
        json!({ "signal": "logs", "limit": 1 }),
    );
    let check = client::query_range(data_dir, &probe).await;
    Status::checked(true, found, &check)
}

/// Whether a call could find a credential, judged without the Keychain.
fn has_credentials(config: &config::SignozConfig) -> bool {
    !config.url.is_empty() && (config::env_api_key().is_some() || !config.account.is_empty())
}

async fn signed_in(data_dir: &Path, outcome: SignozResult<()>) -> Result<Value, PluginError> {
    outcome?;
    reply(status(data_dir).await)
}

impl Plugin for Signoz {
    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir();
            let (method, input) = agent::route(method, input)?;
            match method {
                "status" => reply(status(data_dir).await),
                "signIn" => {
                    signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await
                }
                "useApiKey" => {
                    signed_in(data_dir, auth::use_api_key(data_dir, params(input)?).await).await
                }
                "signOut" => answer(auth::sign_out(data_dir)).await,
                "searchLogs" => answer(logs::search(data_dir, params(input)?)).await,
                "dashboards" => answer(dashboards::list(data_dir)).await,
                "dashboard" => answer(dashboards::get(data_dir, params(input)?)).await,
                "dashboardPanel" => answer(dashboards::saved_panel(data_dir, params(input)?)).await,
                "services" => answer(services::health(data_dir, params(input)?)).await,
                "serviceOverview" => answer(services::overview(data_dir, params(input)?)).await,
                "operations" => answer(services::operations(data_dir, params(input)?)).await,
                "errorGroups" => answer(services::errors(data_dir, params(input)?)).await,
                "trace" => answer(traces::trace(data_dir, params(input)?)).await,
                "searchTraces" => answer(traces::search(data_dir, params(input)?)).await,
                "fieldKeys" => answer(fields::keys(data_dir, params(input)?)).await,
                "fieldValues" => answer(fields::values(data_dir, params(input)?)).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(has_credentials(&auth::load(ctx.data_dir()).await?)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_saved_address_with_a_credential_can_answer_an_agent() {
        let config = |url: &str, account: &str| config::SignozConfig {
            url: url.into(),
            account: account.into(),
            ..Default::default()
        };
        assert!(!has_credentials(&config("", "key")));
        assert!(has_credentials(&config("https://signoz.example", "key")));
        assert_eq!(
            has_credentials(&config("https://signoz.example", "")),
            config::env_api_key().is_some()
        );
    }
}

/// Runs against a real SigNoz only when asked. With an API key already in the Keychain:
/// `SIGNOZ_LIVE_URL=… SIGNOZ_LIVE_ACCOUNT=… cargo test -p sikemux-plugin-signoz -- --ignored`
/// and to sign in as a person, add `SIGNOZ_LIVE_EMAIL` and `SIGNOZ_LIVE_PASSWORD`.
#[cfg(test)]
mod live {
    use super::*;

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    fn scratch(name: &str) -> (PluginContext, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("sikemux-signoz-{name}-{}", std::process::id()));
        (PluginContext::new(dir.clone()), dir)
    }

    async fn reads_everything(plugin: &std::sync::Arc<dyn Plugin>, ctx: &PluginContext) {
        let services = plugin
            .call(ctx, "services", json!({ "minutes": 15 }))
            .await
            .expect("services");
        let busiest = services[0]["service"]
            .as_str()
            .expect("a service")
            .to_string();
        assert!(
            services
                .as_array()
                .expect("rows")
                .iter()
                .any(|row| row["environment"].is_string()),
            "services carry their environment"
        );

        let keys = plugin
            .call(
                ctx,
                "fieldKeys",
                json!({ "signal": "logs", "search": "sta" }),
            )
            .await
            .expect("field keys");
        assert!(keys.as_array().is_some_and(|keys| !keys.is_empty()));
        let names = plugin
            .call(
                ctx,
                "fieldValues",
                json!({ "signal": "logs", "name": "service.name" }),
            )
            .await
            .expect("field values");
        assert!(names.as_array().is_some_and(|names| !names.is_empty()));

        let traces = plugin
            .call(
                ctx,
                "searchTraces",
                json!({ "minutes": 15, "limit": 3, "order": "slowest" }),
            )
            .await
            .expect("traces");
        let slowest = traces["traces"].as_array().expect("trace rows");
        assert!(slowest.len() <= 3);
        if slowest.len() > 1 {
            assert!(slowest[0]["durationMs"].as_f64() >= slowest[1]["durationMs"].as_f64());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;

        let dashboards = plugin
            .call(ctx, "dashboards", Value::Null)
            .await
            .expect("dashboards");
        let mut drawn = 0;
        for summary in dashboards.as_array().expect("dashboard list") {
            let dashboard = plugin
                .call(ctx, "dashboard", json!({ "id": summary["id"] }))
                .await
                .expect("dashboard");
            for panel in dashboard["panels"].as_array().expect("panels") {
                if panel["drawable"] != true {
                    continue;
                }
                let data = plugin
                    .call(ctx, "dashboardPanel", json!({ "dashboardId": summary["id"], "panelId": panel["id"], "minutes": 60 }))
                    .await;
                assert!(
                    data.is_ok(),
                    "{} / {}: {:?}",
                    summary["title"],
                    panel["title"],
                    data.err()
                );
                drawn += 1;
            }
        }
        eprintln!("live: drew {drawn} dashboard panels");

        let filtered = plugin
            .call(
                ctx,
                "searchLogs",
                json!({
                    "filters": [{ "key": "status", "op": "equals", "value": "503" }, { "key": "path", "op": "exists" }],
                    "start": now - 60 * 60_000,
                    "end": now,
                    "limit": 5,
                }),
            )
            .await
            .expect("filtered logs");
        let shared = &filtered["shared"]["attributes"]["status"];
        for line in filtered["lines"].as_array().expect("lines") {
            let status = match &line["attributes"]["status"] {
                Value::Null => shared,
                own => own,
            };
            assert_eq!(status, &json!(503), "{line}");
        }
        let page = plugin
            .call(
                ctx,
                "searchLogs",
                json!({ "severities": ["ERROR"], "minutes": 60, "limit": 5 }),
            )
            .await
            .expect("logs");
        assert!(page["lines"]
            .as_array()
            .is_some_and(|lines| lines.len() <= 5));
        let recent = plugin
            .call(
                ctx,
                "searchLogs",
                json!({ "service": busiest, "minutes": 15, "limit": 1 }),
            )
            .await
            .expect("service logs");
        if let Some(trace_id) = recent["lines"][0]["traceId"].as_str() {
            let trace = plugin
                .call(ctx, "trace", json!({ "traceId": trace_id, "minutes": 60 }))
                .await
                .expect("trace");
            assert_eq!(trace["traceId"], trace_id);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn signs_in_with_a_key_the_keychain_already_has() {
        let (Some(url), Some(account)) = (env("SIGNOZ_LIVE_URL"), env("SIGNOZ_LIVE_ACCOUNT"))
        else {
            return;
        };
        let (ctx, dir) = scratch("key");
        let plugin = plugin();
        let inspection = auth::inspect(
            ctx.data_dir(),
            serde_json::from_value(json!({ "url": url })).expect("reads"),
        )
        .await
        .expect("inspect");
        assert!(inspection.version.is_some());
        let status = plugin
            .call(&ctx, "useApiKey", json!({ "url": url, "account": account }))
            .await
            .expect("use the key");
        assert_eq!(status["ok"], true, "{status}");
        reads_everything(&plugin, &ctx).await;
        plugin
            .call(&ctx, "signOut", Value::Null)
            .await
            .expect("sign out");
        assert!(
            config::API_KEYS.read(&account).expect("keychain").is_some(),
            "signing out must leave a key it did not save"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    #[ignore]
    async fn signs_in_with_email_and_password() {
        let (Some(url), Some(email), Some(password)) = (
            env("SIGNOZ_LIVE_URL"),
            env("SIGNOZ_LIVE_EMAIL"),
            env("SIGNOZ_LIVE_PASSWORD"),
        ) else {
            return;
        };
        let (ctx, dir) = scratch("session");
        let plugin = plugin();
        let inspection = auth::inspect(
            ctx.data_dir(),
            serde_json::from_value(json!({ "url": url, "email": email })).expect("reads"),
        )
        .await
        .expect("inspect");
        assert_eq!(inspection.account_exists, Some(true));
        let status = plugin
            .call(
                &ctx,
                "signIn",
                json!({ "url": url, "email": email, "password": password }),
            )
            .await
            .expect("sign in");
        assert_eq!(status["ok"], true, "{status}");
        reads_everything(&plugin, &ctx).await;

        auth::expire_access().await;
        plugin
            .call(&ctx, "services", json!({ "minutes": 5 }))
            .await
            .expect("a renewed session still reads");

        plugin
            .call(&ctx, "signOut", Value::Null)
            .await
            .expect("sign out");
        let account = config::account_for(&url);
        assert!(config::SESSIONS.read(&account).expect("keychain").is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
