//! Agents at work that have called no tool for a long while, such as one stuck
//! behind a queued build. Its lead hears of it once; with no lead awake to
//! hear, the person sees it with what else needs them.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::AgentStatus;

use super::changed::What;
use super::connection::blocking;
use super::roster::{self, Reach};
use super::{messages, settling, Core};

/// How long an agent at work may go without calling a tool before it counts as stalled.
pub(crate) const STALL_AFTER_MS: u64 = 30 * 60_000;

#[derive(Default)]
pub(crate) struct Stalls(Mutex<HashMap<String, Stall>>);

/// An agent that stalled, since its last sign of work.
#[derive(Clone)]
pub(crate) struct Stall {
    pub agent_id: String,
    pub since: u64,
    /// Whether it waits on the person, as no lead was awake to tell.
    pub for_person: bool,
}

impl Stalls {
    /// The stalled agents the person should hear of.
    pub(crate) fn for_person(&self) -> Vec<Stall> {
        (self.0.locked().values())
            .filter(|stall| stall.for_person)
            .cloned()
            .collect()
    }

    /// The agent called a tool, so it is no longer stalled.
    pub(crate) fn worked(&self, core: &Core, agent_id: &str) {
        let was = self.0.locked().remove(agent_id);
        if was.is_some_and(|stall| stall.for_person) {
            core.changed(What::View);
        }
    }
}

/// Marks the agents at work that stalled since the last look and tells each one's lead,
/// or the person, once. Runs with the core's regular sweep over its agents.
pub(crate) async fn look(core: &Arc<Core>, now: u64) {
    let (chats, _) = core.ranked_infos();
    let working: HashMap<&str, u64> = (chats.iter())
        .filter(|chat| chat.status == AgentStatus::Working && chat.subagents == 0)
        .filter_map(|chat| {
            let id = chat.agent_id.as_str();
            let last = [core.seen.started_at(id), core.seen.tool_at(id)]
                .into_iter()
                .flatten()
                .max()?;
            Some((id, last))
        })
        .collect();
    let (forgot, new) = {
        let mut stalls = core.stalls.0.locked();
        let before = stalls.len();
        stalls.retain(|id, stall| working.get(id.as_str()) == Some(&stall.since));
        let forgot = stalls.len() != before;
        let new: Vec<(String, u64)> = (working.iter())
            .filter(|(id, last)| {
                now.saturating_sub(**last) >= STALL_AFTER_MS && !stalls.contains_key(**id)
            })
            .map(|(id, last)| ((*id).to_owned(), *last))
            .collect();
        (forgot, new)
    };
    if new.is_empty() {
        if forgot {
            core.changed(What::View);
        }
        return;
    }
    let owner = core.clone();
    let awake: HashSet<String> = blocking(move || {
        Ok((roster::listed(&owner).into_iter())
            .filter(|agent| matches!(agent.reach, Reach::Chat | Reach::Terminal))
            .map(|agent| agent.agent_id)
            .collect())
    })
    .await
    .unwrap_or_default();
    let mut for_person = forgot;
    for (agent_id, since) in new {
        let lead = (core.names.get(&agent_id))
            .and_then(|named| named.parent)
            .filter(|lead| awake.contains(lead));
        let told = match lead {
            Some(lead) => {
                let text = note(core, &agent_id, since, now);
                messages::work_from_sikemux(core, lead, text).await.is_ok()
            }
            None => false,
        };
        if !told {
            settling::needs_person(core, &agent_id);
            for_person = true;
        }
        core.stalls.0.locked().insert(
            agent_id.clone(),
            Stall {
                agent_id,
                since,
                for_person: !told,
            },
        );
    }
    if for_person {
        core.changed(What::View);
    }
}

fn note(core: &Core, agent_id: &str, since: u64, now: u64) -> String {
    format!(
        "@{} has been working for {} min without calling a tool; it may be stuck, such as on a queued build. Read it with agent_read and tell it what to do, or close it.",
        core.names.name_or_id(agent_id),
        now.saturating_sub(since) / 60_000
    )
}

/// How long ago `since` was, for the person.
pub(crate) fn quiet_for(since: u64) -> String {
    format!("{} min", epoch_ms().saturating_sub(since) / 60_000)
}
