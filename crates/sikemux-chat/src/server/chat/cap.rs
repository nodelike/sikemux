//! Keeps every chat's vendor session small. Claude compacts it near 150K tokens, told by the
//! core what to keep and handed the core's facts after. The core compacts any big session
//! itself when it sits idle long enough that the prompt cache is about to die, so coming back
//! to it later does not read it all again uncached. The chat the person sees is the core's
//! record, so none of this changes it.

use crate::server::chat_state::ChatParts;
pub use sikemux_calls::chat::KeepHistoryParams;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::AgentState;

use sikemux_view::turn::{TurnPhase, TurnState, TurnWait, TurnWaitKind};

use super::{Chat, ChatCommand};
use crate::server::{Core, CoreResult};
use sikemux_view::chat::ExpiredCache;

/// Claude cuts what a compaction hook answers past this many characters.
const HOOK_CHARS: usize = 10_000;
/// One message of the person's past this is cut, so a pasted log leaves room for the rest.
const LONGEST_SAID: usize = 1_500;
/// One line of what is open past this is cut, so a long command a worker waits on leaves the
/// person's words their room.
pub const LONGEST_OPEN: usize = 300;
/// What is open takes at most this much of the keep-list, so the person's words always keep
/// the rest, however many workers are blocked.
const OPEN_SHARE: usize = 3_000;
/// A compaction Claude never answered stops holding the chat awake after this.
const COMPACTING_MS: u64 = 10 * 60_000;
/// Claude Code keeps the prompt cache an hour; idle this long, a compaction still reads it warm.
const IDLE_COMPACT_MS: u64 = 50 * 60_000;
const CACHE_MS: u64 = 60 * 60_000;
/// Gemini implicit prompt cache expires near 5 minutes; compact at 4 minutes while still warm.
const ANTIGRAVITY_IDLE_COMPACT_MS: u64 = 4 * 60_000;
const ANTIGRAVITY_CACHE_MS: u64 = 5 * 60_000;
const ANTIGRAVITY_IDLE_COMPACT_TOKENS: u64 = 100_000;

/// Providers whose calls the core does not see drop an idle cache as early as this.
const UNSEEN_CACHE_MS: u64 = 5 * 60_000;

fn debug_cache_ms() -> Option<u64> {
    static OVERRIDE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        cfg!(debug_assertions)
            .then(|| std::env::var("SIKEMUX_CACHE_MS").ok()?.parse().ok())
            .flatten()
    })
}

/// How long the prompt cache lives: an hour, or in a debug build whatever `SIKEMUX_CACHE_MS`
/// says, so a throwaway core can watch a cache expire in a minute.
fn cache_ms() -> u64 {
    debug_cache_ms().unwrap_or(CACHE_MS)
}

fn cache_ms_for(provider: &str) -> Option<u64> {
    match provider {
        "claude" => Some(cache_ms()),
        "antigravity" => Some(debug_cache_ms().unwrap_or(ANTIGRAVITY_CACHE_MS)),
        _ => None,
    }
}

fn idle_compact_ms_for(provider: &str) -> Option<u64> {
    match provider {
        "claude" => Some(IDLE_COMPACT_MS),
        "antigravity" => Some(ANTIGRAVITY_IDLE_COMPACT_MS),
        _ => None,
    }
}
/// Below this a cold start costs a chief less than a compaction would save.
const IDLE_COMPACT_CHIEF_TOKENS: u64 = 100_000;
/// Any other chat keeps its whole session unless it is this big: most never wake again, so
/// compacting a smaller one costs more than its next cold start would.
const IDLE_COMPACT_TOKENS: u64 = 150_000;
/// When the chat's session last called the model and how much context that call carried.
#[derive(Default)]
pub struct Meter {
    last_call_ms: AtomicU64,
    context: AtomicU64,
    /// The session was compacted since its last call, so its idle stretch needs no other.
    compacted: AtomicBool,
    /// When the session last lost what it was told: it was compacted or started fresh.
    forgot_at: AtomicU64,
    /// When the core asked for the compaction still running, or 0.
    compacting_since: AtomicU64,
    /// The person chose to keep the full history for the next message after the cache expired.
    keeps_history: AtomicBool,
    /// The person stopped the turn while the chat was summarised ahead of its message.
    summarise_stopped: AtomicBool,
    /// When the last turn finished, or 0 once its suggestion was asked for or cannot be.
    unsuggested_since: AtomicU64,
}

/// What a meter keeps while its chat sleeps or its core starts again.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptMeter {
    last_call_ms: u64,
    context: u64,
    compacted: bool,
    #[serde(default)]
    keeps_history: bool,
    #[serde(default)]
    forgot_at: u64,
}

impl KeptMeter {
    pub fn keep_history(&mut self, keep: bool) {
        self.keeps_history = keep;
    }
}

impl Meter {
    pub fn kept(&self) -> KeptMeter {
        KeptMeter {
            last_call_ms: self.last_call_ms.load(Ordering::Acquire),
            context: self.context.load(Ordering::Acquire),
            compacted: self.compacted.load(Ordering::Acquire),
            keeps_history: self.keeps_history.load(Ordering::Acquire),
            forgot_at: self.forgot_at.load(Ordering::Acquire),
        }
    }

    pub fn take_up(&self, kept: KeptMeter) {
        self.last_call_ms
            .store(kept.last_call_ms, Ordering::Release);
        self.context.store(kept.context, Ordering::Release);
        self.compacted.store(kept.compacted, Ordering::Release);
        self.keeps_history
            .store(kept.keeps_history, Ordering::Release);
        self.forgot_at.store(kept.forgot_at, Ordering::Release);
    }

    pub fn called(&self, context: u64) {
        self.last_call_ms.store(epoch_ms(), Ordering::Release);
        self.context.store(context, Ordering::Release);
        self.compacted.store(false, Ordering::Release);
        self.keeps_history.store(false, Ordering::Release);
    }

    pub fn compacted(&self, context: u64) {
        self.context.store(context, Ordering::Release);
        self.compacted.store(true, Ordering::Release);
        self.suggests_nothing();
        self.forgot_at.store(epoch_ms(), Ordering::Release);
    }

    pub fn forgot_at(&self) -> u64 {
        self.forgot_at.load(Ordering::Acquire)
    }

    /// The core's own compaction runs, so the agent is at work though no turn shows.
    pub fn compacting(&self) -> bool {
        let since = self.compacting_since.load(Ordering::Acquire);
        since > 0 && epoch_ms().saturating_sub(since) < COMPACTING_MS
    }

    pub fn context(&self) -> u64 {
        self.context.load(Ordering::Acquire)
    }

    pub fn last_call_ms(&self) -> u64 {
        self.last_call_ms.load(Ordering::Acquire)
    }

    pub fn is_compacted(&self) -> bool {
        self.compacted.load(Ordering::Acquire)
    }

    /// A fresh session starts with nothing in it, and no call yet.
    pub fn started_fresh(&self) {
        self.last_call_ms.store(0, Ordering::Release);
        self.context.store(0, Ordering::Release);
        self.compacted.store(false, Ordering::Release);
        self.suggests_nothing();
        self.forgot_at.store(epoch_ms(), Ordering::Release);
    }

    pub fn compaction_over(&self) {
        self.compacting_since.store(0, Ordering::Release);
    }

    pub fn turn_ended(&self, finished: bool) {
        let since = if finished { epoch_ms() } else { 0 };
        self.unsuggested_since.store(since, Ordering::Release);
    }

    pub fn suggests_nothing(&self) {
        self.unsuggested_since.store(0, Ordering::Release);
    }

    /// Takes the one suggestion a finished turn may be asked for. On a cold cache the ask would
    /// write the whole context again, so there is none.
    pub fn take_suggestion(&self, calls_seen: bool) -> bool {
        let finished = self.unsuggested_since.swap(0, Ordering::AcqRel);
        let (used, life) = if calls_seen {
            (self.last_call_ms(), cache_ms())
        } else {
            (finished, UNSEEN_CACHE_MS)
        };
        let cleared = calls_seen && self.compacted.load(Ordering::Acquire);
        finished > 0 && !cleared && epoch_ms().saturating_sub(used) < life
    }

    /// The session has sat idle since a call whose cache is still warm, big enough that a
    /// compaction pays, and none has run since.
    fn worth_compacting(&self, now: u64, big_from: u64, cache_life: u64) -> Option<u64> {
        let last = self.last_call_ms.load(Ordering::Acquire);
        let idle = now.saturating_sub(last);
        let warm = last > 0 && idle < cache_life;
        let big = self.context.load(Ordering::Acquire) >= big_from;
        (warm && big && !self.compacted.load(Ordering::Acquire)).then_some(idle)
    }
}

/// What a compaction must keep: what is still open, and the person's own words as the core's
/// record holds them.
pub fn keep_list(core: &Core, chat: &Chat) -> String {
    let mut text = "Sikemux keeps the record of this chat. In the summary, keep under their own \
                    heading, word for word: every message of the person's below, every call still \
                    open to the person, and what you and your workers are waiting on, with what \
                    each waits for. A message ending in [cut] was longer: only its start is here, \
                    so keep that start and say it was cut. Keep the rest of the summary brief: \
                    decisions, current state, file paths and line numbers, not raw command \
                    outputs, diffs or long code blocks."
        .to_owned();
    let open = (core.chat_above().open_work)(core, chat.agent_id());
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
/// unless its system prompt holds that already, and for a chief what is open now.
pub fn after_compaction(core: &Core, chat: &Chat) -> Option<String> {
    let agent_id = chat.agent_id();
    let brief = (chat.brief().is_none())
        .then(|| (core.chat_above().memory_after_compaction)(core, agent_id))
        .flatten();
    let open = if chat.chief() {
        (core.chat_above().open_work)(core, agent_id)
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

pub fn standing(turn: &TurnState, now: u64) -> String {
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

pub fn waited(wait: &TurnWait, now: u64) -> String {
    if wait.kind == TurnWaitKind::Land {
        return format!(
            "its land onto {}, since {}",
            wait.what,
            ago(now, wait.started_at)
        );
    }
    let kind = match wait.kind {
        TurnWaitKind::Ci => "CI",
        TurnWaitKind::Deploy => "a deploy",
        TurnWaitKind::Agent => "an agent",
        TurnWaitKind::Timer => "a timer",
        TurnWaitKind::Process => "a process",
        TurnWaitKind::Check => "a check",
        TurnWaitKind::Watch => "a repeating check",
        TurnWaitKind::Line => "its turn on the host",
        TurnWaitKind::InUse => "something in use",
        TurnWaitKind::Land => "a land",
        TurnWaitKind::Unknown => "something",
    };
    format!("{kind}, {}, since {}", wait.what, ago(now, wait.started_at))
}

pub fn ago(now: u64, at: u64) -> String {
    let minutes = now.saturating_sub(at) / 60_000;
    match minutes {
        0 => "a moment ago".to_owned(),
        1..=89 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

fn big_from(chat: &Chat) -> u64 {
    if chat.provider() == "antigravity" {
        ANTIGRAVITY_IDLE_COMPACT_TOKENS
    } else if chat.chief() {
        IDLE_COMPACT_CHIEF_TOKENS
    } else {
        IDLE_COMPACT_TOKENS
    }
}

fn expired_big_from(provider: &str) -> u64 {
    if provider == "antigravity" {
        ANTIGRAVITY_IDLE_COMPACT_TOKENS
    } else {
        IDLE_COMPACT_TOKENS
    }
}

/// A big chat whose prompt cache has expired since its last call, uncompacted: its next
/// message reads its whole history again unless the chat is summarised first, which it is
/// unless the person keeps the full history.
fn expired_cache(chat: &Chat, now: u64) -> Option<ExpiredCache> {
    if chat.meter.compacting() {
        return None;
    }
    expired_from(chat.provider(), chat.meter.kept(), now)
}

fn expired_from(provider: &str, meter: KeptMeter, now: u64) -> Option<ExpiredCache> {
    let life = cache_ms_for(provider)?;
    let expired = meter.last_call_ms > 0
        && now.saturating_sub(meter.last_call_ms) >= life
        && meter.context >= expired_big_from(provider)
        && !meter.compacted;
    expired.then_some(ExpiredCache {
        tokens: meter.context,
        at: meter.last_call_ms + life,
        summarise: !meter.keeps_history,
    })
}

/// Tells each chat's clients whether its cache has expired, sleeping chats' too.
pub fn tell_expired(core: &Core, now: u64) {
    for agent_id in core.chats().ids() {
        if let Some(chat) = core.chats().get(&agent_id) {
            chat.feed.set_expired_cache(expired_cache(&chat, now));
        }
    }
    for (feed, meter, provider) in core.chats().resting_meters() {
        feed.set_expired_cache(expired_from(&provider, meter, now));
    }
}

/// Whether the message about to go summarises the chat first.
pub fn summarises_first(chat: &Chat) -> bool {
    expired_cache(chat, epoch_ms()).is_some_and(|expired| expired.summarise)
}

/// The person stopped the turn: a summary its message waits on lets the message go no further.
pub fn stop_summarising(chat: &Chat) {
    if chat.meter.compacting() {
        chat.meter.summarise_stopped.store(true, Ordering::Release);
    }
}

/// Whether the person stopped the turn since its summary began.
pub fn summarise_stopped(chat: &Chat) -> bool {
    chat.meter.summarise_stopped.load(Ordering::Acquire)
}

/// The core is summarising the chat ahead of its next message.
pub fn summarising(chat: &Chat) {
    chat.meter.summarise_stopped.store(false, Ordering::Release);
    chat.meter.compacted.store(true, Ordering::Release);
    chat.meter
        .compacting_since
        .store(epoch_ms(), Ordering::Release);
    chat.feed.set_expired_cache(None);
}

/// The person's switch for a chat whose cache expired: summarise it before the next message, or
/// keep its full history.
pub fn keep_history(core: &Core, params: KeepHistoryParams) -> CoreResult<()> {
    if let Some(chat) = core.chats().get(&params.agent_id) {
        chat.meter
            .keeps_history
            .store(params.keep, Ordering::Release);
        chat.feed
            .set_expired_cache(expired_cache(&chat, epoch_ms()));
        return Ok(());
    }
    let (feed, meter, provider) = core
        .chats()
        .resting_keeps_history(&params.agent_id, params.keep)
        .ok_or("no such chat")?;
    feed.set_expired_cache(expired_from(&provider, meter, epoch_ms()));
    Ok(())
}

fn waiting_on_process(core: &Core, chat: &Chat) -> bool {
    chat.feed.running_tasks() > 0
        || chat.feed.running_subagents() > 0
        || (core.chat_above().turn_state_now)(core, chat.agent_id()).is_some_and(|turn| {
            turn.state == TurnPhase::Working
                || turn
                    .wait
                    .as_ref()
                    .is_some_and(|wait| wait.kind == TurnWaitKind::Process)
        })
}

/// Compacts an idle Antigravity chat whose context crossed the 150K cap right after its turn
/// finishes, while its prompt cache is still hot.
pub fn compact_over_cap(core: &Core, chat: &Chat, now: u64) -> bool {
    let due = chat.provider() == "antigravity"
        && chat.is_ready()
        && !chat.prompted()
        && chat.state_now() == AgentState::Idle
        && !chat.feed.has_queued()
        && !chat.meter.compacting()
        && !chat.meter.is_compacted()
        && chat.meter.context() >= sikemux_drivers::server::chat::cap::ANTIGRAVITY_AUTO_COMPACT
        && !waiting_on_process(core, chat);
    if due {
        chat.meter.compacted.store(true, Ordering::Release);
        chat.meter.compacting_since.store(now, Ordering::Release);
    }
    due
}

/// Asks each big chat that has sat idle long enough to compact before its cache dies.
pub fn compact_idle(core: &Core, now: u64) {
    for agent_id in core.chats().ids() {
        let Some(chat) = core.chats().get(&agent_id) else {
            continue;
        };
        let (Some(cache_life), Some(idle_after)) = (
            cache_ms_for(chat.provider()),
            idle_compact_ms_for(chat.provider()),
        ) else {
            continue;
        };
        let due = chat.is_ready()
            && !chat.prompted()
            && chat.state_now() == AgentState::Idle
            && !chat.feed.has_queued()
            && !chat.meter.compacting()
            && !waiting_on_process(core, &chat)
            && chat
                .meter
                .worth_compacting(now, big_from(&chat), cache_life)
                .is_some_and(|idle| idle >= idle_after);
        if due {
            compact(core, &chat, now);
        }
    }
}

/// Compacts a big chat about to sleep while its cache is still warm, as it may sleep past the
/// cache's life. Answers whether it is compacting, to sleep once it is done.
pub fn compact_before_sleep(core: &Core, agent_id: &str, now: u64) -> bool {
    let Some(chat) = core.chats().get(agent_id) else {
        return false;
    };
    let Some(cache_life) = cache_ms_for(chat.provider()) else {
        return false;
    };
    let due = !chat.chief()
        && !chat.prompted()
        && chat.state_now() == AgentState::Idle
        && !chat.feed.has_queued()
        && !chat.meter.compacting()
        && !waiting_on_process(core, &chat)
        && (chat.meter)
            .worth_compacting(now, big_from(&chat), cache_life)
            .is_some();
    if due {
        compact(core, &chat, now);
    }
    due
}

fn compact(core: &Core, chat: &Chat, now: u64) {
    chat.meter.compacted.store(true, Ordering::Release);
    chat.meter.compacting_since.store(now, Ordering::Release);
    let command = if chat.chief() && super::rotation::on(core) {
        ChatCommand::TakeHandoff
    } else {
        ChatCommand::Compact
    };
    let _ = chat.send(command);
}

/// A chat stays awake while it compacts, and a chief's until its idle stretch has been
/// compacted, since a sleeping agent cannot compact.
pub fn holds_awake(core: &Core, agent_id: &str, now: u64) -> bool {
    core.chats().get(agent_id).is_some_and(|chat| {
        chat.meter.compacting()
            || (chat.chief()
                && cache_ms_for(chat.provider()).is_some_and(|life| {
                    chat.meter
                        .worth_compacting(now, IDLE_COMPACT_CHIEF_TOKENS, life)
                        .is_some()
                }))
    })
}
