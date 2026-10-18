//! Where agents get models besides their makers' own accounts: pay-per-token APIs,
//! coding plans and runners on this computer. The catalogue in `model-providers.json`
//! holds every provider's facts. Connecting a keyed provider saves its key in the
//! Keychain, hands it to OpenCode, Pi and OMP as they start, and lets Claude Code
//! and Codex chats run on it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sikemux_core::launch::model_feed::Snapshot;
use sikemux_core::launch::providers::{
    self, Helper, KeySpec, Kind, Provider, Providers, Wiring, KEY_SERVICE, ROUTED_AGENTS,
};
use sikemux_core::protocol::ChatInfo;

use crate::model_checks::{self, Check};

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const RUNNER_TIMEOUT: Duration = Duration::from_secs(2);

fn keyed(id: &str) -> Result<(&'static Provider, &'static KeySpec), String> {
    let provider = providers::find(id)
        .ok_or_else(|| format!("Sikemux does not know a provider called {id}"))?;
    let key = provider
        .key
        .as_ref()
        .ok_or_else(|| format!("{} runs on this computer and needs no key", provider.label))?;
    Ok((provider, key))
}

#[derive(Serialize)]
pub struct ModelProvider {
    #[serde(flatten)]
    provider: &'static Provider,
    /// A keyed provider has its key saved; a local runner is answering.
    connected: bool,
    /// What a running local runner has installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    models: Option<Vec<String>>,
    /// The agents Sikemux can run on it in place of their makers' accounts.
    agents: Vec<&'static str>,
    /// Whether each model checked so far calls tools the way agents need.
    checks: BTreeMap<String, Check>,
    #[serde(flatten)]
    choices: Choices,
}

/// The models a provider serves, with what it starts chats on.
#[derive(Serialize)]
pub struct ServedModels {
    models: Vec<String>,
    #[serde(flatten)]
    choices: Choices,
}

#[derive(Serialize)]
pub struct Choices {
    /// The coding model it starts helpers and checks on.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// The model each agent's chat starts on.
    defaults: BTreeMap<&'static str, String>,
    /// What models.dev says of these models, by API name.
    facts: BTreeMap<String, ModelFacts>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelFacts {
    context: u64,
    /// USD per million tokens; a plan's models cost nothing more.
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<f64>,
    /// Only a hint: a check is the final word.
    tool_call: bool,
}

/// Claude Code's own suffix for a long context, which the APIs do not take.
fn api_name(model: &str) -> &str {
    model.strip_suffix("[1m]").unwrap_or(model)
}

fn choices(provider: &Provider, feed: &Snapshot, served: Option<&[String]>) -> Choices {
    let model = provider.coding_model("", feed, served);
    let defaults: BTreeMap<_, _> = ROUTED_AGENTS
        .iter()
        .filter(|agent| provider.routes(agent))
        .filter_map(|agent| Some((*agent, provider.coding_model(agent, feed, served)?)))
        .collect();
    let listed = provider
        .models_dev
        .as_deref()
        .and_then(|id| feed.models(id));
    let named = served
        .map(|served| served.iter().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            model
                .iter()
                .chain(defaults.values())
                .map(|model| api_name(model))
                .collect()
        });
    let facts = named
        .into_iter()
        .filter_map(|name| {
            let model = listed?.get(name)?;
            let cost = model.cost.unwrap_or_default();
            let facts = ModelFacts {
                context: model.limit.context,
                input: cost.input,
                output: cost.output,
                tool_call: model.tool_call,
            };
            Some((name.to_owned(), facts))
        })
        .collect();
    Choices {
        model,
        defaults,
        facts,
    }
}

static STORE: OnceLock<Providers> = OnceLock::new();

/// What models.dev says of every provider's models. Reads the disk, so it blocks.
pub fn feed() -> Arc<Snapshot> {
    store().map(Providers::feed).unwrap_or_default()
}

pub fn init(data_dir: &Path) {
    let _ = STORE.set(Providers::new(data_dir));
    model_checks::init(data_dir);
}

fn store() -> Result<&'static Providers, String> {
    STORE
        .get()
        .ok_or_else(|| "Model providers are not ready yet".to_string())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce(&'static Providers) -> T + Send + 'static,
) -> Result<T, String> {
    let store = store()?;
    tokio::task::spawn_blocking(move || work(store))
        .await
        .map_err(|error| error.to_string())
}

/// The keys `agent` should start with, under the names it looks for.
pub async fn environment(agent: &str) -> BTreeMap<String, String> {
    let agent = agent.to_owned();
    blocking(move |store| store.environment(&agent))
        .await
        .unwrap_or_default()
}

/// The key of a connected provider.
pub(crate) async fn key(id: &str) -> Option<String> {
    let id = id.to_owned();
    blocking(move |store| store.key(&id)).await.ok().flatten()
}

/// What a chat of `agent` starts with, on `provider` when one is named. A
/// model first used there has its tool calls checked.
pub async fn wiring(
    agent: &str,
    provider: Option<String>,
    model: Option<String>,
) -> Result<Wiring, String> {
    let agent = agent.to_owned();
    let (wiring, first) = blocking(move |store| {
        let model = model.or_else(|| store.coding_model(provider.as_deref()?, &agent));
        let wiring = store.wiring(&agent, provider.as_deref(), model.as_deref())?;
        let routed = provider.as_deref().and_then(providers::find);
        Ok::<_, String>((wiring, routed.zip(model)))
    })
    .await??;
    if let Some((routed, model)) = first {
        model_checks::first_use(routed, &model);
    }
    Ok(wiring)
}

/// A chat the core started on a provider has its model's tool calls checked once.
pub fn chat_begun(chat: &ChatInfo) {
    let provider = chat.model_provider.as_deref().and_then(providers::find);
    if let (Some(provider), Some(model)) = (provider, chat.model.as_deref()) {
        model_checks::first_use(provider, model);
    }
}

pub(crate) fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(CHECK_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

fn check_failure(label: &str, status: StatusCode) -> Option<String> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Some(format!("{label} did not accept that key"))
        }
        status if status.is_success() => None,
        status => Some(format!(
            "{label} could not check the key right now ({status})"
        )),
    }
}

async fn check_key(provider: &Provider, spec: &KeySpec, key: &str) -> Result<(), String> {
    let response = client()
        .get(&spec.check)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| format!("Could not reach {} to check the key", provider.label))?;
    match check_failure(&provider.label, response.status()) {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ListedModel>,
}

#[derive(Deserialize)]
struct ListedModel {
    id: String,
}

/// What the provider serves, by its OpenAI-compatible model list. `None` when
/// it does not answer.
async fn listed_models(
    provider: &Provider,
    key: Option<&str>,
    timeout: Duration,
) -> Option<Vec<String>> {
    let mut request = client()
        .get(format!("{}/models", provider.openai))
        .timeout(timeout);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.ok()?.error_for_status().ok()?;
    let list: ModelList = response.json().await.ok()?;
    Some(list.data.into_iter().map(|model| model.id).collect())
}

/// Local runners are asked what they have on every call, so only when settings ask.
#[tauri::command]
pub async fn model_providers() -> Vec<ModelProvider> {
    let (saved, feed) = blocking(|store| (store.connected(), store.feed()))
        .await
        .unwrap_or_default();
    futures::future::join_all(providers::catalogue().iter().map(|provider| {
        let (saved, feed) = (&saved, &feed);
        async move {
            let models = match provider.kind {
                Kind::Local => listed_models(provider, None, RUNNER_TIMEOUT).await,
                Kind::Api | Kind::Plan => None,
            };
            ModelProvider {
                provider,
                connected: models.is_some() || saved.contains(&provider.id),
                choices: choices(provider, feed, models.as_deref()),
                models,
                agents: ROUTED_AGENTS
                    .iter()
                    .copied()
                    .filter(|agent| provider.routes(agent))
                    .collect(),
                checks: model_checks::of(&provider.id),
            }
        }
    }))
    .await
}

/// The models a connected provider serves, read with its key.
#[tauri::command]
pub async fn model_provider_models(id: String) -> Result<ServedModels, String> {
    let (provider, _) = keyed(&id)?;
    let (key, feed) = blocking(move |store| (store.key(&id), store.feed())).await?;
    let key = key.ok_or_else(|| format!("Connect {} first", provider.label))?;
    let models = listed_models(provider, Some(&key), CHECK_TIMEOUT)
        .await
        .ok_or_else(|| format!("{} did not list its models", provider.label))?;
    Ok(ServedModels {
        choices: choices(provider, &feed, Some(&models)),
        models,
    })
}

/// Asks the provider whether `model` calls tools the way agents need, now.
#[tauri::command]
pub async fn model_provider_check(id: String, model: String) -> Result<Check, String> {
    let provider = providers::find(&id)
        .ok_or_else(|| format!("Sikemux does not know a provider called {id}"))?;
    let key = match provider.key {
        Some(_) => Some(
            key(&id)
                .await
                .ok_or_else(|| format!("Connect {} first", provider.label))?,
        ),
        None => None,
    };
    model_checks::run(provider, key.as_deref(), model.trim()).await
}

/// What helpers run on: workers started without a model named, and Claude Code's subagents.
#[tauri::command]
pub async fn model_helper() -> Result<Option<Helper>, String> {
    blocking(Providers::helper).await
}

#[tauri::command]
pub async fn model_helper_set(helper: Option<Helper>) -> Result<(), String> {
    blocking(move |store| store.set_helper(helper.as_ref())).await?
}

/// Saves the key once the provider accepts it. Agents already running keep
/// the environment they started with.
#[tauri::command]
pub async fn model_provider_connect(id: String, key: String) -> Result<(), String> {
    let (provider, spec) = keyed(&id)?;
    let store = store()?;
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the key first".into());
    }
    check_key(provider, spec, &key).await?;
    let saved = key.clone();
    tokio::task::spawn_blocking(move || sikemux_keychain::write(KEY_SERVICE, &provider.id, &saved))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    store.remember_key(&provider.id, Some(key));
    store.set_connected(&provider.id, true)
}

#[tauri::command]
pub async fn model_provider_disconnect(id: String) -> Result<(), String> {
    let (provider, _) = keyed(&id)?;
    let store = store()?;
    tokio::task::spawn_blocking(move || sikemux_keychain::delete(KEY_SERVICE, &provider.id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    store.remember_key(&provider.id, None);
    store.set_connected(&provider.id, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejected_key_is_told_apart_from_a_provider_that_did_not_answer() {
        assert_eq!(check_failure("Baseten", StatusCode::OK), None);
        assert_eq!(
            check_failure("Baseten", StatusCode::FORBIDDEN).as_deref(),
            Some("Baseten did not accept that key")
        );
        assert_eq!(
            check_failure("Groq", StatusCode::UNAUTHORIZED).as_deref(),
            Some("Groq did not accept that key")
        );
        assert!(check_failure("Groq", StatusCode::BAD_GATEWAY)
            .is_some_and(|failure| failure.contains("could not check")));
    }
}
