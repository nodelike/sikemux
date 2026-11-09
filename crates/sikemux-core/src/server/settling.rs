//! Snoozing and settling agents out of the main list. The person snoozes an agent until a
//! time; the core settles one the person has seen, that has been quiet a while and left
//! nothing uncommitted. Either comes back by itself once the agent needs the person.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_view::inbox::AgentShown;
use sikemux_wire::protocol::AgentGroup;
use tokio::sync::Notify;

use super::chat::reset::{calendar, WallClock, Zone};
use super::connection::blocking;
use super::workspace::{ListPlace, Settled, Snooze};
use super::{inbox, Core, CoreResult};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// How long an agent stays quiet, as far as this core has seen, before the core settles it.
const SETTLE_AFTER_MS: u64 = 3 * HOUR_MS;
/// How often the core looks for agents to settle.
const SWEEP_MS: u64 = 5 * MINUTE_MS;

pub(crate) struct Settling {
    /// A snooze changed, so the next wake-up may be sooner.
    changed: Notify,
    started_at: u64,
}

impl Default for Settling {
    fn default() -> Self {
        Self {
            changed: Notify::new(),
            started_at: epoch_ms(),
        }
    }
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
            () = core.settling.changed.notified() => {}
        }
    }
}

/// Wakes the snoozes whose time came and settles the agents ready for it, answering how
/// long until it should look again.
async fn sweep(core: &Arc<Core>) -> u64 {
    let now = epoch_ms();
    let places = core.workspaces.places();
    let due: Vec<String> = (places.iter())
        .filter(|(_, (place, _))| place.snooze.is_some_and(|snooze| snooze.until <= now))
        .map(|(agent_id, _)| agent_id.clone())
        .collect();
    core.workspaces
        .move_places(&due, |place| place.snooze = None);
    let ready = ready_to_settle(core, &places, now);
    let checking = core.clone();
    let settle = blocking(move || {
        Ok(ready
            .into_iter()
            .filter(|(agent_id, project, cwd)| {
                !inbox::uncommitted(&checking, project, agent_id, cwd)
            })
            .map(|(agent_id, _, _)| agent_id)
            .collect::<Vec<_>>())
    })
    .await
    .unwrap_or_default();
    core.workspaces.move_places(&settle, |place| {
        let written_since = place.said_at.is_some_and(|at| at > now);
        if place.settled.is_none() && place.snooze.is_none() && !written_since {
            place.settled = Some(Settled {
                at: now,
                by_core: true,
            });
        }
    });
    let next_wake = (places.values())
        .filter_map(|(place, _)| place.snooze)
        .map(|snooze| snooze.until)
        .filter(|&until| until > now)
        .min();
    next_wake.map_or(SWEEP_MS, |until| (until - now).min(SWEEP_MS))
}

/// The agents, with their project and folder, that the person has seen, that sit idle and
/// have been quiet long enough. A lead waits for its workers to settle first.
fn ready_to_settle(
    core: &Core,
    places: &HashMap<String, (ListPlace, bool)>,
    now: u64,
) -> Vec<(String, String, std::path::PathBuf)> {
    let (chats, sessions) = core.ranked_infos();
    let mut restless: HashSet<&str> = (chats.iter())
        .filter(|chat| chat.group != AgentGroup::Idle || chat.subagents + chat.tasks > 0)
        .map(|chat| chat.agent_id.as_str())
        .collect();
    restless.extend(sessions.iter().filter_map(|session| {
        let busy = session.group.is_some_and(|group| group != AgentGroup::Idle);
        session.agent_id.as_deref().filter(|_| busy)
    }));
    let leads = core.workspaces.agent_leads();
    let leading: HashSet<&str> = (leads.iter())
        .filter(|(worker, _)| {
            places
                .get(*worker)
                .is_some_and(|(place, _)| place.settled.is_none())
        })
        .map(|(_, lead)| lead.as_str())
        .collect();
    (core.workspace_agents().into_iter())
        .filter(|agent| {
            let Some((place, pinned)) = places.get(&agent.id) else {
                return false;
            };
            let quiet_since = [
                Some(core.settling.started_at),
                Some(agent.created_at),
                place.said_at,
                place.unsettled_at,
                core.seen.quiet_since(&agent.id),
            ]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(now);
            place.settled.is_none()
                && place.snooze.is_none()
                && !pinned
                && !restless.contains(agent.id.as_str())
                && !leading.contains(agent.id.as_str())
                && !core.seen.unread(&agent.id)
                && !core.seen.viewed(&agent.id)
                && now.saturating_sub(quiet_since) >= SETTLE_AFTER_MS
        })
        .map(|agent| (agent.id, agent.project, agent.cwd))
        .collect()
}

/// The agent asked the person something or ran into a problem, so it comes back to the
/// main list whether snoozed or settled.
pub(crate) fn needs_person(core: &Core, agent_id: &str) {
    let now = epoch_ms();
    core.workspaces
        .move_places(&[agent_id.to_owned()], |place| unsettle(place, now));
}

fn unsettle(place: &mut ListPlace, now: u64) {
    place.snooze = None;
    if place.settled.take().is_some() {
        place.unsettled_at = Some(now);
    }
}

/// Its place in the lists as clients are sent it.
pub(crate) fn show(shown: &mut AgentShown, place: &ListPlace, now: u64) {
    shown.snoozed_until = (place.snooze)
        .map(|snooze| snooze.until)
        .filter(|&until| until > now);
    shown.settled_at = place.settled.map(|settled| settled.at);
    shown.auto_settled = place.settled.is_some_and(|settled| settled.by_core);
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeParams {
    pub agent_ids: Vec<String>,
    /// When it wakes, in Unix milliseconds; none wakes it now.
    #[serde(default)]
    pub until: Option<u64>,
}

pub(crate) fn snooze(core: &Core, params: SnoozeParams) -> CoreResult<()> {
    let now = epoch_ms();
    if params.until.is_some_and(|until| until <= now) {
        return Err("that time has already passed".into());
    }
    core.workspaces
        .move_places(&params.agent_ids, |place| match params.until {
            Some(until) => {
                unsettle(place, now);
                place.snooze = Some(Snooze { until, at: now });
            }
            None => place.snooze = None,
        });
    core.settling.changed.notify_one();
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleParams {
    pub agent_ids: Vec<String>,
    pub settled: bool,
}

pub(crate) fn settle(core: &Core, params: SettleParams) -> CoreResult<()> {
    let now = epoch_ms();
    core.workspaces.move_places(&params.agent_ids, |place| {
        if !params.settled {
            unsettle(place, now);
        } else if place.settled.is_none() {
            place.snooze = None;
            place.settled = Some(Settled {
                at: now,
                by_core: false,
            });
        }
    });
    Ok(())
}

/// A time the snooze menu offers, in the host's own clock.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePreset {
    pub label: String,
    pub until: u64,
}

/// The snooze menu's times, soonest first, one per minute: in one and three hours, this
/// evening while it is still to come, tomorrow morning and next Monday morning.
pub(crate) fn presets() -> Vec<SnoozePreset> {
    let now = epoch_ms();
    let at = |days_later: i32, hour: i32| {
        let clock = WallClock {
            zone: Zone::Host,
            date: None,
            hour,
            minute: 0,
        };
        clock.on_day(now, days_later)
    };
    let monday_in = match calendar(now, Zone::Host).map(|today| (8 - today.tm_wday) % 7) {
        Some(0) | None => 7,
        Some(days) => days,
    };
    let mut presets: Vec<SnoozePreset> = [
        ("In 1 hour", Some(now + HOUR_MS)),
        ("In 3 hours", Some(now + 3 * HOUR_MS)),
        ("This evening", at(0, 18)),
        ("Tomorrow", at(1, 9)),
        ("Next week", at(monday_in, 9)),
    ]
    .into_iter()
    .filter_map(|(label, until)| {
        let until = until.filter(|&until| until > now)?;
        Some(SnoozePreset {
            label: label.to_owned(),
            until,
        })
    })
    .collect();
    presets.sort_by_key(|preset| preset.until / MINUTE_MS);
    presets.dedup_by_key(|preset| preset.until / MINUTE_MS);
    presets
}
