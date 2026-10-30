// A run held at an environment until somebody signs it off. GitHub reports
// each waiting environment separately, and says whether this account is one
// of the people allowed to answer.

use std::path::Path;

use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_api::codehost::asks::{DeploymentReview, RunRef};
use sikemux_plugin_api::codehost::PendingApproval;

use crate::client;
use crate::error::{GithubError, GithubResult};
use crate::runs::run_path;

#[derive(Deserialize)]
struct EnvironmentRow {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct ReviewerTarget {
    login: Option<String>,
    name: Option<String>,
    slug: Option<String>,
}

#[derive(Deserialize)]
struct ReviewerRow {
    #[serde(rename = "type")]
    kind: Option<String>,
    reviewer: Option<ReviewerTarget>,
}

#[derive(Deserialize)]
struct PendingRow {
    environment: EnvironmentRow,
    wait_timer: Option<u64>,
    current_user_can_approve: Option<bool>,
    #[serde(default)]
    reviewers: Vec<ReviewerRow>,
}

fn reviewer_name(row: &ReviewerRow) -> Option<String> {
    let target = row.reviewer.as_ref()?;
    let name = target
        .login
        .clone()
        .or_else(|| target.slug.clone())
        .or_else(|| target.name.clone())?;
    Some(match row.kind.as_deref() {
        Some("Team") => format!("@{name} (team)"),
        _ => format!("@{name}"),
    })
}

pub async fn pending(data_dir: &Path, input: RunRef) -> GithubResult<Vec<PendingApproval>> {
    let path = run_path(&input, "/pending_deployments")?;
    let rows: Vec<PendingRow> = client::get(data_dir, &path, &[]).await?;
    Ok(rows
        .into_iter()
        .map(|row| PendingApproval {
            environment_id: row.environment.id,
            environment: row.environment.name,
            wait_minutes: row.wait_timer.unwrap_or(0),
            can_approve: row.current_user_can_approve.unwrap_or(false),
            reviewers: row.reviewers.iter().filter_map(reviewer_name).collect(),
        })
        .collect())
}

/// `state` is `approved` or `rejected`; GitHub accepts nothing else.
pub async fn review(data_dir: &Path, input: DeploymentReview) -> GithubResult<()> {
    if !matches!(input.state.as_str(), "approved" | "rejected") {
        return Err(GithubError::BadArg(format!(
            "`{}` is not approved or rejected",
            input.state
        )));
    }
    if input.environment_ids.is_empty() {
        return Err(GithubError::BadArg(
            "no environment was named to sign off".into(),
        ));
    }
    let path = run_path(&input.run, "/pending_deployments")?;
    let body = json!({
        "environment_ids": input.environment_ids,
        "state": input.state,
        "comment": input.comment,
    });
    client::post_empty(data_dir, &path, Some(&body)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(value: serde_json::Value) -> PendingRow {
        serde_json::from_value(value).expect("parses")
    }

    #[test]
    fn names_a_person_and_a_team_differently() {
        let parsed = row(json!({
            "environment": { "id": 7, "name": "production" },
            "wait_timer": 5,
            "current_user_can_approve": true,
            "reviewers": [
                { "type": "User", "reviewer": { "login": "nodelike" } },
                { "type": "Team", "reviewer": { "slug": "release" } },
            ],
        }));
        let names: Vec<String> = parsed.reviewers.iter().filter_map(reviewer_name).collect();
        assert_eq!(names, ["@nodelike", "@release (team)"]);
    }
}
