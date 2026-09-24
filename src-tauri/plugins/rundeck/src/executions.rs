// Execution endpoints: history, single fetch, run, abort, and the summary of
// a job's latest and last successful run. State/output polling lives in
// watch.rs / logs.rs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::error::{RundeckError, RundeckResult};

use crate::client::{get_json, limited, post_empty_json, post_json, seg};
use crate::config;

#[derive(Serialize, Clone, Deserialize)]
pub struct Execution {
    pub id: u64,
    pub status: Option<String>,
    #[serde(rename = "customStatus", default)]
    pub custom_status: Option<String>,
    pub user: Option<String>,
    pub project: Option<String>,
    #[serde(rename = "date-started")]
    pub date_started: Option<DateField>,
    #[serde(rename = "date-ended")]
    pub date_ended: Option<DateField>,
    pub permalink: Option<String>,
    pub job: Option<JobRef>,
    #[serde(rename = "argstring")]
    pub argstring: Option<String>,
    #[serde(
        rename = "workflowState",
        default,
        skip_deserializing,
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow_state: Option<WorkflowState>,
}

#[derive(Serialize, Clone, Deserialize)]
pub struct JobRef {
    pub id: Option<String>,
    pub name: Option<String>,
    pub group: Option<String>,
    pub project: Option<String>,
    #[serde(default, deserialize_with = "string_map")]
    pub options: Option<BTreeMap<String, String>>,
}

#[derive(Serialize, Clone, Deserialize)]
pub struct DateField {
    pub date: Option<String>,
    pub unixtime: Option<i64>,
}

fn value_text(value: Value) -> String {
    match value {
        Value::String(text) => text,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn string_map<'de, D>(deserializer: D) -> Result<Option<BTreeMap<String, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<BTreeMap<String, Value>>::deserialize(deserializer)?.map(|map| {
            map.into_iter()
                .map(|(key, value)| (key, value_text(value)))
                .collect()
        }),
    )
}

/// Execution ids arrive as numbers from most endpoints and as strings from some.
pub fn id_from_string_or_number<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("execution id is not a whole number")),
        Value::String(text) => text
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom("execution id is not a number")),
        _ => Err(serde::de::Error::custom("execution id is missing")),
    }
}

fn text_from_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        _ => Err(serde::de::Error::custom("expected a string or a number")),
    }
}

#[derive(Deserialize)]
struct ExecutionList {
    executions: Vec<Execution>,
}

async fn job_executions(
    job_id: &str,
    max: u32,
    status: Option<&str>,
) -> RundeckResult<Vec<Execution>> {
    let mut query: Vec<(&str, String)> = vec![("max", max.to_string())];
    if let Some(status) = status {
        query.push(("status", status.to_string()));
    }
    let path = format!("/job/{}/executions", seg(job_id)?);
    let list: ExecutionList = get_json(&path, &query).await?;
    Ok(list.executions)
}

pub async fn newest_succeeded(job_id: &str) -> RundeckResult<Option<Execution>> {
    Ok(job_executions(job_id, 1, Some("succeeded"))
        .await?
        .into_iter()
        .next())
}

pub async fn executions(
    job_id: String,
    project: String,
    max: Option<u32>,
    only_succeeded: Option<bool>,
) -> RundeckResult<Vec<Execution>> {
    let limit = max.unwrap_or(25);
    let succeeded_only = only_succeeded.unwrap_or(false);
    let history = job_executions(&job_id, limit, succeeded_only.then_some("succeeded")).await?;

    let running = if succeeded_only {
        Vec::new()
    } else {
        let running_query = [("jobIdFilter", job_id.clone())];
        let running_path = format!("/project/{}/executions/running", seg(&project)?);
        match get_json::<ExecutionList>(&running_path, &running_query).await {
            Ok(response) => response.executions,
            Err(_) => job_executions(&job_id, limit, Some("running"))
                .await
                .unwrap_or_default(),
        }
    };

    let mut executions = merge_executions(history, running, limit as usize);
    let states = join_all(executions.iter().map(|execution| async move {
        if !status_is_running(&execution.status) {
            return None;
        }
        get_json::<WorkflowState>(&format!("/execution/{}/state", execution.id), &[])
            .await
            .ok()
    }))
    .await;
    for (execution, state) in executions.iter_mut().zip(states) {
        execution.workflow_state = state;
    }
    Ok(executions)
}

fn status_is_running(status: &Option<String>) -> bool {
    status
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("running"))
}

fn merge_executions(
    history: Vec<Execution>,
    running: Vec<Execution>,
    limit: usize,
) -> Vec<Execution> {
    let running_ids: HashSet<_> = running.iter().map(|execution| execution.id).collect();
    let mut by_id = HashMap::with_capacity(history.len() + running.len());
    for execution in history.into_iter().chain(running) {
        by_id.insert(execution.id, execution);
    }
    let mut executions: Vec<_> = by_id.into_values().collect();
    executions.sort_by(|left, right| {
        execution_started_at(right)
            .cmp(&execution_started_at(left))
            .then_with(|| right.id.cmp(&left.id))
    });
    executions
        .into_iter()
        .enumerate()
        .filter_map(|(index, execution)| {
            (index < limit || running_ids.contains(&execution.id)).then_some(execution)
        })
        .collect()
}

fn execution_started_at(execution: &Execution) -> Option<i64> {
    execution
        .date_started
        .as_ref()
        .and_then(|started| started.unixtime)
}

pub async fn execution(execution_id: u64) -> RundeckResult<Execution> {
    get_json(&format!("/execution/{execution_id}"), &[]).await
}

// ---- summaries for the matrix and the plan ---------------------------------

#[derive(Serialize, Clone)]
pub struct ExecSummary {
    pub execution_id: u64,
    pub status: Option<String>,
    pub custom_status: Option<String>,
    pub user: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub permalink: Option<String>,
    pub branch: Option<String>,
    pub options: BTreeMap<String, String>,
}

/// The value of the first option named like one of `branch_options`, trying
/// the names in order and ignoring case.
pub fn branch_from_options(
    options: &BTreeMap<String, String>,
    branch_options: &[String],
) -> Option<String> {
    branch_options.iter().find_map(|wanted| {
        options
            .get(wanted)
            .or_else(|| {
                options
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
                    .map(|(_, value)| value)
            })
            .cloned()
    })
}

pub fn deployed_branch(execution: &Execution, branch_options: &[String]) -> Option<String> {
    let options = execution.job.as_ref()?.options.as_ref()?;
    branch_from_options(options, branch_options)
}

fn summarize(execution: Execution, branch_options: &[String]) -> ExecSummary {
    let options = execution
        .job
        .and_then(|job| job.options)
        .unwrap_or_default();
    ExecSummary {
        execution_id: execution.id,
        status: execution.status,
        custom_status: execution.custom_status,
        user: execution.user,
        started_at: execution.date_started.and_then(|date| date.date),
        ended_at: execution.date_ended.and_then(|date| date.date),
        permalink: execution.permalink,
        branch: branch_from_options(&options, branch_options),
        options,
    }
}

fn is_succeeded(execution: &Execution) -> bool {
    execution
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("succeeded"))
}

/// The job's newest execution and its newest successful one.
pub async fn latest_and_deployed(
    job_id: &str,
    branch_options: &[String],
) -> RundeckResult<(Option<ExecSummary>, Option<ExecSummary>)> {
    let latest = limited(job_executions(job_id, 1, None))
        .await?
        .into_iter()
        .next();
    let deployed = match &latest {
        Some(execution) if is_succeeded(execution) => latest.clone(),
        _ => limited(newest_succeeded(job_id)).await?,
    };
    Ok((
        latest.map(|execution| summarize(execution, branch_options)),
        deployed.map(|execution| summarize(execution, branch_options)),
    ))
}

// ---- run ------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct RunResult {
    pub id: u64,
    pub permalink: Option<String>,
    pub status: Option<String>,
    pub recovered: bool,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    #[serde(default)]
    pub options: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loglevel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_at_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_user: Option<String>,
}

const RECOVERY_WINDOW_MS: i64 = 10_000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// The execution a lost run request most likely started: the newest one by
/// `user` that began no earlier than shortly before the request was sent.
fn recovered_execution(
    executions: Vec<Execution>,
    user: &str,
    requested_at_ms: i64,
) -> Option<Execution> {
    executions
        .into_iter()
        .filter(|execution| execution.user.as_deref() == Some(user))
        .filter(|execution| {
            execution_started_at(execution)
                .is_some_and(|started| started >= requested_at_ms - RECOVERY_WINDOW_MS)
        })
        .max_by_key(|execution| (execution_started_at(execution), execution.id))
}

pub async fn run(job_id: String, request: RunRequest) -> RundeckResult<RunResult> {
    let path = format!("/job/{}/run", seg(&job_id)?);
    let requested_at_ms = now_ms();
    let error = match post_json::<_, Execution>(&path, &request).await {
        Ok(execution) => {
            return Ok(RunResult {
                id: execution.id,
                permalink: execution.permalink,
                status: execution.status,
                recovered: false,
            })
        }
        Err(error @ RundeckError::Transport(_)) => error,
        Err(error) => return Err(error),
    };
    let user = config::refresh_from_disk().await.unwrap_or_default().user;
    let recent = job_executions(&job_id, 5, None).await.unwrap_or_default();
    match recovered_execution(recent, &user, requested_at_ms) {
        Some(execution) if !user.is_empty() => Ok(RunResult {
            id: execution.id,
            permalink: execution.permalink,
            status: execution.status,
            recovered: true,
        }),
        _ => Err(error),
    }
}

// ---- abort ----------------------------------------------------------------

#[derive(Serialize, Clone, Deserialize)]
pub struct AbortResult {
    pub abort: Option<AbortBody>,
    pub execution: Option<AbortedExecution>,
}

#[derive(Serialize, Clone, Deserialize)]
pub struct AbortBody {
    pub status: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, Clone, Deserialize)]
pub struct AbortedExecution {
    #[serde(deserialize_with = "text_from_string_or_number")]
    pub id: String,
    pub status: Option<String>,
}

pub async fn abort(execution_id: u64) -> RundeckResult<AbortResult> {
    post_empty_json(&format!("/execution/{execution_id}/abort")).await
}

// ---- Step / workflow state (used by watch.rs and also by single fetch UI) --
//
// Rundeck's actual /state JSON puts the step lifecycle fields FLAT on each
// step object — NOT in a nested `stepState` wrapper as the docs imply.
// Real shape: { id, stepctx, executionState, startTime, endTime, duration,
//               nodeStates, nodeStep, parameterStates }
// `stepString` (label) isn't in /state at all; it lives in the job
// definition. For now we surface stepctx as the visible label.

#[derive(Serialize, Clone, Deserialize, Default)]
pub struct Step {
    pub id: Option<String>,
    pub stepctx: Option<String>,
    #[serde(rename = "executionState")]
    pub execution_state: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(rename = "endTime")]
    pub end_time: Option<String>,
    #[serde(rename = "nodeStep")]
    pub node_step: Option<bool>,
}

#[derive(Serialize, Clone, Deserialize, Default)]
pub struct WorkflowState {
    #[serde(rename = "executionState")]
    pub execution_state: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(rename = "stepCount")]
    pub step_count: Option<u32>,
    #[serde(rename = "completed")]
    pub completed: Option<bool>,
}

pub async fn execution_state(execution_id: u64) -> RundeckResult<WorkflowState> {
    get_json(&format!("/execution/{execution_id}/state"), &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution(id: u64, started_at: Option<i64>, status: &str) -> Execution {
        Execution {
            id,
            status: Some(status.into()),
            custom_status: None,
            user: None,
            project: None,
            date_started: Some(DateField {
                date: None,
                unixtime: started_at,
            }),
            date_ended: None,
            permalink: None,
            job: None,
            argstring: None,
            workflow_state: None,
        }
    }

    #[test]
    fn merges_running_executions_and_sorts_latest_first() {
        let history = vec![execution(41, Some(1_000), "succeeded")];
        let running = vec![execution(42, Some(2_000), "running")];

        let result = merge_executions(history, running, 25);

        assert_eq!(
            result.iter().map(|item| item.id).collect::<Vec<_>>(),
            [42, 41]
        );
    }

    #[test]
    fn running_snapshot_replaces_duplicate_history_item() {
        let history = vec![execution(42, Some(2_000), "scheduled")];
        let running = vec![execution(42, Some(2_000), "running")];

        let result = merge_executions(history, running, 25);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status.as_deref(), Some("running"));
    }

    #[test]
    fn sorts_equal_or_missing_timestamps_by_execution_id() {
        let history = vec![
            execution(8, None, "succeeded"),
            execution(10, None, "failed"),
        ];

        let result = merge_executions(history, Vec::new(), 1);

        assert_eq!(result[0].id, 10);
    }

    #[test]
    fn retains_long_running_executions_beyond_the_history_limit() {
        let history = vec![
            execution(43, Some(3_000), "succeeded"),
            execution(42, Some(2_000), "succeeded"),
        ];
        let running = vec![execution(41, Some(1_000), "running")];

        let result = merge_executions(history, running, 2);

        assert_eq!(
            result.iter().map(|item| item.id).collect::<Vec<_>>(),
            [43, 42, 41]
        );
    }

    fn by(mut execution: Execution, user: &str) -> Execution {
        execution.user = Some(user.into());
        execution
    }

    #[test]
    fn recovers_the_newest_run_by_this_user_since_the_request() {
        let executions = vec![
            by(execution(50, Some(95_000), "running"), "alice"),
            by(execution(51, Some(99_000), "running"), "bob"),
            by(execution(49, Some(80_000), "succeeded"), "alice"),
        ];
        let found = recovered_execution(executions, "alice", 100_000).unwrap();
        assert_eq!(found.id, 50);
        let old = vec![by(execution(49, Some(80_000), "succeeded"), "alice")];
        assert!(recovered_execution(old, "alice", 100_000).is_none());
    }

    #[test]
    fn abort_accepts_string_or_numeric_execution_ids() {
        let text: AbortResult = serde_json::from_str(
            r#"{"abort":{"status":"pending","reason":null},"execution":{"id":"42","status":"running"}}"#,
        )
        .unwrap();
        let number: AbortResult =
            serde_json::from_str(r#"{"abort":null,"execution":{"id":42,"status":null}}"#).unwrap();
        assert_eq!(text.execution.unwrap().id, "42");
        assert_eq!(number.execution.as_ref().unwrap().id, "42");
        let json = serde_json::to_value(number).unwrap();
        assert_eq!(json.pointer("/execution/id"), Some(&Value::from("42")));
    }

    #[test]
    fn branch_options_match_case_insensitively_in_the_given_order() {
        let options: BTreeMap<String, String> = [
            ("branch".to_string(), "feature".to_string()),
            ("GIT_REF".to_string(), "main".to_string()),
        ]
        .into_iter()
        .collect();
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            branch_from_options(&options, &names(&["BRANCH", "git_ref"])).as_deref(),
            Some("feature")
        );
        assert_eq!(
            branch_from_options(&options, &names(&["git_ref", "BRANCH"])).as_deref(),
            Some("main")
        );
        assert_eq!(branch_from_options(&options, &names(&["TAG"])), None);
        assert_eq!(branch_from_options(&options, &[]), None);
    }

    #[test]
    fn option_values_that_are_not_strings_still_parse() {
        let parsed: Execution = serde_json::from_str(
            r#"{"id":7,"status":"other","customStatus":"partial","job":{"options":{"BRANCH":"main","COUNT":3}}}"#,
        )
        .unwrap();
        assert_eq!(parsed.custom_status.as_deref(), Some("partial"));
        let options = parsed.job.unwrap().options.unwrap();
        assert_eq!(options.get("COUNT").map(String::as_str), Some("3"));
    }
}
