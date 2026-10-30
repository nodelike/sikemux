//! Adding a provider from its base URL and key. A provider in the catalogue
//! is found by its URL and connected; any other is checked against its model
//! list, asked which other APIs it serves, and kept as one the person added.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::launch::providers::{self, Custom, Kind, Provider, Providers};

use super::{
    check_key, client, entry, listed_models, model_list, on_store, ModelProvider, ProviderParams,
    Unlisted, CHECK_TIMEOUT, RUNNER_TIMEOUT,
};
use crate::server::{Core, CoreResult};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_URL_BYTES: usize = 2048;
const MAX_KEY_BYTES: usize = 4096;
const MAX_LABEL_CHARS: usize = 60;

/// The paths an OpenAI- or Anthropic-compatible API serves under its base URL.
const ENDPOINTS: [&str; 4] = ["/chat/completions", "/responses", "/messages", "/models"];

/// What the person entered for a provider.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entered {
    base_url: String,
    #[serde(default)]
    key: Option<String>,
    /// The name the person gave it, in place of the one read from its URL.
    #[serde(default)]
    label: Option<String>,
}

/// The provider at a base URL, checked with the key entered or saved. Never the key.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reading {
    /// Its name: the catalogue's, the one entered, or one from its domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// The base URL as Sikemux keeps it, without a trailing slash or endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    has_key: bool,
    /// The catalogue's provider at that URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    known_id: Option<String>,
    speaks: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runs_with: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    models: Option<Vec<String>>,
    /// Why it cannot be added as it stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
}

/// The provider entered, the key it came with or was saved with, and what it
/// listed with that key.
struct Examined {
    base_url: Option<String>,
    provider: Option<Arc<Provider>>,
    known: bool,
    entered_key: Option<String>,
    key: Option<String>,
    served: Result<Vec<String>, String>,
}

pub(crate) async fn read(core: &Arc<Core>, entered: Entered) -> CoreResult<Reading> {
    let examined = examine(core, entered).await?;
    let provider = examined.provider.as_deref();
    Ok(Reading {
        label: provider.map(|provider| provider.label.clone()),
        base_url: provider
            .map(|provider| provider.openai.clone())
            .or(examined.base_url),
        has_key: examined.key.is_some(),
        known_id: provider
            .filter(|_| examined.known)
            .map(|provider| provider.id.clone()),
        speaks: provider.map(Provider::speaks).unwrap_or_default(),
        runs_with: provider.and_then(Provider::runs_with),
        models: examined.served.as_ref().ok().cloned(),
        failure: examined.served.err(),
    })
}

/// Connects the catalogue's provider at the URL entered, or keeps a new one with its key.
pub(crate) async fn add(core: &Arc<Core>, entered: Entered) -> CoreResult<ModelProvider> {
    let examined = examine(core, entered).await?;
    let served = examined.served?;
    let provider = examined
        .provider
        .ok_or("Enter the provider's base URL, such as https://api.example.com/v1")?;
    let (known, entered_key, key) = (examined.known, examined.entered_key, examined.key);
    let (provider, feed) = on_store(core, move |store| {
        let provider = match (known, entered_key) {
            (true, Some(key)) if provider.key.is_some() => {
                store.save_key(&provider.id, key)?;
                provider
            }
            (true, _) => provider,
            (false, _) => store.add_custom(Custom::of(&provider), key)?,
        };
        Ok::<_, String>((provider, store.feed()))
    })
    .await??;
    if !known {
        core.workspaces.model_checks.forget(&provider.id);
    }
    Ok(entry(core, provider, true, &feed, Some(served)))
}

pub(crate) async fn remove(core: &Arc<Core>, params: ProviderParams) -> CoreResult<()> {
    let id = params.id;
    let removing = id.clone();
    on_store(core, move |store| store.remove_custom(&removing)).await??;
    core.workspaces.model_checks.forget(&id);
    Ok(())
}

async fn examine(core: &Arc<Core>, entered: Entered) -> CoreResult<Examined> {
    if entered.base_url.len() > MAX_URL_BYTES {
        return Err("That base URL is too long".into());
    }
    let entered_key = entered
        .key
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty());
    if entered_key
        .as_ref()
        .is_some_and(|key| key.len() > MAX_KEY_BYTES)
    {
        return Err("That key is too long".into());
    }
    let label = entered
        .label
        .map(|label| {
            label
                .trim()
                .chars()
                .take(MAX_LABEL_CHARS)
                .collect::<String>()
        })
        .filter(|label| !label.is_empty());
    let Some(base_url) = base_of(&entered.base_url) else {
        return Ok(Examined {
            base_url: None,
            provider: None,
            known: false,
            entered_key,
            key: None,
            served: Err("Enter the provider's base URL, such as https://api.example.com/v1".into()),
        });
    };
    let searching = base_url.clone();
    let entered = entered_key.is_some();
    let (found, saved_key) = on_store(core, move |store| {
        let found = find(store, &searching, label);
        let saved_key = match &found {
            Found::Known(provider) if !entered => store.key(&provider.id),
            Found::New(custom) if !entered => store.key(&custom.id),
            _ => None,
        };
        (found, saved_key)
    })
    .await?;
    let key = entered_key.clone().or(saved_key);
    let (provider, known, served) = match found {
        Found::Known(provider) => {
            let served = serving(&provider, key.as_deref()).await;
            (provider, true, served)
        }
        Found::New(mut custom) => {
            let served = model_list(&custom.base_url, key.as_deref(), CHECK_TIMEOUT)
                .await
                .map_err(|unlisted| unlisted_failure(&custom, key.is_some(), unlisted));
            if served.is_ok() {
                (custom.responses, custom.anthropic) =
                    other_apis(&custom.base_url, key.as_deref()).await;
            }
            custom.keyed = key.is_some();
            (Arc::new(custom.provider()), false, served)
        }
    };
    Ok(Examined {
        base_url: Some(base_url),
        provider: Some(provider),
        known,
        entered_key,
        key,
        served,
    })
}

/// The API's base URL from what was entered: trimmed, without a query, a
/// trailing slash, or an endpoint pasted after it.
fn base_of(entered: &str) -> Option<String> {
    let mut url = Url::parse(entered.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let path = ENDPOINTS
        .into_iter()
        .find_map(|endpoint| path.strip_suffix(endpoint))
        .unwrap_or(path)
        .to_owned();
    url.set_path(&path);
    Some(url.as_str().trim_end_matches('/').to_owned())
}

/// A name for the provider at `base_url`, from its domain: `api.lithosai.cloud` is LithosAI.
fn label_of(base_url: &str) -> String {
    let Ok(url) = Url::parse(base_url) else {
        return base_url.to_owned();
    };
    let host = url.host_str().unwrap_or_default();
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return match url.port() {
            Some(port) => format!("Local {port}"),
            None => "Local".to_owned(),
        };
    }
    let parts: Vec<_> = host.split('.').collect();
    let name = match parts.as_slice() {
        [.., name, _] | [name] => name,
        [] => host,
    };
    let mut label: String = name
        .chars()
        .enumerate()
        .map(|(at, c)| if at == 0 { c.to_ascii_uppercase() } else { c })
        .collect();
    if label.len() > 4 && label.ends_with("ai") {
        label.replace_range(label.len() - 2.., "AI");
    }
    label
}

/// A provider id made from `label`, such as `lithosai`.
fn id_of(label: &str) -> String {
    let mut id = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    match id.trim_end_matches('-') {
        "" => "provider".to_owned(),
        id => id.chars().take(40).collect(),
    }
}

enum Found {
    Known(Arc<Provider>),
    New(Custom),
}

/// The catalogue's provider at `base_url`, else the one the person added
/// there, else a new one named `label` or after its domain.
fn find(store: &Providers, base_url: &str, label: Option<String>) -> Found {
    let at = |url: &str| same_api(base_url, url);
    let known = providers::catalogue().iter().find(|provider| {
        at(&provider.openai) || provider.anthropic.as_ref().is_some_and(|api| at(&api.url))
    });
    if let Some(provider) = known {
        return Found::Known(provider.clone());
    }
    let all = store.all();
    let added = all
        .iter()
        .find(|provider| provider.custom && provider.openai == base_url);
    let (id, label) = match added {
        Some(provider) => (
            provider.id.clone(),
            label.unwrap_or_else(|| provider.label.clone()),
        ),
        None => {
            let label = label.unwrap_or_else(|| label_of(base_url));
            let taken: HashSet<_> = all.iter().map(|provider| provider.id.as_str()).collect();
            let wanted = id_of(&label);
            let id = std::iter::once(wanted.clone())
                .chain((2..).map(|n| format!("{wanted}-{n}")))
                .find(|id| !taken.contains(id.as_str()))
                .unwrap_or(wanted);
            (id, label)
        }
    };
    Found::New(Custom {
        id,
        label,
        base_url: base_url.to_owned(),
        keyed: false,
        responses: false,
        anthropic: None,
    })
}

/// Whether two base URLs are one API: the same host, one path within the other.
fn same_api(entered: &str, url: &str) -> bool {
    let (Ok(entered), Ok(url)) = (Url::parse(entered), Url::parse(url)) else {
        return false;
    };
    let (a, b) = (
        entered.path().trim_end_matches('/'),
        url.path().trim_end_matches('/'),
    );
    entered.host_str() == url.host_str()
        && entered.port_or_known_default() == url.port_or_known_default()
        && (a.starts_with(b) || b.starts_with(a))
}

/// What a catalogue provider serves with `key`, once it takes the key.
async fn serving(provider: &Provider, key: Option<&str>) -> Result<Vec<String>, String> {
    match (provider.kind, key) {
        (Kind::Local, _) => listed_models(provider, None, RUNNER_TIMEOUT)
            .await
            .ok_or_else(|| format!("{} is not running on this host", provider.label)),
        (Kind::Api | Kind::Plan, None) => Err(format!("Enter your {} key", provider.label)),
        (Kind::Api | Kind::Plan, Some(key)) => {
            check_key(provider, key)
                .await
                .map_err(|error| error.to_string())?;
            Ok(listed_models(provider, Some(key), CHECK_TIMEOUT)
                .await
                .unwrap_or_default())
        }
    }
}

fn unlisted_failure(custom: &Custom, keyed: bool, unlisted: Unlisted) -> String {
    let (label, base_url) = (&custom.label, &custom.base_url);
    match unlisted {
        Unlisted::Refused if keyed => format!("{label} did not accept that key"),
        Unlisted::Refused => format!("Enter your {label} key"),
        Unlisted::Unreachable => format!("Could not reach {label} at {base_url}"),
        Unlisted::Answered(status) => format!("{label} answered {status} at {base_url}/models"),
        Unlisted::NotAList => {
            format!("{base_url} does not answer as an OpenAI-compatible API")
        }
    }
}

/// Whether the API at `base_url` also serves OpenAI's Responses API, and the
/// base URL of the Anthropic-compatible API beside it, when it serves one.
async fn other_apis(base_url: &str, key: Option<&str>) -> (bool, Option<String>) {
    let anthropic = base_url.strip_suffix("/v1");
    let (responses, messages) =
        tokio::join!(answers(format!("{base_url}/responses"), key), async {
            match anthropic {
                Some(_) => answers(format!("{base_url}/messages"), key).await,
                None => false,
            }
        });
    (responses, anthropic.filter(|_| messages).map(str::to_owned))
}

/// Whether `url` takes requests: an empty one is refused as malformed, not as missing.
async fn answers(url: String, key: Option<&str>) -> bool {
    let Ok(client) = client() else {
        return false;
    };
    let mut request = client
        .post(url)
        .timeout(PROBE_TIMEOUT)
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({}));
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    request.send().await.is_ok_and(|response| {
        matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        )
    })
}
