//! Replaces a capped chat's vendor session with a fresh one, behind the person's switch, and a
//! big Claude chat's when it moves to another account, whose cache it would write anew. The
//! chat the person sees is the core's record and stays one conversation. A session found full
//! when a turn ends only writes its handoff note then; the fresh session starts with the next
//! message, which carries the brief, so it never sits without one. The old session stays the
//! chat's session until the fresh one has answered, and carries on if the fresh one fails.

use crate::server::parts::Parts;
pub use sikemux_calls::chat::{RotationBriefParams, RotationBriefRead};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{
    BriefCounts, BriefSection, BriefSectionKind, RotationBrief, RotationReason,
};
use sikemux_wire::protocol::ChatContext;

use super::{cap, Chat};
use crate::server::{Core, CoreResult};

/// Past this much context a turn's end readies a fresh session.
const FULL_TOKENS: u64 = 120_000;
/// Away this long, the prompt cache is gone and a fresh session costs less than the old one.
const AWAY_MS: u64 = 60 * 60_000;
/// A rotation that failed is not tried again for this long.
const RETRY_MS: u64 = 10 * 60_000;
/// A handoff note older than this no longer says where the chat stands.
const NOTE_KEPT_MS: u64 = 2 * AWAY_MS;
const EXCHANGES: usize = 20;
/// Each side of an exchange past this is cut.
const LONGEST_SAID: usize = 2_000;
const DECISION_DAYS: u64 = 14;
const MOST_DECISIONS: usize = 40;
/// What each part of the brief holds at most, in characters, so the brief stays near 10K to
/// 20K tokens in all.
const DECISIONS_CHARS: usize = 8_000;
const WORK_CHARS: usize = 6_000;
const NOTE_CHARS: usize = 2_000;
const EXCHANGES_CHARS: usize = 40_000;
/// One decision or line of work past this is cut, so it cannot take its part's whole room.
const LONGEST_LINE: usize = 600;
/// The newest briefs kept to read again, per chat.
const BRIEFS_KEPT: usize = 20;
/// What a fresh Claude session sends before any history: its system prompt, tools and rules.
/// Measured over three days of the person's chats, 62K to 68K.
const FRESH_START_TOKENS: u64 = 65_000;
/// On another account a session's history is written to a cold cache again. A brief takes its
/// place only when the history is this many times the brief and at least the floor, for what
/// the brief leaves out.
const HISTORY_PER_BRIEF: u64 = 3;
const LEAST_HISTORY: u64 = 30_000;

/// What the chat's session started on, to tell when it moved to another model or account.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Footing {
    pub session: String,
    pub model: Option<String>,
    pub account: Option<String>,
}

/// What rotation keeps about a chat between its sessions.
#[derive(Default)]
pub(crate) struct State {
    inner: std::sync::Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    footing: Option<Footing>,
    /// A note the old session wrote while its cache was warm, and when.
    handoff: Option<(String, u64)>,
    /// The session found full as a turn ended; its next message starts the fresh one.
    full_session: Option<String>,
    failed_at: u64,
    /// A fresh session started and has not answered a turn yet.
    unanswered: bool,
}

impl State {
    /// The session the chat runs on now. A session it already knew keeps the footing it started on.
    pub(crate) fn runs_on(&self, now: Footing) {
        let mut inner = self.inner.locked();
        if inner
            .footing
            .as_ref()
            .is_none_or(|was| was.session != now.session)
        {
            inner.footing = Some(now);
        }
    }

    pub(crate) fn note_handoff(&self, note: String) {
        self.inner.locked().handoff = Some((note, epoch_ms()));
    }

    fn handoff(&self) -> Option<String> {
        let inner = self.inner.locked();
        let (note, at) = inner.handoff.as_ref()?;
        (epoch_ms().saturating_sub(*at) < NOTE_KEPT_MS).then(|| note.clone())
    }

    /// The fresh session started: what readied it is spent.
    pub(crate) fn rotated(&self, now: Footing) {
        let mut inner = self.inner.locked();
        inner.handoff = None;
        inner.full_session = None;
        inner.footing = Some(now);
        inner.unanswered = true;
    }

    pub(crate) fn answered(&self) {
        self.inner.locked().unanswered = false;
    }

    /// The session the chat rotated to last has answered a turn.
    pub(crate) fn fresh_answered(&self) -> bool {
        !self.inner.locked().unanswered
    }

    /// The fresh session did not start. Its note waits for the next try.
    pub(crate) fn failed(&self) {
        self.inner.locked().failed_at = epoch_ms();
    }
}

/// Where a chat's session stands, kept while the chat sleeps or its core starts again, so an
/// away or full session still gives way when the chat wakes.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeptSession {
    footing: Option<Footing>,
    handoff: Option<(String, u64)>,
    full_session: Option<String>,
    meter: cap::KeptMeter,
}

pub(crate) fn kept(chat: &Chat) -> KeptSession {
    let inner = chat.rotation.inner.locked();
    KeptSession {
        footing: inner.footing.clone(),
        handoff: inner.handoff.clone(),
        full_session: inner.full_session.clone(),
        meter: chat.meter.kept(),
    }
}

pub(crate) fn take_up(chat: &Chat, kept: KeptSession) {
    let mut inner = chat.rotation.inner.locked();
    inner.footing = kept.footing;
    inner.handoff = kept.handoff;
    inner.full_session = kept.full_session;
    chat.meter.take_up(kept.meter);
}

/// Whether the person turned rotation on.
pub(crate) fn on(core: &Core) -> bool {
    core.preferences().get().chief_rotation
}

/// Whether a fresh session may take over now: a Claude chat not lately failed, with nothing of
/// the old session still running that a new one would cut off.
fn may_start_fresh(chat: &Chat) -> bool {
    chat.provider() == "claude"
        && epoch_ms().saturating_sub(chat.rotation.inner.locked().failed_at) >= RETRY_MS
        && chat.feed.running_tasks() == 0
        && chat.feed.running_subagents() == 0
}

/// Whether rotation may touch the chat at all now: a capped chat with the switch on.
fn may(core: &Core, chat: &Chat) -> bool {
    chat.capped() && on(core) && may_start_fresh(chat)
}

/// Whether, at rest after a turn, `session` has grown full and its note should be taken now,
/// while its cache is warm.
pub(crate) fn full_at_rest(core: &Core, chat: &Chat, session: &str) -> bool {
    may(core, chat)
        && chat.rotation.inner.locked().full_session.as_deref() != Some(session)
        && chat.meter.last_call_ms() > 0
        && chat.meter.context() >= FULL_TOKENS
}

/// `session` was found full and its note is taken, or could not be.
pub(crate) fn readied(chat: &Chat, session: &str, note: Option<String>) {
    if let Some(note) = note {
        chat.rotation.note_handoff(note);
    }
    chat.rotation.inner.locked().full_session = Some(session.to_owned());
}

/// The session was compacted or cleared, so it is no longer full.
pub(crate) fn shrank(chat: &Chat) {
    chat.rotation.inner.locked().full_session = None;
}

/// Why the chat's session should give way to a fresh one as a message is about to start a turn.
/// Any Claude chat that moved to another account may; the rest wait on rotation's switch.
pub(crate) fn due(core: &Core, chat: &Chat, now: &Footing) -> Option<RotationReason> {
    if !may_start_fresh(chat) {
        return None;
    }
    let rotating = chat.capped() && on(core);
    let inner = chat.rotation.inner.locked();
    let was = inner.footing.as_ref()?;
    if was.session != now.session {
        return None;
    }
    let last_call = chat.meter.last_call_ms();
    if rotating && inner.full_session.as_ref() == Some(&now.session) {
        return Some(RotationReason::Full);
    }
    if last_call == 0 {
        return None;
    }
    if rotating && was.model != now.model {
        return Some(RotationReason::Model);
    }
    if was.account != now.account {
        return Some(RotationReason::Account);
    }
    (rotating && epoch_ms().saturating_sub(last_call) >= AWAY_MS).then_some(RotationReason::Away)
}

/// Whether starting fresh from `brief` costs the new account less than its cold cache reading
/// the whole history. A rotation the person switched on always goes ahead.
pub(crate) fn pays(
    core: &Core,
    chat: &Chat,
    reason: RotationReason,
    brief: &RotationBrief,
) -> bool {
    if reason != RotationReason::Account || (chat.capped() && on(core)) {
        return true;
    }
    let briefed: u64 = (brief.sections.iter())
        .filter(|section| section.kind != BriefSectionKind::Seat)
        .map(|section| section.tokens)
        .sum();
    let history = chat.meter.context().saturating_sub(FRESH_START_TOKENS);
    history >= (HISTORY_PER_BRIEF * briefed).max(LEAST_HISTORY)
}

/// The chat goes on in its session on the account it moved to.
pub(crate) fn stays(chat: &Chat, now: Footing) {
    chat.rotation.inner.locked().footing = Some(now);
}

/// What a fresh session starts from: its instructions, the message that goes with the next
/// prompt, and the brief whole, which the chat's line reads again on demand.
pub(crate) struct Brief {
    pub instructions: String,
    pub message: ChatContext,
    pub whole: RotationBrief,
}

fn tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn section(kind: BriefSectionKind, counts: BriefCounts, text: String) -> BriefSection {
    BriefSection {
        kind,
        tokens: tokens(&text),
        counts,
        text,
    }
}

/// `lines` joined in order within `most` characters. A line past `LONGEST_LINE` keeps its
/// start, marked `[cut]`, and one that no longer fits is left out while shorter ones after it
/// still go in.
fn within(lines: impl IntoIterator<Item = String>, most: usize) -> String {
    let mut text = String::new();
    let mut used = 0;
    for line in lines {
        let line = if line.chars().count() > LONGEST_LINE {
            line.chars()
                .take(LONGEST_LINE)
                .chain(" [cut]".chars())
                .collect()
        } else {
            line
        };
        let length = line.chars().count() + 1;
        if used + length > most {
            continue;
        }
        used += length;
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
    }
    text
}

pub(crate) fn brief(core: &Core, chat: &Chat) -> Brief {
    let mut sections = Vec::new();
    let instructions = if crate::server::chief::is_chief(core, chat.agent_id()) {
        crate::server::chief::brief(core, &chat.launch.cwd)
    } else {
        crate::server::memory::for_chat(core, &chat.launch.cwd).unwrap_or_default()
    };
    let rules = instructions
        .lines()
        .filter(|line| line.starts_with("- #"))
        .count() as u32;
    sections.push(section(
        BriefSectionKind::Seat,
        BriefCounts {
            rules: Some(rules),
            ..BriefCounts::default()
        },
        instructions.clone(),
    ));

    let project = core.workspaces().project_of(&chat.launch.cwd);
    let since = epoch_ms().saturating_sub(DECISION_DAYS * 24 * 60 * 60_000);
    let decided = crate::server::board::goal::standing_since(core, &project, since, MOST_DECISIONS);
    let decisions = within(
        decided.iter().map(|decision| match &decision.why {
            Some(why) => format!("- {} ({why})", decision.text),
            None => format!("- {}", decision.text),
        }),
        DECISIONS_CHARS,
    );
    if !decisions.is_empty() {
        sections.push(section(
            BriefSectionKind::Decisions,
            BriefCounts {
                decisions: Some(decisions.lines().count() as u32),
                days: Some(DECISION_DAYS as u32),
                ..BriefCounts::default()
            },
            decisions,
        ));
    }

    let open = cap::open_work(core, chat.agent_id());
    let work = within(open.iter().cloned(), WORK_CHARS);
    if !work.is_empty() {
        let count = |opening: &str| {
            work.lines()
                .filter(|line| line.starts_with(opening))
                .count()
        };
        let waiting = (work.lines())
            .filter(|line| line.starts_with("- Your worker") && line.contains(". Waiting on "))
            .count();
        sections.push(section(
            BriefSectionKind::Work,
            BriefCounts {
                workers: Some(count("- Your worker") as u32),
                waits: Some((count("- You wait") + waiting) as u32),
                calls: Some(count("- A call") as u32),
                ..BriefCounts::default()
            },
            work,
        ));
    }

    if let Some(note) = chat.rotation.handoff() {
        let note: String = note.trim().chars().take(NOTE_CHARS).collect();
        if !note.is_empty() {
            sections.push(section(
                BriefSectionKind::Handoff,
                BriefCounts::default(),
                note,
            ));
        }
    }

    let mut exchanges: Vec<String> = Vec::new();
    let mut room = EXCHANGES_CHARS;
    for (asked, answered) in chat.feed.exchanges(EXCHANGES, LONGEST_SAID).iter().rev() {
        let exchange = format!("The person: {asked}\n\nYou: {answered}");
        let Some(left) = room.checked_sub(exchange.chars().count() + 7) else {
            break;
        };
        room = left;
        exchanges.push(exchange);
    }
    exchanges.reverse();
    if !exchanges.is_empty() {
        sections.push(section(
            BriefSectionKind::Exchanges,
            BriefCounts {
                exchanges: Some(exchanges.len() as u32),
                ..BriefCounts::default()
            },
            exchanges.join("\n\n---\n\n"),
        ));
    }

    let headed: Vec<String> = (sections.iter())
        .filter(|section| section.kind != BriefSectionKind::Seat)
        .map(|section| format!("## {}\n\n{}", heading(section.kind), section.text))
        .collect();
    let message = format!(
        "Sikemux carries this chat on in a fresh session. The person sees one conversation; what \
         earlier sessions did is in Sikemux's record, which agent_read and agent_sessions search. \
         Below is what you need to go on.\n\n{}",
        headed.join("\n\n")
    );
    let whole = RotationBrief {
        tokens: sections.iter().map(|section| section.tokens).sum(),
        sections,
    };
    Brief {
        instructions,
        message: crate::sessions::for_agent::sikemux_context(
            "rotation",
            "Where this chat stands",
            &message,
        ),
        whole,
    }
}

fn heading(kind: BriefSectionKind) -> &'static str {
    match kind {
        BriefSectionKind::Decisions => "Decided in the last two weeks",
        BriefSectionKind::Work => "Work in hand",
        BriefSectionKind::Handoff => "Your previous session's handoff note",
        BriefSectionKind::Exchanges => "Your latest exchanges with the person, word for word",
        BriefSectionKind::Seat | BriefSectionKind::Unknown => "",
    }
}

/// The brief as the chat's line carries it: each part's size and counts, without its text.
pub(crate) fn shown(whole: &RotationBrief) -> RotationBrief {
    RotationBrief {
        tokens: whole.tokens,
        sections: (whole.sections.iter())
            .map(|section| BriefSection {
                text: String::new(),
                ..section.clone()
            })
            .collect(),
    }
}

/// Where a chat's briefs are kept to read again, newest last.
fn briefs_folder(core: &Core, agent_id: &str) -> Option<PathBuf> {
    let file = crate::server::kept::file_for(agent_id);
    Some(
        core.config()
            .data_dir
            .as_ref()?
            .join("rotations")
            .join(file.trim_end_matches(".json")),
    )
}

/// Keeps the brief a fresh session started from, under when it started.
pub(crate) fn keep(core: &Core, agent_id: &str, at: u64, whole: &RotationBrief) {
    let Some(folder) = briefs_folder(core, agent_id) else {
        return;
    };
    let written = std::fs::create_dir_all(&folder).and_then(|()| {
        let text = serde_json::to_vec(whole).unwrap_or_default();
        std::fs::write(folder.join(format!("{at}.json")), text)
    });
    if let Err(error) = written {
        eprintln!("sikemux core: a rotation's brief was not kept: {error}");
        return;
    }
    let mut kept: Vec<PathBuf> = std::fs::read_dir(&folder)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    kept.sort();
    for old in kept.iter().rev().skip(BRIEFS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

/// Lets go of the briefs a closed chat's fresh sessions started from.
pub(crate) fn forget(core: &Core, agent_id: &str) {
    let Some(folder) = briefs_folder(core, agent_id) else {
        return;
    };
    if let Err(error) = std::fs::remove_dir_all(&folder) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("sikemux core: a closed chat's briefs were not removed: {error}");
        }
    }
}

/// The whole brief a fresh session started from, for "Read the whole brief".
pub(crate) fn read(core: &Core, params: RotationBriefParams) -> CoreResult<RotationBriefRead> {
    let folder = briefs_folder(core, &params.agent_id).ok_or("Sikemux keeps no briefs here")?;
    let text = std::fs::read(folder.join(format!("{}.json", params.at)))
        .map_err(|_| "Sikemux no longer keeps that brief")?;
    let brief = serde_json::from_slice(&text).map_err(|_| "that brief could not be read")?;
    Ok(RotationBriefRead { brief })
}
