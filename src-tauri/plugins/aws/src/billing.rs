// Cost Explorer monthly breakdown.
//
// Two group-by dimensions: RECORD_TYPE + SERVICE. CE applies credits *into*
// each service's row when you group by SERVICE alone, hiding the real Usage
// charge (e.g. Claude Haiku once showed as $0 because its gross usage exactly
// cancelled with its applied credit). Grouping by RECORD_TYPE first keeps
// Usage / Credit / Tax as distinct rows — Usage now matches the Console
// "Cost and usage" widget exactly. Total per month is computed by SUMMING
// the Usage groups, because Cost Explorer's top-level `Total` block returns
// empty when `--group-by` is in play.

use serde::Serialize;

use crate::error::{AwsError, AwsResult};

use crate::common::aws_json;

#[derive(Serialize, Clone)]
pub struct BillingMonth {
    period_start: String,
    period_end: String,
    total: String,
    unit: String,
    is_current: bool,
    by_service: Vec<BillingService>,
}

#[derive(Serialize, Clone)]
pub struct BillingService {
    service: String,
    amount: String,
    unit: String,
}

pub(crate) async fn months(profile: String, months_back: u32) -> AwsResult<Vec<BillingMonth>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| AwsError::Aws(e.to_string()))?
        .as_secs();
    let (cy, cm, cd) = ymd_utc(now);

    let mut start_y = cy;
    let mut start_m = cm as i32 - months_back as i32;
    while start_m < 1 {
        start_m += 12;
        start_y -= 1;
    }
    let start = format!("{:04}-{:02}-01", start_y, start_m);
    let end = format!("{:04}-{:02}-{:02}", cy, cm, cd.min(28) + 1).min({
        let (ny, nm) = if cm == 12 { (cy + 1, 1) } else { (cy, cm + 1) };
        format!("{:04}-{:02}-01", ny, nm)
    });

    let time_period = format!("Start={start},End={end}");
    let resp_v: serde_json::Value = aws_json(
        &profile,
        &[
            "ce",
            "get-cost-and-usage",
            "--time-period",
            &time_period,
            "--granularity",
            "MONTHLY",
            "--metrics",
            "UnblendedCost",
            "--group-by",
            "Type=DIMENSION,Key=RECORD_TYPE",
            "Type=DIMENSION,Key=SERVICE",
            "--output",
            "json",
        ],
    )
    .await?;

    let results = resp_v
        .get("ResultsByTime")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let current_period_start = format!("{:04}-{:02}-01", cy, cm);

    let mut months: Vec<BillingMonth> = Vec::new();
    for r in results {
        let period_start = r
            .get("TimePeriod")
            .and_then(|t| t.get("Start"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let period_end = r
            .get("TimePeriod")
            .and_then(|t| t.get("End"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut by_service: Vec<BillingService> = Vec::new();
        let mut unit = "USD".to_string();
        let mut gross_total_f = 0.0_f64;
        if let Some(groups) = r.get("Groups").and_then(|v| v.as_array()) {
            for g in groups {
                let keys = g.get("Keys").and_then(|k| k.as_array());
                let record_type = keys
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let service = keys
                    .and_then(|a| a.get(1))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                if record_type == "Tax" {
                    continue;
                }
                let amount = g
                    .get("Metrics")
                    .and_then(|m| m.get("UnblendedCost"))
                    .and_then(|m| m.get("Amount"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string();
                let unit2 = g
                    .get("Metrics")
                    .and_then(|m| m.get("UnblendedCost"))
                    .and_then(|m| m.get("Unit"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("USD")
                    .to_string();
                if unit == "USD" {
                    unit = unit2.clone();
                }
                if record_type == "Usage" {
                    gross_total_f += amount.parse::<f64>().unwrap_or(0.0);
                }
                by_service.push(BillingService {
                    service,
                    amount,
                    unit: unit2,
                });
            }
            by_service.sort_by(|a, b| {
                let aa: f64 = a.amount.parse().unwrap_or(0.0);
                let bb: f64 = b.amount.parse().unwrap_or(0.0);
                bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let total = format!("{:.2}", gross_total_f);
        let is_current = period_start == current_period_start;

        months.push(BillingMonth {
            period_start,
            period_end,
            total,
            unit,
            is_current,
            by_service,
        });
    }

    Ok(months)
}

// Cheap UTC YMD from unix seconds — avoids pulling chrono just for this.
// Howard Hinnant's date algorithm.
fn ymd_utc(secs: u64) -> (i32, u32, u32) {
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
