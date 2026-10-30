// Every check on a commit, as a pull request's Checks tab lists them: each job
// of its workflow runs and each status an app outside Actions posted, marked
// where the base branch's rules require it.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use sikemux_plugin_api::codehost::asks::ChecksQuery;
use sikemux_plugin_api::codehost::{Check, Outcome};

use crate::client;
use crate::error::GithubResult;
use crate::repo::RepoPath;

/// GitHub lists at most 300 of a commit's checks this way.
const CHECK_PAGES: u32 = 3;

#[derive(Deserialize)]
struct CheckRunPage {
    check_runs: Vec<CheckRunRow>,
}

#[derive(Deserialize)]
struct CheckRunRow {
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    details_url: Option<String>,
    html_url: Option<String>,
    app: Option<AppRow>,
}

#[derive(Deserialize)]
struct AppRow {
    slug: Option<String>,
}

#[derive(Deserialize)]
struct CombinedStatus {
    statuses: Vec<StatusRow>,
}

#[derive(Deserialize)]
struct StatusRow {
    context: String,
    state: String,
    target_url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct RuleRow {
    #[serde(rename = "type")]
    kind: String,
    parameters: Option<RuleParameters>,
}

#[derive(Deserialize)]
struct RuleParameters {
    #[serde(default)]
    required_status_checks: Vec<RequiredCheck>,
}

#[derive(Deserialize)]
struct RequiredCheck {
    context: String,
}

#[derive(Deserialize)]
struct BranchRow {
    protection: Option<Protection>,
}

#[derive(Deserialize)]
struct Protection {
    required_status_checks: Option<RequiredChecks>,
}

#[derive(Deserialize)]
struct RequiredChecks {
    #[serde(default)]
    contexts: Vec<String>,
}

pub async fn list(data_dir: &Path, input: ChecksQuery) -> GithubResult<Vec<Check>> {
    let runs_path = input
        .repo
        .path(&format!("/commits/{}/check-runs", input.sha))?;
    let status_path = input.repo.path(&format!("/commits/{}/status", input.sha))?;
    let latest = [("filter", "latest".to_owned())];
    let (runs, statuses, required) = futures::join!(
        client::get_all(
            data_dir,
            &runs_path,
            &latest,
            CHECK_PAGES,
            |page: CheckRunPage| page.check_runs
        ),
        client::get::<CombinedStatus>(data_dir, &status_path, &[]),
        required(data_dir, &input),
    );
    let mut checks: Vec<Check> = runs?
        .into_iter()
        .map(|row| {
            let (run_id, job_id) = row
                .details_url
                .as_deref()
                .and_then(run_and_job)
                .map_or((None, None), |(run, job)| (Some(run), job));
            let app = row
                .app
                .and_then(|app| app.slug)
                .filter(|slug| slug != "github-actions");
            Check {
                required: required.contains(&row.name),
                outcome: Outcome::of(&row.status, row.conclusion.as_deref()),
                run_id,
                job_id,
                app,
                started_at: row.started_at,
                finished_at: row.completed_at,
                url: row.html_url.or(row.details_url),
                name: row.name,
            }
        })
        .collect();
    let posted = statuses
        .map(|combined| combined.statuses)
        .unwrap_or_default();
    checks.extend(posted.into_iter().map(|row| {
        let outcome = match row.state.as_str() {
            "success" => Outcome::Success,
            "failure" | "error" => Outcome::Failure,
            "pending" => Outcome::Running,
            _ => Outcome::Unknown,
        };
        let finished = outcome != Outcome::Running;
        Check {
            required: required.contains(&row.context),
            outcome,
            run_id: None,
            job_id: None,
            app: None,
            started_at: row.created_at,
            finished_at: row.updated_at.filter(|_| finished),
            url: row.target_url,
            name: row.context,
        }
    }));
    Ok(checks)
}

/// The checks the base branch, the one the pull request merges into, requires
/// by its rulesets and its protection, as far as the signed-in account may read them.
async fn required(data_dir: &Path, input: &ChecksQuery) -> HashSet<String> {
    let Some(base) = input.base.as_deref().filter(|base| !base.is_empty()) else {
        return HashSet::new();
    };
    let branch = crate::workflows::encode_path(base);
    let (Ok(rules_path), Ok(branch_path)) = (
        input.repo.path(&format!("/rules/branches/{branch}")),
        input.repo.path(&format!("/branches/{branch}")),
    ) else {
        return HashSet::new();
    };
    let (rules, protected) = futures::join!(
        client::get::<Vec<RuleRow>>(data_dir, &rules_path, &[]),
        client::get::<BranchRow>(data_dir, &branch_path, &[]),
    );
    let from_rules = rules
        .unwrap_or_default()
        .into_iter()
        .filter(|rule| rule.kind == "required_status_checks")
        .filter_map(|rule| rule.parameters)
        .flat_map(|parameters| parameters.required_status_checks)
        .map(|check| check.context);
    let from_protection = protected
        .ok()
        .and_then(|branch| branch.protection)
        .and_then(|protection| protection.required_status_checks)
        .map(|checks| checks.contexts)
        .unwrap_or_default();
    from_rules.chain(from_protection).collect()
}

/// The run and job a check's address names, as Actions writes it:
/// `.../actions/runs/<run>/job/<job>`.
fn run_and_job(url: &str) -> Option<(String, Option<String>)> {
    let (_, rest) = url.split_once("/actions/runs/")?;
    let mut parts = rest.split('/');
    let run = parts
        .next()
        .filter(|run| run.chars().all(|c| c.is_ascii_digit()))?;
    let job = match (parts.next(), parts.next()) {
        (Some("job"), Some(job)) => Some(job.split(['?', '#']).next().unwrap_or(job).to_owned()),
        _ => None,
    };
    Some((run.to_owned(), job))
}
