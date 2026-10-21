//! Where agents get models besides their makers' own accounts: pay-per-token
//! APIs, coding plans and runners on this host. The catalogue in
//! `model-providers.json` holds every provider's facts. Connecting a keyed
//! provider saves its key in the host's Keychain, hands it to OpenCode, Pi and
//! OMP as they start, and lets Claude Code and Codex chats run on it.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::launch::model_feed::Snapshot;
use crate::launch::providers::{self, Helper, KeySpec, Kind, Provider, Providers, KEY_SERVICE};
use crate::protocol::ChatLaunch;

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};

pub(crate) mod checks;

use checks::{api_model, Check};

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const RUNNER_TIMEOUT: Duration = Duration::from_secs(2);

fn keyed(id: &str) -> CoreResult<(&'static Provider, &'static KeySpec)> {
    let provider = known(id)?;
    let key = provider.key.as_ref().ok_or_else(|| {
        CoreError::from(format!(
            "{} runs on this host and needs no key",
            provider.label
        ))
    })?;
    Ok((provider, key))
}

fn known(id: &str) -> CoreResult<&'static Provider> {
    providers::find(id)
        .ok_or_else(|| CoreError::from(format!("Sikemux does not know a provider called {id}")))
}

#[derive(Serialize)]
pub(crate) struct ModelProvider {
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
pub(crate) struct ServedModels {
    models: Vec<String>,
    #[serde(flatten)]
    choices: Choices,
}

#[derive(Serialize)]
struct Choices {
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

fn choices(provider: &Provider, feed: &Snapshot, served: Option<&[String]>) -> Choices {
    let model = provider.coding_model("", feed, served);
    let defaults: BTreeMap<_, _> = providers::routed_agents()
        .filter(|agent| provider.routes(agent))
        .filter_map(|agent| Some((agent, provider.coding_model(agent, feed, served)?)))
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
                .map(|model| api_model(model))
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

fn store(core: &Core) -> CoreResult<&Providers> {
    core.workspaces
        .providers()
        .ok_or_else(|| CoreError::from("Sikemux keeps no provider keys here"))
}

/// `work` on the host's providers, off the async threads: they read the disk and the Keychain.
async fn on_store<T: Send + 'static>(
    core: &Arc<Core>,
    work: impl FnOnce(&Providers) -> T + Send + 'static,
) -> CoreResult<T> {
    store(core)?;
    let core = core.clone();
    blocking(move || Ok(work(store(&core)?))).await
}

/// The key of a connected provider.
pub(crate) async fn key(core: &Arc<Core>, id: &str) -> Option<String> {
    let id = id.to_owned();
    on_store(core, move |store| store.key(&id))
        .await
        .ok()
        .flatten()
}

/// A chat started on a provider has its model's tool calls checked once.
pub(crate) fn chat_begun(core: &Arc<Core>, launch: &ChatLaunch) {
    let routed = launch
        .env
        .get(providers::ROUTED_ENV)
        .and_then(|id| providers::find(id));
    if let (Some(provider), Some(model)) = (routed, launch.model.as_deref()) {
        checks::first_use(core, provider, model);
    }
}

fn client() -> &'static Client {
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

async fn check_key(provider: &Provider, spec: &KeySpec, key: &str) -> CoreResult<()> {
    let response = client()
        .get(&spec.check)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| format!("Could not reach {} to check the key", provider.label))?;
    match check_failure(&provider.label, response.status()) {
        Some(failure) => Err(failure.into()),
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

/// Every provider, and whether it is connected. Runners on this host are
/// asked what they have on every call, so only when settings ask.
pub(crate) async fn list(core: &Arc<Core>) -> CoreResult<Vec<ModelProvider>> {
    let (saved, feed) = on_store(core, |store| (store.connected(), store.feed())).await?;
    let asking: Vec<_> = providers::catalogue()
        .iter()
        .map(|provider| {
            tokio::spawn(async move {
                match provider.kind {
                    Kind::Local => listed_models(provider, None, RUNNER_TIMEOUT).await,
                    Kind::Api | Kind::Plan => None,
                }
            })
        })
        .collect();
    let mut listed = Vec::with_capacity(asking.len());
    for (provider, asked) in providers::catalogue().iter().zip(asking) {
        let models = asked.await.ok().flatten();
        listed.push(ModelProvider {
            provider,
            connected: models.is_some() || saved.contains(&provider.id),
            choices: choices(provider, &feed, models.as_deref()),
            models,
            agents: providers::routed_agents()
                .filter(|agent| provider.routes(agent))
                .collect(),
            checks: core.workspaces.model_checks.of(&provider.id),
        });
    }
    Ok(listed)
}

#[derive(Deserialize)]
pub(crate) struct ProviderParams {
    id: String,
}

/// The models a connected provider serves, read with its key.
pub(crate) async fn models(core: &Arc<Core>, params: ProviderParams) -> CoreResult<ServedModels> {
    let (provider, _) = keyed(&params.id)?;
    let id = params.id;
    let (key, feed) = on_store(core, move |store| (store.key(&id), store.feed())).await?;
    let key = key.ok_or_else(|| format!("Connect {} first", provider.label))?;
    let models = listed_models(provider, Some(&key), CHECK_TIMEOUT)
        .await
        .ok_or_else(|| format!("{} did not list its models", provider.label))?;
    Ok(ServedModels {
        choices: choices(provider, &feed, Some(&models)),
        models,
    })
}

#[derive(Deserialize)]
pub(crate) struct CheckParams {
    id: String,
    model: String,
}

/// Asks the provider whether `model` calls tools the way agents need, now.
pub(crate) async fn check(core: &Arc<Core>, params: CheckParams) -> CoreResult<Check> {
    let provider = known(&params.id)?;
    let key = match provider.key {
        Some(_) => Some(
            key(core, &params.id)
                .await
                .ok_or_else(|| format!("Connect {} first", provider.label))?,
        ),
        None => None,
    };
    Ok(core
        .workspaces
        .model_checks
        .run(provider, key.as_deref(), params.model.trim())
        .await?)
}

/// What helpers run on: workers started without a model named, and Claude Code's subagents.
pub(crate) async fn helper(core: &Arc<Core>) -> CoreResult<Option<Helper>> {
    on_store(core, Providers::helper).await
}

#[derive(Deserialize)]
pub(crate) struct HelperParams {
    helper: Option<Helper>,
}

pub(crate) async fn set_helper(core: &Arc<Core>, params: HelperParams) -> CoreResult<()> {
    Ok(on_store(core, move |store| store.set_helper(params.helper.as_ref())).await??)
}

#[derive(Deserialize)]
pub(crate) struct ConnectParams {
    id: String,
    key: String,
}

/// Saves the key once the provider accepts it. Agents already running keep
/// the environment they started with.
pub(crate) async fn connect(core: &Arc<Core>, params: ConnectParams) -> CoreResult<()> {
    let (provider, spec) = keyed(&params.id)?;
    let key = params.key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the key first".into());
    }
    check_key(provider, spec, &key).await?;
    Ok(on_store(core, move |store| {
        sikemux_keychain::write(KEY_SERVICE, &provider.id, &key)
            .map_err(|error| error.to_string())?;
        store.remember_key(&provider.id, Some(key));
        store.set_connected(&provider.id, true)
    })
    .await??)
}

pub(crate) async fn disconnect(core: &Arc<Core>, params: ProviderParams) -> CoreResult<()> {
    let (provider, _) = keyed(&params.id)?;
    Ok(on_store(core, move |store| {
        sikemux_keychain::delete(KEY_SERVICE, &provider.id).map_err(|error| error.to_string())?;
        store.remember_key(&provider.id, None);
        store.set_connected(&provider.id, false)
    })
    .await??)
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
