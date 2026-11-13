use crate::launch::model_feed::{Cost, Model, Snapshot};
pub use sikemux_calls::activity_log::{ModelRate, PriceOverride, PricingInfo, Rates};
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

pub(crate) trait ModelRateExt: Sized {
    fn at(&self, speed: &str) -> Rates;

    /// A speed missing its cache rates keeps the standard speed's ratio to input.
    fn of(model: &Model) -> Option<Self>;
}

impl ModelRateExt for ModelRate {
    fn at(&self, speed: &str) -> Rates {
        match speed {
            "fast" => self.fast,
            "ultrafast" => self.ultrafast,
            _ => None,
        }
        .unwrap_or(self.standard)
    }

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

pub(crate) struct Prices {
    table: HashMap<String, ModelRate>,
    overrides: HashMap<String, ModelRate>,
    pub info: PricingInfo,
}

impl Prices {
    pub(crate) fn new(feed: &Snapshot, overrides: &HashMap<String, PriceOverride>) -> Self {
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
                overrides: HashMap::new(),
            },
        }
    }

    pub(crate) fn lookup(&self, model: &str) -> Option<ModelRate> {
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
