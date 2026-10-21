// Billing: whether the project bills, to which account, and that account's
// budgets. Reading the account and its budgets needs the Billing Viewer
// role, so without it the answer says only what the project can.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, short, text};
use crate::error::GcpResult;

const API: &str = "https://cloudbilling.googleapis.com/v1";
const BUDGETS: &str = "https://billingbudgets.googleapis.com/v1";

/// A budget as the dashboard reads it: its amount and the thresholds it alerts at.
pub fn budget(value: &Value) -> Value {
    let amount = at(value, "/amount/specifiedAmount");
    let units = at(amount, "/units")
        .as_str()
        .and_then(|units| units.parse::<f64>().ok())
        .or_else(|| at(amount, "/units").as_f64());
    json!({
        "name": at(value, "/displayName"),
        "amount": units,
        "currency": at(amount, "/currencyCode"),
        "lastPeriod": value.pointer("/amount/lastPeriodAmount").is_some(),
        "thresholds": at(value, "/thresholdRules").as_array().into_iter().flatten().filter_map(|rule| at(rule, "/thresholdPercent").as_f64()).map(|percent| (percent * 100.0).round()).collect::<Vec<_>>(),
        "projects": at(value, "/budgetFilter/projects").as_array().map(|projects| projects.len()),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Whether the project bills, the account it bills to, and that account's budgets when they can be read.
pub async fn billing(input: Listing) -> GcpResult<Value> {
    let project = input.target.project()?;
    let info = client::get(
        &input.target,
        &format!("{API}/projects/{project}/billingInfo"),
        &[],
    )
    .await?;
    let account = text(&info, "/billingAccountName");
    let id = short(&account);
    let (details, budgets) = if id.is_empty() {
        (Value::Null, Value::Null)
    } else {
        let (details, budgets) = futures::future::join(
            client::get(&input.target, &format!("{API}/billingAccounts/{id}"), &[]),
            client::list(
                &input.target,
                &format!("{BUDGETS}/billingAccounts/{id}/budgets"),
                &[("pageSize", "100".into())],
                "budgets",
                100,
            ),
        )
        .await;
        (
            details.unwrap_or(Value::Null),
            budgets
                .map(|found| json!(found.iter().map(budget).collect::<Vec<_>>()))
                .unwrap_or(Value::Null),
        )
    };
    Ok(json!({
        "enabled": at(&info, "/billingEnabled").as_bool() == Some(true),
        "account": if id.is_empty() { Value::Null } else { json!(id) },
        "accountName": at(&details, "/displayName"),
        "accountOpen": at(&details, "/open"),
        "budgets": budgets,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_reads_with_its_amount_and_thresholds() {
        let read = budget(
            &json!({ "displayName": "Monthly cap", "amount": { "specifiedAmount": { "currencyCode": "USD", "units": "500" } }, "thresholdRules": [{ "thresholdPercent": 0.5 }, { "thresholdPercent": 0.9 }, { "thresholdPercent": 1.0 }], "budgetFilter": { "projects": ["projects/1"] } }),
        );
        assert_eq!(read["amount"], 500.0);
        assert_eq!(read["currency"], "USD");
        assert_eq!(read["thresholds"], json!([50.0, 90.0, 100.0]));
        assert_eq!(read["projects"], 1);
        assert_eq!(
            budget(&json!({ "amount": { "lastPeriodAmount": {} } }))["lastPeriod"],
            true
        );
    }
}
