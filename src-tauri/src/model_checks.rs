//! Whether a model on a provider calls tools the way agents need: one short
//! request offering a single tool, to each API the provider serves it on,
//! kept with its date so it is asked once.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_core::launch::providers::{AnthropicAuth, Provider};

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_KEPT: usize = 200;
/// Room for a model that thinks before it calls, and still a tiny request.
const MAX_TOKENS: u32 = 1024;
const TOOL: &str = "get_weather";
const ASK: &str = "What is the weather in Paris right now? Use the tool.";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub works: bool,
    /// When it was checked, in Unix milliseconds.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Checks by provider, then model.
type Kept = BTreeMap<String, BTreeMap<String, Check>>;

struct Checks {
    path: PathBuf,
    kept: Mutex<Kept>,
    running: Mutex<HashSet<(String, String)>>,
}

static CHECKS: OnceLock<Checks> = OnceLock::new();

pub fn init(data_dir: &Path) {
    let path = data_dir.join("model-checks.json");
    let kept = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let _ = CHECKS.set(Checks {
        path,
        kept: Mutex::new(kept),
        running: Mutex::default(),
    });
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Claude Code's own suffix for a long context, which the APIs do not take.
fn api_model(model: &str) -> &str {
    model.strip_suffix("[1m]").unwrap_or(model)
}

pub fn of(provider: &str) -> BTreeMap<String, Check> {
    CHECKS
        .get()
        .and_then(|checks| lock(&checks.kept).get(provider).cloned())
        .unwrap_or_default()
}

fn keep(provider: &str, model: &str, check: &Check) {
    let Some(checks) = CHECKS.get() else {
        return;
    };
    let mut kept = lock(&checks.kept);
    kept.entry(provider.to_owned())
        .or_default()
        .insert(model.to_owned(), check.clone());
    while kept.values().map(BTreeMap::len).sum::<usize>() > MAX_KEPT {
        let oldest = kept
            .iter()
            .flat_map(|(provider, models)| {
                models
                    .iter()
                    .map(move |(model, check)| (check.at, provider, model))
            })
            .min()
            .map(|(_, provider, model)| (provider.clone(), model.clone()));
        let Some((provider, model)) = oldest else {
            break;
        };
        if let Some(models) = kept.get_mut(&provider) {
            models.remove(&model);
            if models.is_empty() {
                kept.remove(&provider);
            }
        }
    }
    let written = serde_json::to_vec(&*kept)
        .map_err(std::io::Error::from)
        .and_then(|bytes| sikemux_process::files::write_atomic(&checks.path, &bytes, false));
    if let Err(error) = written {
        eprintln!("Sikemux could not keep the tool-call checks: {error}");
    }
}

/// What one API answered: a verdict on the model, or a failure that says
/// nothing about it, such as a key it refused or a provider that was down.
enum Answer {
    Works,
    Fails(String),
}

fn verdict(
    api: &str,
    status: StatusCode,
    body: &Value,
    called: Option<&Value>,
) -> Result<Answer, String> {
    if status.is_success() {
        let arguments = match called {
            None => {
                return Ok(Answer::Fails(format!(
                    "{api}: answered without calling the tool"
                )))
            }
            Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
            Some(arguments) => arguments.clone(),
        };
        return Ok(match arguments["city"].as_str() {
            Some(city) if !city.trim().is_empty() => Answer::Works,
            _ => Answer::Fails(format!(
                "{api}: called the tool with arguments that are not the JSON it asks for"
            )),
        });
    }
    let said = body["error"]["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .or_else(|| body["message"].as_str())
        .unwrap_or_default();
    let said: String = said.chars().take(200).collect();
    match status {
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            Ok(Answer::Fails(
                format!("{api} refused it ({status}) {said}")
                    .trim()
                    .to_owned(),
            ))
        }
        _ => Err(format!("{api} could not be checked right now ({status})")),
    }
}

async fn ask(
    request: RequestBuilder,
    api: &str,
    body: Value,
    called: fn(&Value) -> Option<&Value>,
) -> Result<Answer, String> {
    let response = request
        .timeout(TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                format!("{api} did not answer within {} seconds", TIMEOUT.as_secs())
            } else {
                format!("Could not reach {api}")
            }
        })?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    verdict(api, status, &answer, called(&answer))
}

fn tool_parameters() -> Value {
    json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] })
}

async fn openai(provider: &Provider, key: Option<&str>, model: &str) -> Result<Answer, String> {
    let request = crate::model_providers::client()
        .post(format!("{}/chat/completions", provider.openai))
        .bearer_auth(key.unwrap_or(&provider.id));
    let body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "messages": [{ "role": "user", "content": ASK }],
        "tools": [{ "type": "function", "function": { "name": TOOL, "description": "The weather in a city now", "parameters": tool_parameters() } }],
    });
    ask(request, "OpenAI-compatible API", body, |answer| {
        answer["choices"][0]["message"]["tool_calls"]
            .as_array()?
            .iter()
            .find(|call| call["function"]["name"] == TOOL)
            .map(|call| &call["function"]["arguments"])
    })
    .await
}

async fn anthropic(
    provider: &Provider,
    key: Option<&str>,
    model: &str,
) -> Option<Result<Answer, String>> {
    let api = provider.anthropic.as_ref()?;
    let key = key.unwrap_or(&provider.id);
    let request = crate::model_providers::client()
        .post(format!("{}/v1/messages", api.url))
        .header("anthropic-version", "2023-06-01");
    let request = match api.auth {
        AnthropicAuth::Bearer => request.bearer_auth(key),
        AnthropicAuth::XApiKey => request.header("x-api-key", key),
    };
    let body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "messages": [{ "role": "user", "content": ASK }],
        "tools": [{ "name": TOOL, "description": "The weather in a city now", "input_schema": tool_parameters() }],
    });
    Some(
        ask(request, "Anthropic-compatible API", body, |answer| {
            answer["content"]
                .as_array()?
                .iter()
                .find(|block| block["type"] == "tool_use" && block["name"] == TOOL)
                .map(|block| &block["input"])
        })
        .await,
    )
}

/// Asks the provider now, on every API it serves the model on, and keeps the verdict.
pub async fn run(provider: &Provider, key: Option<&str>, model: &str) -> Result<Check, String> {
    let model = api_model(model);
    let (openai, anthropic) = futures::join!(
        openai(provider, key, model),
        anthropic(provider, key, model)
    );
    let mut fails = Vec::new();
    for answer in std::iter::once(openai).chain(anthropic) {
        if let Answer::Fails(detail) = answer? {
            fails.push(detail);
        }
    }
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default();
    let check = Check {
        works: fails.is_empty(),
        at,
        detail: (!fails.is_empty()).then(|| fails.join("; ")),
    };
    keep(&provider.id, model, &check);
    Ok(check)
}

/// Checks a model the first time a chat runs on it, in the background.
pub fn first_use(provider: &'static Provider, model: &str) {
    let model = api_model(model).to_owned();
    let Some(checks) = CHECKS.get() else {
        return;
    };
    if of(&provider.id).contains_key(&model)
        || !lock(&checks.running).insert((provider.id.clone(), model.clone()))
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let key = match provider.key {
            Some(_) => crate::model_providers::key(&provider.id).await,
            None => None,
        };
        if provider.key.is_none() || key.is_some() {
            if let Err(error) = run(provider, key.as_deref(), &model).await {
                eprintln!(
                    "Sikemux could not check tool calls on {} {model}: {error}",
                    provider.label
                );
            }
        }
        lock(&checks.running).remove(&(provider.id.clone(), model));
    });
}
