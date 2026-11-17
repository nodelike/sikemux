// What the account spent, from Cost Explorer, grouped by service or another
// dimension and cut to the biggest groups.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{Duration, OffsetDateTime};

use crate::common::{aws_json, cli_value, Target};
use crate::error::AwsResult;

const MAX_DAYS: i64 = 365;
const MAX_GROUPS: usize = 20;
const METRIC: &str = "UnblendedCost";

pub(crate) struct CostQuery<'a> {
    pub(crate) days: Option<i64>,
    pub(crate) daily: bool,
    pub(crate) group_by: Option<&'a str>,
    pub(crate) service: Option<&'a str>,
}

#[derive(Serialize)]
pub struct Costs {
    unit: Option<String>,
    group_by: String,
    periods: Vec<Period>,
}

#[derive(Serialize)]
pub struct Period {
    start: String,
    end: String,
    estimated: bool,
    total: f64,
    /// The biggest first, as name and amount.
    groups: Vec<(String, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    other: Option<f64>,
}

fn cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

/// Spending over the last `days`, ending yesterday, since today's is not in yet.
pub(crate) async fn costs(target: &Target, query: CostQuery<'_>) -> AwsResult<Costs> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Resp {
        results_by_time: Vec<ByTime>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct ByTime {
        time_period: Span,
        #[serde(default)]
        estimated: bool,
        #[serde(default)]
        total: BTreeMap<String, Amount>,
        #[serde(default)]
        groups: Vec<Group>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Span {
        start: String,
        end: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Group {
        keys: Vec<String>,
        metrics: BTreeMap<String, Amount>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Amount {
        amount: String,
        unit: Option<String>,
    }

    let days = query.days.unwrap_or(30).clamp(1, MAX_DAYS);
    let today = OffsetDateTime::now_utc().date();
    let period = format!("Start={},End={today}", today - Duration::days(days));
    let group_by = cli_value(query.group_by.unwrap_or("SERVICE"))?.to_uppercase();
    let grouping = format!("Type=DIMENSION,Key={group_by}");
    let mut args = vec![
        "ce",
        "get-cost-and-usage",
        "--time-period",
        &period,
        "--granularity",
        if query.daily { "DAILY" } else { "MONTHLY" },
        "--metrics",
        METRIC,
        "--group-by",
        &grouping,
    ];
    let filter = query.service.map(|service| {
        json!({ "Dimensions": { "Key": "SERVICE", "Values": [service] } }).to_string()
    });
    if let Some(filter) = &filter {
        args.extend(["--filter", filter]);
    }
    let global = Target {
        region: Some("us-east-1".into()),
        ..target.clone()
    };
    let resp: Resp = aws_json(&global, &args).await?;

    let amount = |metrics: &BTreeMap<String, Amount>| {
        metrics
            .get(METRIC)
            .and_then(|amount| amount.amount.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let mut unit = None;
    let periods = resp
        .results_by_time
        .into_iter()
        .map(|by_time| {
            unit = unit.take().or_else(|| {
                by_time
                    .groups
                    .iter()
                    .find_map(|group| group.metrics.get(METRIC)?.unit.clone())
            });
            let mut groups: Vec<(String, f64)> = by_time
                .groups
                .iter()
                .map(|group| (group.keys.join(" / "), amount(&group.metrics)))
                .filter(|(_, spent)| spent.abs() >= 0.005)
                .collect();
            groups.sort_by(|a, b| b.1.total_cmp(&a.1));
            let total = groups.iter().map(|(_, spent)| spent).sum::<f64>() + amount(&by_time.total);
            let rest: f64 = groups.iter().skip(MAX_GROUPS).map(|(_, spent)| spent).sum();
            let other = (groups.len() > MAX_GROUPS).then(|| cents(rest));
            groups.truncate(MAX_GROUPS);
            Period {
                start: by_time.time_period.start,
                end: by_time.time_period.end,
                estimated: by_time.estimated,
                total: cents(total),
                groups: groups
                    .into_iter()
                    .map(|(key, spent)| (key, cents(spent)))
                    .collect(),
                other,
            }
        })
        .collect();
    Ok(Costs {
        unit,
        group_by,
        periods,
    })
}
