//! Replaces a capped chat's vendor session with a fresh one, behind the person's switch. The
//! chat the person sees is the core's record and stays one conversation. A session found full
//! when a turn ends only writes its handoff note then; the fresh session starts with the next
//! message, which carries the brief, so it never sits without one. The old session stays the
//! chat's session until the fresh one has answered, and carries on if the fresh one fails.

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
/// The newest briefs kept to read again, per chat.
const BRIEFS_KEPT: usize = 20;
/// What the old session is asked for while its cache is warm.
pub(crate) const HANDOFF_ASK: &str = "Sikemux is about to carry this chat on in a fresh session, which starts from what Sikemux keeps. Write a handoff note of under 150 words for it: what you were in the middle of, what you meant to do next, and anything you suspect but have not checked. Plain text only. Do not use any tool.";

/// What the chat's session started on, to tell when it moved to another model or account.
#[derive(Clone, PartialEq, Eq)]
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
    /// The session was found full as a turn ended; the next message starts the fresh one.
    full: bool,
    failed_at: u64,
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
        inner.full = false;
        inner.footing = Some(now);
    }

    /// The fresh session did not start. Its note waits for the next try.
    pub(crate) fn failed(&self) {
        self.inner.locked().failed_at = epoch_ms();
    }
}

/// Whether the person turned rotation on.
pub(crate) fn on(core: &Core) -> bool {
    core.preferences.get().chief_rotation
}

/// Whether rotation may touch the chat at all now: a capped Claude chat with the switch on, not
/// lately failed, and nothing of the old session still running that a new one would cut off.
fn may(core: &Core, chat: &Chat) -> bool {
    chat.capped()
        && on(core)
        && chat.provider() == "claude"
        && epoch_ms().saturating_sub(chat.rotation.inner.locked().failed_at) >= RETRY_MS
        && chat.feed.running_tasks() == 0
        && chat.feed.running_subagents() == 0
}

/// Whether, at rest after a turn, the session has grown full and its note should be taken now,
/// while its cache is warm.
pub(crate) fn full_at_rest(core: &Core, chat: &Chat) -> bool {
    may(core, chat)
        && !chat.rotation.inner.locked().full
        && chat.meter.last_call_ms() > 0
        && chat.meter.context() >= FULL_TOKENS
}

/// The session was found full and its note is taken, or could not be.
pub(crate) fn readied(chat: &Chat, note: Option<String>) {
    if let Some(note) = note {
        chat.rotation.note_handoff(note);
    }
    chat.rotation.inner.locked().full = true;
}

/// Why the chat's session should give way to a fresh one as a message is about to start a turn.
pub(crate) fn due(core: &Core, chat: &Chat, now: &Footing) -> Option<RotationReason> {
    if !may(core, chat) {
        return None;
    }
    let inner = chat.rotation.inner.locked();
    let was = inner.footing.as_ref()?;
    if was.session != now.session {
        return None;
    }
    let last_call = chat.meter.last_call_ms();
    if inner.full {
        return Some(RotationReason::Full);
    }
    if last_call == 0 {
        return None;
    }
    if was.model != now.model {
        return Some(RotationReason::Model);
    }
    if was.account != now.account {
        return Some(RotationReason::Account);
    }
    (epoch_ms().saturating_sub(last_call) >= AWAY_MS).then_some(RotationReason::Away)
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

/// `lines` joined while they fit in `most` characters, the first kept first.
fn within(lines: impl IntoIterator<Item = String>, most: usize) -> String {
    let mut text = String::new();
    for line in lines {
        let room = most.saturating_sub(text.chars().count());
        if line.chars().count() + 1 > room {
            break;
        }
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

    let project = core.workspaces.project_of(&chat.launch.cwd);
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
        core.config
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationBriefParams {
    pub agent_id: String,
    /// When the fresh session started, as its line says.
    pub at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationBriefRead {
    pub brief: RotationBrief,
}

/// The whole brief a fresh session started from, for "Read the whole brief".
pub(crate) fn read(core: &Core, params: RotationBriefParams) -> CoreResult<RotationBriefRead> {
    let folder = briefs_folder(core, &params.agent_id).ok_or("Sikemux keeps no briefs here")?;
    let text = std::fs::read(folder.join(format!("{}.json", params.at)))
        .map_err(|_| "Sikemux no longer keeps that brief")?;
    let brief = serde_json::from_slice(&text).map_err(|_| "that brief could not be read")?;
    Ok(RotationBriefRead { brief })
}
