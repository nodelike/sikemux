//! Snoozing and settling agents out of the main list. The person snoozes an agent until a
//! time, or settles one they are done with. Either comes back by itself once the agent needs
//! the person.

use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::settling::SnoozePreset;
pub use sikemux_calls::settling::{parse_time, presets, HOUR_MS, MINUTE_MS};
pub use sikemux_calls::settling::{SettleParams, SnoozeParams};
use std::sync::Arc;
use std::time::Duration;

use sikemux_process::clock::epoch_ms;
use sikemux_view::inbox::AgentShown;
use tokio::sync::Notify;

use super::workspace::{ListPlace, Settled, Snooze};
use super::{Core, CoreResult};

/// How often the core looks over its agents when no snooze wakes sooner.
const SWEEP_MS: u64 = 5 * MINUTE_MS;

#[derive(Default)]
pub(crate) struct Settling {
    /// A snooze changed, so the next wake-up may be sooner.
    changed: Notify,
}

impl super::part::Part for Settling {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(run(core.clone()))]
    }
}

async fn run(core: Arc<Core>) {
    loop {
        let next = sweep(&core).await;
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(next)) => {}
            () = core.settling().changed.notified() => {}
        }
    }
}

/// Wakes the snoozes whose time came, answering how long until it should look again.
async fn sweep(core: &Arc<Core>) -> u64 {
    let now = epoch_ms();
    super::stalls::look(core, now).await;
    let places = core.workspaces().places();
    let due: Vec<String> = (places.iter())
        .filter(|(_, (place, _))| place.snooze.is_some_and(|snooze| snooze.until <= now))
        .map(|(agent_id, _)| agent_id.clone())
        .collect();
    core.workspaces()
        .move_places(&due, |place| place.snooze = None);
    let next_wake = (places.values())
        .filter_map(|(place, _)| place.snooze)
        .map(|snooze| snooze.until)
        .filter(|&until| until > now)
        .min();
    next_wake.map_or(SWEEP_MS, |until| (until - now).min(SWEEP_MS))
}

/// The agent asked the person something or ran into a problem, so it comes back to the
/// main list whether snoozed or settled.
pub(crate) fn needs_person(core: &Core, agent_id: &str) {
    core.workspaces()
        .move_places(&[agent_id.to_owned()], unsettle);
}

/// The agent began a turn, whoever asked for it, so it no longer sits settled out of sight.
/// A snooze the person chose still runs out.
pub(crate) fn turn_started(core: &Core, agent_id: &str) {
    core.workspaces()
        .move_places(&[agent_id.to_owned()], |place| place.settled = None);
}

fn unsettle(place: &mut ListPlace) {
    place.snooze = None;
    place.settled = None;
}

/// When the agent at `place` wakes from a snooze still to run out.
pub(crate) fn snoozed_until(place: &ListPlace, now: u64) -> Option<u64> {
    (place.snooze)
        .map(|snooze| snooze.until)
        .filter(|&until| until > now)
}

/// Its place in the lists as clients are sent it.
pub(crate) fn show(shown: &mut AgentShown, place: &ListPlace, now: u64) {
    shown.snoozed_until = snoozed_until(place, now);
    shown.settled_at = place.settled.map(|settled| settled.at);
}

/// The agents in the main list, then the snoozed ones soonest to wake first, then the
/// settled ones most recently settled first.
pub(crate) fn sections(
    agents: Vec<AgentShown>,
) -> (Vec<AgentShown>, Vec<AgentShown>, Vec<AgentShown>) {
    let (mut settled, rest): (Vec<_>, Vec<_>) = agents
        .into_iter()
        .partition(|agent| agent.settled_at.is_some());
    let (mut snoozed, main): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|agent| agent.snoozed_until.is_some());
    snoozed.sort_by_key(|agent| agent.snoozed_until);
    settled.sort_by_key(|agent| std::cmp::Reverse(agent.settled_at));
    (main, snoozed, settled)
}

pub(crate) fn snooze(core: &Core, params: SnoozeParams) -> CoreResult<()> {
    let now = epoch_ms();
    let until = match &params.typed {
        Some(typed) => Some(parse_time(typed).ok_or("that is not a time to come")?),
        None => params.until,
    };
    if until.is_some_and(|until| until <= now) {
        return Err("that time has already passed".into());
    }
    let agent_ids = match until {
        Some(_) => not_chiefs(core, params.agent_ids)?,
        None => params.agent_ids,
    };
    core.workspaces()
        .move_places(&agent_ids, |place| match until {
            Some(until) => {
                place.settled = None;
                place.snooze = Some(Snooze { until, at: now });
            }
            None => place.snooze = None,
        });
    core.settling().changed.notify_one();
    Ok(())
}

pub(crate) fn settle(core: &Core, params: SettleParams) -> CoreResult<()> {
    let now = epoch_ms();
    let agent_ids = match params.settled {
        true => not_chiefs(core, params.agent_ids)?,
        false => params.agent_ids,
    };
    core.workspaces().move_places(&agent_ids, |place| {
        if !params.settled {
            unsettle(place);
        } else if place.settled.is_none() {
            place.snooze = None;
            place.settled = Some(Settled { at: now });
        }
    });
    Ok(())
}

/// The chats of `agent_ids` that may be put out of the way: a project's chief keeps its row.
fn not_chiefs(core: &Core, agent_ids: Vec<String>) -> CoreResult<Vec<String>> {
    let chiefs = core.workspaces().chiefs();
    let others: Vec<String> = (agent_ids.iter())
        .filter(|agent_id| !chiefs.contains(*agent_id))
        .cloned()
        .collect();
    if others.is_empty() && !agent_ids.is_empty() {
        return Err("@Chief keeps its row; it is never snoozed or settled".into());
    }
    Ok(others)
}
