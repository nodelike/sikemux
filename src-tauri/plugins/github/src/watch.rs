use std::path::Path;
use std::time::Duration;

use sikemux_plugin_api::codehost::{Job, Run};
use sikemux_plugin_api::watch::{self, Failure, Follow, Reading};
use sikemux_plugin_api::{PluginResult, StreamSink};

use crate::runs::{self, Followed, RunRef};

/// Below this many requests left in the hour, the run is read less often.
const SPARING_BELOW: u64 = 500;
/// One more read after a run reports itself finished, so the last job's steps
/// arrive rather than the view stopping on a half-finished picture.
const SETTLE_POLLS: u32 = 1;

struct Github<'a> {
    data_dir: &'a Path,
    reference: RunRef,
    held: Followed,
    reads_since_finished: u32,
}

impl Follow for Github<'_> {
    type Run = Run;
    type Job = Job;

    const POLL_INTERVAL: Duration = Duration::from_secs(3);

    async fn read(&mut self) -> Result<Reading<Run, Job>, Failure> {
        let (detail, remaining) = runs::follow(self.data_dir, &self.reference, &mut self.held)
            .await
            .map_err(Failure::from)?;
        Ok(Reading {
            run: detail.run,
            jobs: detail.jobs,
            sparing: remaining.is_some_and(|left| left < SPARING_BELOW),
        })
    }

    fn settled(&mut self, run: &Run, jobs: &[Job]) -> bool {
        if !runs::is_finished(&run.status) {
            self.reads_since_finished = 0;
            return false;
        }
        self.reads_since_finished = self.reads_since_finished.saturating_add(1);
        let jobs_over = !jobs.is_empty() && jobs.iter().all(|job| runs::is_finished(&job.status));
        jobs_over || self.reads_since_finished > SETTLE_POLLS
    }
}

pub async fn run(data_dir: &Path, reference: RunRef, sink: StreamSink) -> PluginResult<()> {
    let host = Github {
        data_dir,
        reference,
        held: Followed::default(),
        reads_since_finished: 0,
    };
    watch::follow(host, sink).await
}
