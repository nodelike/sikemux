//! Keeps a capped chat's vendor session small. Claude compacts it near 150K tokens, told by
//! the core what to keep and handed the core's facts after, and the core compacts it itself
//! when it sits idle long enough that the prompt cache is about to die. The chat the person
//! sees is the core's record, so none of this changes it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::AgentState;

use super::{Chat, ChatCommand};
use crate::server::Core;

/// Claude compacts about 33K tokens under the window it is given, so this compacts near 150K.
pub(crate) const CLAUDE_WINDOW: &str = "183k";
/// The callbacks Claude calls before it compacts and as it starts again after.
pub(crate) const KEEP_HOOK: &str = "sikemux-keep";
pub(crate) const FACTS_HOOK: &str = "sikemux-facts";
/// Claude cuts what a compaction hook answers past this many characters.
const HOOK_CHARS: usize = 10_000;
/// One message of the person's past this is cut, so a pasted log leaves room for the rest.
const LONGEST_SAID: usize = 1_500;
/// Claude Code keeps the prompt cache an hour; idle this long, a compaction still reads it warm.
const IDLE_COMPACT_MS: u64 = 50 * 60_000;
const CACHE_MS: u64 = 60 * 60_000;
/// Below this a cold start costs less than a compaction would save.
const IDLE_COMPACT_TOKENS: u64 = 40_000;
/// What the core asks for when it compacts an idle chat itself.
pub(crate) const IDLE_FOCUS: &str =
    "Keep what the person asked for and decided, and the work still open.";

/// Whether the chat's vendor session is kept small.
pub(crate) fn capped(agent_id: &str) -> bool {
    std::env::var("SIKEMUX_DEV_CAPPED")
        .is_ok_and(|ids| ids.split(',').any(|id| id.trim() == agent_id))
}

/// The hooks a capped Claude chat registers as it starts.
pub(crate) fn claude_hooks() -> Value {
    json!({
        "PreCompact": [{ "hookCallbackIds": [KEEP_HOOK] }],
        "SessionStart": [{ "matcher": "compact", "hookCallbackIds": [FACTS_HOOK] }],
    })
}

/// When the chat's session last called the model and how much context that call carried.
#[derive(Default)]
pub(crate) struct Meter {
    last_call_ms: AtomicU64,
    context: AtomicU64,
    /// The session was compacted since its last call, so its idle stretch needs no other.
    compacted: AtomicBool,
}

impl Meter {
    pub(crate) fn called(&self, context: u64) {
        self.last_call_ms.store(epoch_ms(), Ordering::Release);
        self.context.store(context, Ordering::Release);
        self.compacted.store(false, Ordering::Release);
    }

    pub(crate) fn compacted(&self, context: u64) {
        self.context.store(context, Ordering::Release);
        self.compacted.store(true, Ordering::Release);
    }

    /// The session has sat idle since a call whose cache is still warm, big enough that a
    /// compaction pays, and none has run since.
    fn worth_compacting(&self, now: u64) -> Option<u64> {
        let last = self.last_call_ms.load(Ordering::Acquire);
        let idle = now.saturating_sub(last);
        let warm = last > 0 && idle < CACHE_MS;
        let big = self.context.load(Ordering::Acquire) >= IDLE_COMPACT_TOKENS;
        (warm && big && !self.compacted.load(Ordering::Acquire)).then_some(idle)
    }
}

/// What a compaction must keep: the person's own words as the core's record holds them,
/// and what is still open.
pub(crate) fn keep_list(chat: &Chat) -> String {
    let head = "Sikemux keeps the record of this chat. In the summary, keep word for word, under \
                their own heading: every message of the person's below, the questions put to the \
                person that are still open, and what you or your workers are waiting on, with \
                what each waits for.";
    let budget = HOOK_CHARS.saturating_sub(head.len() + 64);
    let said = chat.feed.persons_words(budget, LONGEST_SAID);
    if said.is_empty() {
        return head.to_owned();
    }
    let lines: Vec<String> = said.iter().map(|text| format!("- {text}")).collect();
    format!(
        "{head}\n\nThe person's messages, oldest first:\n{}",
        lines.join("\n")
    )
}

/// What the core knows now that the session should have back after a compaction.
pub(crate) fn facts(core: &Core, agent_id: &str) -> Option<String> {
    let workers = workers(core, agent_id);
    if workers.is_empty() {
        return None;
    }
    let text = format!(
        "Sikemux's facts as the session goes on after its compaction.\n\nYour workers now:\n{}",
        workers.join("\n")
    );
    Some(text.chars().take(HOOK_CHARS).collect())
}

fn workers(core: &Core, agent_id: &str) -> Vec<String> {
    let mut workers: Vec<String> = core
        .workspaces
        .agent_leads()
        .into_iter()
        .filter(|(_, lead)| lead == agent_id)
        .filter_map(|(worker, _)| {
            let agent = core.workspaces.agent(&worker)?;
            let state = core
                .chats
                .get(&worker)
                .map_or(AgentState::Stopped, |chat| chat.state_now());
            let state = match state {
                AgentState::Working => "working",
                AgentState::Blocked => "waiting on the person",
                AgentState::Idle => "idle",
                _ => "stopped",
            };
            Some(format!(
                "- {}: {}, {state}",
                crate::server::messages::called_now(core, &worker),
                agent.title,
            ))
        })
        .collect();
    workers.sort();
    workers
}

/// Asks each capped chat that has sat idle long enough to compact before its cache dies.
pub(crate) fn compact_idle(core: &Core, now: u64) {
    for agent_id in core.chats.ids() {
        let Some(chat) = core.chats.get(&agent_id) else {
            continue;
        };
        let wait = std::env::var("SIKEMUX_DEV_IDLE_COMPACT_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(IDLE_COMPACT_MS);
        let due = capped(&agent_id)
            && chat.is_ready()
            && !chat.prompted()
            && chat.state_now() == AgentState::Idle
            && chat
                .meter
                .worth_compacting(now)
                .is_some_and(|idle| idle >= wait);
        if due {
            chat.meter.compacted.store(true, Ordering::Release);
            let _ = chat.send(ChatCommand::Compact);
        }
    }
}

/// A capped chat stays awake until its idle stretch has been compacted, since a sleeping
/// agent cannot compact.
pub(crate) fn holds_awake(core: &Core, agent_id: &str, now: u64) -> bool {
    capped(agent_id)
        && core
            .chats
            .get(agent_id)
            .is_some_and(|chat| chat.meter.worth_compacting(now).is_some())
}
