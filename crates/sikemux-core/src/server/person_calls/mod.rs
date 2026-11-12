//! Calls: every decision put to the person, whatever raised it. A question an agent's `ask`
//! takes to the person is one, and so is an action held at a production place. Each is asked
//! once and waits until the person answers it; nothing times it out. Every client draws the same
//! card from the device view and answers it with `answerCall`, and the answer reaches whatever
//! raised the call. A production call passes only on a client that drew it, by its button.

mod log;

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    AnswerCallParams, CallAnswer, CallAsker, CallBrief, CallClient, CallEvidence, CallKind,
    CallOption, CallOutcome, CallParams, CallPlace, CallResult, CallState, CallWhy, PersonCall,
};

use self::log::{CallLog, Entry};
use super::access::Peer;
use super::activity_log::ActivityLog;
use super::changed::What;
use super::questions::AnswerParams;
use super::{messages, questions, ship, Core, CoreError, CoreResult};
use crate::text::clipped;

/// How long an answered or closed call stays in the device view.
const SHOWN_AFTER_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_SHOWN_CLOSED: usize = 16;

/// What raised a call, which its answer goes back to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "from", content = "id")]
pub(crate) enum Source {
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
pub(crate) struct Also {
    source: Source,
    agent: String,
}

struct Kept {
    call: PersonCall,
    source: Source,
    /// What else raised the same decision since, each waiting on its answer.
    also: Vec<Also>,
    /// Calls raised with the same key are joined into one.
    key: Option<String>,
    /// The option that lets what it holds run, on a call where one does.
    yes: Option<String>,
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
struct AfterStart {
    /// Calls whose yes had not started what it lets run: their askers hear it lapsed.
    lapsed: Vec<String>,
    /// Calls whose yes had started what it lets run, unseen to its end: their askers hear it may
    /// have run.
    interrupted: Vec<String>,
    /// Held actions, which a restart lets go of: their agents hear to run them again.
    dropped: Vec<String>,
    /// Other calls whose answer may not have reached their askers.
    undelivered: Vec<String>,
}

/// Every call open, and those settled lately, as their entries left them.
type Book = Vec<Kept>;

pub(crate) struct PersonCalls {
    book: Mutex<Book>,
    /// What a restart left to do once the core runs, as `PersonCalls::new` found it.
    after_start: Mutex<AfterStart>,
    log: CallLog,
    /// Counts how long each call waited and who answered it.
    activity: Arc<ActivityLog>,
}

/// A call to put to the person, as [`raise`] takes it.
#[expect(
    dead_code,
    reason = "the gates by place raise production calls through it"
)]
pub(crate) struct Raise {
    pub project: String,
    pub kind: CallKind,
    pub asker: CallAsker,
    pub asked_for: Option<String>,
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
}

impl Raise {
    fn into_call(self, id: String) -> (PersonCall, Option<String>, Option<String>) {
        let call = PersonCall {
            id,
            project: self.project,
            can_hand_back: self.kind == CallKind::Taste,
            kind: self.kind,
            asker: self.asker,
            joined: Vec::new(),
            asked_for: self.asked_for,
            question: self.question,
            context: self.context,
            options: self.options,
            pick: self.pick,
            against: self.against,
            evidence: self.evidence,
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
        (call, self.key, self.yes)
    }
}

/// The open call an agent waits on longest, with how many it waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCall {
    pub id: String,
    pub question: String,
    pub waiting_since: u64,
    pub count: usize,
}

fn client_of(peer: &Peer) -> Option<CallClient> {
    match peer {
        Peer::App => Some(CallClient::App),
        Peer::Device { id } => Some(CallClient::Device { id: id.clone() }),
        Peer::Local | Peer::Share { .. } => None,
    }
}

const NOT_A_CLIENT: &str =
    "only the person answers a call, from Sikemux, its notch or a paired phone";
const GONE: &str = "that call no longer waits on you";

fn waits_on(call: &PersonCall, agent_id: &str) -> bool {
    call.asker.agent_id == agent_id || call.joined.iter().any(|asker| asker.agent_id == agent_id)
}

fn outcome(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

fn kept<'a>(book: &'a Book, id: &str) -> Option<&'a Kept> {
    book.iter().find(|kept| kept.call.id == id)
}

fn open<'a>(book: &'a Book, id: &str) -> CoreResult<&'a Kept> {
    Ok(kept(book, id)
        .filter(|kept| kept.call.state == CallState::Open)
        .ok_or(GONE)?)
}

/// What `entry` changes of the calls kept.
fn apply(book: &mut Book, entry: Entry) {
    let id = entry.id().to_owned();
    if let Entry::Raised {
        call,
        source,
        also,
        key,
        yes,
    } = entry
    {
        let now = epoch_ms();
        let seen = (book.iter())
            .find(|kept| kept.call.id == id)
            .map(|kept| kept.seen.clone())
            .unwrap_or_default();
        book.retain(|kept| {
            kept.call.id != id
                && (kept.call.state == CallState::Open
                    || now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS)
        });
        book.push(Kept {
            call: *call,
            source,
            also,
            key,
            yes,
            started: None,
            seen,
        });
        return;
    }
    let Some(kept) = book.iter_mut().find(|kept| kept.call.id == id) else {
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
        Entry::Started { at, .. } => kept.started = Some(at),
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
    pub(crate) fn new(
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
                Source::Raised => false,
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
        Self {
            book: Mutex::new(book),
            after_start: Mutex::new(after_start),
            log,
            activity,
        }
    }

    /// Writes `entry` down in its project's file and applies it, even when the file could not
    /// take it: what the person sees goes on, and the next restart knows less.
    fn write(&self, book: &mut Book, entry: Entry) {
        if let Err(error) = self.record(book, entry, false) {
            eprintln!("sikemux core: a call was not written down: {error}");
        }
    }

    /// Writes `entry` down and on to the disk itself before applying it, for what must outlast a
    /// power loss before anything runs on it. Nothing is applied when it could not be.
    fn write_sure(&self, book: &mut Book, entry: Entry) -> Result<(), String> {
        self.record(book, entry, true)
    }

    /// Writes `entry` down, sure of a production call's raising: one the file could not take
    /// is not raised, so it never lives in memory alone, and the next look raises it again.
    fn put(&self, book: &mut Book, entry: Entry) -> Result<(), String> {
        match &entry {
            Entry::Raised { call, .. } if call.kind == CallKind::Production => {
                self.write_sure(book, entry)
            }
            _ => {
                self.write(book, entry);
                Ok(())
            }
        }
    }

    fn record(&self, book: &mut Book, entry: Entry, sure: bool) -> Result<(), String> {
        let project = match &entry {
            Entry::Raised { call, .. } => Some(call.project.clone()),
            entry => kept(book, entry.id()).map(|kept| kept.call.project.clone()),
        };
        let Some(project) = project else {
            return Err(GONE.to_owned());
        };
        let written = (self.log).append(&project, &entry, sure, || still_open(book, &project));
        if let (true, Err(error)) = (sure, &written) {
            return Err(format!(
                "Sikemux could not record it, so nothing ran: {error}"
            ));
        }
        let closes = closing(book, &entry);
        apply(book, entry);
        if let Some((kind, by, waited)) = closes {
            let (kind, waited) = (kind_name(kind), std::time::Duration::from_millis(waited));
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
        written
    }

    fn changed(&self, core: &Core) {
        core.changed(What::View);
    }

    /// Every call the device view shows: those still open, oldest first, then those answered or
    /// closed lately, newest first.
    pub(crate) fn shown(&self) -> Vec<PersonCall> {
        let now = epoch_ms();
        let book = self.book.locked();
        let open = (book.iter())
            .filter(|kept| kept.call.state == CallState::Open)
            .map(|kept| kept.call.clone());
        let mut closed: Vec<PersonCall> = (book.iter())
            .filter(|kept| kept.call.state != CallState::Open)
            .filter(|kept| now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS)
            .map(|kept| kept.call.clone())
            .collect();
        closed.sort_by_key(|call| std::cmp::Reverse(settled_at(call)));
        closed.truncate(MAX_SHOWN_CLOSED);
        open.chain(closed).collect()
    }

    /// The open call `agent_id` has waited on longest, and how many it waits on.
    pub(crate) fn of_agent(&self, agent_id: &str) -> Option<AgentCall> {
        let book = self.book.locked();
        let open: Vec<&PersonCall> = (book.iter())
            .map(|kept| &kept.call)
            .filter(|call| call.state == CallState::Open && waits_on(call, agent_id))
            .collect();
        let top = open.iter().min_by_key(|call| call.raised_at)?;
        Some(AgentCall {
            id: top.id.clone(),
            question: top.question.clone(),
            waiting_since: top.raised_at,
            count: open.len(),
        })
    }

    /// What came of the newest production call `agent_id` asked or joined that has an outcome.
    #[expect(
        dead_code,
        reason = "the turn state shows how an agent's last ship went through it"
    )]
    pub(crate) fn latest_outcome(&self, agent_id: &str) -> Option<CallOutcome> {
        (self.book.locked().iter())
            .map(|kept| &kept.call)
            .filter(|call| call.kind == CallKind::Production && waits_on(call, agent_id))
            .filter_map(|call| call.outcome.clone())
            .max_by_key(|outcome| outcome.at)
    }

    /// The first call of `project` that `matches`, open or not.
    #[expect(
        dead_code,
        reason = "the gates by place raise production calls through it"
    )]
    pub(crate) fn find(
        &self,
        project: &str,
        matches: impl Fn(&PersonCall) -> bool,
    ) -> Option<PersonCall> {
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
                    }),
                },
            }
        }
        let moved = !entries.is_empty();
        for entry in entries {
            if let Err(error) = self.put(&mut book, entry) {
                eprintln!("sikemux core: a call was not raised: {error}");
            }
        }
        drop(book);
        if moved {
            self.changed(core);
        }
    }

    fn seen_by(&self, id: &str, client: &CallClient) -> bool {
        kept(&self.book.locked(), id)
            .is_some_and(|kept| kept.seen.iter().any(|(seer, _)| seer == client))
    }

    fn source_of(&self, id: &str) -> Option<Source> {
        kept(&self.book.locked(), id).map(|kept| kept.source.clone())
    }

    fn id_of(&self, source: &Source) -> Option<String> {
        (self.book.locked().iter())
            .find(|kept| raised_by(kept, source) && kept.call.state == CallState::Open)
            .map(|kept| kept.call.id.clone())
    }

    /// The questions waiting on the call `id`: the one that raised it, then those that joined.
    fn questions_of(&self, id: &str) -> Vec<String> {
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
    fn take_answer(&self, id: &str, answer: CallAnswer) -> CoreResult<(PersonCall, Source)> {
        let mut book = self.book.locked();
        let production = open(&book, id)?.call.kind == CallKind::Production;
        let answered = Entry::Answered {
            id: id.to_owned(),
            answer,
        };
        match production {
            true => self.write_sure(&mut book, answered)?,
            false => self.write(&mut book, answered),
        }
        let kept = kept(&book, id).ok_or(GONE)?;
        Ok((kept.call.clone(), kept.source.clone()))
    }

    /// The answer could not reach what raised the call, so it waits on the person again.
    fn reopen(&self, id: &str) {
        let mut book = self.book.locked();
        self.write(
            &mut book,
            Entry::Reopened {
                id: id.to_owned(),
                at: epoch_ms(),
            },
        );
    }

    fn settle(&self, id: &str, result: CallResult) {
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

/// Whether answering `option` lets what the call holds run: its yes, or always allowing a held
/// action.
fn allows(kept: &Kept, option: &str) -> bool {
    kept.yes.as_deref() == Some(option)
        || (matches!(kept.source, Source::Hold(_)) && option == HOLD_ALWAYS)
}

/// Whether `source` raised the call, first or since.
fn raised_by(kept: &Kept, source: &Source) -> bool {
    kept.sources().any(|raised| raised == source)
}

/// Whether two calls put the same decision: the same kind, in the same project, asking the same
/// thing with the same options.
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
}

/// What the person types to a secret or an access call is never kept: it may be a password or a
/// token.
fn hides_words(call: &PersonCall) -> bool {
    call.secret || call.kind == CallKind::Access
}

fn settled_at(call: &PersonCall) -> u64 {
    (call.outcome.as_ref().map(|outcome| outcome.at))
        .or(call.answer.as_ref().map(|answer| answer.at))
        .unwrap_or(call.raised_at)
}

/// Puts a call to the person, or joins `raise.asker` to the open call of the project with the
/// same key. Answers the call's id, or why a production call could not be raised.
#[expect(
    dead_code,
    reason = "the gates by place raise production calls through it"
)]
pub(crate) fn raise(core: &Core, raise: Raise) -> Result<String, String> {
    let id = {
        let mut book = core.person_calls.book.locked();
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
                core.person_calls.write(&mut book, joined);
                id
            }
            None => {
                let (call, key, yes) = raise.into_call(uuid::Uuid::new_v4().simple().to_string());
                let id = call.id.clone();
                let raised = Entry::Raised {
                    call: Box::new(call),
                    source: Source::Raised,
                    also: Vec::new(),
                    key,
                    yes,
                };
                core.person_calls.put(&mut book, raised)?;
                id
            }
        }
    };
    core.person_calls.changed(core);
    Ok(id)
}

/// Puts a call the person is only told of, already settled with `outcome`: a production change
/// at a place they asked to hear of, which ran. It never waits on them.
#[expect(
    dead_code,
    reason = "the gates by place tell the person of a change through it"
)]
pub(crate) fn told(core: &Core, raise: Raise, outcome: CallOutcome) -> String {
    let (call, key, yes) = raise.into_call(uuid::Uuid::new_v4().simple().to_string());
    let id = call.id.clone();
    {
        let mut book = core.person_calls.book.locked();
        let raised = Entry::Raised {
            call: Box::new(call),
            source: Source::Raised,
            also: Vec::new(),
            key,
            yes,
        };
        core.person_calls.write(&mut book, raised);
        core.person_calls.write(
            &mut book,
            Entry::Came {
                id: id.clone(),
                outcome,
            },
        );
    }
    core.person_calls.changed(core);
    id
}

/// The newest call of `project` raised with `key`, open or not.
#[expect(
    dead_code,
    reason = "the gates by place raise production calls through it"
)]
pub(crate) fn find_key(core: &Core, project: &str, key: &str) -> Option<PersonCall> {
    (core.person_calls.book.locked().iter())
        .filter(|kept| kept.call.project == project && kept.key.as_deref() == Some(key))
        .max_by_key(|kept| kept.call.raised_at)
        .map(|kept| kept.call.clone())
}

/// Says what came of an answered call, such as that it shipped and its place stayed healthy.
pub(crate) fn set_outcome(core: &Core, id: &str, outcome: CallOutcome) {
    let mut book = core.person_calls.book.locked();
    if kept(&book, id).is_some() {
        core.person_calls.write(
            &mut book,
            Entry::Came {
                id: id.to_owned(),
                outcome,
            },
        );
        drop(book);
        core.person_calls.changed(core);
    }
}

/// What should be put to the person now: what raised it, the call, and the key it is found by.
pub(crate) struct Wanted {
    pub source: Source,
    pub call: PersonCall,
    /// What a gate finds the call's yes by.
    pub key: Option<String>,
    /// The option that lets what it holds run, on a call where one does.
    pub yes: Option<String>,
}

/// The questions now put to the person, as calls.
pub(crate) fn sync_questions(core: &Core, wanted: Vec<Wanted>) {
    (core.person_calls).sync(core, |source| matches!(source, Source::Question(_)), wanted);
}

/// The actions now held for the person, as calls.
pub(crate) fn sync_holds(core: &Core, wanted: Vec<(String, PersonCall)>) {
    let wanted = (wanted.into_iter())
        .map(|(id, call)| Wanted {
            source: Source::Hold(id),
            call,
            key: None,
            yes: Some(HOLD_ALLOW.to_owned()),
        })
        .collect();
    (core.person_calls).sync(core, |source| matches!(source, Source::Hold(_)), wanted);
}

/// What the yes of the call `id` lets run is about to start, so a restart says it may have run
/// rather than that nothing did. When that can't be recorded for sure, nothing may run.
pub(crate) fn started(core: &Core, id: &str) -> Result<(), String> {
    let mut book = core.person_calls.book.locked();
    match kept(&book, id) {
        Some(kept) if kept.started.is_none() => {
            let at = epoch_ms();
            let started = Entry::Started {
                id: id.to_owned(),
                at,
            };
            core.person_calls.write_sure(&mut book, started)
        }
        _ => Ok(()),
    }
}

impl PersonCalls {
    /// Marks the call `id` started when its answer is the yes that runs what it holds now, as an
    /// approval's plugin call runs with the answer. A held action runs later, and says so itself.
    fn starting_on_yes(&self, id: &str) -> Result<(), String> {
        let mut book = self.book.locked();
        let Some(kept) = kept(&book, id) else {
            return Ok(());
        };
        let picked = (kept.call.answer.as_ref()).and_then(|answer| answer.option.as_deref());
        let now = !matches!(kept.source, Source::Hold(_))
            && picked.is_some_and(|option| allows(kept, option));
        if !now || kept.started.is_some() {
            return Ok(());
        }
        let started = Entry::Started {
            id: id.to_owned(),
            at: epoch_ms(),
        };
        self.write_sure(&mut book, started)
    }

    /// Takes back an answer whose yes could not be recorded as starting, so nothing ran on it.
    fn unanswer(&self, core: &Core, id: &str, error: String) -> CoreError {
        self.reopen(id);
        self.changed(core);
        CoreError::from(error)
    }
}

/// Every question an open call still waits on, which an agent's next turn leaves open.
impl PersonCalls {
    pub(crate) fn open_questions(&self) -> Vec<String> {
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
pub(crate) fn close_left(core: &Core, gone: impl Fn(&str) -> bool) {
    let mut book = core.person_calls.book.locked();
    let left: Vec<String> = (book.iter())
        .filter(|kept| kept.source == Source::Raised && kept.call.state == CallState::Open)
        .filter(|kept| {
            let askers = std::iter::once(&kept.call.asker).chain(&kept.call.joined);
            askers.into_iter().all(|asker| gone(&asker.agent_id))
        })
        .map(|kept| kept.call.id.clone())
        .collect();
    if left.is_empty() {
        return;
    }
    for id in left {
        let outcome = outcome(CallResult::Withdrawn);
        core.person_calls
            .write(&mut book, Entry::Came { id, outcome });
    }
    drop(book);
    core.person_calls.changed(core);
}

/// The longest answer typed in a chat that a call keeps.
const MAX_WRITTEN_CHARS: usize = 2000;

/// The person wrote to `agent_id` in its chat. When exactly one call it waits on there can be
/// answered by writing, one of taste, money or access its next turn takes back, that call is
/// answered in the chat; with several, none is, and they stay open. The words reach only this
/// chat, so a call other agents also wait on stays open for them. A production call is never
/// answered by writing, and an access call never keeps what was typed.
pub(crate) fn written(core: &Core, agent_id: &str, text: &str) {
    let questions = core.harness.questions.answered_by_writing(agent_id);
    if questions.is_empty() {
        return;
    }
    let mut book = core.person_calls.book.locked();
    let answerable: Vec<&Kept> = (book.iter())
        .filter(|kept| {
            let call = &kept.call;
            let asked = kept
                .sources()
                .any(|source| matches!(source, Source::Question(id) if questions.contains(id)));
            asked
                && call.state == CallState::Open
                && call.kind != CallKind::Production
                && !call.secret
        })
        .collect();
    let kept = match answerable.as_slice() {
        [] => return,
        [kept] => *kept,
        _ => return,
    };
    let shared = std::iter::once(&kept.call.asker)
        .chain(&kept.call.joined)
        .any(|asker| asker.agent_id != agent_id);
    if shared {
        let id = kept.call.id.clone();
        let mine = kept
            .sources()
            .filter(|source| matches!(source, Source::Question(id) if questions.contains(id)))
            .cloned()
            .collect::<Vec<_>>();
        for source in mine {
            let left = Entry::Left {
                id: id.clone(),
                source,
                at: epoch_ms(),
            };
            core.person_calls.write(&mut book, left);
        }
        drop(book);
        core.person_calls.changed(core);
        return;
    }
    let call = &kept.call;
    let words = clipped(text.trim(), MAX_WRITTEN_CHARS);
    let answer = CallAnswer {
        option: None,
        options: Vec::new(),
        note: None,
        words: Some(words).filter(|words| !words.is_empty() && call.kind != CallKind::Access),
        by: CallClient::Chat,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    let id = call.id.clone();
    let write = |book: &mut Book, entry| core.person_calls.write(book, entry);
    write(
        &mut book,
        Entry::Answered {
            id: id.clone(),
            answer,
        },
    );
    write(
        &mut book,
        Entry::Came {
            id,
            outcome: outcome(CallResult::Heard),
        },
    );
    drop(book);
    core.person_calls.changed(core);
}

/// A client drew the call.
pub(crate) fn seen(core: &Core, peer: &Peer, params: CallParams) -> CoreResult<()> {
    let by = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let mut book = core.person_calls.book.locked();
    let kept = kept(&book, &params.id).ok_or(GONE)?;
    if !kept.seen.iter().any(|(seer, _)| *seer == by) {
        let shown = Entry::Shown {
            id: params.id,
            by,
            at: epoch_ms(),
        };
        core.person_calls.write(&mut book, shown);
    }
    Ok(())
}

/// Whether a production place may open on this answer. The gates by place decide it.
fn may_pass(
    _core: &Core,
    _call: &PersonCall,
    _peer: &Peer,
    _option: Option<&str>,
) -> Result<(), String> {
    Ok(())
}

/// What the person's answer is, as the call keeps it, and what they typed with it. A secret is
/// never kept.
fn answer_of(
    call: &PersonCall,
    client: CallClient,
    params: &AnswerCallParams,
) -> CoreResult<(CallAnswer, Option<String>)> {
    let text = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let (note, words) = (text(&params.note), text(&params.words));
    let known = |id: &String| call.options.iter().any(|option| option.id == *id);
    if params.option.as_ref().is_some_and(|id| !known(id)) || !params.options.iter().all(known) {
        return Err("that is not one of the call's options".into());
    }
    if !params.options.is_empty() && !call.multi {
        return Err("that call takes one option".into());
    }
    let picked = params.option.is_some() || !params.options.is_empty();
    if picked && words.is_some() {
        return Err("answer with an option and a note, or in your own words".into());
    }
    if !picked && words.is_none() {
        return Err("pick an option or answer in your own words".into());
    }
    let typed = note.clone().or(words.clone());
    let refused = call.kind == CallKind::Production
        && !picked
        && call.options.iter().any(|option| option.id == HOLD_REFUSE);
    let (option, note, words) = match refused {
        true => (Some(HOLD_REFUSE.to_owned()), words, None),
        false => (params.option.clone(), note, words),
    };
    let answer = CallAnswer {
        option,
        options: params.options.clone(),
        note: note.filter(|_| !hides_words(call)),
        words: words.filter(|_| !hides_words(call)),
        by: client,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    Ok((answer, typed))
}

fn call_of(core: &Core, id: &str) -> CoreResult<PersonCall> {
    Ok(open(&core.person_calls.book.locked(), id)?.call.clone())
}

/// The person answers a call from the app, the notch or a phone. A production call is answered
/// only on a client that drew it, and only an option picked there can pass its place: words say
/// no, or what to do instead.
pub(crate) async fn answer(
    core: &Arc<Core>,
    peer: &Peer,
    params: AnswerCallParams,
) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let call = call_of(core, &params.id)?;
    let (answer, typed) = answer_of(&call, client.clone(), &params)?;
    if call.kind == CallKind::Production {
        if !core.person_calls.seen_by(&call.id, &client) {
            return Err(
                "open the call to answer it: a production call is answered where it was seen"
                    .into(),
            );
        }
        may_pass(core, &call, peer, answer.option.as_deref())?;
    }
    let (call, source) = core.person_calls.take_answer(&call.id, answer)?;
    if let Err(error) = core.person_calls.starting_on_yes(&call.id) {
        return Err(core.person_calls.unanswer(core, &call.id, error));
    }
    core.person_calls.changed(core);
    let reached = deliver(core, &call, &source, typed).await;
    if let Err(error) = reached {
        core.person_calls.reopen(&call.id);
        core.person_calls.changed(core);
        return Err(error);
    }
    Ok(())
}

/// Takes the answer to whatever raised the call.
async fn deliver(
    core: &Arc<Core>,
    call: &PersonCall,
    source: &Source,
    typed: Option<String>,
) -> CoreResult<()> {
    let answer = call.answer.as_ref().ok_or(GONE)?;
    match source {
        Source::Question(_) => {
            let index = |option: &String| call.options.iter().position(|known| known.id == *option);
            let mut reached = false;
            let mut failed = None;
            for id in core.person_calls.questions_of(&call.id) {
                let params = AnswerParams {
                    id,
                    answer: typed.clone(),
                    choice: answer.option.as_ref().and_then(index),
                    choices: (!answer.options.is_empty())
                        .then(|| answer.options.iter().filter_map(index).collect()),
                    ..AnswerParams::default()
                };
                match questions::answer(core, params).await {
                    Ok(()) => reached = true,
                    Err(error) => failed = failed.or(Some(error)),
                }
            }
            if !reached {
                return Err(failed.unwrap_or_else(|| GONE.to_owned()).into());
            }
            core.person_calls.settle(&call.id, CallResult::Heard);
        }
        Source::Hold(hold) => {
            let params = ship::HoldParams { hold: hold.clone() };
            match answer.option.as_deref() {
                Some(HOLD_ALLOW) => ship::allow(core, params).await?,
                Some(HOLD_ALWAYS) => ship::let_go(core, params).await?,
                _ => {
                    ship::refuse(core, params).await?;
                    core.person_calls.settle(&call.id, CallResult::NotShipped);
                }
            }
            if let Some(typed) = typed {
                let text = format!("The person added: {typed}");
                let _ = messages::for_person(core, call.asker.agent_id.clone(), text).await;
            }
        }
        Source::Raised if call.place.is_some() => call_answered(core, call, typed).await,
        Source::Raised => {
            tell_askers(core, call, typed).await;
            core.person_calls.settle(&call.id, CallResult::Heard);
        }
    }
    Ok(())
}

/// A production call the gates by place raised was answered.
async fn call_answered(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    tell_askers(core, call, typed).await;
}

/// Tells every agent waiting on the call what the person answered.
async fn tell_askers(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    let Some(said) = said(call, typed) else {
        return;
    };
    let text = format!("The person answered \"{}\":\n{said}", call.question);
    tell(core, call, &text).await;
}

/// What the person answered, in words: the option they picked and what they typed with it.
fn said(call: &PersonCall, typed: Option<String>) -> Option<String> {
    let answer = call.answer.as_ref()?;
    let picked = (answer.option.iter().chain(&answer.options))
        .filter_map(|id| call.options.iter().find(|option| option.id == *id))
        .map(|option| option.label.clone());
    let said: Vec<String> = picked.chain(typed).collect();
    (!said.is_empty()).then(|| said.join("\n\n"))
}

/// The option of a held action that lets it run once.
pub(crate) const HOLD_ALLOW: &str = "allow";
pub(crate) const HOLD_REFUSE: &str = "refuse";
/// The option that always lets what it lands in go, where that reads as no production.
pub(crate) const HOLD_ALWAYS: &str = "always";

/// An answer from a released client that names the question, the hold or the action: it counts as
/// the call's, and drawing the call is taken as given, since only a client that drew it has its id.
fn answered_before(
    core: &Core,
    peer: &Peer,
    source: &Source,
    answer: impl FnOnce(&PersonCall, CallClient) -> CallAnswer,
) -> CoreResult<Option<String>> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let Some(id) = core.person_calls.id_of(source) else {
        let settled = (core.person_calls.book.locked().iter()).any(|kept| raised_by(kept, source));
        return match settled {
            true => Err(GONE.into()),
            false => Ok(None),
        };
    };
    let call = call_of(core, &id)?;
    let given = answer(&call, client);
    if call.kind == CallKind::Production {
        may_pass(core, &call, peer, given.option.as_deref())?;
    }
    core.person_calls.take_answer(&id, given)?;
    if let Err(error) = core.person_calls.starting_on_yes(&id) {
        return Err(core.person_calls.unanswer(core, &id, error));
    }
    core.person_calls.changed(core);
    Ok(Some(id))
}

/// `answerQuestion` and `answerQuestions`, from clients released before calls: each answer is its
/// call's.
pub(crate) async fn answer_questions(
    core: &Arc<Core>,
    peer: &Peer,
    given: questions::AnswersParams,
) -> CoreResult<()> {
    let answered: Vec<&str> = given
        .answers
        .iter()
        .map(|params| params.id.as_str())
        .collect();
    let mut joined = Vec::new();
    for params in &given.answers {
        let Some(call) = core
            .person_calls
            .id_of(&Source::Question(params.id.clone()))
        else {
            continue;
        };
        let waiting = core.person_calls.questions_of(&call);
        for id in waiting
            .into_iter()
            .filter(|id| !answered.contains(&id.as_str()))
        {
            joined.push(AnswerParams {
                id,
                answer: params.answer.clone(),
                paths: params.paths.clone(),
                choice: params.choice,
                choices: params.choices.clone(),
                fine: params.fine,
                ..AnswerParams::default()
            });
        }
    }
    let mut taken = Vec::new();
    for params in &given.answers {
        let taken_now = answered_before(
            core,
            peer,
            &Source::Question(params.id.clone()),
            |call, by| {
                let option = |index: usize| call.options.get(index).map(|option| option.id.clone());
                let typed = (params.answer.clone())
                    .filter(|text| !text.trim().is_empty() && !hides_words(call));
                let picked = params.choice.is_some() || params.choices.is_some();
                CallAnswer {
                    option: params.choice.and_then(option),
                    options: (params.choices.iter().flatten())
                        .filter_map(|&index| option(index))
                        .collect(),
                    note: typed.clone().filter(|_| picked),
                    words: typed.filter(|_| !picked),
                    by,
                    at: epoch_ms(),
                    waited_ms: epoch_ms().saturating_sub(call.raised_at),
                }
            },
        );
        match taken_now {
            Ok(id) => taken.extend(id),
            Err(error) => {
                taken
                    .iter()
                    .for_each(|id: &String| core.person_calls.reopen(id));
                return Err(error);
            }
        }
    }
    match questions::answer_all(core, given).await {
        Ok(()) => {
            for params in joined {
                let _ = questions::answer(core, params).await;
            }
            for id in &taken {
                core.person_calls.settle(id, CallResult::Heard);
            }
            core.person_calls.changed(core);
            Ok(())
        }
        Err(error) => {
            taken.iter().for_each(|id| core.person_calls.reopen(id));
            core.person_calls.changed(core);
            Err(error.into())
        }
    }
}

/// `allowShip`, `refuseShip` and `letShipGo`, from clients released before calls.
pub(crate) async fn answer_hold(
    core: &Arc<Core>,
    peer: &Peer,
    params: ship::HoldParams,
    option: &str,
) -> CoreResult<()> {
    let source = Source::Hold(params.hold.clone());
    let taken = answered_before(core, peer, &source, |call, by| CallAnswer {
        option: Some(option.to_owned()),
        options: Vec::new(),
        note: None,
        words: None,
        by,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    })?;
    let ran = match option {
        HOLD_ALLOW => ship::allow(core, params).await,
        HOLD_ALWAYS => ship::let_go(core, params).await,
        _ => ship::refuse(core, params).await,
    };
    if let Some(id) = &taken {
        match &ran {
            Ok(()) if option == HOLD_REFUSE => core.person_calls.settle(id, CallResult::NotShipped),
            Ok(()) => {}
            Err(_) => core.person_calls.reopen(id),
        }
        core.person_calls.changed(core);
    }
    ran
}

/// The person hands a taste or direction call back: to the lead the question went to first,
/// which decides it, or else to the asker, which goes with its pick.
pub(crate) async fn not_mine(core: &Arc<Core>, peer: &Peer, params: CallParams) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let call = call_of(core, &params.id)?;
    if !call.can_hand_back {
        return Err("only a call of taste or direction can be handed back".into());
    }
    let source = core.person_calls.source_of(&call.id).ok_or(GONE)?;
    let answer = CallAnswer {
        option: None,
        options: Vec::new(),
        note: None,
        words: None,
        by: client,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    core.person_calls.take_answer(&call.id, answer)?;
    core.person_calls.changed(core);
    let handed = match &source {
        Source::Question(_) => {
            let mut handed = Err(GONE.to_owned());
            for id in core.person_calls.questions_of(&call.id) {
                let back = questions::hand_back(core, &id).await;
                handed = handed.or(back);
            }
            handed
        }
        Source::Hold(_) | Source::Raised => {
            Err("only a call of taste or direction can be handed back".to_owned())
        }
    };
    match handed {
        Ok(()) => core.person_calls.settle(&call.id, CallResult::HandedBack),
        Err(error) => {
            core.person_calls.reopen(&call.id);
            core.person_calls.changed(core);
            return Err(error.into());
        }
    }
    core.person_calls.changed(core);
    Ok(())
}

/// The call `id` while it waits on the person.
pub(crate) fn open_call(core: &Core, id: &str) -> Option<PersonCall> {
    call_of(core, id).ok()
}

/// The open call the question `id` raised or joined.
pub(crate) fn asked_by(core: &Core, id: &str) -> Option<PersonCall> {
    let source = Source::Question(id.to_owned());
    let call = core.person_calls.id_of(&source)?;
    open_call(core, &call)
}

/// The open calls only this module tells the person of: those not raised by a question or a
/// held action, which are told of as they always were.
pub(crate) fn raised_open(core: &Core) -> Vec<PersonCall> {
    (core.person_calls.book.locked().iter())
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

/// Finishes what a restart cut short. The agents waiting on a yes hear whether what it let run
/// never started, or started and may have run; an agent whose action was held hears to run it
/// again; and each other answer reaches its askers if it had not. A stopped agent hears when it
/// starts again.
async fn after_restart(core: &Arc<Core>, left: AfterStart) {
    let said = [
        (left.lapsed, "Sikemux restarted before your action ran, so the person's yes to \"{}\" lapsed. Nothing ran. Try it again when you need it; that asks them again."),
        (left.interrupted, "Sikemux restarted while your action ran after the person's yes to \"{}\", so it may have run. Check what happened before you try it again."),
        (left.dropped, "Sikemux restarted, so your action held for the person's answer to \"{}\" no longer waits. Run it again when you need it; that asks them again."),
    ];
    for (ids, words) in said {
        for id in ids {
            if let Some(call) = call_any(core, &id) {
                let text = words.replace("{}", &call.question);
                tell(core, &call, &text).await;
            }
        }
    }
    for id in left.undelivered {
        redeliver(core, &id).await;
    }
    core.person_calls.changed(core);
}

fn call_any(core: &Core, id: &str) -> Option<PersonCall> {
    kept(&core.person_calls.book.locked(), id).map(|kept| kept.call.clone())
}

/// Tells every agent waiting on the call, waiting for any that has stopped.
async fn tell(core: &Arc<Core>, call: &PersonCall, text: &str) {
    for asker in std::iter::once(&call.asker).chain(&call.joined) {
        let to = asker.agent_id.clone();
        let _ = messages::for_person_waiting(core, to, text.to_owned()).await;
    }
}

/// Gives an answer a restart may have kept from its askers. An answer whose question is no longer
/// open may have been lost between being taken and being sent, so it is sent as a plain message;
/// one whose words were never kept, as an access call's, waits on the person again.
async fn redeliver(core: &Arc<Core>, id: &str) {
    let Some((call, source)) = kept(&core.person_calls.book.locked(), id)
        .map(|kept| (kept.call.clone(), kept.source.clone()))
    else {
        return;
    };
    let Some(answer) = &call.answer else {
        return;
    };
    let typed = answer.note.clone().or(answer.words.clone());
    let picked = answer.option.is_some() || !answer.options.is_empty();
    if let Source::Hold(_) = source {
        if let Some(said) = said(&call, typed) {
            let text = format!(
                "Sikemux restarted as the person answered \"{}\", so the action it held no longer waits. They said:\n{said}",
                call.question
            );
            tell(core, &call, &text).await;
        }
        core.person_calls.settle(id, CallResult::NotShipped);
        return;
    }
    if let Source::Question(_) = source {
        let questions = core.person_calls.questions_of(id);
        let open = (questions.iter()).any(|question| core.harness.questions.is_open(question));
        if !open {
            let text = match said(&call, typed.clone()) {
                Some(said) => format!(
                    "The person answered \"{}\" as Sikemux restarted, and the answer may not have reached you:\n{said}",
                    call.question
                ),
                None => format!(
                    "The person answered \"{}\" as Sikemux restarted, and the answer was not kept. Ask them again if you still need it.",
                    call.question
                ),
            };
            tell(core, &call, &text).await;
            core.person_calls.settle(id, CallResult::Heard);
            return;
        }
        if !picked && typed.is_none() {
            core.person_calls.reopen(id);
            return;
        }
    }
    if deliver(core, &call, &source, typed).await.is_err() {
        core.person_calls.reopen(id);
    }
}

/// Every call the device view shows.
pub(crate) fn shown(core: &Core) -> Vec<PersonCall> {
    core.person_calls.shown()
}
