// Follows one pipeline while it is going. Every tick carries the whole run and
// its steps, so even an unchanged tick tells the view the watch is still alive.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::time::sleep;

use crate::error::BitbucketError;
use crate::pipelines::{self, Job, Run, RunRef};

const POLL_INTERVAL: Duration = Duration::from_secs(4);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const ERROR_GIVEUP: u32 = 6;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tick {
    run: Option<Run>,
    jobs: Vec<Job>,
    error: Option<String>,
    finished: bool,
    fatal: bool,
    signed_out: bool,
}

/// Signed out, a pipeline that is gone, or a request Bitbucket cannot take:
/// asking again will not help.
fn is_final(error: &BitbucketError) -> bool {
    matches!(
        error,
        BitbucketError::Auth(_)
            | BitbucketError::Unconfigured
            | BitbucketError::NotFound(_)
            | BitbucketError::BadArg(_)
    )
}

fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return POLL_INTERVAL;
    }
    POLL_INTERVAL
        .saturating_mul(1u32 << failures.min(5))
        .min(MAX_BACKOFF)
}

pub async fn run(data_dir: &Path, input: RunRef, sink: StreamSink) -> PluginResult<()> {
    let mut failures: u32 = 0;
    let mut last: (Option<Run>, Vec<Job>) = (None, Vec::new());
    loop {
        let (error, gave_up, signed_out, rate_wait) =
            match pipelines::detail(data_dir, input.clone()).await {
                Ok(detail) => {
                    last = (Some(detail.run), detail.jobs);
                    (None, false, false, None)
                }
                Err(error) => (
                    Some(error.to_string()),
                    is_final(&error),
                    matches!(
                        error,
                        BitbucketError::Auth(_) | BitbucketError::Unconfigured
                    ),
                    match error {
                        BitbucketError::RateLimited { resets_in_secs } => Some(resets_in_secs),
                        _ => None,
                    },
                ),
            };
        failures = if error.is_some() && rate_wait.is_none() {
            failures.saturating_add(1)
        } else {
            0
        };
        let run_over =
            error.is_none() && last.0.as_ref().is_some_and(|run| run.status == "completed");
        let finished = gave_up || run_over || failures >= ERROR_GIVEUP;
        sink.send(reply(Tick {
            run: last.0.clone(),
            jobs: last.1.clone(),
            error,
            finished,
            fatal: gave_up,
            signed_out,
        })?)?;
        if finished {
            return Ok(());
        }
        sleep(match rate_wait {
            Some(secs) => Duration::from_secs(secs.max(1)),
            None => backoff(failures),
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_at_once_when_asking_again_cannot_help() {
        assert!(is_final(&BitbucketError::Unconfigured));
        assert!(is_final(&BitbucketError::NotFound("gone".into())));
        assert!(!is_final(&BitbucketError::Transport("offline".into())));
        assert!(!is_final(&BitbucketError::RateLimited {
            resets_in_secs: 5
        }));
    }

    #[test]
    fn failures_slow_the_watch_down_to_a_limit() {
        assert_eq!(backoff(0), POLL_INTERVAL);
        assert!(backoff(2) > backoff(1));
        assert_eq!(backoff(30), MAX_BACKOFF);
    }
}
