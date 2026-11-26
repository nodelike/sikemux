//! Calls: every decision put to the person, whatever raised it. A question an agent's `ask`
//! takes to the person is one, and so is an action held at a production place. Each is asked
//! once and waits until the person answers it; nothing times it out. Every client draws the same
//! card from the device view and answers it with `answerCall`, and the answer reaches whatever
//! raised the call. A production call passes only on a client that drew it, by its button.

mod delivery;
mod legacy;
mod log;
mod writer;

pub use delivery::{answer, HOLD_ALLOW, HOLD_REFUSE};
pub use legacy::{answer_hold, answer_questions, not_mine};

use delivery::{after_restart, call_of};
use writer::LogWriter;

use crate::server::changed::ChangedCore;
use crate::server::repo_state::RepoParts;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    CallAnswer, CallAsker, CallBrief, CallClient, CallEvidence, CallKind, CallOption, CallOutcome,
    CallParams, CallPlace, CallResult, CallState, CallWhy, PersonCall,
};
use sikemux_wire::protocol::ErrorKind;

use self::log::{CallLog, Entry};
use super::access::Peer;
use super::activity_log::ActivityLog;
use super::changed::What;
use super::ship::Attempt;
use super::{Core, CoreError, CoreResult};

/// How long an answered or closed call stays in the device view.
const SHOWN_AFTER_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_SHOWN_CLOSED: usize = 16;

/// What raised a call, which its answer goes back to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "from", content = "id")]
pub enum Source {
    /// A question an agent put to the person, by the question's id.
    Question(String),
    /// An action held for the person, by the hold's id.
    Hold(String),
    /// Raised by [`raise`], which hears the answer through its `key`.
    Raised,
}

/// Another question that raised the same decision since, and the agent that asked it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Also {
    pub(super) source: Source,
    agent: String,
}

pub(super) struct Kept {
    pub(super) call: PersonCall,
    pub(super) source: Source,
    /// What else raised the same decision since, each waiting on its answer.
    also: Vec<Also>,
    /// Calls raised with the same key are joined into one.
    key: Option<String>,
    /// The option that lets what it holds run, on a call where one does.
    yes: Option<String>,
    /// What the agent tried to run, on a call the gate raised.
    attempt: Option<Box<Attempt>>,
    /// When what its yes lets run started, so a restart can't say nothing ran.
    started: Option<u64>,
    /// The clients that drew it, and when each first did.
    seen: Vec<(CallClient, u64)>,
}

impl Kept {
    /// What raised it, first and since.
    fn sources(&self) -> impl Iterator<Item = &Source> {
        std::iter::once(&self.source).chain(self.also.iter().map(|also| &also.source))
    }
}

/// Calls a restart caught between the person's answer and what came of it.
#[derive(Default)]
pub(super) struct AfterStart {
    /// Calls whose yes had not started what it lets run: their askers hear it lapsed.
    pub(super) lapsed: Vec<String>,
    /// Calls whose yes had started what it lets run, unseen to its end: their askers hear it may
    /// have run.
    pub(super) interrupted: Vec<String>,
    /// Held actions, which a restart lets go of: their agents hear to run them again.
    pub(super) dropped: Vec<String>,
    /// The gate's open calls, which are taken back if a yes to them would no longer count.
    pub(super) gated: Vec<String>,
    /// Other calls whose answer may not have reached their askers.
    pub(super) undelivered: Vec<String>,
}

/// Every call open, and those settled lately, as their entries left them.
#[derive(Default)]
pub(super) struct Book {
    entries: HashMap<String, Kept>,
}

impl Book {
    fn new() -> Self {
        Self::default()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Kept> {
        self.entries.values()
    }

    fn get(&self, id: &str) -> Option<&Kept> {
        self.entries.get(id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Kept> {
        self.entries.get_mut(id)
    }

    fn into_values(self) -> impl Iterator<Item = Kept> {
        self.entries.into_values()
    }
}

pub struct PersonCalls {
    pub(super) book: Mutex<Book>,
    /// What a restart left to do once the core runs, as `PersonCalls::new` found it.
    after_start: Mutex<AfterStart>,
    log: CallLog,
    writer: LogWriter,
    history_cache: Mutex<HashMap<String, Vec<PersonCall>>>,
    /// Counts how long each call waited and who answered it.
    activity: Arc<ActivityLog>,
}

/// A call to put to the person, as [`raise`] takes it.
#[derive(Clone)]
pub struct Raise {
    pub project: String,
    pub kind: CallKind,
    pub asker: CallAsker,
    pub question: String,
    pub context: Option<String>,
    pub options: Vec<CallOption>,
    pub pick: Option<String>,
    pub against: Option<String>,
    pub evidence: Vec<CallEvidence>,
    pub action: Option<String>,
    pub brief: Option<CallBrief>,
    pub place: Option<CallPlace>,
    /// What the call is about, such as one piece of work at one place. Agents that raise a call
    /// with the key of one still open join it.
    pub key: Option<String>,
    /// The option that lets what the call holds run, such as a deploy's yes.
    pub yes: Option<String>,
    /// What the agent tried to run, on a call the gate raises.
    pub attempt: Option<Box<Attempt>>,
}

impl Raise {
    fn into_entry(self, id: String) -> Entry {
        let call = PersonCall {
            id,
            project: self.project,
            can_hand_back: false,
            kind: self.kind,
            title: None,
            asker: self.asker,
            joined: Vec::new(),
            about: None,
            page: None,
            more_pages: Vec::new(),
            shown_in: Vec::new(),
            link: None,
            question: self.question,
            headline: String::new(),
            context: self.context,
            assumption: None,
            options: self.options,
            pick: self.pick,
            against: self.against,
            evidence: self.evidence,
            opened: Vec::new(),
            action: self.action,
            brief: self.brief,
            place: self.place,
            multi: false,
            secret: false,
            raised_at: epoch_ms(),
            state: CallState::Open,
            answer: None,
            outcome: None,
        };
        Entry::Raised {
            call: Box::new(call),
            source: Source::Raised,
            also: Vec::new(),
            key: self.key,
            yes: self.yes,
            attempt: self.attempt,
        }
    }
}

/// The open call an agent waits on longest, with how many it waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCall {
    pub id: String,
    pub question: String,
    pub waiting_since: u64,
    pub count: usize,
}

pub(super) fn client_of(peer: &Peer) -> Option<CallClient> {
    match peer {
        Peer::App => Some(CallClient::App),
        Peer::Device { id } => Some(CallClient::Device { id: id.clone() }),
        Peer::Local | Peer::Share { .. } => None,
    }
}

pub(super) const NOT_A_CLIENT: &str =
    "only the person answers a call, from Sikemux, its notch or a paired phone";
pub(super) const GONE: &str = "that call no longer waits on you";

fn waits_on(call: &PersonCall, agent_id: &str) -> bool {
    call.asker.agent_id == agent_id || call.joined.iter().any(|asker| asker.agent_id == agent_id)
}

pub(super) fn outcome(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

/// The start of the key of a call Sikemux raises for chats an account's provider refused.
pub const ACCOUNT_REFUSED: &str = "account-refused:";
/// The start of the key of a call Sikemux raises for an entry waiting on an org's list.
pub const ORG_WAITING: &str = "org-waiting:";

/// The open calls of `project` raised with a key starting `prefix`, by id, with their keys.
pub fn open_keyed(core: &Core, project: &str, prefix: &str) -> Vec<(String, String)> {
    (core.person_calls().book.locked().iter())
        .filter(|kept| kept.call.project == project && kept.call.state == CallState::Open)
        .filter_map(|kept| Some((kept.call.id.clone(), kept.key.clone()?)))
        .filter(|(_, key)| key.starts_with(prefix))
        .collect()
}

/// The key the call `id` was raised with.
pub fn key_of(core: &Core, id: &str) -> Option<String> {
    kept(&core.person_calls().book.locked(), id).and_then(|kept| kept.key.clone())
}

pub(super) fn kept<'a>(book: &'a Book, id: &str) -> Option<&'a Kept> {
    book.get(id)
}

pub(super) fn open<'a>(book: &'a Book, id: &str) -> CoreResult<&'a Kept> {
    (kept(book, id))
        .filter(|kept| kept.call.state == CallState::Open)
        .ok_or_else(gone)
}

/// The call was answered or closed already, which a client tells apart from other refusals.
pub(super) fn gone() -> CoreError {
    CoreError::of(ErrorKind::NotPending, GONE)
}

/// What `entry` changes of the calls kept.
fn apply(book: &mut Book, entry: Entry) {
    apply_with(book, entry, true);
}

/// [`apply`], keeping every settled call when not `prune`, as reading a project's whole history
/// does.
fn apply_with(book: &mut Book, entry: Entry, prune: bool) {
    let id = entry.id().to_owned();
    if let Entry::Raised {
        call,
        source,
        also,
        key,
        yes,
        attempt,
    } = entry
    {
        let seen = book
            .get(&id)
            .map(|kept| kept.seen.clone())
            .unwrap_or_default();
        if prune {
            let now = epoch_ms();
            book.entries.retain(|_, kept| {
                kept.call.state == CallState::Open
                    || now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS
            });
        }
        book.entries.insert(
            id,
            Kept {
                call: *call,
                source,
                also,
                key,
                yes,
                attempt,
                started: None,
                seen,
            },
        );
        return;
    }
    let Some(kept) = book.get_mut(&id) else {
        return;
    };
    let call = &mut kept.call;
    match entry {
        Entry::Raised { .. } => {}
        Entry::Joined { asker, source, .. } => {
            let agent = asker.agent_id.clone();
            if !waits_on(call, &agent) {
                call.joined.push(asker);
            }
            kept.also
                .extend(source.map(|source| Also { source, agent }));
        }
        Entry::Left { source, .. } => {
            let agent = match kept.also.iter().position(|also| also.source == source) {
                Some(at) => kept.also.remove(at).agent,
                None if kept.source == source && !kept.also.is_empty() => {
                    let next = kept.also.remove(0);
                    let agent = std::mem::replace(&mut call.asker.agent_id, next.agent.clone());
                    kept.source = next.source;
                    if let Some(at) = call
                        .joined
                        .iter()
                        .position(|asker| asker.agent_id == next.agent)
                    {
                        call.asker = call.joined.remove(at);
                    }
                    agent
                }
                None => return,
            };
            let still =
                kept.also.iter().any(|also| also.agent == agent) || call.asker.agent_id == agent;
            if !still {
                call.joined.retain(|asker| asker.agent_id != agent);
            }
        }
        Entry::Quit { agent, .. } => {
            if call.asker.agent_id == agent && !call.joined.is_empty() {
                call.asker = call.joined.remove(0);
            }
            call.joined.retain(|asker| asker.agent_id != agent);
        }
        Entry::Started { at, .. } => kept.started = Some(at),
        Entry::Edited { call: edited, .. } => {
            call.title = edited.title;
            call.question = edited.question;
            call.options = edited.options;
            call.pick = edited.pick;
            call.page = edited.page;
            call.more_pages = edited.more_pages;
            call.link = edited.link;
        }
        Entry::Shown { by, at, .. } => {
            if !kept.seen.iter().any(|(seer, _)| *seer == by) {
                kept.seen.push((by, at));
            }
        }
        Entry::Answered { answer, .. } => {
            call.state = CallState::Answered;
            call.answer = Some(answer);
        }
        Entry::Reopened { .. } => {
            call.state = CallState::Open;
            call.answer = None;
            call.outcome = None;
            kept.started = None;
        }
        Entry::Came { outcome, .. } => {
            if call.state == CallState::Open {
                call.state = CallState::Closed;
            }
            call.outcome = Some(outcome);
        }
    }
}

/// Whether `now` asks something other than what the open `call` shows.
fn edited(call: &PersonCall, now: &PersonCall) -> bool {
    (
        &call.title,
        &call.question,
        &call.options,
        &call.pick,
        &call.page,
        &call.more_pages,
        &call.link,
    ) != (
        &now.title,
        &now.question,
        &now.options,
        &now.pick,
        &now.page,
        &now.more_pages,
        &now.link,
    )
}

/// What `entry` closes of a call still open, as the usage report counts it: its kind, who
/// answered it, and how long it waited.
fn closing(book: &Book, entry: &Entry) -> Option<(CallKind, Option<CallClient>, u64)> {
    let call = &kept(book, entry.id())?.call;
    if call.state != CallState::Open {
        return None;
    }
    match entry {
        Entry::Answered { answer, .. } => {
            Some((call.kind, Some(answer.by.clone()), answer.waited_ms))
        }
        Entry::Came { outcome, .. } => {
            Some((call.kind, None, outcome.at.saturating_sub(call.raised_at)))
        }
        _ => None,
    }
}

fn kind_name(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Production => "production",
        CallKind::Money => "money",
        CallKind::Taste => "taste",
        CallKind::Access => "access",
        CallKind::Do => "do",
        CallKind::Look => "look",
        CallKind::Unknown => "unknown",
    }
}

/// Entries that write each of the project's open calls down again whole.
fn still_open(book: &Book, project: &str) -> Vec<Entry> {
    let open = book
        .iter()
        .filter(|kept| kept.call.project == project && kept.call.state == CallState::Open);
    open.flat_map(|kept| {
        let raised = Entry::Raised {
            call: Box::new(kept.call.clone()),
            source: kept.source.clone(),
            also: kept.also.clone(),
            key: kept.key.clone(),
            yes: kept.yes.clone(),
            attempt: kept.attempt.clone(),
        };
        let shown = kept.seen.iter().map(|(by, at)| Entry::Shown {
            id: kept.call.id.clone(),
            by: by.clone(),
            at: *at,
        });
        std::iter::once(raised).chain(shown)
    })
    .collect()
}

impl PersonCalls {
    /// Takes up the calls the files kept. One raised by an action held before the core started,
    /// or by a question `question_open` no longer knows, closes unanswered.
    pub fn new(
        data_dir: Option<&Path>,
        activity: Arc<ActivityLog>,
        question_open: impl Fn(&str) -> bool,
    ) -> Self {
        let log = CallLog::new(data_dir);
        let mut book = Book::new();
        for entry in log.read_all() {
            apply(&mut book, entry);
        }
        let mut after_start = AfterStart::default();
        let mut closing = Vec::new();
        for kept in book
            .iter()
            .filter(|kept| kept.call.state == CallState::Open)
        {
            let id = kept.call.id.clone();
            let dead = match &kept.source {
                Source::Hold(_) => {
                    after_start.dropped.push(id.clone());
                    true
                }
                Source::Question(question) => !question_open(question),
                Source::Raised => {
                    if kept.call.place.is_some() {
                        after_start.gated.push(id.clone());
                    }
                    false
                }
            };
            if dead {
                let outcome = outcome(CallResult::Withdrawn);
                closing.push(Entry::Came { id, outcome });
            }
        }
        for kept in book.iter() {
            let id = kept.call.id.clone();
            let ended =
                |started: u64| (kept.call.outcome.as_ref()).is_some_and(|came| came.at > started);
            if kept.started.is_some_and(|started| !ended(started)) {
                let mut outcome = outcome(CallResult::MayHaveRun);
                outcome.why = Some(CallWhy::Restarted);
                after_start.interrupted.push(id.clone());
                closing.push(Entry::Came { id, outcome });
                continue;
            }
            if kept.call.state != CallState::Answered || kept.call.outcome.is_some() {
                continue;
            }
            let picked = (kept.call.answer.as_ref()).and_then(|answer| answer.option.as_deref());
            let yes = kept.yes.is_some() && picked.is_some_and(|option| allows(kept, option));
            if !yes {
                after_start.undelivered.push(id);
                continue;
            }
            let mut outcome = outcome(CallResult::NotShipped);
            outcome.why = Some(CallWhy::Restarted);
            after_start.lapsed.push(id.clone());
            closing.push(Entry::Came { id, outcome });
        }
        for entry in closing {
            if let Some(project) = kept(&book, entry.id()).map(|kept| kept.call.project.clone()) {
                if let Err(error) =
                    log.append(&project, &entry, false, || still_open(&book, &project))
                {
                    eprintln!("sikemux core: a call was not written down: {error}");
                }
            }
            apply(&mut book, entry);
        }
        let writer = LogWriter::start(log.clone());
        Self {
            book: Mutex::new(book),
            after_start: Mutex::new(after_start),
            log,
            writer,
            history_cache: Mutex::default(),
            activity,
        }
    }

    fn entry_project(book: &Book, entry: &Entry) -> Option<String> {
        match entry {
            Entry::Raised { call, .. } => Some(call.project.clone()),
            entry => kept(book, entry.id()).map(|kept| kept.call.project.clone()),
        }
    }

    fn note_closed(&self, closes: Option<(CallKind, Option<CallClient>, u64)>) {
        if let Some((kind, by, waited)) = closes {
            let (kind, waited) = (kind_name(kind), Duration::from_millis(waited));
            match by {
                Some(CallClient::App) => self.activity.call_closed(kind, Some(&Peer::App), waited),
                Some(CallClient::Device { id }) => {
                    let peer = Peer::Device { id };
                    self.activity.call_closed(kind, Some(&peer), waited);
                }
                Some(CallClient::Chat) => self.activity.call_answered_in_chat(kind, waited),
                Some(CallClient::Unknown) | None => self.activity.call_closed(kind, None, waited),
            }
        }
    }

    /// Writes `entry` down in its project's file via the serialized writer and applies it
    /// without doing disk I/O while holding `book`.
    pub(super) fn write(&self, book: &mut Book, entry: Entry) {
        let Some(project) = Self::entry_project(book, &entry) else {
            return;
        };
        let open_entries = still_open(book, &project);
        let closes = closing(book, &entry);
        self.history_cache.locked().remove(&project);
        self.writer.enqueue(project, entry.clone(), open_entries);
        apply(book, entry);
        self.note_closed(closes);
    }

    /// Writes `entry` durably to disk via the serialized writer WITHOUT holding `self.book`.
    fn append_sure_off_lock(
        &self,
        project: String,
        entry: Entry,
        open_entries: Vec<Entry>,
    ) -> Result<(), String> {
        self.history_cache.locked().remove(&project);
        self.writer
            .append_sure(&self.log, project, entry, open_entries)
            .map_err(|error| format!("Sikemux could not record it, so nothing ran: {error}"))
    }

    fn apply_recorded(&self, book: &mut Book, entry: Entry) {
        let closes = closing(book, &entry);
        apply(book, entry);
        self.note_closed(closes);
    }

    pub(super) fn changed(&self, core: &Core) {
        core.changed(What::View);
    }

    /// Every call the device view shows: those still open, oldest first, then those answered or
    /// closed lately, newest first.
    pub fn shown(&self) -> Vec<PersonCall> {
        let now = epoch_ms();
        let book = self.book.locked();
        let mut open: Vec<PersonCall> = (book.iter())
            .filter(|kept| kept.call.state == CallState::Open)
            .map(|kept| kept.call.clone())
            .collect();
        open.sort_by_key(|call| call.raised_at);
        let mut closed: Vec<PersonCall> = (book.iter())
            .filter(|kept| kept.call.state != CallState::Open)
            .filter(|kept| now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS)
            .map(|kept| kept.call.clone())
            .collect();
        closed.sort_by_key(|call| std::cmp::Reverse(settled_at(call)));
        closed.truncate(MAX_SHOWN_CLOSED);
        open.into_iter().chain(closed).collect()
    }

    /// The open call `agent_id` has waited on longest, and how many it waits on; a look call
    /// waits on nothing.
    pub fn of_agent(&self, agent_id: &str) -> Option<AgentCall> {
        let book = self.book.locked();
        let open: Vec<&PersonCall> = (book.iter())
            .map(|kept| &kept.call)
            .filter(|call| call.state == CallState::Open && waits_on(call, agent_id))
            .filter(|call| call.kind != CallKind::Look)
            .collect();
        let top = open.iter().min_by_key(|call| call.raised_at)?;
        Some(AgentCall {
            id: top.id.clone(),
            question: sikemux_fold::first_words(&top.question),
            waiting_since: top.raised_at,
            count: open.len(),
        })
    }

    /// What came of the newest production call `agent_id` asked or joined that has an outcome.
    pub fn latest_outcome(&self, agent_id: &str) -> Option<CallOutcome> {
        (self.book.locked().iter())
            .map(|kept| &kept.call)
            .filter(|call| call.kind == CallKind::Production && waits_on(call, agent_id))
            .filter_map(|call| call.outcome.clone())
            .max_by_key(|outcome| outcome.at)
    }

    pub fn known_ids(&self) -> HashSet<String> {
        self.book.locked().entries.keys().cloned().collect()
    }

    /// The first call of `project` that `matches`, open or not.
    pub fn find(&self, project: &str, matches: impl Fn(&PersonCall) -> bool) -> Option<PersonCall> {
        (self.book.locked().iter())
            .map(|kept| &kept.call)
            .find(|call| call.project == project && matches(call))
            .cloned()
    }

    /// Puts the calls `owns` names among `wanted` to the person, and closes the open calls it
    /// names that are no longer wanted. A call already raised keeps what it was raised with, and
    /// one closed unanswered opens again.
    fn sync(&self, core: &Core, owns: impl Fn(&Source) -> bool, wanted: Vec<Wanted>) {
        let mut entries = Vec::new();
        let mut book = self.book.locked();
        let is_wanted = |source: &Source| wanted.iter().any(|wanted| wanted.source == *source);
        for kept in book.iter() {
            if kept.call.state != CallState::Open || !owns(&kept.source) {
                continue;
            }
            let id = kept.call.id.clone();
            let sources = kept.sources();
            let (live, gone): (Vec<&Source>, Vec<&Source>) =
                sources.partition(|source| is_wanted(source));
            if live.is_empty() {
                let outcome = outcome(CallResult::Withdrawn);
                entries.push(Entry::Came { id, outcome });
                continue;
            }
            for source in gone {
                let source = source.clone();
                let at = epoch_ms();
                entries.push(Entry::Left {
                    id: id.clone(),
                    source,
                    at,
                });
            }
        }
        for Wanted {
            source,
            call,
            key,
            yes,
        } in wanted
        {
            let same = |kept: &&Kept| {
                kept.call.state == CallState::Open
                    && matches!(kept.source, Source::Question(_))
                    && matches!(source, Source::Question(_))
                    && call.kind != CallKind::Production
                    && kept.call.secret == call.secret
                    && kept.call.multi == call.multi
                    && same_decision(&kept.call, &call)
            };
            match book.iter().find(|kept| raised_by(kept, &source)) {
                Some(kept) if kept.call.state == CallState::Closed => {
                    let id = kept.call.id.clone();
                    entries.push(Entry::Reopened { id, at: epoch_ms() });
                }
                Some(kept) if kept.source == source && edited(&kept.call, &call) => {
                    entries.push(Entry::Edited {
                        id: kept.call.id.clone(),
                        call: Box::new(call),
                    });
                }
                Some(_) => {}
                None => match book.iter().find(same) {
                    Some(kept) => entries.push(Entry::Joined {
                        id: kept.call.id.clone(),
                        asker: call.asker.clone(),
                        source: Some(source),
                        at: epoch_ms(),
                    }),
                    None => entries.push(Entry::Raised {
                        call: Box::new(call),
                        source,
                        also: Vec::new(),
                        key,
                        yes,
                        attempt: None,
                    }),
                },
            }
        }
        let mut sure_entries = Vec::new();
        let moved = !entries.is_empty();
        for entry in entries {
            let is_sure =
                matches!(&entry, Entry::Raised { call, .. } if call.kind == CallKind::Production);
            if is_sure {
                if let Some(project) = Self::entry_project(&book, &entry) {
                    let open_entries = still_open(&book, &project);
                    sure_entries.push((project, entry, open_entries));
                }
            } else {
                self.write(&mut book, entry);
            }
        }
        drop(book);
        for (project, entry, open_entries) in sure_entries {
            match self.append_sure_off_lock(project, entry.clone(), open_entries) {
                Ok(()) => {
                    let mut book = self.book.locked();
                    self.apply_recorded(&mut book, entry);
                }
                Err(error) => eprintln!("sikemux core: a call was not raised: {error}"),
            }
        }
        if moved {
            self.changed(core);
        }
    }

    fn seen_by(&self, id: &str, client: &CallClient) -> bool {
        kept(&self.book.locked(), id)
            .is_some_and(|kept| kept.seen.iter().any(|(seer, _)| seer == client))
    }

    pub(super) fn id_of(&self, source: &Source) -> Option<String> {
        (self.book.locked().iter())
            .find(|kept| raised_by(kept, source) && kept.call.state == CallState::Open)
            .map(|kept| kept.call.id.clone())
    }

    /// The questions waiting on the call `id`: the one that raised it, then those that joined.
    pub(super) fn questions_of(&self, id: &str) -> Vec<String> {
        let book = self.book.locked();
        let Some(kept) = kept(&book, id) else {
            return Vec::new();
        };
        kept.sources()
            .filter_map(|source| match source {
                Source::Question(id) => Some(id.clone()),
                Source::Hold(_) | Source::Raised => None,
            })
            .collect()
    }

    /// Takes the person's answer to the open call `id`, which waits for what came of it.
    pub(super) fn take_answer(
        &self,
        id: &str,
        answer: CallAnswer,
    ) -> CoreResult<(PersonCall, Source)> {
        let answered = Entry::Answered {
            id: id.to_owned(),
            answer,
        };
        let sure_info = {
            let mut book = self.book.locked();
            let kept_open = open(&book, id)?;
            let production = kept_open.call.kind == CallKind::Production;
            if !production {
                self.write(&mut book, answered);
                let kept = kept(&book, id).ok_or(GONE)?;
                return Ok((kept.call.clone(), kept.source.clone()));
            }
            let project = kept_open.call.project.clone();
            let open_entries = still_open(&book, &project);
            (project, open_entries)
        };
        self.append_sure_off_lock(sure_info.0, answered.clone(), sure_info.1)?;
        let mut book = self.book.locked();
        open(&book, id)?;
        self.apply_recorded(&mut book, answered);
        let kept = kept(&book, id).ok_or(GONE)?;
        Ok((kept.call.clone(), kept.source.clone()))
    }

    /// The answer could not reach what raised the call, so it waits on the person again.
    pub(super) fn reopen(&self, id: &str) {
        let mut book = self.book.locked();
        self.write(
            &mut book,
            Entry::Reopened {
                id: id.to_owned(),
                at: epoch_ms(),
            },
        );
    }

    pub(super) fn settle(&self, id: &str, result: CallResult) {
        let mut book = self.book.locked();
        self.write(
            &mut book,
            Entry::Came {
                id: id.to_owned(),
                outcome: outcome(result),
            },
        );
    }
}

/// Whether answering `option` lets what the call holds run.
fn allows(kept: &Kept, option: &str) -> bool {
    kept.yes.as_deref() == Some(option)
        || (kept.yes.is_some() && matches!(option, super::ship::FROM_NOW_ON | super::ship::ALWAYS))
}

/// Whether `source` raised the call, first or since.
pub(super) fn raised_by(kept: &Kept, source: &Source) -> bool {
    kept.sources().any(|raised| raised == source)
}

/// Whether two calls put the same decision: the same kind, in the same project, asking the same
/// thing with the same options, and both askers waiting or both going on, since letting an
/// assumption stand answers only an asker that goes on.
fn same_decision(one: &PersonCall, other: &PersonCall) -> bool {
    let plain = |text: &str| {
        let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
        words.trim_end_matches(['?', '.', '!']).to_lowercase()
    };
    let labels = |call: &PersonCall| -> Vec<String> {
        call.options
            .iter()
            .map(|option| plain(&option.label))
            .collect()
    };
    one.kind == other.kind
        && one.project == other.project
        && plain(&one.question) == plain(&other.question)
        && labels(one) == labels(other)
        && one.assumption.is_some() == other.assumption.is_some()
}

fn settled_at(call: &PersonCall) -> u64 {
    (call.outcome.as_ref().map(|outcome| outcome.at))
        .or(call.answer.as_ref().map(|answer| answer.at))
        .unwrap_or(call.raised_at)
}

/// Puts a call to the person, or joins `raise.asker` to the open call of the project with the
/// same key. Answers the call's id, or why a production call could not be raised.
pub fn raise(core: &Core, raise: Raise) -> Result<String, String> {
    let calls = core.person_calls();
    let pending_sure = {
        let mut book = calls.book.locked();
        let same = book.iter().find(|kept| {
            kept.call.state == CallState::Open
                && kept.call.project == raise.project
                && kept.key.is_some()
                && kept.key == raise.key
        });
        match same.map(|kept| kept.call.id.clone()) {
            Some(id) => {
                let asker = raise.asker;
                let joined = Entry::Joined {
                    id: id.clone(),
                    asker,
                    source: None,
                    at: epoch_ms(),
                };
                calls.write(&mut book, joined);
                Ok(id)
            }
            None => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let is_prod = raise.kind == CallKind::Production;
                let project = raise.project.clone();
                let raised = raise.into_entry(id.clone());
                if is_prod {
                    let open_entries = still_open(&book, &project);
                    Err((id, project, raised, open_entries))
                } else {
                    calls.write(&mut book, raised);
                    Ok(id)
                }
            }
        }
    };
    let id = match pending_sure {
        Ok(id) => id,
        Err((id, project, raised, open_entries)) => {
            calls.append_sure_off_lock(project, raised.clone(), open_entries)?;
            let mut book = calls.book.locked();
            calls.apply_recorded(&mut book, raised);
            id
        }
    };
    calls.changed(core);
    Ok(id)
}

/// Puts a call the person is only told of, already settled with `outcome`: a production change
/// at a place they asked to hear of, which ran. It never waits on them.
pub fn told(core: &Core, raise: Raise, outcome: CallOutcome) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let raised = raise.into_entry(id.clone());
    {
        let mut book = core.person_calls().book.locked();
        core.person_calls().write(&mut book, raised);
        core.person_calls().write(
            &mut book,
            Entry::Came {
                id: id.clone(),
                outcome,
            },
        );
    }
    core.person_calls().changed(core);
    id
}

/// Puts `reason`, why `agent_id` needs what the gate's open call `id` holds, on that call. False
/// when that agent raised no such call.
pub fn explain(core: &Core, agent_id: &str, id: &str, reason: &str) -> bool {
    let mut book = core.person_calls().book.locked();
    let held = (book.iter())
        .find(|kept| kept.call.id == id && kept.call.state == CallState::Open)
        .filter(|kept| kept.source == Source::Raised && kept.call.place.is_some())
        .filter(|kept| kept.call.asker.agent_id == agent_id);
    let Some(mut call) = held.map(|kept| kept.call.clone()) else {
        return false;
    };
    call.question = super::ship::with_reason(&call.question, reason);
    let edited = Entry::Edited {
        id: id.to_owned(),
        call: Box::new(call),
    };
    core.person_calls().write(&mut book, edited);
    drop(book);
    core.person_calls().changed(core);
    true
}

/// What came of an agent taking back a production call it waits on.
pub enum TakenBack {
    Done,
    /// The person had answered it, or it had closed.
    TooLate,
    /// The agent waits on no such call.
    NotTheirs,
}

/// Takes back `agent_id`'s wait on the call `id` of an action held for it: the call closes,
/// unless an agent that isn't `gone` waits on it too.
pub fn take_back(
    core: &Core,
    agent_id: &str,
    id: &str,
    why: Option<CallWhy>,
    gone: impl Fn(&str) -> bool,
) -> TakenBack {
    let mut book = core.person_calls().book.locked();
    let Some(kept) = kept(&book, id)
        .filter(|kept| match kept.source {
            Source::Hold(_) => true,
            Source::Raised => kept.call.place.is_some(),
            Source::Question(_) => false,
        })
        .filter(|kept| waits_on(&kept.call, agent_id))
    else {
        return TakenBack::NotTheirs;
    };
    if kept.call.state != CallState::Open {
        return TakenBack::TooLate;
    }
    let others = std::iter::once(&kept.call.asker)
        .chain(&kept.call.joined)
        .any(|asker| asker.agent_id != agent_id && !gone(&asker.agent_id));
    let id = id.to_owned();
    let entry = match others {
        true => Entry::Quit {
            id,
            agent: agent_id.to_owned(),
            at: epoch_ms(),
        },
        false => Entry::Came {
            id,
            outcome: CallOutcome {
                why,
                ..outcome(CallResult::Withdrawn)
            },
        },
    };
    core.person_calls().write(&mut book, entry);
    drop(book);
    core.person_calls().changed(core);
    TakenBack::Done
}

/// The newest call of `project` raised with `key`, open or not.
pub fn find_key(core: &Core, project: &str, key: &str) -> Option<PersonCall> {
    (core.person_calls().book.locked().iter())
        .filter(|kept| kept.call.project == project && kept.key.as_deref() == Some(key))
        .max_by_key(|kept| kept.call.raised_at)
        .map(|kept| kept.call.clone())
}

/// Every call of `project` whose key `matches`, with that key.
pub fn find_keys(
    core: &Core,
    project: &str,
    matches: impl Fn(&str) -> bool,
) -> Vec<(String, PersonCall)> {
    (core.person_calls().book.locked().iter())
        .filter(|kept| kept.call.project == project)
        .filter_map(|kept| Some((kept.key.clone()?, kept)))
        .filter(|(key, _)| matches(key))
        .map(|(key, kept)| (key, kept.call.clone()))
        .collect()
}

/// Says what came of an answered call, such as that it shipped and its place stayed healthy.
pub fn set_outcome(core: &Core, id: &str, outcome: CallOutcome) {
    let mut book = core.person_calls().book.locked();
    if kept(&book, id).is_some() {
        core.person_calls().write(
            &mut book,
            Entry::Came {
                id: id.to_owned(),
                outcome,
            },
        );
        drop(book);
        core.person_calls().changed(core);
    }
}

/// What should be put to the person now: what raised it, the call, and the key it is found by.
pub struct Wanted {
    pub source: Source,
    pub call: PersonCall,
    /// What a gate finds the call's yes by.
    pub key: Option<String>,
    /// The option that lets what it holds run, on a call where one does.
    pub yes: Option<String>,
}

/// The questions now put to the person, as calls.
pub fn sync_questions(core: &Core, wanted: Vec<Wanted>) {
    (core.person_calls()).sync(core, |source| matches!(source, Source::Question(_)), wanted);
}

/// The actions now held for the person, as calls.
pub fn sync_holds(core: &Core, wanted: Vec<(String, PersonCall)>) {
    let wanted = (wanted.into_iter())
        .map(|(id, call)| Wanted {
            source: Source::Hold(id),
            call,
            key: None,
            yes: Some(HOLD_ALLOW.to_owned()),
        })
        .collect();
    (core.person_calls()).sync(core, |source| matches!(source, Source::Hold(_)), wanted);
}

/// What the yes of the call `id` lets run is about to start, so a restart says it may have run
/// rather than that nothing did: the first time, and again once what ran last has an outcome.
/// When that can't be recorded for sure, nothing may run.
pub fn started(core: &Core, id: &str) -> Result<(), String> {
    let calls = core.person_calls();
    let pending = {
        let book = calls.book.locked();
        let since = |kept: &Kept| match kept.started {
            None => true,
            Some(started) => (kept.call.outcome.as_ref()).is_some_and(|came| came.at > started),
        };
        match kept(&book, id) {
            Some(kept) if since(kept) => {
                let project = kept.call.project.clone();
                let open_entries = still_open(&book, &project);
                let started = Entry::Started {
                    id: id.to_owned(),
                    at: epoch_ms(),
                };
                Some((project, started, open_entries))
            }
            _ => None,
        }
    };
    let Some((project, started, open_entries)) = pending else {
        return Ok(());
    };
    calls.append_sure_off_lock(project, started.clone(), open_entries)?;
    let mut book = calls.book.locked();
    calls.apply_recorded(&mut book, started);
    Ok(())
}

impl PersonCalls {
    /// Marks the call `id` started when its answer is the yes that runs what it holds now, as an
    /// approval's plugin call runs with the answer. A held action runs later, and says so itself.
    pub(super) fn starting_on_yes(&self, id: &str) -> Result<(), String> {
        let pending = {
            let book = self.book.locked();
            let Some(kept) = kept(&book, id) else {
                return Ok(());
            };
            let picked = (kept.call.answer.as_ref()).and_then(|answer| answer.option.as_deref());
            let gate = matches!(kept.source, Source::Raised)
                && kept.call.place.is_some()
                && kept.key.is_some();
            let now = !matches!(kept.source, Source::Hold(_))
                && !gate
                && picked.is_some_and(|option| allows(kept, option));
            if !now || kept.started.is_some() {
                return Ok(());
            }
            let project = kept.call.project.clone();
            let open_entries = still_open(&book, &project);
            let started = Entry::Started {
                id: id.to_owned(),
                at: epoch_ms(),
            };
            (project, started, open_entries)
        };
        self.append_sure_off_lock(pending.0, pending.1.clone(), pending.2)?;
        let mut book = self.book.locked();
        self.apply_recorded(&mut book, pending.1);
        Ok(())
    }

    /// Takes back an answer whose yes could not be recorded as starting, so nothing ran on it.
    pub(super) fn unanswer(&self, core: &Core, id: &str, error: String) -> CoreError {
        self.reopen(id);
        self.changed(core);
        CoreError::from(error)
    }
}

/// Every question an open call still waits on, which an agent's next turn leaves open.
impl PersonCalls {
    pub fn open_questions(&self) -> Vec<String> {
        (self.book.locked().iter())
            .filter(|kept| kept.call.state == CallState::Open)
            .flat_map(Kept::sources)
            .filter_map(|source| match source {
                Source::Question(id) => Some(id.clone()),
                Source::Hold(_) | Source::Raised => None,
            })
            .collect()
    }
}

/// Closes each call `raise` put whose askers have all `gone`: nobody is left to hear its answer.
pub fn close_left(core: &Core, gone: impl Fn(&str) -> bool) {
    let mut book = core.person_calls().book.locked();
    let left: Vec<String> = (book.iter())
        .filter(|kept| kept.source == Source::Raised && kept.call.state == CallState::Open)
        .filter(|kept| {
            let askers = std::iter::once(&kept.call.asker).chain(&kept.call.joined);
            (askers.into_iter()).all(|asker| !asker.agent_id.is_empty() && gone(&asker.agent_id))
        })
        .map(|kept| kept.call.id.clone())
        .collect();
    if left.is_empty() {
        return;
    }
    for id in left {
        let outcome = outcome(CallResult::Withdrawn);
        core.person_calls()
            .write(&mut book, Entry::Came { id, outcome });
    }
    drop(book);
    core.person_calls().changed(core);
}

/// A client drew the call.
pub fn seen(core: &Core, peer: &Peer, params: CallParams) -> CoreResult<()> {
    let by = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let mut book = core.person_calls().book.locked();
    let kept = kept(&book, &params.id).ok_or(GONE)?;
    if !kept.seen.iter().any(|(seer, _)| *seer == by) {
        let shown = Entry::Shown {
            id: params.id,
            by,
            at: epoch_ms(),
        };
        core.person_calls().write(&mut book, shown);
    }
    Ok(())
}

/// The call `id` while it waits on the person.
pub fn open_call(core: &Core, id: &str) -> Option<PersonCall> {
    call_of(core, id).ok()
}

/// The open call the question `id` raised or joined.
pub fn asked_by(core: &Core, id: &str) -> Option<PersonCall> {
    let source = Source::Question(id.to_owned());
    let call = core.person_calls().id_of(&source)?;
    open_call(core, &call)
}

/// The open calls only this module tells the person of: those not raised by a question or a
/// held action, which are told of as they always were.
pub fn raised_open(core: &Core) -> Vec<PersonCall> {
    (core.person_calls().book.locked().iter())
        .filter(|kept| kept.source == Source::Raised && kept.call.state == CallState::Open)
        .map(|kept| kept.call.clone())
        .collect()
}

impl super::part::Part for PersonCalls {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        let left = std::mem::take(&mut *self.after_start.locked());
        let nothing = left.lapsed.is_empty()
            && left.interrupted.is_empty()
            && left.dropped.is_empty()
            && left.gated.is_empty()
            && left.undelivered.is_empty();
        if nothing {
            return Vec::new();
        }
        let core = core.clone();
        vec![tokio::spawn(
            async move { after_restart(&core, left).await },
        )]
    }
}

/// What the agent of the gate's call `id` tried to run.
pub fn attempt_of(core: &Core, id: &str) -> Option<Attempt> {
    let book = core.person_calls().book.locked();
    kept(&book, id)?.attempt.as_deref().cloned()
}

/// Every call `project` has had, as its file keeps them, newest first, or `None` when its file
/// could not be read. Reads the disk, so it blocks.
pub fn history(core: &Core, project: &str) -> Option<Vec<PersonCall>> {
    let calls = core.person_calls();
    if let Some(cached) = calls.history_cache.locked().get(project).cloned() {
        return Some(cached);
    }
    calls.writer.flush();
    let mut book = Book::new();
    for entry in calls.log.read_project(project)? {
        apply_with(&mut book, entry, false);
    }
    let mut listed: Vec<PersonCall> = book.into_values().map(|kept| kept.call).collect();
    listed.sort_by_key(|call| std::cmp::Reverse(call.raised_at));
    calls
        .history_cache
        .locked()
        .insert(project.to_owned(), listed.clone());
    Some(listed)
}

/// The calls of `project` the person answered from `from` to `to`, read from its whole record:
/// whether each answer was the asker's pick, chosen or typed as its label.
pub fn answered_since(core: &Core, project: &str, from: u64, to: u64) -> Vec<bool> {
    (history(core, project).unwrap_or_default().into_iter())
        .filter_map(|call| {
            let answer = (call.answer.as_ref()).filter(|answer| (from..to).contains(&answer.at))?;
            let pick = (call.options.iter()).find(|option| Some(&option.id) == call.pick.as_ref());
            let typed_pick = (answer.words.as_deref().zip(pick)).is_some_and(|(words, pick)| {
                words.trim().to_lowercase() == pick.label.trim().to_lowercase()
            });
            Some((call.pick.is_some() && answer.option == call.pick) || typed_pick)
        })
        .collect()
}

/// Every call the device view shows, with its headline, each asker marked when it is a project's
/// chief.
pub fn shown(core: &Core) -> Vec<PersonCall> {
    let chiefs = (core.repo_above().chief_all)(core);
    let mut calls = core.person_calls().shown();
    for call in &mut calls {
        call.headline = sikemux_fold::first_words(&call.question);
        for asker in std::iter::once(&mut call.asker).chain(call.joined.iter_mut()) {
            asker.chief = chiefs.contains(&asker.agent_id);
        }
    }
    calls
}

/// The files given with an answer as its text lists them.
pub fn attached(paths: &[String]) -> String {
    let listed: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
    format!("Attached:\n{}", listed.join("\n"))
}

/// An answer's text without its list of `paths`, for an agent that takes the files themselves.
pub fn without_attached(text: &str, paths: &[String]) -> String {
    let list = attached(paths);
    let text = text.replacen(&format!("\n\n{list}"), "", 1);
    text.replacen(&list, "", 1).trim().to_owned()
}
