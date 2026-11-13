use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct CommandUsed {
    pub id: String,
}

#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTotals {
    pub sessions: i64,
    pub resumed: i64,
    pub turns: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub agent_commits: i64,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub first_at_ms: Option<i64>,
    /// What the tokens would cost at API prices. Subscription plans bill separately.
    pub cost_usd: f64,
    pub costs: TokenCosts,
    /// Tokens with no price: no model was recorded, or no price is known for it.
    pub unpriced_tokens: i64,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    pub day: i64,
    pub sessions: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    /// How full the calendar draws the day for each measure.
    pub levels: ActivityLevels,
}

/// From 0 for nothing to 4, by which quarter of the days with anything the
/// day falls in, so one huge day does not wash out the rest.
#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLevels {
    pub sessions: u8,
    pub agent_ms: u8,
    pub commits: u8,
    pub tokens: u8,
    pub cost_usd: u8,
}

/// Days in a row with any activity. A streak that reached yesterday still
/// counts as current until today is over.
#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStreaks {
    pub active_days: usize,
    pub longest: usize,
    pub current: usize,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityShare {
    pub name: String,
    pub sessions: i64,
    pub agent_ms: i64,
    pub commits: i64,
    pub tokens: i64,
    pub cost_usd: f64,
}

#[derive(Serialize, Deserialize, PartialEq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityModel {
    pub model: String,
    pub agent: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: f64,
    pub costs: TokenCosts,
    pub fast_cost_usd: f64,
    pub priced: bool,
}

/// What each kind of token cost at API prices.
#[derive(Serialize, Deserialize, PartialEq, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct TokenCosts {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl TokenCosts {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummary {
    pub totals: ActivityTotals,
    /// Local calendar days, counted from the Unix epoch, with anything on them.
    pub days: Vec<ActivityDay>,
    pub streaks: ActivityStreaks,
    pub agents: Vec<ActivityShare>,
    pub projects: Vec<ActivityShare>,
    /// Models by cost, then by tokens.
    pub models: Vec<ActivityModel>,
    pub pricing: PricingInfo,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummaryParams {
    /// Minutes the person's clock is ahead of UTC, which places each event on their own day.
    pub utc_offset_minutes: i64,
    pub project: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ActivityPriceParams {
    pub model: String,
    pub price: Option<PriceOverride>,
}

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

/// A price the person entered, in USD per million tokens.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PriceOverride {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl PriceOverride {
    pub fn rate(&self) -> ModelRate {
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

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricingInfo {
    pub fetched_at_ms: Option<i64>,
    /// The person's own prices, by model.
    pub overrides: HashMap<String, PriceOverride>,
}

/// A context this large, read mostly without the cache, is a cold call.
const COLD_CONTEXT: u64 = 100_000;

/// One call to the model, with how much context it carried and how much of that the cache held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCall {
    pub at_ms: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl ModelCall {
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    pub fn cold(&self) -> bool {
        self.context() > COLD_CONTEXT && self.cache_read * 2 < self.context()
    }
}

/// The session was compacted: by its agent as it filled, by the person, or by the core while idle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compaction {
    pub at_ms: u64,
    pub trigger: String,
    pub before: u64,
    pub after: u64,
    pub ms: u64,
    /// What writing the summary cost, when nothing else called the model beside it.
    pub cost: Option<ModelCall>,
}
