// Live execution watcher — polls /execution/{id} + /execution/{id}/state
// every 1.5s and sends one combined payload per tick (so even unchanged ticks
// act as a heartbeat). Ends when the execution reaches a terminal state.

use std::time::Duration;

use serde::Serialize;
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::time::sleep;

use crate::client::get_json;
use crate::error::RundeckResult;
use crate::executions::{Execution, WorkflowState};

#[derive(Serialize, Clone)]
pub struct WatchUpdate {
    pub execution: Option<Execution>,
    pub state: Option<WorkflowState>,
    pub error: Option<String>,
    pub terminal: bool,
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

pub async fn watch(execution_id: u64, sink: StreamSink) -> PluginResult<()> {
    let mut last_status: Option<String> = None;
    let mut consecutive_errors: u32 = 0;
    let mut terminal_settle_polls: u32 = 0;
    const POLL_INTERVAL: Duration = Duration::from_millis(1500);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    const ERROR_GIVEUP: u32 = 8;

    loop {
        let execution_path = format!("/execution/{execution_id}");
        let state_path = format!("/execution/{execution_id}/state");
        let (exec_res, state_res): (RundeckResult<Execution>, RundeckResult<WorkflowState>) =
            tokio::join!(get_json(&execution_path, &[]), get_json(&state_path, &[]));

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
        sink.send(reply(WatchUpdate {
            execution,
            state,
            error,
            terminal,
        })?)?;
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
    fn recognizes_execution_and_workflow_terminal_states_case_insensitively() {
        assert!(is_terminal(&Some("other-failed".into())));
        assert!(is_terminal(&Some("SUCCEEDED".into())));
        assert!(state_is_terminal(&WorkflowState {
            execution_state: Some("FAILED".into()),
            ..WorkflowState::default()
        }));
    }

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
}
