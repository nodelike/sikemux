use std::path::Path;
use std::time::Duration;

use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::codehost::{Job, Run};
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::watch::{self, Failure, Follow, Reading};
use sikemux_plugin_api::{PluginResult, StreamSink};

use crate::pipelines::{self, RunRef};
use crate::{client, config};

struct Bitbucket<'a> {
    data_dir: &'a Path,
    reference: RunRef,
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
            .map_err(Failure::from)?;
        let account = config::load(self.data_dir)
            .map_err(Failure::from)?
            .account(chosen().as_deref())
            .map(|account| account.id.clone())
            .unwrap_or_default();
        Ok(Reading {
            run: detail.run,
            jobs: detail.jobs,
            sparing: client::LIMITS.budget(&account).near,
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
