//! Keeps a capped chat's vendor session small. Claude compacts it near 150K tokens, told by
//! the core what to keep and handed the core's facts after. The core compacts any big session
//! itself when it sits idle long enough that the prompt cache is about to die, so coming back
//! to it later does not read it all again uncached. The chat the person sees is the core's
//! record, so none of this changes it.

use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::AgentState;

use sikemux_view::calls::CallState;
use sikemux_view::turn::{TurnPhase, TurnState, TurnWait, TurnWaitKind};

use super::{Chat, ChatCommand};
use crate::server::messages::called_now;
use crate::server::{inbox, person_calls, roster, Core, CoreResult};
use sikemux_view::chat::ExpiredCache;

/// Claude cuts what a compaction hook answers past this many characters.
const HOOK_CHARS: usize = 10_000;
/// One message of the person's past this is cut, so a pasted log leaves room for the rest.
const LONGEST_SAID: usize = 1_500;
/// One line of what is open past this is cut, so a long command a worker waits on leaves the
/// person's words their room.
const LONGEST_OPEN: usize = 300;
/// What is open takes at most this much of the keep-list, so the person's words always keep
/// the rest, however many workers are blocked.
const OPEN_SHARE: usize = 3_000;
/// A compaction Claude never answered stops holding the chat awake after this.
const COMPACTING_MS: u64 = 10 * 60_000;
/// Claude Code keeps the prompt cache an hour; idle this long, a compaction still reads it warm.
const IDLE_COMPACT_MS: u64 = 50 * 60_000;
const CACHE_MS: u64 = 60 * 60_000;
/// Below this a cold start costs less than a compaction would save.
const IDLE_COMPACT_TOKENS: u64 = 40_000;
/// A chat that is not capped keeps its whole session unless it is this big, as the person may
/// come back for its detail.
const IDLE_COMPACT_UNCAPPED_TOKENS: u64 = 150_000;
/// When the chat's session last called the model and how much context that call carried.
#[derive(Default)]
pub(crate) struct Meter {
    last_call_ms: AtomicU64,
    context: AtomicU64,
    /// The session was compacted since its last call, so its idle stretch needs no other.
    compacted: AtomicBool,
    /// When the core asked for the compaction still running, or 0.
    compacting_since: AtomicU64,
    /// The person chose to keep the full history for the next message after the cache expired.
    keeps_history: AtomicBool,
    /// The person stopped the turn while the chat was summarised ahead of its message.
    summarise_stopped: AtomicBool,
}

/// What a meter keeps while its chat sleeps or its core starts again.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeptMeter {
    last_call_ms: u64,
    context: u64,
    compacted: bool,
}

impl Meter {
    pub(crate) fn kept(&self) -> KeptMeter {
        KeptMeter {
            last_call_ms: self.last_call_ms.load(Ordering::Acquire),
            context: self.context.load(Ordering::Acquire),
            compacted: self.compacted.load(Ordering::Acquire),
        }
    }

    pub(crate) fn take_up(&self, kept: KeptMeter) {
        self.last_call_ms
            .store(kept.last_call_ms, Ordering::Release);
        self.context.store(kept.context, Ordering::Release);
        self.compacted.store(kept.compacted, Ordering::Release);
    }

    pub(crate) fn called(&self, context: u64) {
        self.last_call_ms.store(epoch_ms(), Ordering::Release);
        self.context.store(context, Ordering::Release);
        self.compacted.store(false, Ordering::Release);
        self.keeps_history.store(false, Ordering::Release);
    }

    pub(crate) fn compacted(&self, context: u64) {
        self.context.store(context, Ordering::Release);
        self.compacted.store(true, Ordering::Release);
    }

    /// The core's own compaction runs, so the agent is at work though no turn shows.
    pub(crate) fn compacting(&self) -> bool {
        let since = self.compacting_since.load(Ordering::Acquire);
        since > 0 && epoch_ms().saturating_sub(since) < COMPACTING_MS
    }

    pub(crate) fn context(&self) -> u64 {
        self.context.load(Ordering::Acquire)
    }

    pub(crate) fn last_call_ms(&self) -> u64 {
        self.last_call_ms.load(Ordering::Acquire)
    }

    /// A fresh session starts with nothing in it, and no call yet.
    pub(crate) fn started_fresh(&self) {
        self.last_call_ms.store(0, Ordering::Release);
        self.context.store(0, Ordering::Release);
        self.compacted.store(false, Ordering::Release);
    }

    pub(crate) fn compaction_over(&self) {
        self.compacting_since.store(0, Ordering::Release);
    }

    /// The session has sat idle since a call whose cache is still warm, big enough that a
    /// compaction pays, and none has run since.
    fn worth_compacting(&self, now: u64, big_from: u64) -> Option<u64> {
        let last = self.last_call_ms.load(Ordering::Acquire);
        let idle = now.saturating_sub(last);
        let warm = last > 0 && idle < CACHE_MS;
        let big = self.context.load(Ordering::Acquire) >= big_from;
        (warm && big && !self.compacted.load(Ordering::Acquire)).then_some(idle)
    }
}

/// What a compaction must keep: what is still open, and the person's own words as the core's
/// record holds them.
pub(crate) fn keep_list(core: &Core, chat: &Chat) -> String {
    let mut text = "Sikemux keeps the record of this chat. In the summary, keep under their own \
                    heading, word for word: every message of the person's below, every call still \
                    open to the person, and what you and your workers are waiting on, with what \
                    each waits for. A message ending in [cut] was longer: only its start is here, \
                    so keep that start and say it was cut."
        .to_owned();
    let open = open_work(core, chat.agent_id());
    if !open.is_empty() {
        text.push_str("\n\nStill open, as Sikemux has it now:");
        let mut room = OPEN_SHARE;
        for (shown, line) in open.iter().enumerate() {
            let Some(left) = room.checked_sub(line.chars().count() + 1) else {
                text.push_str(&format!("\n- and {} more", open.len() - shown));
                break;
            };
            room = left;
            text.push('\n');
            text.push_str(line);
        }
    }
    const SAID: &str = "\n\nThe person's messages, oldest first:";
    let budget = HOOK_CHARS.saturating_sub(text.chars().count() + SAID.len());
    let said = chat.feed.persons_words(budget, LONGEST_SAID);
    if !said.is_empty() {
        text.push_str(SAID);
        for said in said {
            text.push_str("\n- ");
            text.push_str(&said);
        }
    }
    text
}

/// What a session is handed back after its compaction: what Sikemux keeps for the project,
/// unless its system prompt holds that already, and for a capped chat what is open now.
pub(crate) fn after_compaction(core: &Core, chat: &Chat) -> Option<String> {
    let agent_id = chat.agent_id();
    let brief = (chat.brief().is_none())
        .then(|| crate::server::memory::brief(core, agent_id, None))
        .flatten();
    let open = if chat.capped() {
        open_work(core, agent_id)
    } else {
        Vec::new()
    };
    let facts = (!open.is_empty()).then(|| {
        format!(
            "Sikemux's facts as the session goes on after its compaction:\n{}",
            open.join("\n")
        )
    });
    let text: Vec<String> = [brief, facts].into_iter().flatten().collect();
    if text.is_empty() {
        return None;
    }
    Some(text.join("\n\n").chars().take(HOOK_CHARS).collect())
}

/// The calls open to the person in the chat's project, what the chat waits on, and where each
/// of its workers stands.
pub(crate) fn open_work(core: &Core, agent_id: &str) -> Vec<String> {
    let listed = roster::listed(core);
    let Some(own) = listed.iter().find(|agent| agent.agent_id == agent_id) else {
        return Vec::new();
    };
    let (chats, sessions) = core.ranked_infos();
    let turns: HashMap<String, TurnState> = inbox::view(core, &chats, &sessions)
        .agents
        .into_iter()
        .filter_map(|agent| Some((agent.agent_id, agent.turn?)))
        .collect();
    let now = epoch_ms();
    let mut lines: Vec<String> = person_calls::shown(core)
        .into_iter()
        .filter(|call| call.state == CallState::Open && call.project == own.project)
        .map(|call| {
            format!(
                "- A call to the person from @{}, open {}: {}",
                call.asker.name,
                ago(now, call.raised_at),
                call.question
            )
        })
        .collect();
    let turn = |agent: &roster::Listed| turns.get(&agent.agent_id);
    if let Some(wait) = turn(own).and_then(|turn| turn.wait.as_ref()) {
        lines.push(format!("- You wait on {}", waited(wait, now)));
    }
    let leads = core.workspaces().agent_leads();
    let mut workers: Vec<String> = (listed.iter())
        .filter(|agent| leads.get(&agent.agent_id).map(String::as_str) == Some(agent_id))
        .map(|worker| {
            let title = worker.title.as_deref().unwrap_or("no title yet");
            let stands =
                turn(worker).map_or_else(|| "not started".to_owned(), |turn| standing(turn, now));
            format!(
                "- Your worker {}: {title}. {stands}",
                called_now(core, &worker.agent_id)
            )
        })
        .collect();
    workers.sort();
    lines.extend(workers);
    for line in &mut lines {
        if line.chars().count() > LONGEST_OPEN {
            *line = line
                .chars()
                .take(LONGEST_OPEN)
                .chain(" [cut]".chars())
                .collect();
        }
    }
    lines
}

pub(crate) fn standing(turn: &TurnState, now: u64) -> String {
    match turn.state {
        TurnPhase::Working => match &turn.on {
            Some(on) => format!("Working on {on}"),
            None => "Working".to_owned(),
        },
        TurnPhase::Waiting => match &turn.wait {
            Some(wait) => format!("Waiting on {}", waited(wait, now)),
            None => "Waiting".to_owned(),
        },
        TurnPhase::NeedsYou => match &turn.call {
            Some(call) => format!("Needs the person: {}", call.question),
            None => "Needs the person".to_owned(),
        },
        TurnPhase::Done => "Done".to_owned(),
        TurnPhase::Stopped => match turn
            .stopped
            .as_ref()
            .and_then(|stopped| stopped.left.as_deref())
        {
            Some(left) => format!("Stopped with this left: {left}"),
            None => "Stopped".to_owned(),
        },
        TurnPhase::Failed => "Failed".to_owned(),
        TurnPhase::Unknown => "Unknown".to_owned(),
    }
}

fn waited(wait: &TurnWait, now: u64) -> String {
    let kind = match wait.kind {
        TurnWaitKind::Ci => "CI",
        TurnWaitKind::Deploy => "a deploy",
        TurnWaitKind::Agent => "an agent",
        TurnWaitKind::Timer => "a timer",
        TurnWaitKind::Process => "a process",
        TurnWaitKind::Check => "a check",
        TurnWaitKind::Unknown => "something",
    };
    format!("{kind}, {}, since {}", wait.what, ago(now, wait.started_at))
}

fn ago(now: u64, at: u64) -> String {
    let minutes = now.saturating_sub(at) / 60_000;
    match minutes {
        0 => "a moment ago".to_owned(),
        1..=89 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

fn big_from(chat: &Chat) -> u64 {
    if chat.capped() {
        IDLE_COMPACT_TOKENS
    } else {
        IDLE_COMPACT_UNCAPPED_TOKENS
    }
}

/// A big Claude chat whose prompt cache has expired since its last call, uncompacted: its next
/// message reads its whole history again unless the chat is summarised first, which it is
/// unless the person keeps the full history.
fn expired_cache(chat: &Chat, now: u64) -> Option<ExpiredCache> {
    let last = chat.meter.last_call_ms();
    let tokens = chat.meter.context();
    let expired = chat.provider() == "claude"
        && last > 0
        && now.saturating_sub(last) >= CACHE_MS
        && tokens >= IDLE_COMPACT_UNCAPPED_TOKENS
        && !chat.meter.compacted.load(Ordering::Acquire)
        && !chat.meter.compacting();
    expired.then(|| ExpiredCache {
        tokens,
        at: last + CACHE_MS,
        summarise: !chat.meter.keeps_history.load(Ordering::Acquire),
    })
}

/// Tells each chat's clients whether its cache has expired.
pub(crate) fn tell_expired(core: &Core, now: u64) {
    for agent_id in core.chats().ids() {
        if let Some(chat) = core.chats().get(&agent_id) {
            chat.feed.set_expired_cache(expired_cache(&chat, now));
        }
    }
}

/// Whether the message about to go summarises the chat first.
pub(crate) fn summarises_first(chat: &Chat) -> bool {
    expired_cache(chat, epoch_ms()).is_some_and(|expired| expired.summarise)
}

/// The person stopped the turn: a summary its message waits on lets the message go no further.
pub(crate) fn stop_summarising(chat: &Chat) {
    if chat.meter.compacting() {
        chat.meter.summarise_stopped.store(true, Ordering::Release);
    }
}

/// Whether the person stopped the turn since its summary began.
pub(crate) fn summarise_stopped(chat: &Chat) -> bool {
    chat.meter.summarise_stopped.load(Ordering::Acquire)
}

/// The core is summarising the chat ahead of its next message.
pub(crate) fn summarising(chat: &Chat) {
    chat.meter.summarise_stopped.store(false, Ordering::Release);
    chat.meter.compacted.store(true, Ordering::Release);
    chat.meter
        .compacting_since
        .store(epoch_ms(), Ordering::Release);
    chat.feed.set_expired_cache(None);
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepHistoryParams {
    pub agent_id: String,
    /// Off, the chat is summarised before the next message.
    pub keep: bool,
}

/// The person's switch for a chat whose cache expired: summarise it before the next message, or
/// keep its full history.
pub(crate) fn keep_history(core: &Core, params: KeepHistoryParams) -> CoreResult<()> {
    let chat = core.chats().get(&params.agent_id).ok_or("no such chat")?;
    chat.meter
        .keeps_history
        .store(params.keep, Ordering::Release);
    chat.feed
        .set_expired_cache(expired_cache(&chat, epoch_ms()));
    Ok(())
}

/// Asks each big chat that has sat idle long enough to compact before its cache dies.
pub(crate) fn compact_idle(core: &Core, now: u64) {
    for agent_id in core.chats().ids() {
        let Some(chat) = core.chats().get(&agent_id) else {
            continue;
        };
        let due = chat.is_ready()
            && !chat.prompted()
            && chat.state_now() == AgentState::Idle
            && chat
                .meter
                .worth_compacting(now, big_from(&chat))
                .is_some_and(|idle| idle >= IDLE_COMPACT_MS);
        if due {
            compact(core, &chat, now);
        }
    }
}

/// Compacts a big chat about to sleep while its cache is still warm, as it may sleep past the
/// cache's life. Answers whether it is compacting, to sleep once it is done.
pub(crate) fn compact_before_sleep(core: &Core, agent_id: &str, now: u64) -> bool {
    let Some(chat) = core.chats().get(agent_id) else {
        return false;
    };
    let due = !chat.capped()
        && (chat.meter)
            .worth_compacting(now, IDLE_COMPACT_UNCAPPED_TOKENS)
            .is_some();
    if due {
        compact(core, &chat, now);
    }
    due
}

fn compact(core: &Core, chat: &Chat, now: u64) {
    chat.meter.compacted.store(true, Ordering::Release);
    chat.meter.compacting_since.store(now, Ordering::Release);
    let command = if chat.capped() && super::rotation::on(core) {
        ChatCommand::TakeHandoff
    } else {
        ChatCommand::Compact
    };
    let _ = chat.send(command);
}

/// A chat stays awake while it compacts, and a capped one until its idle stretch has been
/// compacted, since a sleeping agent cannot compact.
pub(crate) fn holds_awake(core: &Core, agent_id: &str, now: u64) -> bool {
    core.chats().get(agent_id).is_some_and(|chat| {
        chat.meter.compacting()
            || (chat.capped() && (chat.meter.worth_compacting(now, IDLE_COMPACT_TOKENS)).is_some())
    })
}
