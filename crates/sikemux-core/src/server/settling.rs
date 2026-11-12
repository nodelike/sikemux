//! Snoozing and settling agents out of the main list. The person snoozes an agent until a
//! time; the core settles one the person has seen, that has been quiet a while and left
//! nothing uncommitted. Either comes back by itself once the agent needs the person.

use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
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
            () = core.settling().changed.notified() => {}
        }
    }
}

/// Wakes the snoozes whose time came and settles the agents ready for it, answering how
/// long until it should look again.
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
    let chiefs = core.workspaces().chiefs();
    let ready = ready_to_settle(core, &places, now);
    let checking = core.clone();
    let settle = blocking(move || {
        Ok(ready
            .into_iter()
            .filter(|(agent_id, project, cwd)| {
                !chiefs.contains(agent_id) && !inbox::uncommitted(&checking, project, agent_id, cwd)
            })
            .map(|(agent_id, _, _)| agent_id)
            .collect::<Vec<_>>())
    })
    .await
    .unwrap_or_default();
    core.workspaces().move_places(&settle, |place| {
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
    let leads = core.workspaces().agent_leads();
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
                Some(core.settling().started_at),
                Some(agent.created_at),
                place.said_at,
                place.unsettled_at,
                core.seen().quiet_since(&agent.id),
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
                && !core.seen().unread(&agent.id)
                && !core.seen().viewed(&agent.id)
                && now.saturating_sub(quiet_since) >= SETTLE_AFTER_MS
        })
        .map(|agent| (agent.id, agent.project, agent.cwd))
        .collect()
}

/// The agent asked the person something or ran into a problem, so it comes back to the
/// main list whether snoozed or settled.
pub(crate) fn needs_person(core: &Core, agent_id: &str) {
    let now = epoch_ms();
    core.workspaces()
        .move_places(&[agent_id.to_owned()], |place| unsettle(place, now));
}

/// The agent began a turn, whoever asked for it, so it no longer sits settled out of sight.
/// A snooze the person chose still runs out.
pub(crate) fn turn_started(core: &Core, agent_id: &str) {
    let now = epoch_ms();
    core.workspaces()
        .move_places(&[agent_id.to_owned()], |place| {
            if place.settled.take().is_some() {
                place.unsettled_at = Some(now);
            }
        });
}

fn unsettle(place: &mut ListPlace, now: u64) {
    place.snooze = None;
    if place.settled.take().is_some() {
        place.unsettled_at = Some(now);
    }
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
    /// When it wakes, in Unix milliseconds; none wakes it now, unless `typed` names a time.
    #[serde(default)]
    pub until: Option<u64>,
    /// A time in words this host reads on its own clock, such as "fri 2pm" or "in 90m".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<String>,
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
                unsettle(place, now);
                place.snooze = Some(Snooze { until, at: now });
            }
            None => place.snooze = None,
        });
    core.settling().changed.notify_one();
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
    let agent_ids = match params.settled {
        true => not_chiefs(core, params.agent_ids)?,
        false => params.agent_ids,
    };
    core.workspaces().move_places(&agent_ids, |place| {
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

/// A time the snooze menu offers, on this computer's clock.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePreset {
    pub label: String,
    pub until: u64,
}

/// `hour` o'clock `days_later` days from now on this computer's clock.
fn at_hour(now: u64, days_later: i32, hour: i32, minute: i32) -> Option<u64> {
    let clock = WallClock {
        zone: Zone::Host,
        date: None,
        hour,
        minute,
    };
    clock.on_day(now, days_later)
}

/// Days from today until `weekday`, 0 for Sunday, never 0 itself: today's weekday is a week on.
fn days_until(now: u64, weekday: i32) -> i32 {
    match calendar(now, Zone::Host).map(|today| (weekday - today.tm_wday).rem_euclid(7)) {
        Some(0) | None => 7,
        Some(days) => days,
    }
}

/// The snooze menu's times, soonest first, one per minute: in one and three hours, this
/// evening while it is still to come, tomorrow morning and next Monday morning.
pub fn presets() -> Vec<SnoozePreset> {
    let now = epoch_ms();
    let mut presets: Vec<SnoozePreset> = [
        ("In 1 hour", Some(now + HOUR_MS)),
        ("In 3 hours", Some(now + 3 * HOUR_MS)),
        ("This evening", at_hour(now, 0, 18, 0)),
        ("Tomorrow", at_hour(now, 1, 9, 0)),
        ("Next week", at_hour(now, days_until(now, 1), 9, 0)),
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

const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// When a time the person typed comes next, such as "in 90m", "2h", "tomorrow 8",
/// "fri 2pm" or "14:30"; none for words it cannot read or a time already past.
pub fn parse_time(typed: &str) -> Option<u64> {
    let now = epoch_ms();
    let typed = typed.trim().to_lowercase();
    let words: Vec<&str> = typed
        .strip_prefix("in ")
        .unwrap_or(&typed)
        .split_whitespace()
        .collect();
    if let Some(span) = span(&words) {
        return Some(now + span);
    }
    let (day, clock) = match words.as_slice() {
        [day, clock] => (Some(*day), Some(*clock)),
        [one] if day_offset(now, one).is_some() => (Some(*one), None),
        [one] => (None, Some(*one)),
        _ => return None,
    };
    let days = match day {
        Some(day) => day_offset(now, day)?,
        None => 0,
    };
    let (hour, minute) = match clock {
        Some(clock) => wall_clock(clock)?,
        None => (9, 0),
    };
    let until = at_hour(now, days, hour, minute)?;
    match until > now {
        true => Some(until),
        false if day.is_none() => at_hour(now, 1, hour, minute),
        false => None,
    }
}

/// "90m", "2 h", "3 days" as milliseconds.
fn span(words: &[&str]) -> Option<u64> {
    let joined = words.concat();
    let digits = joined.find(|c: char| !c.is_ascii_digit())?;
    let (count, unit) = joined.split_at(digits);
    let count: u64 = count.parse().ok().filter(|&count| count > 0)?;
    let unit = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => MINUTE_MS,
        "h" | "hr" | "hrs" | "hour" | "hours" => HOUR_MS,
        "d" | "day" | "days" => 24 * HOUR_MS,
        _ => return None,
    };
    count.checked_mul(unit)
}

fn day_offset(now: u64, word: &str) -> Option<i32> {
    match word {
        "today" => Some(0),
        "tomorrow" | "tmrw" => Some(1),
        word if word.len() >= 3 => WEEKDAYS
            .iter()
            .position(|day| word.starts_with(day))
            .map(|weekday| days_until(now, weekday as i32)),
        _ => None,
    }
}

/// "2pm", "2:30pm", "14:30" or "8" as the hour of the day and its minute.
fn wall_clock(word: &str) -> Option<(i32, i32)> {
    let (clock, pm) = match (word.strip_suffix("pm"), word.strip_suffix("am")) {
        (Some(clock), _) => (clock, Some(true)),
        (_, Some(clock)) => (clock, Some(false)),
        _ => (word, None),
    };
    let (hour, minute) = clock.split_once(':').unwrap_or((clock, "0"));
    let (hour, minute): (i32, i32) = (hour.parse().ok()?, minute.parse().ok()?);
    if !(0..60).contains(&minute) {
        return None;
    }
    match pm {
        Some(pm) if (1..=12).contains(&hour) => Some((hour % 12 + if pm { 12 } else { 0 }, minute)),
        None if (0..24).contains(&hour) => Some((hour, minute)),
        _ => None,
    }
}
