// CloudWatch metric statistics, with the period widened so a reply never
// carries more than a screenful of points.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::{aws_json, cli_value, Target};
use crate::error::{AwsError, AwsResult};
use crate::logs::since_millis;

const MAX_POINTS: u64 = 120;

#[derive(Serialize)]
pub struct MetricSeries {
    namespace: String,
    metric: String,
    stat: String,
    period_seconds: u64,
    unit: Option<String>,
    points: Vec<MetricPoint>,
    note: Option<String>,
}

#[derive(Serialize)]
pub struct MetricPoint {
    at: String,
    value: f64,
}

pub(crate) struct MetricQuery<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) metric: &'a str,
    pub(crate) dimensions: &'a BTreeMap<String, String>,
    pub(crate) stat: Option<&'a str>,
    pub(crate) since: Option<&'a str>,
    pub(crate) period: Option<u64>,
}

/// One statistic of a metric over time, oldest first.
pub(crate) async fn statistics(target: &Target, query: MetricQuery<'_>) -> AwsResult<MetricSeries> {
    let now = SystemTime::now();
    let end_ms = since_millis("0s", now)?;
    let start_ms = since_millis(query.since.unwrap_or("3h"), now)?;
    let span = ((end_ms - start_ms) / 1000).max(60) as u64;
    let fewest = span.div_ceil(MAX_POINTS).div_ceil(60) * 60;
    let period = query.period.unwrap_or(0).max(fewest).max(60);

    let stat = query.stat.unwrap_or("Average");
    let extended = stat.strip_prefix('p').is_some_and(|percent| {
        !percent.is_empty() && percent.chars().all(|c| c.is_ascii_digit() || c == '.')
    });
    if !extended
        && !matches!(
            stat,
            "Average" | "Sum" | "Minimum" | "Maximum" | "SampleCount"
        )
    {
        return Err(AwsError::BadArg(
            "stat is Average, Sum, Minimum, Maximum, SampleCount or a percentile like p99",
        ));
    }
    let dimensions = Value::Array(
        query
            .dimensions
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
    .to_string();
    let (start, end, period_text) = (
        (start_ms / 1000).to_string(),
        (end_ms / 1000).to_string(),
        period.to_string(),
    );
    let args = [
        "cloudwatch",
        "get-metric-statistics",
        "--namespace",
        cli_value(query.namespace)?,
        "--metric-name",
        cli_value(query.metric)?,
        "--dimensions",
        &dimensions,
        "--start-time",
        &start,
        "--end-time",
        &end,
        "--period",
        &period_text,
        if extended {
            "--extended-statistics"
        } else {
            "--statistics"
        },
        cli_value(stat)?,
    ];

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Resp {
        #[serde(default)]
        datapoints: Vec<Value>,
    }
    let resp: Resp = aws_json(target, &args).await?;
    let mut unit = None;
    let mut points: Vec<MetricPoint> = resp
        .datapoints
        .into_iter()
        .filter_map(|point| {
            let value = if extended {
                point.get("ExtendedStatistics")?.get(stat)?.as_f64()?
            } else {
                point.get(stat)?.as_f64()?
            };
            if unit.is_none() {
                unit = point.get("Unit").and_then(Value::as_str).map(String::from);
            }
            Some(MetricPoint {
                at: point.get("Timestamp")?.as_str()?.to_string(),
                value,
            })
        })
        .collect();
    points.sort_by(|a, b| a.at.cmp(&b.at));
    let note = points.is_empty().then(|| {
        "No data points. A metric matches only its exact set of dimensions; check them, the namespace and the region.".to_string()
    });
    Ok(MetricSeries {
        namespace: query.namespace.to_string(),
        metric: query.metric.to_string(),
        stat: stat.to_string(),
        period_seconds: period,
        unit,
        points,
        note,
    })
}
