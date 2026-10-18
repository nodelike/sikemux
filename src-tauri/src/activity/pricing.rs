use serde::{Deserialize, Serialize};
use sikemux_core::launch::model_feed::{Cost, Model, Snapshot};
use std::collections::HashMap;

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

    /// A speed missing its cache rates keeps the standard speed's ratio to input.
    fn of(model: &Model) -> Option<Self> {
        let standard = rates(model.cost?, None)?;
        let speed = |name| rates(model.cost_at(name)?, Some(&standard));
        Some(Self {
            standard,
            fast: speed("fast"),
            ultrafast: speed("ultrafast"),
        })
    }
}

/// Without both an input and an output rate there is no price: half a price
/// would quietly under-report.
fn rates(cost: Cost, standard: Option<&Rates>) -> Option<Rates> {
    let per_token = |value: f64| value / 1_000_000.0;
    let input = per_token(cost.input?);
    let output = per_token(cost.output?);
    let cache = |rate: Option<f64>, pick: fn(&Rates) -> f64| match (rate, standard) {
        (Some(rate), _) => per_token(rate),
        (None, Some(standard)) if standard.input > 0.0 => pick(standard) / standard.input * input,
        (None, _) => input,
    };
    Some(Rates {
        input,
        output,
        cache_read: cache(cost.cache_read, |rates| rates.cache_read),
        cache_write: cache(cost.cache_write, |rates| rates.cache_write),
    })
}

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

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PricingInfo {
    pub fetched_at_ms: Option<i64>,
}

pub struct Prices {
    table: HashMap<String, ModelRate>,
    overrides: HashMap<String, ModelRate>,
    pub info: PricingInfo,
}

impl Prices {
    pub fn new(feed: &Snapshot, overrides: &HashMap<String, PriceOverride>) -> Self {
        Self {
            table: feed
                .prices()
                .into_iter()
                .filter_map(|(name, model)| Some((name, ModelRate::of(model)?)))
                .collect(),
            overrides: overrides
                .iter()
                .map(|(model, price)| (normalize(model), price.rate()))
                .collect(),
            info: PricingInfo {
                fetched_at_ms: (feed.fetched_at_ms > 0).then_some(feed.fetched_at_ms),
            },
        }
    }

    pub fn lookup(&self, model: &str) -> Option<ModelRate> {
        let key = normalize(model);
        if let Some(rate) = self.overrides.get(&key) {
            return Some(*rate);
        }
        let bare = key.rsplit('/').next().unwrap_or(&key);
        if bare.is_empty() || UNPRICEABLE.contains(&bare) {
            return None;
        }
        self.table
            .get(&key)
            .or_else(|| self.table.get(bare))
            .copied()
    }
}

/// `claude-fable-5-1[1m]` is the 1M context tier, which is priced by its base name.
fn normalize(model: &str) -> String {
    let key = model.trim().to_lowercase();
    match key.find('[') {
        Some(at) => key[..at].to_string(),
        None => key,
    }
}
