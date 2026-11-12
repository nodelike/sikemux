//! Replaces a capped chat's vendor session with a fresh one, behind the person's switch. The
//! chat the person sees is the core's record and stays one conversation; the new session starts
//! from a brief the core builds from what it keeps, and the old one carries on if that fails.

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{
    BriefCounts, BriefSection, BriefSectionKind, RotationBrief, RotationReason,
};
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

use super::{cap, Chat};
use crate::server::Core;

/// Past this much context a turn's end starts a fresh session.
const FULL_TOKENS: u64 = 120_000;
/// Away this long, the prompt cache is gone and a fresh session costs less than the old one.
const AWAY_MS: u64 = 60 * 60_000;
/// A rotation that failed is not tried again for this long.
const RETRY_MS: u64 = 10 * 60_000;
const EXCHANGES: usize = 20;
/// Each side of an exchange past this is cut.
const LONGEST_SAID: usize = 2_000;
const DECISION_DAYS: u64 = 14;
const MOST_DECISIONS: usize = 40;
/// What the old session is asked for while its cache is warm.
pub(crate) const HANDOFF_ASK: &str = "Sikemux is about to carry this chat on in a fresh session, which starts from what Sikemux keeps. Write a handoff note of under 150 words for it: what you were in the middle of, what you meant to do next, and anything you suspect but have not checked. Plain text, no tools, nothing else.";

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

    fn take_handoff(&self) -> Option<String> {
        let (note, at) = self.inner.locked().handoff.take()?;
        (epoch_ms().saturating_sub(at) < 2 * AWAY_MS).then_some(note)
    }

    pub(crate) fn failed(&self) {
        self.inner.locked().failed_at = epoch_ms();
    }
}

/// Whether the person turned rotation on.
pub(crate) fn on(core: &Core) -> bool {
    core.preferences.get().chief_rotation
}

/// Why the chat's session should give way to a fresh one now. `asked` when a message is about
/// to start a turn; else the chat is at rest after one.
pub(crate) fn due(core: &Core, chat: &Chat, now: Footing, asked: bool) -> Option<RotationReason> {
    if !chat.capped() || !on(core) || chat.provider() != "claude" {
        return None;
    }
    let at = epoch_ms();
    let inner = chat.rotation.inner.locked();
    if at.saturating_sub(inner.failed_at) < RETRY_MS {
        return None;
    }
    let was = inner.footing.as_ref()?;
    if was.session != now.session {
        return None;
    }
    let last_call = chat.meter.last_call_ms();
    if last_call == 0 {
        return None;
    }
    if was.model != now.model {
        return Some(RotationReason::Model);
    }
    if was.account != now.account {
        return Some(RotationReason::Account);
    }
    if chat.meter.context() >= FULL_TOKENS {
        return Some(RotationReason::Full);
    }
    (asked && at.saturating_sub(last_call) >= AWAY_MS).then_some(RotationReason::Away)
}

/// Whether the old session is warm enough to ask for its handoff note.
pub(crate) fn asks_handoff(reason: RotationReason) -> bool {
    reason != RotationReason::Away
}

/// A note the old session wrote earlier, for a rotation that cannot ask it now.
pub(crate) fn earlier_handoff(chat: &Chat) -> Option<String> {
    chat.rotation.take_handoff()
}

/// What a fresh session starts from: its instructions, the message that goes with the next
/// prompt, and the brief as the chat's line shows it.
pub(crate) struct Brief {
    pub instructions: String,
    pub message: ChatContext,
    pub shown: RotationBrief,
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

pub(crate) fn brief(core: &Core, chat: &Chat, handoff: Option<String>) -> Brief {
    let mut sections = Vec::new();
    let instructions = crate::server::memory::for_chat(core, &chat.launch.cwd).unwrap_or_default();
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
    if !decided.is_empty() {
        let lines: Vec<String> = (decided.iter().rev())
            .map(|decision| match &decision.why {
                Some(why) => format!("- {} ({why})", decision.text),
                None => format!("- {}", decision.text),
            })
            .collect();
        sections.push(section(
            BriefSectionKind::Decisions,
            BriefCounts {
                decisions: Some(decided.len() as u32),
                days: Some(DECISION_DAYS as u32),
                ..BriefCounts::default()
            },
            lines.join("\n"),
        ));
    }

    let open = cap::open_work(core, chat.agent_id());
    if !open.is_empty() {
        let count = |opening: &str| open.iter().filter(|line| line.starts_with(opening)).count();
        sections.push(section(
            BriefSectionKind::Work,
            BriefCounts {
                workers: Some(count("- Your worker") as u32),
                waits: Some((count("- You wait") + count_waiting(&open)) as u32),
                calls: Some(count("- A call") as u32),
                ..BriefCounts::default()
            },
            open.join("\n"),
        ));
    }

    if let Some(note) = handoff.filter(|note| !note.trim().is_empty()) {
        sections.push(section(
            BriefSectionKind::Handoff,
            BriefCounts::default(),
            note.trim().to_owned(),
        ));
    }

    let exchanges = chat.feed.exchanges(EXCHANGES, LONGEST_SAID);
    if !exchanges.is_empty() {
        let lines: Vec<String> = (exchanges.iter())
            .map(|(asked, answered)| format!("The person: {asked}\n\nYou: {answered}"))
            .collect();
        sections.push(section(
            BriefSectionKind::Exchanges,
            BriefCounts {
                exchanges: Some(exchanges.len() as u32),
                ..BriefCounts::default()
            },
            lines.join("\n\n---\n\n"),
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
    let shown = RotationBrief {
        tokens: sections.iter().map(|section| section.tokens).sum(),
        sections,
    };
    Brief {
        instructions,
        message: ChatContext {
            uri: format!("{FOR_AGENT_URI}rotation"),
            title: "Where this chat stands".to_owned(),
            text: message,
        },
        shown,
    }
}

fn count_waiting(open: &[String]) -> usize {
    (open.iter())
        .filter(|line| line.starts_with("- Your worker") && line.contains(". Waiting on "))
        .count()
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
