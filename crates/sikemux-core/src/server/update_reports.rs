//! One report to Sentry for each update once it ends: started well, failed on the way, or put
//! back. A machine that went back reports from the build it went back to. Sentry's own filter
//! drops it when the person does not share usage, as it does crashes.

use std::collections::BTreeMap;

use sentry_core::protocol::{Event, Level};
use serde::{Deserialize, Serialize};
use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::{UpdateOutcome, UpdateOutcomeState, UpdateState};

/// How one update ended, from whichever build saw it end.
pub(crate) struct Ended<'a> {
    pub id: &'a str,
    pub from: &'a str,
    pub to: Option<&'a str>,
    pub channel: UpdateChannel,
    pub platform: &'a str,
    pub result: EndedAs,
    /// What the update was doing when it stopped.
    pub step: &'static str,
    pub reason: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) enum EndedAs {
    Done,
    Failed,
    RolledBack,
}

impl EndedAs {
    fn name(self) -> &'static str {
        match self {
            EndedAs::Done => "done",
            EndedAs::Failed => "failed",
            EndedAs::RolledBack => "rolled back",
        }
    }
}

/// The step an update in `state` was at.
pub(crate) fn step_of(state: UpdateState) -> &'static str {
    match state {
        UpdateState::Downloading => "download",
        UpdateState::Installing => "install",
        UpdateState::Ready => "hand over",
        UpdateState::Idle | UpdateState::Failed => "check",
    }
}

/// The report for a written outcome, once it is final. `put_in_by_root` is a host's core, whose
/// failed outcomes root writes while it installs.
pub(crate) fn of_outcome<'a>(
    outcome: &'a UpdateOutcome,
    channel: UpdateChannel,
    platform: &'a str,
    put_in_by_root: bool,
) -> Option<Ended<'a>> {
    let (result, step) = match outcome.state {
        UpdateOutcomeState::Pending => return None,
        UpdateOutcomeState::Done => (EndedAs::Done, "ready"),
        UpdateOutcomeState::RolledBack => (EndedAs::RolledBack, "start"),
        UpdateOutcomeState::Failed if put_in_by_root => (EndedAs::Failed, "install"),
        UpdateOutcomeState::Failed => (EndedAs::Failed, "start"),
    };
    Some(Ended {
        id: &outcome.id,
        from: &outcome.from,
        to: outcome.to.as_deref(),
        channel,
        platform,
        result,
        step,
        reason: outcome.reason.as_deref(),
    })
}

/// Sends `ended` unless this update was reported already. Blocks on a small file.
pub(crate) fn send(ended: Ended<'_>) {
    if !first_report(ended.id) {
        return;
    }
    let to = ended.to.unwrap_or("unknown");
    let result = ended.result.name();
    let mut event = Event {
        level: match ended.result {
            EndedAs::Done => Level::Info,
            EndedAs::Failed | EndedAs::RolledBack => Level::Error,
        },
        logger: Some("update".into()),
        message: Some(match ended.result {
            EndedAs::Done => format!("update to {to} done"),
            _ => format!("update {result} at {}", ended.step),
        }),
        fingerprint: vec!["update".into(), result.into(), ended.step.into()].into(),
        ..Default::default()
    };
    let channel = match ended.channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Nightly => "nightly",
        UpdateChannel::Staff => "staff",
    };
    event.tags = BTreeMap::from([
        ("update.result".into(), result.into()),
        ("update.step".into(), ended.step.into()),
        ("update.from".into(), ended.from.into()),
        ("update.to".into(), to.into()),
        ("update.channel".into(), channel.into()),
        ("update.platform".into(), ended.platform.into()),
    ]);
    if let Some(reason) = ended.reason {
        event.extra.insert("reason".into(), reason.into());
    }
    event.extra.insert("update".into(), ended.id.into());
    sentry_core::Hub::main().capture_event(event);
}

/// Records `id` as the update reported last, and says whether it was not already.
fn first_report(id: &str) -> bool {
    #[derive(Serialize, Deserialize)]
    struct Reported {
        id: String,
    }
    let Some(path) = sikemux_process::paths::config_file("update-reported", "json") else {
        return true;
    };
    let before = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Reported>(&bytes).ok());
    if before.is_some_and(|before| before.id == id) {
        return false;
    }
    if let Ok(bytes) = serde_json::to_vec(&Reported { id: id.to_owned() }) {
        let _ = sikemux_process::files::write_atomic(&path, &bytes, true);
    }
    true
}
