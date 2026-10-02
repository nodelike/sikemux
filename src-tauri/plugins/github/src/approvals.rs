// A run held at an environment until somebody signs it off. GitHub reports
// each waiting environment separately, and says whether this account is one
// of the people allowed to answer.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client;
use crate::error::{GithubError, GithubResult};
use crate::runs::RunRef;
use crate::workflows::RepoRef;

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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub environment_id: u64,
    pub environment: String,
    pub wait_minutes: u64,
    /// False when somebody else has to sign this off, which is worth saying
    /// rather than offering a button that will be refused.
    pub can_approve: bool,
    pub reviewers: Vec<String>,
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

pub async fn pending(data_dir: &Path, input: RunRef) -> GithubResult<Vec<Pending>> {
    let path = input.repo.path(&format!(
        "/actions/runs/{}/pending_deployments",
        input.run_id
    ))?;
    let rows: Vec<PendingRow> = client::get(data_dir, &path, &[]).await?;
    Ok(rows
        .into_iter()
        .map(|row| Pending {
            environment_id: row.environment.id,
            environment: row.environment.name,
            wait_minutes: row.wait_timer.unwrap_or(0),
            can_approve: row.current_user_can_approve.unwrap_or(false),
            reviewers: row.reviewers.iter().filter_map(reviewer_name).collect(),
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub run_id: u64,
    pub environment_ids: Vec<u64>,
    /// `approved` or `rejected`; GitHub accepts nothing else.
    pub state: String,
    #[serde(default)]
    pub comment: String,
}

pub async fn review(data_dir: &Path, input: Review) -> GithubResult<()> {
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
    let path = input.repo.path(&format!(
        "/actions/runs/{}/pending_deployments",
        input.run_id
    ))?;
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

    #[test]
    fn a_run_nobody_here_can_sign_off_says_so() {
        let parsed = row(json!({
            "environment": { "id": 7, "name": "production" },
            "reviewers": [],
        }));
        assert_eq!(parsed.current_user_can_approve, None);
        let pending = Pending {
            environment_id: parsed.environment.id,
            environment: parsed.environment.name,
            wait_minutes: parsed.wait_timer.unwrap_or(0),
            can_approve: parsed.current_user_can_approve.unwrap_or(false),
            reviewers: Vec::new(),
        };
        assert!(!pending.can_approve);
        assert_eq!(pending.wait_minutes, 0);
    }

    #[tokio::test]
    async fn refuses_a_verdict_github_would_not_understand() {
        let dir = std::env::temp_dir();
        let bad = Review {
            repo: RepoRef {
                owner: "a".into(),
                name: "b".into(),
            },
            run_id: 1,
            environment_ids: vec![7],
            state: "maybe".into(),
            comment: String::new(),
        };
        assert!(review(&dir, bad).await.is_err());

        let empty = Review {
            repo: RepoRef {
                owner: "a".into(),
                name: "b".into(),
            },
            run_id: 1,
            environment_ids: Vec::new(),
            state: "approved".into(),
            comment: String::new(),
        };
        assert!(review(&dir, empty).await.is_err());
    }
}
