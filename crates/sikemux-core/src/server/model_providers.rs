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
use crate::launch::providers::{self, Helper, Kind, Provider, Providers, KEY_SERVICE};
use crate::protocol::ChatLaunch;

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};

mod adding;
pub(crate) mod checks;

pub(crate) use adding::{add, read, remove};

use checks::{api_model, Check};

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const RUNNER_TIMEOUT: Duration = Duration::from_secs(2);

fn keyed(core: &Core, id: &str) -> CoreResult<Arc<Provider>> {
    let provider = known(core, id)?;
    if provider.key.is_none() {
        return Err(format!("{} runs on this host and needs no key", provider.label).into());
    }
    Ok(provider)
}

fn known(core: &Core, id: &str) -> CoreResult<Arc<Provider>> {
    store(core)?
        .find(id)
        .ok_or_else(|| CoreError::from(format!("Sikemux does not know a provider called {id}")))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelProvider {
    #[serde(flatten)]
    provider: Arc<Provider>,
    /// A keyed provider has its key saved; a local runner is answering.
    connected: bool,
    /// What a running local runner has installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    models: Option<Vec<String>>,
    /// The agents Sikemux can run on it in place of their makers' accounts.
    agents: Vec<&'static str>,
    /// The agent Sikemux runs it with.
    runs_with: Option<&'static str>,
    speaks: Vec<&'static str>,
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

fn entry(
    core: &Core,
    provider: Arc<Provider>,
    connected: bool,
    feed: &Snapshot,
    models: Option<Vec<String>>,
) -> ModelProvider {
    ModelProvider {
        connected,
        choices: choices(&provider, feed, models.as_deref()),
        models,
        agents: providers::routed_agents()
            .filter(|agent| provider.routes(agent))
            .collect(),
        runs_with: provider.runs_with(),
        speaks: provider.speaks(),
        checks: core.workspaces.model_checks.of(&provider.id),
        provider,
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
        .model_provider
        .as_deref()
        .and_then(|id| store(core).ok()?.find(id));
    if let (Some(provider), Some(model)) = (routed, launch.model.as_deref()) {
        let model = provider.api_model(&launch.provider, model).to_owned();
        checks::first_use(core, provider, &model);
    }
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    crate::http::shared(&CLIENT, || crate::http::builder().timeout(CHECK_TIMEOUT))
        .map_err(|error| format!("no HTTP client: {error}"))
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

async fn check_key(provider: &Provider, key: &str) -> CoreResult<()> {
    let spec = provider
        .key
        .as_ref()
        .ok_or_else(|| format!("{} needs no key", provider.label))?;
    let response = client()?
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

/// Why an API at a base URL listed no models.
enum Unlisted {
    Refused,
    Unreachable,
    Answered(StatusCode),
    NotAList,
}

/// What the OpenAI-compatible API at `base_url` lists.
async fn model_list(
    base_url: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>, Unlisted> {
    let mut request = client()
        .map_err(|_| Unlisted::Unreachable)?
        .get(format!("{base_url}/models"))
        .timeout(timeout);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|_| Unlisted::Unreachable)?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(Unlisted::Refused),
        status if !status.is_success() => return Err(Unlisted::Answered(status)),
        _ => {}
    }
    let list: ModelList = response.json().await.map_err(|_| Unlisted::NotAList)?;
    Ok(list.data.into_iter().map(|model| model.id).collect())
}

/// What the provider serves, by its OpenAI-compatible model list. `None` when
/// it does not answer.
async fn listed_models(
    provider: &Provider,
    key: Option<&str>,
    timeout: Duration,
) -> Option<Vec<String>> {
    model_list(&provider.openai, key, timeout).await.ok()
}

/// Every provider, and whether it is connected. Runners on this host are
/// asked what they have on every call, so only when settings ask.
pub(crate) async fn list(core: &Arc<Core>) -> CoreResult<Vec<ModelProvider>> {
    let (all, saved, feed) =
        on_store(core, |store| (store.all(), store.connected(), store.feed())).await?;
    let asking: Vec<_> = all
        .iter()
        .map(|provider| {
            let provider = provider.clone();
            tokio::spawn(async move {
                match provider.kind {
                    Kind::Local => listed_models(&provider, None, RUNNER_TIMEOUT).await,
                    Kind::Api | Kind::Plan => None,
                }
            })
        })
        .collect();
    let mut listed = Vec::with_capacity(asking.len());
    for (provider, asked) in all.into_iter().zip(asking) {
        let models = asked.await.ok().flatten();
        let connected = models.is_some() || provider.custom || saved.contains(&provider.id);
        listed.push(entry(core, provider, connected, &feed, models));
    }
    Ok(listed)
}

#[derive(Deserialize)]
pub(crate) struct ProviderParams {
    id: String,
}

/// The models a connected provider serves, read with its key.
pub(crate) async fn models(core: &Arc<Core>, params: ProviderParams) -> CoreResult<ServedModels> {
    let provider = keyed(core, &params.id)?;
    let id = params.id;
    let (key, feed) = on_store(core, move |store| (store.key(&id), store.feed())).await?;
    let key = key.ok_or_else(|| format!("Connect {} first", provider.label))?;
    let models = listed_models(&provider, Some(&key), CHECK_TIMEOUT)
        .await
        .ok_or_else(|| format!("{} did not list its models", provider.label))?;
    Ok(ServedModels {
        choices: choices(&provider, &feed, Some(&models)),
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
    let provider = known(core, &params.id)?;
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
        .run(&provider, key.as_deref(), params.model.trim())
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
    let provider = keyed(core, &params.id)?;
    if provider.custom {
        return Err(format!("{} keeps the key it was added with", provider.label).into());
    }
    let key = params.key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the key first".into());
    }
    check_key(&provider, &key).await?;
    Ok(on_store(core, move |store| store.save_key(&provider.id, key)).await??)
}

pub(crate) async fn disconnect(core: &Arc<Core>, params: ProviderParams) -> CoreResult<()> {
    let provider = keyed(core, &params.id)?;
    if provider.custom {
        return Err(format!("Remove {} rather than disconnect it", provider.label).into());
    }
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
