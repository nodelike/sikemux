// Live execution watcher — polls /execution/{id} + /execution/{id}/state
// every 1.5s and sends one combined payload per tick (so even unchanged ticks
// act as a heartbeat). Ends when the execution reaches a terminal state.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::rundeck::RunNotice;
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::time::sleep;

use crate::client::{get_json, Session};
use crate::error::RundeckResult;
use crate::executions::{Execution, JobRef, WorkflowState};
use crate::notice::{lost_run_notice, run_notice};

#[derive(Serialize, Clone)]
pub struct WatchUpdate {
    pub execution: Option<Execution>,
    pub state: Option<WorkflowState>,
    pub error: Option<String>,
    pub terminal: bool,
    /// What to tell the person, once the run has ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<RunNotice>,
}

/// The job a watched run belongs to, to name it when Rundeck stops saying.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchedJob {
    job_id: String,
    name: String,
    #[serde(default)]
    group: Option<String>,
    project: String,
}

/// Polls allowed after the execution ends for the step view to catch up.
const SETTLE_POLLS: u32 = 3;

/// Every status but the in-progress ones is final, including custom `other`
/// statuses and `failed-with-retry`.
fn is_terminal(status: &Option<String>) -> bool {
    status.as_deref().is_some_and(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "running" | "scheduled" | "queued"
        )
    })
}

fn state_is_terminal(state: &WorkflowState) -> bool {
    state.completed.unwrap_or(false)
        || state.execution_state.as_deref().is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "succeeded" | "failed" | "aborted" | "node_partial_succeeded" | "node_mixed"
            )
        })
}

pub async fn watch(
    session: Session,
    execution_id: u64,
    job: Option<WatchedJob>,
    branch_options: Vec<String>,
    sink: StreamSink,
) -> PluginResult<()> {
    follow(&session, execution_id, |mut update| {
        if update.terminal {
            update.notice = Some(ended_notice(
                &update,
                execution_id,
                job.as_ref(),
                &branch_options,
            ));
        }
        sink.send(reply(update)?)
    })
    .await
}

/// The last update can lack the run itself when Rundeck stopped answering; its
/// step state may still say how it ended.
fn ended_notice(
    update: &WatchUpdate,
    execution_id: u64,
    job: Option<&WatchedJob>,
    branch_options: &[String],
) -> RunNotice {
    if let Some(execution) = &update.execution {
        return run_notice(execution, branch_options);
    }
    let status = update
        .state
        .as_ref()
        .and_then(|state| state.execution_state.clone());
    match (status, job) {
        (Some(status), job) => run_notice(
            &Execution {
                id: execution_id,
                status: Some(status),
                custom_status: None,
                user: None,
                project: job.map(|job| job.project.clone()),
                date_started: None,
                date_ended: None,
                permalink: None,
                job: job.map(|job| JobRef {
                    id: Some(job.job_id.clone()),
                    name: Some(job.name.clone()),
                    group: job.group.clone(),
                    project: Some(job.project.clone()),
                    options: None,
                }),
                argstring: None,
                workflow_state: None,
            },
            branch_options,
        ),
        (None, Some(job)) => lost_run_notice(&job.name, job.group.as_deref(), execution_id),
        (None, None) => lost_run_notice(&format!("Execution #{execution_id}"), None, execution_id),
    }
}

/// Hands `each` the execution's state on every poll until it ends.
pub async fn follow(
    session: &Session,
    execution_id: u64,
    mut each: impl FnMut(WatchUpdate) -> PluginResult<()>,
) -> PluginResult<()> {
    let mut last_status: Option<String> = None;
    let mut consecutive_errors: u32 = 0;
    let mut terminal_settle_polls: u32 = 0;
    const POLL_INTERVAL: Duration = Duration::from_millis(1500);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    const ERROR_GIVEUP: u32 = 8;

    loop {
        let execution_path = format!("/execution/{execution_id}");
        let state_path = format!("/execution/{execution_id}/state");
        let (exec_res, state_res): (RundeckResult<Execution>, RundeckResult<WorkflowState>) = tokio::join!(
            get_json(session, &execution_path, &[]),
            get_json(session, &state_path, &[])
        );

        let both_failed = exec_res.is_err() && state_res.is_err();
        let (execution, state, error) = match (exec_res, state_res) {
            (Ok(e), Ok(s)) => {
                last_status = e.status.clone();
                (Some(e), Some(s), None)
            }
            (Ok(e), Err(se)) => {
                last_status = e.status.clone();
                (Some(e), None, Some(se.to_string()))
            }
            (Err(ee), Ok(s)) => (None, Some(s), Some(ee.to_string())),
            (Err(ee), Err(_)) => (None, None, Some(ee.to_string())),
        };

        if both_failed {
            consecutive_errors = consecutive_errors.saturating_add(1);
        } else {
            consecutive_errors = 0;
        }

        let execution_terminal = is_terminal(&last_status);
        let workflow_terminal = state.as_ref().is_some_and(state_is_terminal);
        if execution_terminal && !workflow_terminal {
            terminal_settle_polls = terminal_settle_polls.saturating_add(1);
        } else {
            terminal_settle_polls = 0;
        }
        let terminal = (workflow_terminal && (execution_terminal || execution.is_none()))
            || terminal_settle_polls > SETTLE_POLLS
            || consecutive_errors >= ERROR_GIVEUP;
        each(WatchUpdate {
            execution,
            state,
            error,
            terminal,
            notice: None,
        })?;
        if terminal {
            return Ok(());
        }

        let sleep_dur = if consecutive_errors == 0 {
            POLL_INTERVAL
        } else {
            let exp = 1u64 << consecutive_errors.min(6);
            Duration::from_millis((POLL_INTERVAL.as_millis() as u64).saturating_mul(exp))
                .min(MAX_BACKOFF)
        };
        sleep(sleep_dur).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_in_progress_statuses_keep_the_watch_open() {
        for status in ["other", "failed-with-retry", "Succeeded", "aborted"] {
            assert!(is_terminal(&Some(status.into())), "{status}");
        }
        for status in ["running", "SCHEDULED", "queued"] {
            assert!(!is_terminal(&Some(status.into())), "{status}");
        }
        assert!(!is_terminal(&None));
    }

    #[test]
    fn a_waiting_workflow_is_not_finished() {
        assert!(!state_is_terminal(&WorkflowState {
            execution_state: Some("WAITING".into()),
            ..WorkflowState::default()
        }));
    }

    #[test]
    fn completed_workflow_is_terminal_even_without_a_state_name() {
        assert!(state_is_terminal(&WorkflowState {
            completed: Some(true),
            ..WorkflowState::default()
        }));
    }

    fn job() -> WatchedJob {
        WatchedJob {
            job_id: "j1".to_owned(),
            name: "api".to_owned(),
            group: Some("staging/backend".to_owned()),
            project: "shop".to_owned(),
        }
    }

    fn ended(state: Option<WorkflowState>) -> WatchUpdate {
        WatchUpdate {
            execution: None,
            state,
            error: Some("502".to_owned()),
            terminal: true,
            notice: None,
        }
    }

    #[test]
    fn says_how_the_run_ended_from_its_steps_when_rundeck_stopped_returning_the_run() {
        let state = WorkflowState {
            execution_state: Some("FAILED".to_owned()),
            steps: Vec::new(),
            step_count: Some(2),
            completed: Some(true),
        };
        let notice = ended_notice(&ended(Some(state)), 7, Some(&job()), &[]);
        assert!(!notice.ok);
        assert_eq!(notice.title, "api on staging/backend failed");
    }

    #[test]
    fn says_it_lost_track_of_a_run_when_rundeck_stopped_answering() {
        assert_eq!(
            ended_notice(&ended(None), 7, Some(&job()), &[]),
            RunNotice {
                ok: false,
                title: "Lost track of staging/backend/api".to_owned(),
                body: "Rundeck stopped answering before run #7 ended. Check it in Rundeck."
                    .to_owned(),
            }
        );
    }
}
