//! Follows one CI run while it is going. Every tick carries the whole run and
//! its jobs, so even an unchanged tick tells the view the watch is still alive.

use std::future::Future;
use std::time::Duration;

use serde::Serialize;
use tokio::time::sleep;

use crate::service::{Service, ServiceError};
use crate::{reply, PluginResult, StreamSink};

/// With few requests left, a run is read far less often, so the rest of the
/// app still has some to use.
const SPARING_INTERVAL: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// The longest a spent rate limit is waited out in one go before trying again.
const MAX_RATE_WAIT: Duration = Duration::from_secs(15 * 60);
const ERROR_GIVEUP: u32 = 6;

pub struct Reading<R, J> {
    pub run: R,
    pub jobs: Vec<J>,
    /// The host says few requests are left.
    pub sparing: bool,
}

pub struct Failure {
    pub message: String,
    /// Asking again will not help.
    pub fatal: bool,
    pub signed_out: bool,
    /// A spent rate limit, and how long until it resets.
    pub rate_reset_secs: Option<u64>,
}

/// Signed out, a run that is gone, or a request the service cannot take:
/// asking again will not help.
impl<S: Service> From<ServiceError<S>> for Failure {
    fn from(error: ServiceError<S>) -> Self {
        Failure {
            fatal: matches!(
                error,
                ServiceError::Auth(_)
                    | ServiceError::Unconfigured
                    | ServiceError::NotFound(_)
                    | ServiceError::BadArg(_)
            ),
            signed_out: matches!(error, ServiceError::Auth(_) | ServiceError::Unconfigured),
            rate_reset_secs: match error {
                ServiceError::RateLimited { resets_in_secs } => resets_in_secs,
                _ => None,
            },
            message: error.to_string(),
        }
    }
}

pub trait Follow: Send {
    type Run: Serialize + Send + Sync;
    type Job: Serialize + Send + Sync;

    const POLL_INTERVAL: Duration;

    fn read(
        &mut self,
    ) -> impl Future<Output = Result<Reading<Self::Run, Self::Job>, Failure>> + Send;

    /// Asked after every successful read; true ends the watch.
    fn settled(&mut self, run: &Self::Run, jobs: &[Self::Job]) -> bool;
}

/// `run` and `jobs` are the last ones read, so a tick that failed still
/// carries them. A finished tick with an error is the watch giving up;
/// `fatal` says starting it again will not help.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tick<'a, R, J> {
    run: Option<&'a R>,
    jobs: &'a [J],
    error: Option<String>,
    finished: bool,
    fatal: bool,
    signed_out: bool,
}

pub async fn follow<F: Follow>(mut host: F, sink: StreamSink) -> PluginResult<()> {
    let mut failures: u32 = 0;
    let mut last: (Option<F::Run>, Vec<F::Job>) = (None, Vec::new());
    loop {
        let (error, fatal, signed_out, rate_reset, sparing, run_over) = match host.read().await {
            Ok(reading) => {
                let over = host.settled(&reading.run, &reading.jobs);
                last = (Some(reading.run), reading.jobs);
                (None, false, false, None, reading.sparing, over)
            }
            Err(failure) => (
                Some(failure.message),
                failure.fatal,
                failure.signed_out,
                failure.rate_reset_secs,
                false,
                false,
            ),
        };
        failures = if error.is_some() && rate_reset.is_none() {
            failures.saturating_add(1)
        } else {
            0
        };
        let finished = fatal || run_over || failures >= ERROR_GIVEUP;
        sink.send(reply(Tick {
            run: last.0.as_ref(),
            jobs: &last.1,
            error,
            finished,
            fatal,
            signed_out,
        })?)?;
        if finished {
            return Ok(());
        }
        sleep(next_wait(F::POLL_INTERVAL, failures, rate_reset, sparing)).await;
    }
}

/// A spent rate limit is waited out for as long as the host says, and does
/// not count as the watch failing.
fn next_wait(poll: Duration, failures: u32, rate_reset: Option<u64>, sparing: bool) -> Duration {
    if let Some(seconds) = rate_reset {
        return Duration::from_secs(seconds.max(1)).min(MAX_RATE_WAIT);
    }
    if sparing {
        return backoff(poll, failures).max(SPARING_INTERVAL);
    }
    backoff(poll, failures)
}

fn backoff(poll: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return poll;
    }
    poll.saturating_mul(1u32 << failures.min(5))
        .min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLL: Duration = Duration::from_secs(3);

    #[test]
    fn a_spent_rate_limit_is_waited_out_as_long_as_the_host_says() {
        assert_eq!(
            next_wait(POLL, 0, Some(120), false),
            Duration::from_secs(120)
        );
        assert_eq!(next_wait(POLL, 0, Some(0), false), Duration::from_secs(1));
        assert_eq!(next_wait(POLL, 0, Some(99_999), false), MAX_RATE_WAIT);
    }

    #[test]
    fn a_nearly_spent_limit_slows_the_watch_down() {
        assert_eq!(next_wait(POLL, 0, None, false), POLL);
        assert_eq!(next_wait(POLL, 0, None, true), SPARING_INTERVAL);
    }

    #[test]
    fn failures_back_off_and_stop_growing_at_the_ceiling() {
        assert_eq!(backoff(POLL, 0), POLL);
        assert!(backoff(POLL, 1) > backoff(POLL, 0));
        assert!(backoff(POLL, 3) > backoff(POLL, 1));
        assert_eq!(backoff(POLL, 20), MAX_BACKOFF);
    }
}
