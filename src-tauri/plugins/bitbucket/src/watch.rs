use std::path::Path;
use std::time::Duration;

use sikemux_plugin_api::watch::{self, Failure, Follow, Reading};
use sikemux_plugin_api::{PluginResult, StreamSink};

use crate::error::BitbucketError;
use crate::pipelines::{self, Job, Run, RunRef};
use crate::{client, config, ratelimit};

struct Bitbucket<'a> {
    data_dir: &'a Path,
    reference: RunRef,
}

/// Signed out, a pipeline that is gone, or a request Bitbucket cannot take:
/// asking again will not help.
fn failure(error: BitbucketError) -> Failure {
    Failure {
        fatal: matches!(
            error,
            BitbucketError::Auth(_)
                | BitbucketError::Unconfigured
                | BitbucketError::NotFound(_)
                | BitbucketError::BadArg(_)
        ),
        signed_out: matches!(
            error,
            BitbucketError::Auth(_) | BitbucketError::Unconfigured
        ),
        rate_reset_secs: match error {
            BitbucketError::RateLimited { resets_in_secs } => Some(resets_in_secs),
            _ => None,
        },
        message: error.to_string(),
    }
}

/// A pipeline held at a manual step waits on a person, so it is as settled as a finished one.
pub(crate) fn settled(status: &str) -> bool {
    matches!(status, "completed" | "waiting")
}

impl Follow for Bitbucket<'_> {
    type Run = Run;
    type Job = Job;

    const POLL_INTERVAL: Duration = Duration::from_secs(4);

    async fn read(&mut self) -> Result<Reading<Run, Job>, Failure> {
        let detail = pipelines::detail(self.data_dir, self.reference.clone())
            .await
            .map_err(failure)?;
        let account = config::load(self.data_dir)
            .account(client::chosen().as_deref())
            .map(|account| account.id.clone())
            .unwrap_or_default();
        Ok(Reading {
            run: detail.run,
            jobs: detail.jobs,
            sparing: ratelimit::budget(&account).near,
        })
    }

    fn settled(&mut self, run: &Run, _jobs: &[Job]) -> bool {
        settled(&run.status)
    }
}

pub async fn run(data_dir: &Path, reference: RunRef, sink: StreamSink) -> PluginResult<()> {
    watch::follow(
        Bitbucket {
            data_dir,
            reference,
        },
        sink,
    )
    .await
}
