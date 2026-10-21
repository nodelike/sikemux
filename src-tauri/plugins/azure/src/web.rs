// App Service and Functions: each app's state and address, its deployment slots,
// its settings by name, and the log stream the portal shows.

use std::time::Duration;

use futures::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::StreamSink;

use crate::client::{self, at, maybe, text};
use crate::error::{AzureError, AzureResult};

const WEB_API: &str = "2024-11-01";
/// A log stream is dropped when the pane stops listening, so it may run as long as the pane is open.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub id: String,
    pub name: String,
    pub group: String,
    pub location: String,
    /// Running or Stopped.
    pub state: String,
    /// app, functionapp or app,linux as Azure spells it.
    pub kind: String,
    /// Whether it is a Functions app rather than a web app.
    pub functions: bool,
    pub host: Option<String>,
    pub https_only: bool,
    /// The runtime it is set to run, such as NODE|22-lts.
    pub runtime: Option<String>,
    pub plan: Option<String>,
    pub tags: Vec<(String, String)>,
}

pub fn app_of(app: &Value) -> Option<App> {
    let id = maybe(app, "/id")?;
    let kind = text(app, "/kind");
    Some(App {
        name: maybe(app, "/name").unwrap_or_else(|| client::short(&id)),
        group: client::group_of(&id),
        location: text(app, "/location"),
        state: maybe(app, "/properties/state").unwrap_or_default(),
        functions: kind.contains("functionapp"),
        host: maybe(app, "/properties/defaultHostName"),
        https_only: at(app, "/properties/httpsOnly").as_bool().unwrap_or(false),
        runtime: maybe(app, "/properties/siteConfig/linuxFxVersion")
            .or_else(|| maybe(app, "/properties/siteConfig/windowsFxVersion")),
        plan: maybe(app, "/properties/serverFarmId").map(|plan| client::short(&plan)),
        tags: at(app, "/tags")
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect(),
        kind,
        id,
    })
}

async fn subscription_of(named: Option<&str>) -> AzureResult<String> {
    if let Some(named) = named.filter(|named| !named.is_empty()) {
        return client::segment("subscription", named);
    }
    crate::auth::status()
        .await
        .current
        .map(|subscription| subscription.id)
        .ok_or(AzureError::Unconfigured)
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListApps {
    pub subscription: Option<String>,
    pub group: Option<String>,
}

pub async fn list(input: ListApps) -> AzureResult<Vec<App>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let path = match input.group.as_deref().filter(|group| !group.is_empty()) {
        Some(group) => format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Web/sites",
            client::segment("resource group", group)?
        ),
        None => format!("/subscriptions/{subscription}/providers/Microsoft.Web/sites"),
    };
    let listed = client::list(&path, WEB_API, &[]).await?;
    let mut apps: Vec<App> = listed.iter().filter_map(app_of).collect();
    apps.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    Ok(apps)
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppRef {
    #[serde(default)]
    pub subscription: Option<String>,
    pub group: String,
    pub name: String,
}

impl AppRef {
    pub async fn path(&self) -> AzureResult<String> {
        let subscription = subscription_of(self.subscription.as_deref()).await?;
        Ok(format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Web/sites/{}",
            client::segment("resource group", &self.group)?,
            client::segment("app", &self.name)?
        ))
    }
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub name: String,
    pub state: String,
    pub host: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDetail {
    #[serde(flatten)]
    pub app: App,
    pub slots: Vec<Slot>,
    /// Setting names only, with whether each comes from Key Vault. No value ever reaches Sikemux.
    pub settings: Vec<Setting>,
    pub always_on: bool,
    pub inbound_ip: Option<String>,
    pub outbound_ips: Vec<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub name: String,
    /// A Key Vault reference, rather than a value kept in the app.
    pub from_key_vault: bool,
}

/// App settings by name. Azure's answer carries values; they are dropped here and never sent on.
pub fn settings_of(answer: &Value) -> Vec<Setting> {
    let mut settings: Vec<Setting> = at(answer, "/properties")
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| Setting {
            name: name.clone(),
            from_key_vault: value
                .as_str()
                .is_some_and(|value| value.starts_with("@Microsoft.KeyVault(")),
        })
        .collect();
    settings.sort_by(|a, b| a.name.cmp(&b.name));
    settings
}

pub async fn app(input: AppRef) -> AzureResult<AppDetail> {
    let path = input.path().await?;
    let found = client::get(&path, WEB_API).await?;
    let app = app_of(&found)
        .ok_or_else(|| AzureError::Response("Azure sent an app without an id".into()))?;
    let config_path = format!("{path}/config/web");
    let settings_path = format!("{path}/config/appsettings/list");
    let slots_path = format!("{path}/slots");
    let nothing = json!({});
    let (config, settings, slots) = tokio::join!(
        client::get(&config_path, WEB_API),
        client::post(&settings_path, WEB_API, &nothing),
        client::list(&slots_path, WEB_API, &[])
    );
    let config = config.unwrap_or(Value::Null);
    Ok(AppDetail {
        always_on: at(&config, "/properties/alwaysOn")
            .as_bool()
            .unwrap_or(false),
        settings: settings.as_ref().map(settings_of).unwrap_or_default(),
        slots: slots
            .unwrap_or_default()
            .iter()
            .filter_map(|slot| {
                let name = maybe(slot, "/name")?;
                Some(Slot {
                    name: name.rsplit('/').next().unwrap_or(&name).to_string(),
                    state: maybe(slot, "/properties/state").unwrap_or_default(),
                    host: maybe(slot, "/properties/defaultHostName"),
                })
            })
            .collect(),
        inbound_ip: maybe(&found, "/properties/inboundIpAddress"),
        outbound_ips: maybe(&found, "/properties/possibleOutboundIpAddresses")
            .unwrap_or_default()
            .split(',')
            .filter(|address| !address.is_empty())
            .take(8)
            .map(str::to_string)
            .collect(),
        app,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAction {
    #[serde(flatten)]
    pub app: AppRef,
    /// start, stop or restart.
    pub action: String,
}

#[derive(Serialize)]
pub struct Done {
    pub message: String,
}

pub async fn act(input: AppAction) -> AzureResult<Done> {
    let (verb, saying) = match input.action.as_str() {
        "start" => ("start", "starting"),
        "stop" => ("stop", "stopping"),
        "restart" => ("restart", "restarting"),
        other => {
            return Err(AzureError::BadArg(format!(
                "action {other:?}: use start, stop or restart"
            )))
        }
    };
    let name = input.app.name.clone();
    let path = input.app.path().await?;
    client::post(&format!("{path}/{verb}"), WEB_API, &json!({})).await?;
    Ok(Done {
        message: format!("{name} is {saying}"),
    })
}

/// The address the app's log stream is read from, which is the scm host, not the app's own.
pub fn stream_url(host: &str) -> String {
    format!("https://{host}/api/logstream")
}

/// The scm host of an app, which Azure names in its publishing profile and spells
/// predictably from the app's own host.
pub fn scm_host(app: &App) -> Option<String> {
    let host = app.host.as_deref()?;
    let (name, rest) = host.split_once('.')?;
    Some(format!("{name}.scm.{rest}"))
}

/// Follows an app's log stream, sending each line on as it arrives.
pub async fn tail_logs(input: AppRef, sink: &StreamSink) -> AzureResult<()> {
    let detail = app(input).await?;
    let host = scm_host(&detail.app)
        .ok_or_else(|| AzureError::NotFound("this app has no address to read logs from".into()))?;
    let token = crate::auth::token().await?;
    let response = reqwest::Client::builder()
        .user_agent("sikemux-azure/0.1")
        .timeout(STREAM_TIMEOUT)
        .build()
        .map_err(|_| AzureError::Transport("could not start the HTTP client".into()))?
        .request(Method::GET, stream_url(&host))
        .bearer_auth(token)
        .send()
        .await?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return client::checked(
            status,
            serde_json::from_str(&body).unwrap_or(Value::String(body)),
        )
        .map(|_| ());
    }
    let mut stream = response.bytes_stream();
    let mut held = Vec::new();
    while let Some(chunk) = stream.next().await {
        held.extend_from_slice(&chunk?);
        while let Some(end) = held.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = held.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line).trim_end().to_string();
            if !text.is_empty() && sink.send(json!({ "line": text })).is_err() {
                return Ok(());
            }
        }
        if held.len() > MAX_LINE_BYTES {
            held.clear();
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn app_json() -> Value {
        json!({
            "id": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Web/sites/shop",
            "name": "shop", "location": "uaenorth", "kind": "app,linux",
            "tags": { "env": "prod" },
            "properties": {
                "state": "Running", "defaultHostName": "shop.azurewebsites.net", "httpsOnly": true,
                "serverFarmId": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Web/serverfarms/plan-1",
                "siteConfig": { "linuxFxVersion": "NODE|22-lts" },
                "inboundIpAddress": "20.1.2.3",
                "possibleOutboundIpAddresses": "20.1.2.4,20.1.2.5"
            }
        })
    }

    #[test]
    fn an_app_reads_its_state_host_runtime_and_plan() {
        let app = app_of(&app_json()).expect("reads");
        assert_eq!(app.state, "Running");
        assert_eq!(app.host.as_deref(), Some("shop.azurewebsites.net"));
        assert_eq!(app.runtime.as_deref(), Some("NODE|22-lts"));
        assert_eq!(app.plan.as_deref(), Some("plan-1"));
        assert!(app.https_only);
        assert!(!app.functions);
        let function =
            app_of(&json!({ "id": "/s/x", "kind": "functionapp,linux" })).expect("reads");
        assert!(function.functions);
    }

    #[test]
    fn settings_are_read_by_name_only() {
        let settings = settings_of(&json!({ "properties": {
            "DATABASE_URL": "@Microsoft.KeyVault(SecretUri=https://v.vault.azure.net/secrets/db/)",
            "API_URL": "https://api.acme.dev"
        } }));
        assert_eq!(
            settings,
            [
                Setting {
                    name: "API_URL".into(),
                    from_key_vault: false
                },
                Setting {
                    name: "DATABASE_URL".into(),
                    from_key_vault: true
                }
            ]
        );
        let json = serde_json::to_string(&settings).expect("writes");
        assert!(!json.contains("api.acme.dev"));
        assert!(!json.contains("vault.azure.net"));
    }

    #[test]
    fn logs_are_read_from_the_apps_scm_host() {
        let app = app_of(&app_json()).expect("reads");
        assert_eq!(
            scm_host(&app).as_deref(),
            Some("shop.scm.azurewebsites.net")
        );
        assert_eq!(
            stream_url("shop.scm.azurewebsites.net"),
            "https://shop.scm.azurewebsites.net/api/logstream"
        );
        assert_eq!(
            scm_host(&app_of(&json!({ "id": "/s/x" })).expect("reads")),
            None
        );
    }

    #[tokio::test]
    async fn an_unknown_action_is_turned_away_before_any_request() {
        let input = AppAction {
            app: AppRef {
                subscription: Some("s1".into()),
                group: "g".into(),
                name: "shop".into(),
            },
            action: "delete".into(),
        };
        assert!(matches!(act(input).await, Err(AzureError::BadArg(_))));
    }
}
