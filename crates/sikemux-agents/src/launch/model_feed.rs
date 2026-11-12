//! What models.dev publishes about each provider's models: context, prices,
//! tool calls and release dates. Fetched at most once a day when something
//! asks, trimmed to the providers Sikemux knows, and kept on disk with its
//! date. The `models-dev.json` beside the catalogue is such a copy, shipped
//! so a first run offline still has them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::providers;

const URL: &str = "https://models.dev/api.json";
const FRESH_FOR_MS: i64 = 24 * 60 * 60 * 1000;
const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Kept whatever the catalogue says, to price what Claude Code and Codex report.
const MAKERS: [&str; 2] = ["anthropic", "openai"];
/// The speeds Claude Code and Codex report besides standard.
const SPEEDS: [&str; 2] = ["fast", "ultrafast"];

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub fetched_at_ms: i64,
    providers: BTreeMap<String, Listing>,
}

#[derive(Serialize, Deserialize, Default)]
struct Listing {
    #[serde(default)]
    models: BTreeMap<String, Model>,
}

/// One model as models.dev describes it, with only the fields Sikemux reads.
#[derive(Serialize, Deserialize, Default)]
pub struct Model {
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub release_date: String,
    /// `alpha`, `beta` or `deprecated`; none for a model in general use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Limit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// It reads pictures as well as text; none when the copy did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(default, skip_serializing_if = "Experimental::is_empty")]
    experimental: Experimental,
    #[serde(default, skip_serializing)]
    modalities: Modalities,
    #[serde(skip_serializing)]
    family: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Limit {
    #[serde(default)]
    pub context: u64,
}

/// USD per million tokens.
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
pub struct Cost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

#[derive(Serialize, Deserialize, Default)]
struct Experimental {
    #[serde(default)]
    modes: BTreeMap<String, Mode>,
}

impl Experimental {
    fn is_empty(&self) -> bool {
        self.modes.is_empty()
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Mode {
    cost: Option<Cost>,
}

#[derive(Deserialize, Default)]
struct Modalities {
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

impl Model {
    /// The price at one of [`SPEEDS`].
    pub fn cost_at(&self, speed: &str) -> Option<Cost> {
        self.experimental.modes.get(speed)?.cost
    }

    fn reads_and_writes_text(&self) -> bool {
        let text = |kinds: &[String]| kinds.iter().any(|kind| kind == "text");
        text(&self.modalities.input)
            && text(&self.modalities.output)
            && !self
                .family
                .as_deref()
                .is_some_and(|family| family.contains("embed"))
    }

    fn priced(&self) -> bool {
        self.cost
            .is_some_and(|cost| cost.input.unwrap_or(0.0) > 0.0 || cost.output.unwrap_or(0.0) > 0.0)
    }
}

impl Snapshot {
    /// What models.dev lists for one of its providers.
    pub fn models(&self, models_dev: &str) -> Option<&BTreeMap<String, Model>> {
        self.providers
            .get(models_dev)
            .map(|listing| &listing.models)
    }

    /// Every priced model by its lowercase name, a maker's own price first,
    /// for pricing what an agent reported without saying where it ran.
    pub fn prices(&self) -> HashMap<String, &Model> {
        let mut prices = HashMap::new();
        let makers = MAKERS.iter().filter_map(|id| self.providers.get(*id));
        let others = self
            .providers
            .iter()
            .filter(|(id, _)| !MAKERS.contains(&id.as_str()))
            .map(|(_, listing)| listing);
        for listing in makers.chain(others) {
            for (id, model) in &listing.models {
                if model.priced() {
                    prices.entry(id.to_lowercase()).or_insert(model);
                }
            }
        }
        prices
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// The whole feed cut down to the providers Sikemux knows and the models
    /// that read and write text.
    fn trim(feed: &[u8], fetched_at_ms: i64) -> Option<Self> {
        let all: HashMap<String, Listing> = serde_json::from_slice(feed).ok()?;
        let providers: BTreeMap<String, Listing> = all
            .into_iter()
            .filter(|(id, _)| wanted(id))
            .map(|(id, mut listing)| {
                listing
                    .models
                    .retain(|_, model| model.reads_and_writes_text());
                for model in listing.models.values_mut() {
                    model.images = Some(model.modalities.input.iter().any(|kind| kind == "image"));
                    model.experimental.modes.retain(|speed, mode| {
                        SPEEDS.contains(&speed.as_str()) && mode.cost.is_some()
                    });
                }
                (id, listing)
            })
            .collect();
        (!providers.is_empty()).then_some(Self {
            fetched_at_ms,
            providers,
        })
    }
}

fn wanted(id: &str) -> bool {
    MAKERS.contains(&id)
        || providers::catalogue()
            .iter()
            .any(|provider| provider.models_dev.as_deref() == Some(id))
}

fn embedded() -> Snapshot {
    Snapshot::parse(include_bytes!("../../models-dev.json")).unwrap_or_default()
}

#[derive(Default)]
struct Held {
    snapshot: Option<Arc<Snapshot>>,
    tried_at_ms: i64,
}

pub struct Feed {
    path: PathBuf,
    held: Arc<Mutex<Held>>,
}

impl Feed {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("models-dev.json"),
            held: Arc::default(),
        }
    }

    fn read(&self) -> Option<Snapshot> {
        Snapshot::parse(&std::fs::read(&self.path).ok()?)
    }

    /// What is known now. A copy a day old is replaced in the background, so
    /// this never waits on the network; it reads the disk, so it blocks.
    pub fn current(&self) -> Arc<Snapshot> {
        let now = epoch_ms() as i64;
        let mut held = self.held.locked();
        let snapshot = match &held.snapshot {
            Some(snapshot) => snapshot.clone(),
            None => {
                let shipped = embedded();
                let kept = self.read();
                Arc::new(
                    kept.filter(|kept| kept.fetched_at_ms > shipped.fetched_at_ms)
                        .unwrap_or(shipped),
                )
            }
        };
        held.snapshot = Some(snapshot.clone());
        if now - snapshot.fetched_at_ms < FRESH_FOR_MS || now - held.tried_at_ms < RETRY_AFTER_MS {
            return snapshot;
        }
        held.tried_at_ms = now;
        if let Some(newer) = self
            .read()
            .filter(|read| now - read.fetched_at_ms < FRESH_FOR_MS)
        {
            let newer = Arc::new(newer);
            held.snapshot = Some(newer.clone());
            return newer;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(refresh(self.path.clone(), self.held.clone()));
        }
        snapshot
    }
}

async fn refresh(path: PathBuf, held: Arc<Mutex<Held>>) {
    let Some(feed) = download().await else {
        return;
    };
    let trimmed = tokio::task::spawn_blocking(move || {
        let snapshot = Snapshot::trim(&feed, epoch_ms() as i64)?;
        if let Ok(bytes) = serde_json::to_vec(&snapshot) {
            let _ = sikemux_process::files::write_atomic(&path, &bytes, false);
        }
        Some(snapshot)
    })
    .await;
    if let Ok(Some(snapshot)) = trimmed {
        held.locked().snapshot = Some(Arc::new(snapshot));
    }
}

async fn download() -> Option<Vec<u8>> {
    let client = crate::accounts::client().ok()?;
    let response = client
        .get(URL)
        .timeout(TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    Some(response.bytes().await.ok()?.to_vec())
}
