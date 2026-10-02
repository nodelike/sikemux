// Follows one run while it is going. Every tick carries the whole run and its
// jobs, so even an unchanged tick tells the view the watch is still alive.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::time::sleep;

use crate::error::GithubError;
use crate::runs::{self, Followed, Job, Run, RunRef};
use crate::workflows::RepoRef;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// With few requests left in the hour, a run is read far less often, so the
/// rest of the app still has some to use.
const SPARING_INTERVAL: Duration = Duration::from_secs(30);
const SPARING_BELOW: u64 = 500;
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// The longest a spent rate limit is waited out in one go before trying again.
const MAX_RATE_WAIT: Duration = Duration::from_secs(15 * 60);
const ERROR_GIVEUP: u32 = 6;
/// One more read after a run reports itself finished, so the last job's steps
/// arrive rather than the view stopping on a half-finished picture.
const SETTLE_POLLS: u32 = 1;

/// One read of the run. `run` and `jobs` are the last ones read, so a tick
/// that failed still carries them. A finished tick with an error is the
/// watch giving up; `fatal` says starting it again will not help.
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

/// Signed out, a run that is gone, or a request GitHub cannot take: asking
/// again will not help.
fn is_final(error: &GithubError) -> bool {
    matches!(
        error,
        GithubError::Auth(_)
            | GithubError::Unconfigured
            | GithubError::NotFound(_)
            | GithubError::BadArg(_)
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub run_id: u64,
}

/// How long to wait before the next read. A spent rate limit is waited out
/// for as long as GitHub says, and does not count as the watch failing.
fn next_wait(failures: u32, rate_reset: Option<u64>, remaining: Option<u64>) -> Duration {
    if let Some(seconds) = rate_reset {
        return Duration::from_secs(seconds.max(1)).min(MAX_RATE_WAIT);
    }
    if remaining.is_some_and(|left| left < SPARING_BELOW) {
        return backoff(failures).max(SPARING_INTERVAL);
    }
    backoff(failures)
}

fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return POLL_INTERVAL;
    }
    let factor = 1u32 << failures.min(5);
    POLL_INTERVAL.saturating_mul(factor).min(MAX_BACKOFF)
}

pub async fn run(data_dir: &Path, input: Watch, sink: StreamSink) -> PluginResult<()> {
    let mut failures: u32 = 0;
    let mut settled: u32 = 0;
    let mut held = Followed::default();
    let reference = RunRef {
        repo: RepoRef {
            owner: input.repo.owner.clone(),
            name: input.repo.name.clone(),
        },
        run_id: input.run_id,
    };
    loop {
        let (run, jobs, error, rate_reset, remaining, gave_up, signed_out) =
            match runs::follow(data_dir, &reference, &mut held).await {
                Ok((detail, remaining)) => (
                    Some(detail.run),
                    detail.jobs,
                    None,
                    None,
                    remaining,
                    false,
                    false,
                ),
                Err(error) => {
                    let (run, jobs) = held.last();
                    let rate_reset = match error {
                        GithubError::RateLimited { resets_in_secs } => Some(resets_in_secs),
                        _ => None,
                    };
                    let gave_up = is_final(&error);
                    let signed_out =
                        matches!(error, GithubError::Auth(_) | GithubError::Unconfigured);
                    (
                        run,
                        jobs,
                        Some(error.to_string()),
                        rate_reset,
                        None,
                        gave_up,
                        signed_out,
                    )
                }
            };
        failures = if error.is_some() && rate_reset.is_none() {
            failures.saturating_add(1)
        } else {
            0
        };

        let run_over = error.is_none()
            && run
                .as_ref()
                .is_some_and(|run| runs::is_finished(&run.status));
        let jobs_over = !jobs.is_empty() && jobs.iter().all(|job| runs::is_finished(&job.status));
        if run_over {
            settled = settled.saturating_add(1);
        } else {
            settled = 0;
        }
        let finished = gave_up
            || (run_over && (jobs_over || settled > SETTLE_POLLS))
            || failures >= ERROR_GIVEUP;

        sink.send(reply(Tick {
            run,
            jobs,
            error,
            finished,
            fatal: gave_up,
            signed_out,
        })?)?;
        if finished {
            return Ok(());
        }
        sleep(next_wait(failures, rate_reset, remaining)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_at_once_when_asking_again_cannot_help() {
        assert!(is_final(&GithubError::Unconfigured));
        assert!(is_final(&GithubError::Auth("bad token".into())));
        assert!(is_final(&GithubError::NotFound("gone".into())));
        assert!(is_final(&GithubError::BadArg("bad".into())));
        assert!(!is_final(&GithubError::Transport("offline".into())));
        assert!(!is_final(&GithubError::RateLimited { resets_in_secs: 5 }));
        assert!(!is_final(&GithubError::Http {
            status: 502,
            message: "Bad Gateway".into()
        }));
    }

    #[test]
    fn a_healthy_watch_polls_at_a_steady_pace() {
        assert_eq!(backoff(0), POLL_INTERVAL);
    }

    #[test]
    fn a_spent_rate_limit_is_waited_out_as_long_as_github_says() {
        assert_eq!(next_wait(0, Some(120), None), Duration::from_secs(120));
        assert_eq!(next_wait(0, Some(0), None), Duration::from_secs(1));
        assert_eq!(next_wait(0, Some(99_999), None), MAX_RATE_WAIT);
    }

    #[test]
    fn a_nearly_spent_limit_slows_the_watch_down() {
        assert_eq!(next_wait(0, None, Some(4_000)), POLL_INTERVAL);
        assert_eq!(next_wait(0, None, Some(100)), SPARING_INTERVAL);
        assert_eq!(next_wait(0, None, None), POLL_INTERVAL);
    }

    #[test]
    fn failures_back_off_and_stop_growing_at_the_ceiling() {
        assert!(backoff(1) > backoff(0));
        assert!(backoff(3) > backoff(1));
        assert_eq!(backoff(20), MAX_BACKOFF);
    }
}
