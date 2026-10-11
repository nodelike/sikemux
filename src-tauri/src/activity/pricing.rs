use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RATES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const RATES_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// After a failed fetch, the page uses the old copy for a while instead of waiting on the network again.
const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;

/// Bare family names could be any generation, and synthetic messages were never billed.
const UNPRICEABLE: [&str; 6] = [
    "<synthetic>",
    "synthetic",
    "opus",
    "sonnet",
    "haiku",
    "fable",
];

/// USD per token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRate {
    pub standard: Rates,
    pub fast: Option<Rates>,
    pub ultrafast: Option<Rates>,
}

impl ModelRate {
    pub fn at(&self, speed: &str) -> Rates {
        match speed {
            "fast" => self.fast,
            "ultrafast" => self.ultrafast,
            _ => None,
        }
        .unwrap_or(self.standard)
    }
}

pub type RateTable = HashMap<String, ModelRate>;

/// A price the person entered, in USD per million tokens.
#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PriceOverride {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl PriceOverride {
    fn rate(&self) -> ModelRate {
        let per_token = |value: f64| value.max(0.0) / 1_000_000.0;
        let rates = Rates {
            input: per_token(self.input),
            output: per_token(self.output),
            cache_read: per_token(self.cache_read.unwrap_or(self.input)),
            cache_write: per_token(self.cache_write.unwrap_or(self.input)),
        };
        ModelRate {
            standard: rates,
            fast: None,
            ultrafast: None,
        }
    }
}

#[derive(Serialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum PricingStatus {
    Fresh,
    Cached,
    #[default]
    Unavailable,
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PricingInfo {
    pub status: PricingStatus,
    pub fetched_at_ms: Option<i64>,
}

pub struct Prices {
    table: Arc<RateTable>,
    overrides: HashMap<String, ModelRate>,
    pub info: PricingInfo,
}

impl Prices {
    pub fn new(
        table: Arc<RateTable>,
        overrides: &HashMap<String, PriceOverride>,
        info: PricingInfo,
    ) -> Self {
        Self {
            table,
            overrides: overrides
                .iter()
                .map(|(model, price)| (normalize(model), price.rate()))
                .collect(),
            info,
        }
    }

    pub fn lookup(&self, model: &str) -> Option<ModelRate> {
        let key = normalize(model);
        if let Some(rate) = self.overrides.get(&key) {
            return Some(*rate);
        }
        let bare = bare_name(&key);
        if bare.is_empty() || UNPRICEABLE.contains(&bare) {
            return None;
        }
        self.table
            .get(&key)
            .or_else(|| self.table.get(bare))
            .copied()
    }
}

/// `claude-fable-5-1[1m]` is the 1M context tier, which the table only lists by its base name.
fn normalize(model: &str) -> String {
    let key = model.trim().to_lowercase();
    match key.find('[') {
        Some(at) => key[..at].to_string(),
        None => key,
    }
}

fn bare_name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

fn number(entry: &Value, key: &str) -> Option<f64> {
    entry
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

/// A tier missing its cache rates keeps the standard tier's ratio to input.
fn read_rates(entry: &Value, suffix: &str, standard: Option<&Rates>) -> Option<Rates> {
    let input = number(entry, &format!("input_cost_per_token{suffix}"))?;
    let output = number(entry, &format!("output_cost_per_token{suffix}"))?;
    let cache = |name: &str, pick: fn(&Rates) -> f64| {
        number(entry, &format!("{name}{suffix}")).unwrap_or_else(|| match standard {
            Some(standard) if standard.input > 0.0 => pick(standard) / standard.input * input,
            _ => input,
        })
    };
    Some(Rates {
        input,
        output,
        cache_read: cache("cache_read_input_token_cost", |rates| rates.cache_read),
        cache_write: cache("cache_creation_input_token_cost", |rates| rates.cache_write),
    })
}

fn scaled(rates: Rates, multiple: f64) -> Rates {
    Rates {
        input: rates.input * multiple,
        output: rates.output * multiple,
        cache_read: rates.cache_read * multiple,
        cache_write: rates.cache_write * multiple,
    }
}

/// Entries without both an input and an output rate are left out: half a price
/// would quietly under-report. A bare name like `gpt-5` also answers for
/// `openai/gpt-5` when every prefixed entry agrees on the rate.
pub fn parse_rate_table(document: &Value) -> RateTable {
    let mut table = RateTable::new();
    let Some(entries) = document.as_object() else {
        return table;
    };
    for (name, entry) in entries {
        let Some(standard) = read_rates(entry, "", None) else {
            continue;
        };
        let key = normalize(name);
        if key.is_empty() {
            continue;
        }
        let multiple = entry
            .pointer("/provider_specific_entry/fast")
            .and_then(Value::as_f64)
            .filter(|multiple| *multiple > 0.0);
        let fast = match multiple {
            Some(multiple) => Some(scaled(standard, multiple)),
            None => read_rates(entry, "_priority", Some(&standard)),
        };
        let ultrafast = read_rates(entry, "_ultrafast", Some(&standard));
        table.insert(
            key,
            ModelRate {
                standard,
                fast,
                ultrafast,
            },
        );
    }
    let mut aliases: HashMap<String, Option<ModelRate>> = HashMap::new();
    for (key, rate) in &table {
        let bare = bare_name(key);
        if bare == key || table.contains_key(bare) {
            continue;
        }
        aliases
            .entry(bare.to_string())
            .and_modify(|held| {
                if held.is_some_and(|held| held != *rate) {
                    *held = None;
                }
            })
            .or_insert(Some(*rate));
    }
    for (alias, rate) in aliases {
        if let Some(rate) = rate {
            table.insert(alias, rate);
        }
    }
    table
}

struct Loaded {
    fetched_at_ms: i64,
    table: Arc<RateTable>,
}

fn loaded() -> &'static Mutex<Option<Loaded>> {
    static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
    &LOADED
}

fn cache_path() -> Option<PathBuf> {
    Some(crate::state::state_path()?.with_file_name("model-prices.json"))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    fetched_at_ms: i64,
    document: Value,
}

fn read_cache() -> Option<Loaded> {
    let file: CacheFile = serde_json::from_slice(&fs::read(cache_path()?).ok()?).ok()?;
    Some(Loaded {
        fetched_at_ms: file.fetched_at_ms,
        table: Arc::new(parse_rate_table(&file.document)),
    })
}

fn write_cache(fetched_at_ms: i64, document: Value) {
    let Some(path) = cache_path() else {
        return;
    };
    let file = CacheFile {
        fetched_at_ms,
        document,
    };
    if let Ok(bytes) = serde_json::to_vec(&file) {
        let partial = path.with_extension("json.partial");
        if fs::write(&partial, bytes).is_ok() {
            let _ = fs::rename(partial, path);
        }
    }
}

async fn fetch() -> Option<Vec<u8>> {
    let response = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?
        .get(RATES_URL)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    Some(response.bytes().await.ok()?.to_vec())
}

fn snapshot() -> Option<(i64, Arc<RateTable>)> {
    loaded()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|loaded| (loaded.fetched_at_ms, loaded.table.clone()))
}

/// Downloads the table and keeps it, in memory and on disk. Parsing a few
/// megabytes of JSON stays off the async threads.
async fn refresh(now_ms: i64) -> Option<(i64, Arc<RateTable>)> {
    static LAST_ATTEMPT_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
    let last = LAST_ATTEMPT_MS.load(Ordering::Relaxed);
    if now_ms - last < RETRY_AFTER_MS
        || LAST_ATTEMPT_MS
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return None;
    }
    let bytes = fetch().await?;
    tauri::async_runtime::spawn_blocking(move || {
        let document: Value = serde_json::from_slice(&bytes).ok()?;
        let table = Arc::new(parse_rate_table(&document));
        if table.is_empty() {
            return None;
        }
        *loaded().lock().unwrap_or_else(|p| p.into_inner()) = Some(Loaded {
            fetched_at_ms: now_ms,
            table: table.clone(),
        });
        write_cache(now_ms, document);
        Some((now_ms, table))
    })
    .await
    .ok()
    .flatten()
}

/// The rate table, refreshed at most once a day. A stale copy answers at once
/// and is refreshed in the background; only a first run with no copy waits.
pub async fn rate_table(now_ms: i64) -> (Arc<RateTable>, PricingInfo) {
    let mut held = snapshot();
    if held.is_none() {
        if let Ok(Some(cached)) = tauri::async_runtime::spawn_blocking(read_cache).await {
            let mut guard = loaded().lock().unwrap_or_else(|p| p.into_inner());
            if guard.is_none() {
                *guard = Some(cached);
            }
            drop(guard);
            held = snapshot();
        }
    }
    let held = match held {
        Some(held) => {
            if now_ms - held.0 >= RATES_TTL_MS {
                tauri::async_runtime::spawn(refresh(now_ms));
            }
            Some(held)
        }
        None => refresh(now_ms).await,
    };
    match held {
        Some((fetched_at_ms, table)) => (
            table,
            PricingInfo {
                status: if now_ms - fetched_at_ms < RATES_TTL_MS {
                    PricingStatus::Fresh
                } else {
                    PricingStatus::Cached
                },
                fetched_at_ms: Some(fetched_at_ms),
            },
        ),
        None => (Arc::default(), PricingInfo::default()),
    }
}
