use std::path::Path;
use std::time::Duration;

use sikemux_plugin_api::watch::{self, Failure, Follow, Reading};
use sikemux_plugin_api::{PluginResult, StreamSink};

use crate::error::GitlabError;
use crate::pipelines::{self, Job, Run, RunRef};
use crate::{client, config, ratelimit};

struct Gitlab<'a> {
    data_dir: &'a Path,
    reference: RunRef,
}

/// Signed out, a pipeline that is gone, or a request GitLab cannot take:
/// asking again will not help.
fn failure(error: GitlabError) -> Failure {
    Failure {
        fatal: matches!(
            error,
            GitlabError::Auth(_)
                | GitlabError::Unconfigured
                | GitlabError::NotFound(_)
                | GitlabError::BadArg(_)
        ),
        signed_out: matches!(error, GitlabError::Auth(_) | GitlabError::Unconfigured),
        rate_reset_secs: match error {
            GitlabError::RateLimited { resets_in_secs } => Some(resets_in_secs),
            _ => None,
        },
        message: error.to_string(),
    }
}

/// A pipeline held at a manual step waits on a person, so it is as settled as a finished one.
fn settled(status: &str) -> bool {
    matches!(status, "completed" | "waiting")
}

impl Follow for Gitlab<'_> {
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
        Gitlab {
            data_dir,
            reference,
        },
        sink,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipeline_waiting_on_a_manual_job_stops_the_watch() {
        assert!(settled(&pipelines::status_of("manual", false).0));
        assert!(settled(&pipelines::status_of("success", false).0));
        assert!(!settled(&pipelines::status_of("running", false).0));
        assert!(!settled(&pipelines::status_of("pending", false).0));
    }
}
