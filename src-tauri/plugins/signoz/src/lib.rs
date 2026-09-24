// SigNoz: logs, service health and traces for the projects Sikemux has open.
//
//   config    — where SigNoz is, and the key in the Keychain the shell CLI shares
//   client    — the HTTP client, size limits, and SigNoz's error shapes
//   query     — v5 query_range builders, filter quoting, and time windows
//   logs      — search, and a tail that polls for new lines
//   services  — health per service, and one service's charts, endpoints and errors
//   traces    — every span of one trace, ordered for a waterfall

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
use std::sync::Arc;

use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture, StreamSink,
};

use crate::config::AuthMode;
use crate::error::{SignozError, SignozResult};

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Signoz {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
    }))
}

struct Signoz {
    manifest: Manifest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    configured: bool,
    url: String,
    auth: AuthMode,
    email: String,
    key_from_environment: bool,
    version: Option<String>,
    ok: bool,
    auth_failed: bool,
    message: Option<String>,
}

async fn status(data_dir: &Path) -> Status {
    let config = auth::load(data_dir).await.unwrap_or_default();
    let key_from_environment = config::env_api_key().is_some();
    let base = |ok, auth_failed, version, message| Status {
        configured: !config.url.is_empty(),
        url: config.url.clone(),
        auth: config.auth,
        email: config.email.clone(),
        key_from_environment,
        version,
        ok,
        auth_failed,
        message,
    };
    if config.url.is_empty() {
        return base(false, false, None, None);
    }
    let version = client::send(
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
    match client::query_range(data_dir, &probe).await {
        Ok(_) => base(true, false, version, None),
        Err(error) => {
            let auth_failed = matches!(
                error,
                SignozError::Unconfigured
                    | SignozError::Auth(_)
                    | SignozError::Http {
                        status: 401 | 403,
                        ..
                    }
            );
            base(false, auth_failed, version, Some(error.to_string()))
        }
    }
}

async fn signed_in(data_dir: &Path, outcome: SignozResult<()>) -> Result<Value, PluginError> {
    outcome?;
    reply(status(data_dir).await)
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = SignozResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Signoz {
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
                "status" => reply(status(data_dir).await),
                "inspect" => answer(auth::inspect(data_dir, params(input)?)).await,
                "signIn" => {
                    signed_in(data_dir, auth::sign_in(data_dir, params(input)?).await).await
                }
                "useApiKey" => {
                    signed_in(data_dir, auth::use_api_key(data_dir, params(input)?).await).await
                }
                "signOut" => answer(auth::sign_out(data_dir)).await,
                "searchLogs" => answer(logs::search(data_dir, params(input)?)).await,
                "logVolume" => answer(logs::volume(data_dir, params(input)?)).await,
                "dashboards" => answer(dashboards::list(data_dir)).await,
                "dashboard" => answer(dashboards::get(data_dir, params(input)?)).await,
                "panel" => answer(dashboards::panel(data_dir, params(input)?)).await,
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

    fn stream<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
        sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            match method {
                "tailLogs" => logs::tail(ctx.data_dir(), params(input)?, sink).await,
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_manifest_parses() {
        assert_eq!(
            plugin().expect("manifest parses").manifest().id,
            "sikemux.signoz"
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

    async fn reads_everything(plugin: &Arc<dyn Plugin>, ctx: &PluginContext) {
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

        let volume = plugin
            .call(ctx, "logVolume", json!({ "minutes": 15, "buckets": 30 }))
            .await
            .expect("log volume");
        assert!(volume.as_array().is_some_and(|buckets| buckets.len() >= 30));

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
            let variables: serde_json::Map<String, Value> = dashboard["variables"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|variable| {
                    (
                        variable["name"].as_str().unwrap_or_default().to_string(),
                        variable["selected"].clone(),
                    )
                })
                .collect();
            for panel in dashboard["panels"].as_array().expect("panels") {
                if panel["drawable"] != true {
                    continue;
                }
                let data = plugin
                    .call(ctx, "panel", json!({ "kind": panel["kind"], "query": panel["query"], "variables": variables, "minutes": 60 }))
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
        for line in filtered["lines"].as_array().expect("lines") {
            assert_eq!(line["attributes"]["status"], json!(503), "{line}");
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
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let sink = StreamSink::new(move |item| sender.send(item).is_ok());
        let tail = plugin.stream(ctx, "tailLogs", json!({ "minutes": 5, "limit": 3 }), sink);
        let first = tokio::select! {
            item = receiver.recv() => item,
            _ = tail => None,
        };
        assert!(first.expect("the tail sends its backlog")["error"].is_null());
    }

    #[tokio::test]
    #[ignore]
    async fn signs_in_with_a_key_the_keychain_already_has() {
        let (Some(url), Some(account)) = (env("SIGNOZ_LIVE_URL"), env("SIGNOZ_LIVE_ACCOUNT"))
        else {
            return;
        };
        let (ctx, dir) = scratch("key");
        let plugin = plugin().expect("plugin loads");
        let inspection = plugin
            .call(&ctx, "inspect", json!({ "url": url }))
            .await
            .expect("inspect");
        assert!(inspection["version"].is_string(), "{inspection}");
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
            config::keychain_read(config::API_KEY_SERVICE, &account)
                .expect("keychain")
                .is_some(),
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
        let plugin = plugin().expect("plugin loads");
        let inspection = plugin
            .call(&ctx, "inspect", json!({ "url": url, "email": email }))
            .await
            .expect("inspect");
        assert_eq!(inspection["accountExists"], true, "{inspection}");
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
        assert!(config::keychain_read(config::SESSION_SERVICE, &account)
            .expect("keychain")
            .is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
