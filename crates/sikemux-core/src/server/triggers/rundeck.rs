//! Triggers that fire once a Rundeck run ends. Rundeck tells nobody when a run moves, so the
//! trigger reads the run through the Rundeck plugin, signed in as the person, every half minute.

use crate::server::parts::Parts;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::{fire, remove_where, Firing, Trigger};
use crate::server::{plugged, Core};

const RUNDECK: &str = "sikemux.rundeck";
pub(super) const LOOK_AGAIN_MS: u64 = 30 * 1000;
/// How many looks in a row may fail to read the run before the wait ends as unread.
const MAX_UNREAD: u32 = 10;
const ADDRESS_PATH: &str = "/execution/show/";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RundeckRun {
    execution_id: u64,
    /// When it was set, in Unix milliseconds.
    since: u64,
    /// When it fires anyway, in Unix milliseconds.
    pub(super) gives_up_at: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    unread: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// What setting the trigger found: the run already ended, or the run to wait on.
pub(super) enum Asked {
    Ended(String),
    Waits(RundeckRun, String),
}

/// The run `on` names, by its execution id or its address, and whether it already ended.
pub(super) async fn asked(
    core: &Core,
    on: &Value,
    since: u64,
    gives_up_at: u64,
) -> Result<Asked, String> {
    let execution_id = match on {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => {
            let text = text.trim();
            let id = text
                .rsplit_once(ADDRESS_PATH)
                .map_or(text, |(_, rest)| rest);
            let id = id.split(['/', '?', '#']).next().unwrap_or_default();
            id.parse().ok()
        }
        _ => None,
    }
    .ok_or("rundeck must be a Rundeck run's execution id or address")?;
    let shown = format!("Rundeck run {execution_id}");
    Ok(match ended(core, execution_id).await? {
        Some(said) => Asked::Ended(said),
        None => Asked::Waits(
            RundeckRun {
                execution_id,
                since,
                gives_up_at,
                unread: 0,
            },
            shown,
        ),
    })
}

/// How the run ended, in a line, or none while it goes on.
async fn ended(core: &Core, execution_id: u64) -> Result<Option<String>, String> {
    let read = plugged::call(
        core,
        RUNDECK,
        "agentExecution",
        json!({ "executionId": execution_id }),
    )
    .await
    .map_err(|failure| plugged::explain(&failure))?;
    if read["ended"].is_null() {
        return Ok(None);
    }
    let status = read["status"].as_str().unwrap_or("ended");
    let url = read["url"].as_str().unwrap_or_default();
    Ok(Some(
        format!("Rundeck run {execution_id} ended {status}. {url}")
            .trim_end()
            .to_owned(),
    ))
}

/// Reads the run trigger `id` waits on, and fires it once the run ended.
pub(super) async fn look(core: Arc<Core>, id: u64) {
    let run = {
        let state = core.triggers().state.locked();
        let trigger = state.triggers.iter().find(|trigger| trigger.id == id);
        trigger.and_then(|trigger| trigger.rundeck.clone())
    };
    let Some(run) = run else { return };
    let said = match ended(&core, run.execution_id).await {
        Ok(Some(said)) => said,
        Ok(None) if run.unread == 0 => return,
        Ok(None) => return read_again(&core, id, 0),
        Err(error) if run.unread + 1 < MAX_UNREAD => {
            eprintln!("sikemux core: Rundeck trigger {id} could not read its run: {error}");
            return read_again(&core, id, run.unread + 1);
        }
        Err(error) => format!(
            "Rundeck run {} could not be read {MAX_UNREAD} times in a row, so this stopped waiting: {error}",
            run.execution_id
        ),
    };
    let Some(mut trigger) = remove_where(&core, |trigger: &Trigger| trigger.id == id) else {
        return;
    };
    trigger.prompt = format!("{}\n\n{said}", trigger.prompt);
    fire(
        core,
        Firing {
            trigger,
            late: None,
            last: true,
            request: None,
        },
    )
    .await;
}

fn read_again(core: &Core, id: u64, unread: u32) {
    core.triggers().change(|state| {
        let trigger = state.triggers.iter_mut().find(|trigger| trigger.id == id);
        if let Some(run) = trigger.and_then(|trigger| trigger.rundeck.as_mut()) {
            run.unread = unread;
        }
    });
}

impl RundeckRun {
    pub(super) fn timed_out(&self, now: u64) -> String {
        let minutes = now.saturating_sub(self.since) / 60_000;
        format!(
            "Timed out: after {minutes} min Rundeck run {} had not ended. Look with rundeck_execution, then set the trigger again or carry on.",
            self.execution_id
        )
    }
}
