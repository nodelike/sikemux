//! Agents at work that have called no tool for a long while, such as one stuck
//! behind a queued build. Whoever hears of the agent ([`roster::hears_of`]) is told
//! once; in a project with no chief, the person sees it with what else needs them.

use crate::server::changed::ChangedCore;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::AgentStatus;

use super::changed::What;
use super::roster::{self, Hears};
use super::{messages, settling, Core};

/// How long an agent at work may go without calling a tool before it counts as stalled.
pub(crate) const STALL_AFTER_MS: u64 = 30 * 60_000;
/// How long an agent at work with no running tools, tasks or subagents may stay completely
/// silent before warning whoever watches it.
pub(crate) const SILENCE_AFTER_MS: u64 = 10 * 60_000;

#[derive(Default)]
pub(crate) struct Stalls(Mutex<HashMap<String, Stall>>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StallKind {
    NoTool,
    Silent,
}

/// An agent that stalled, since its last sign of work.
#[derive(Clone)]
pub(crate) struct Stall {
    pub agent_id: String,
    pub since: u64,
    pub kind: StallKind,
    /// Whether it waits on the person, as nobody else hears of it.
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

/// Marks the agents at work that stalled since the last look and tells whoever hears of each,
/// once. Runs with the core's regular sweep over its agents.
pub(crate) async fn look(core: &Arc<Core>, now: u64) {
    let (chats, _) = core.ranked_infos();
    let working: HashMap<&str, (u64, u64, bool)> = (chats.iter())
        .filter(|chat| chat.status == AgentStatus::Working && chat.subagents == 0)
        .filter_map(|chat| {
            let id = chat.agent_id.as_str();
            let last_tool = [core.seen().started_at(id), core.seen().tool_at(id)]
                .into_iter()
                .flatten()
                .max()?;
            let live = core.chats().get(id);
            let last_byte = [
                Some(last_tool),
                live.as_deref().and_then(|chat| chat.streamed_at()),
            ]
            .into_iter()
            .flatten()
            .max()?;
            let silent_eligible =
                chat.tasks == 0 && live.as_deref().is_some_and(|chat| !chat.has_open_tools());
            Some((id, (last_tool, last_byte, silent_eligible)))
        })
        .collect();
    let (forgot, new) = {
        let mut stalls = core.stalls().0.locked();
        let before = stalls.len();
        stalls.retain(|id, stall| {
            working
                .get(id.as_str())
                .is_some_and(
                    |&(last_tool, last_byte, silent_eligible)| match stall.kind {
                        StallKind::NoTool => last_tool == stall.since,
                        StallKind::Silent => silent_eligible && last_byte == stall.since,
                    },
                )
        });
        let forgot = stalls.len() != before;
        let new: Vec<(String, StallKind, u64)> = (working.iter())
            .filter_map(|(id, &(last_tool, last_byte, silent_eligible))| {
                if stalls.contains_key(*id) {
                    return None;
                }
                if now.saturating_sub(last_tool) >= STALL_AFTER_MS {
                    Some(((*id).to_owned(), StallKind::NoTool, last_tool))
                } else if silent_eligible && now.saturating_sub(last_byte) >= SILENCE_AFTER_MS {
                    Some(((*id).to_owned(), StallKind::Silent, last_byte))
                } else {
                    None
                }
            })
            .collect();
        (forgot, new)
    };
    if new.is_empty() {
        if forgot {
            core.changed(What::View);
        }
        return;
    }
    let mut for_person = forgot;
    for (agent_id, kind, since) in new {
        let hears = roster::hears_of(core, &agent_id);
        let why = match hears {
            Hears::Lead(_) => "because a worker of yours has stalled",
            _ => "because an agent of the project you are chief of has stalled",
        };
        let told = match hears.agent() {
            Some(to) => {
                let text = note(core, &agent_id, kind, since, now);
                messages::work_from_sikemux(core, to.to_owned(), text, why)
                    .await
                    .is_ok()
            }
            None => false,
        };
        if !told {
            settling::needs_person(core, &agent_id);
            for_person = true;
        }
        core.stalls().0.locked().insert(
            agent_id.clone(),
            Stall {
                agent_id,
                since,
                kind,
                for_person: !told,
            },
        );
    }
    if for_person {
        core.changed(What::View);
    }
}

fn note(core: &Core, agent_id: &str, kind: StallKind, since: u64, now: u64) -> String {
    let name = core.names().name_or_id(agent_id);
    let mins = now.saturating_sub(since) / 60_000;
    match kind {
        StallKind::NoTool => format!(
            "@{name} has been working for {mins} min without calling a tool; it may be stuck, such as on a queued build. Read it with agent_read and tell it what to do, or close it."
        ),
        StallKind::Silent => format!(
            "@{name} has been working for {mins} min with no streamed output or tool calls; it may be stuck waiting on the model. Read it with agent_read and tell it what to do, or stop it."
        ),
    }
}

/// How long ago `since` was, for the person.
pub(crate) fn quiet_for(since: u64) -> String {
    format!("{} min", epoch_ms().saturating_sub(since) / 60_000)
}
