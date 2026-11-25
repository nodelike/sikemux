use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_calls::messages::MessageKind;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::format::{digest, joined, wrapped, Letter, Sender};
use super::log::MessageLog;
use super::{
    AUTO_SETTLE_MS, FILE, LOOP_MESSAGES, MAX_HELD, MAX_WAITING, PAIR_LIMIT, PAIR_WINDOW_MS,
    REPEAT_WINDOW_MS, REPORTED_MS, REPORT_DUE_MS, SETTLE_MS, TASK,
};
use crate::server::kept::Kept;
use crate::server::wake::Wake;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// The messages still waiting, as each agent reads them, kept in the data
/// folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MailboxRecord {
    waiting: HashMap<String, Vec<Queued>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    capped: HashMap<String, Vec<Queued>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    auto_turns: HashMap<String, AutoTurns>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(super) enum CalmWait {
    At { at: u64 },
    UntilSiblingsIdle { capped_at: u64 },
}

impl CalmWait {
    fn due_at(self) -> u64 {
        match self {
            CalmWait::At { at } => at,
            CalmWait::UntilSiblingsIdle { capped_at } => capped_at,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Origin {
    pub(super) from: String,
    pub(super) kind: MessageKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) awaited: bool,
    /// When a calm report may start the agent's turn; until then it rides on the next one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) calm: Option<CalmWait>,
}

/// A message waiting for its agent, as the agent reads it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Queued {
    pub(super) text: String,
    /// Unknown for a message that came back unread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<Origin>,
    /// What [`withdraw`] and [`is_waiting`] know it by: the question it puts, or [`TASK`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mark: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) paths: Vec<String>,
}

/// Whether every one of `queued` came from the person, so what they say is the person's words.
pub(super) fn by_person<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> bool {
    let person = Sender::Person.id();
    queued
        .into_iter()
        .all(|queued| (queued.origin.as_ref()).is_some_and(|origin| origin.from == person))
}

/// Whether anyone waits on one of `queued`: the person's words or something they just did, or an
/// answer the agent asked for. Those are read soon after a turn, and never wait for the person.
pub(super) fn awaited<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> bool {
    let person = Sender::Person.id();
    queued.into_iter().any(|queued| {
        queued.origin.as_ref().is_none_or(|origin| {
            origin.awaited || origin.from == person || origin.kind == MessageKind::Answer
        })
    })
}

/// Whether `queued` counts toward [`MAX_AUTO_TURNS`] and waits once the agent reaches it: nobody
/// waits on it, and it is no report from one of the agent's own workers nor anything from those
/// in charge of it.
pub(super) fn capped(core: &Core, agent_id: &str, queued: &Queued) -> bool {
    let own_report = queued.origin.as_ref().is_some_and(|origin| {
        origin.kind == MessageKind::Report
            && core
                .names()
                .get(&origin.from)
                .and_then(|named| named.parent)
                .as_deref()
                == Some(agent_id)
    });
    let from_above =
        (queued.origin.as_ref()).is_some_and(|origin| in_charge(core, &origin.from, agent_id));
    !own_report && !from_above && !awaited([queued])
}

/// Whether `from` is in charge of `agent_id`: the agent's lead, or its project's chief.
pub(super) fn in_charge(core: &Core, from: &str, agent_id: &str) -> bool {
    from != agent_id
        && (core
            .names()
            .get(agent_id)
            .and_then(|named| named.parent)
            .as_deref()
            == Some(from)
            || crate::server::chief::chief_over(core, agent_id).as_deref() == Some(from))
}

pub(super) fn settle_ms<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> u64 {
    match awaited(queued) {
        true => SETTLE_MS,
        false => AUTO_SETTLE_MS,
    }
}

impl Queued {
    pub(super) fn is_calm(&self) -> bool {
        (self.origin.as_ref()).is_some_and(|origin| origin.calm.is_some())
    }
}

pub(super) struct Sent {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) kind: MessageKind,
    pub(super) digest: u64,
    pub(super) at: u64,
}

/// A message held because its sender and the agent it is for keep messaging
/// each other and nothing else.
pub(super) struct Held {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) queued: Queued,
    pub(super) at: u64,
}

/// Two agents whose messages to each other are held, from when.
pub(crate) struct Loop {
    pub agents: (String, String),
    pub since: u64,
}

/// Messages typed into a terminal agent's prompt that it has not yet been
/// seen to take.
pub(super) struct Pasted {
    pub(super) texts: Vec<Queued>,
    /// When Enter was sent, once it was.
    pub(super) entered_at: Option<u64>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) waiting: HashMap<String, Vec<Queued>>,
    pub(super) sent: VecDeque<Sent>,
    pub(super) delivering: HashSet<String>,
    pub(super) wake_failed: HashMap<String, u64>,
    pub(super) pasted: HashMap<String, Pasted>,
    /// How many pastes in a row each terminal agent did not take.
    pub(super) missed_pastes: HashMap<String, u8>,
    /// When each agent last did something besides messaging other agents,
    /// within the pair window.
    pub(super) worked: HashMap<String, u64>,
    pub(super) held: Vec<Held>,
    /// How many turns each agent started in a row on messages nobody waits on, since the person
    /// last wrote to it, and when it reached [`MAX_AUTO_TURNS`].
    pub(super) auto_turns: HashMap<String, AutoTurns>,
    /// Messages held for each agent stopped at [`MAX_AUTO_TURNS`], until the person writes to it.
    pub(super) capped: HashMap<String, Vec<Queued>>,
    /// When each agent last reported, and to whom.
    pub(super) reported: HashMap<String, (String, u64)>,
    /// The agents whose chats show messages waiting for them.
    pub(super) shown: HashSet<String>,
    /// The agents whose turn at work would not take what waits for them, so it is not
    /// offered to that turn again.
    pub(super) unsteered: HashSet<String>,
    /// What each agent's turn at work was handed and is not yet known to have read, a batch
    /// for each message it went as.
    pub(super) offered: HashMap<String, Vec<Vec<Queued>>>,
    /// What each agent's last turn was handed, once that turn stopped. A stopped turn can say
    /// what it left unread only after it is seen to have stopped.
    pub(super) offered_last: HashMap<String, Vec<Vec<Queued>>>,
    /// Until when each worker's report of the turn it just ended may still be on its way.
    pub(super) reporting: HashMap<String, u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AutoTurns {
    pub(super) taken: u32,
    pub(super) stopped_at: Option<u64>,
}

/// An agent whose messages wait for the person, as it took [`MAX_AUTO_TURNS`] turns on its own.
pub(crate) struct Stopped {
    pub agent_id: String,
    pub turns: u32,
    pub since: u64,
}

impl State {
    /// Whether `from` and `to` have messaged each other, both ways, enough
    /// times since either last did other work to count as a loop.
    pub(super) fn looping(&self, from: &str, to: &str) -> bool {
        let since = [from, to]
            .iter()
            .filter_map(|agent| self.worked.get(*agent))
            .max()
            .copied()
            .unwrap_or(0);
        let exchanged: Vec<&Sent> = self
            .sent
            .iter()
            .filter(|sent| {
                sent.at > since
                    && sent.kind != MessageKind::Question
                    && ((sent.from == from && sent.to == to)
                        || (sent.from == to && sent.to == from))
            })
            .collect();
        exchanged.len() >= LOOP_MESSAGES && exchanged.iter().any(|sent| sent.from == to)
    }
}

impl State {
    /// Takes back the batch the agent's turn was handed as `text`, when it was one.
    pub(super) fn take_offered(&mut self, agent_id: &str, text: &str) -> Option<Vec<Queued>> {
        [&mut self.offered, &mut self.offered_last]
            .into_iter()
            .find_map(|offered| {
                let batches = offered.get_mut(agent_id)?;
                let at = batches.iter().position(|batch| joined(batch) == text)?;
                let batch = batches.remove(at);
                if batches.is_empty() {
                    offered.remove(agent_id);
                }
                Some(batch)
            })
    }

    /// What is kept: a paste not yet taken waits ahead of the rest; capped messages stay capped.
    pub(super) fn record(&self) -> MailboxRecord {
        let mut waiting = self.waiting.clone();
        for (agent_id, pasted) in &self.pasted {
            let queued = waiting.entry(agent_id.clone()).or_default();
            let mut texts = pasted.texts.clone();
            texts.append(queued);
            *queued = texts;
        }
        let auto_turns = self
            .auto_turns
            .iter()
            .filter(|(_, turns)| turns.taken > 0 || turns.stopped_at.is_some())
            .map(|(agent_id, turns)| (agent_id.clone(), turns.clone()))
            .collect();
        MailboxRecord {
            waiting,
            capped: self.capped.clone(),
            auto_turns,
        }
    }
}

pub(crate) struct Mailbox {
    pub(super) state: Mutex<State>,
    pub(super) kept: Kept,
    pub(crate) log: MessageLog,
    pub(crate) asks: crate::server::ask_aside::Asks,
    /// Hands waiting messages on, when an agent comes free.
    pub(crate) wake: Wake,
}

pub(super) enum Admitted {
    Yes,
    Repeat {
        seconds_ago: u64,
    },
    /// It is held, as one of a loop between two agents.
    Held,
}

impl Mailbox {
    /// Takes up the messages left waiting for agents `known` says still
    /// exist.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut record: MailboxRecord = kept.load();
        record.waiting.retain(|agent_id, _| known(agent_id));
        record.capped.retain(|agent_id, _| known(agent_id));
        record.auto_turns.retain(|agent_id, _| known(agent_id));
        for capped in record.capped.values_mut() {
            if capped.len() > MAX_WAITING {
                let drop_n = capped.len() - MAX_WAITING;
                capped.drain(..drop_n);
            }
        }
        Self {
            state: Mutex::new(State {
                waiting: record.waiting,
                capped: record.capped,
                auto_turns: record.auto_turns,
                ..State::default()
            }),
            kept,
            log: MessageLog::new(data_dir),
            asks: crate::server::ask_aside::Asks::new(data_dir),
            wake: Wake::default(),
        }
    }

    /// Changes the messages waiting, and keeps what they are now.
    pub(super) fn change<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.locked();
        let changed = change(&mut state);
        state.waiting.retain(|_, waiting| !waiting.is_empty());
        state.capped.retain(|_, capped| !capped.is_empty());
        self.kept.save_soon(&state.record());
        drop(state);
        self.wake.now();
        changed
    }

    pub(crate) fn flush(&self) {
        self.kept.flush();
    }

    pub(super) fn admit(
        &self,
        letter: &Letter,
        to: &str,
        to_name: &str,
    ) -> Result<Admitted, String> {
        let mut state = self.state.locked();
        let now = letter.at;
        state
            .sent
            .retain(|sent| now.saturating_sub(sent.at) < PAIR_WINDOW_MS);
        let digest = digest(&wrapped(letter));
        let pair: Vec<&Sent> = state
            .sent
            .iter()
            .filter(|sent| sent.from == letter.from_id && sent.to == to)
            .collect();
        let repeat = pair
            .iter()
            .rev()
            .find(|sent| sent.digest == digest && now.saturating_sub(sent.at) < REPEAT_WINDOW_MS);
        // Answers, reports and questions each stand for an event of their own,
        // however alike they read, and so do a board note, a trigger's firing
        // and Sikemux's word of what the person allowed.
        if let Some(repeat) = repeat.filter(|_| {
            letter.kind == MessageKind::Message && !letter.by_sikemux() && !letter.awaited
        }) {
            return Ok(Admitted::Repeat {
                seconds_ago: now.saturating_sub(repeat.at) / 1000,
            });
        }
        if letter.kind == MessageKind::Message && state.looping(&letter.from_id, to) {
            return Ok(Admitted::Held);
        }
        let report = letter.kind == MessageKind::Report;
        if pair.len() >= PAIR_LIMIT && !report {
            return Err(format!(
                "You have sent {to_name} {PAIR_LIMIT} messages in the last {} minutes, so Sikemux holds the rest; two agents must not keep each other busy without end. Wait, or ask the person.",
                PAIR_WINDOW_MS / 60_000
            ));
        }
        if !report && state.waiting.get(to).map_or(0, Vec::len) >= MAX_WAITING {
            return Err(format!(
                "{to_name} already has {MAX_WAITING} messages waiting for its turn to end; send more once it has read them"
            ));
        }
        Ok(Admitted::Yes)
    }

    pub(super) fn note_sent(&self, letter: &Letter, to: &str) {
        self.state.locked().sent.push_back(Sent {
            from: letter.from_id.clone(),
            to: to.to_owned(),
            kind: letter.kind,
            digest: digest(&wrapped(letter)),
            at: letter.at,
        });
    }

    /// Holds a message of a loop, keeping the newest few each pair sent.
    /// Answers those it no longer keeps.
    pub(super) fn hold(&self, from: &str, to: &str, queued: Queued) -> Vec<Queued> {
        let mut state = self.state.locked();
        let pair = |held: &Held| {
            (held.from == from && held.to == to) || (held.from == to && held.to == from)
        };
        let mut dropped = Vec::new();
        while state.held.iter().filter(|held| pair(held)).count() >= MAX_HELD {
            if let Some(oldest) = state.held.iter().position(&pair) {
                dropped.push(state.held.remove(oldest).queued);
            }
        }
        state.held.push(Held {
            from: from.to_owned(),
            to: to.to_owned(),
            queued,
            at: epoch_ms(),
        });
        dropped
    }

    pub(crate) fn loops(&self) -> Vec<Loop> {
        let state = self.state.locked();
        let mut loops: Vec<Loop> = Vec::new();
        for held in &state.held {
            let known = loops.iter().any(|known| {
                let (a, b) = &known.agents;
                (*a == held.from && *b == held.to) || (*a == held.to && *b == held.from)
            });
            if !known {
                loops.push(Loop {
                    agents: (held.from.clone(), held.to.clone()),
                    since: held.at,
                });
            }
        }
        loops
    }

    /// Whether the agent took [`MAX_AUTO_TURNS`] turns on its own, so only what someone waits on
    /// reaches it until the person writes to it.
    pub(super) fn stopped(&self, agent_id: &str) -> bool {
        let state = self.state.locked();
        let turns = state.auto_turns.get(agent_id);
        turns.is_some_and(|turns| turns.stopped_at.is_some())
    }

    /// How long the agent rests after a turn before what waits for it goes.
    pub(super) fn rest_for(&self, agent_id: &str) -> u64 {
        let state = self.state.locked();
        state.waiting.get(agent_id).map_or(SETTLE_MS, settle_ms)
    }

    /// Holds `queued` for the agent stopped at [`MAX_AUTO_TURNS`], until the person writes to it.
    pub(super) fn cap(&self, agent_id: &str, queued: Queued) -> usize {
        self.change(|state| {
            let capped = state.capped.entry(agent_id.to_owned()).or_default();
            while capped.len() >= MAX_WAITING {
                capped.remove(0);
            }
            capped.push(queued);
            capped.len()
        })
    }

    /// The agents stopped after [`MAX_AUTO_TURNS`] turns of their own that have messages held.
    pub(crate) fn stopped_agents(&self) -> Vec<Stopped> {
        let state = self.state.locked();
        (state.auto_turns.iter())
            .filter(|(agent_id, _)| state.capped.contains_key(*agent_id))
            .filter_map(|(agent_id, turns)| {
                Some(Stopped {
                    agent_id: agent_id.clone(),
                    turns: turns.taken,
                    since: turns.stopped_at?,
                })
            })
            .collect()
    }

    pub(crate) fn has_waiting(&self, agent_id: &str) -> bool {
        self.state.locked().waiting.contains_key(agent_id)
    }

    /// Whether a report from `worker` still waits for its lead to read. A calm one, read
    /// only with the lead's next turn, does not count.
    pub(crate) fn report_unread(&self, worker: &str) -> bool {
        let state = self.state.locked();
        let on_its_way = state.reporting.get(worker);
        if on_its_way.is_some_and(|until| *until > epoch_ms()) {
            return true;
        }
        let offered = state.offered.values().flatten().flatten();
        state
            .waiting
            .values()
            .flatten()
            .chain(offered)
            .any(|queued| {
                !queued.is_calm()
                    && queued.origin.as_ref().is_some_and(|origin| {
                        origin.kind == MessageKind::Report && origin.from == worker
                    })
            })
    }

    /// The worker's turn just ended, and its report may follow. Answers until when it is
    /// taken to be on its way.
    pub(crate) fn report_due(&self, worker: &str) -> u64 {
        let now = epoch_ms();
        let until = now.saturating_add(REPORT_DUE_MS);
        let mut state = self.state.locked();
        state.reporting.retain(|_, until| *until > now);
        state.reporting.insert(worker.to_owned(), until);
        until
    }

    /// The worker's report is being written, until [`Self::report_over`].
    pub(crate) fn report_begins(&self, worker: &str) {
        let mut state = self.state.locked();
        state.reporting.insert(worker.to_owned(), u64::MAX);
    }

    /// The worker's report waits for its lead, or none is coming.
    pub(crate) fn report_over(&self, worker: &str) {
        self.state.locked().reporting.remove(worker);
    }

    /// Leaves a report from `worker` waiting for `lead`, as one sent while the lead is mid-turn.
    #[cfg(test)]
    pub(crate) fn hold_report(&self, worker: &str, lead: &str, text: &str) {
        self.wait(
            lead,
            Queued {
                text: text.to_owned(),
                origin: Some(Origin {
                    from: worker.to_owned(),
                    kind: MessageKind::Report,
                    awaited: false,
                    calm: None,
                }),
                mark: None,
                paths: Vec::new(),
            },
        );
    }

    /// Whether something the person wrote waits for the agent.
    pub(crate) fn person_waiting(&self, agent_id: &str) -> bool {
        let state = self.state.locked();
        (state.waiting.get(agent_id))
            .is_some_and(|waiting| waiting.iter().any(|queued| by_person([queued])))
    }

    /// Whether something besides calm reports waits for the agent.
    pub(super) fn has_urgent_waiting(&self, agent_id: &str) -> bool {
        let state = self.state.locked();
        (state.waiting.get(agent_id)).is_some_and(|waiting| !waiting.iter().all(Queued::is_calm))
    }

    /// When the calm reports that are all that waits for the agent may start a turn: the soonest
    /// any of them is due.
    pub(super) fn calm_until(&self, agent_id: &str) -> Option<u64> {
        let state = self.state.locked();
        let waiting = state.waiting.get(agent_id)?;
        let first = (waiting.iter())
            .map(|queued| queued.origin.as_ref().and_then(|origin| origin.calm))
            .collect::<Option<Vec<CalmWait>>>()?
            .into_iter()
            .map(CalmWait::due_at)
            .min()?;
        Some(first)
    }

    pub(super) fn has_calm_waiting_on_siblings(&self, agent_id: &str) -> bool {
        let state = self.state.locked();
        state.waiting.get(agent_id).is_some_and(|waiting| {
            waiting.iter().any(|queued| {
                matches!(
                    queued.origin.as_ref().and_then(|origin| origin.calm),
                    Some(CalmWait::UntilSiblingsIdle { .. })
                )
            })
        })
    }

    pub(super) fn promote_calm_waiting(&self, agent_id: &str, now: u64) {
        self.change(|state| {
            let Some(waiting) = state.waiting.get_mut(agent_id) else {
                return;
            };
            for queued in waiting.iter_mut() {
                if let Some(origin) = queued.origin.as_mut() {
                    if matches!(origin.calm, Some(CalmWait::UntilSiblingsIdle { .. })) {
                        origin.calm = Some(CalmWait::At { at: now });
                    }
                }
            }
        });
    }

    /// Takes the calm reports waiting for the agent, leaving the rest.
    pub(super) fn take_calm(&self, agent_id: &str) -> Vec<Queued> {
        if !self.has_waiting(agent_id) {
            return Vec::new();
        }
        self.change(|state| {
            let Some(waiting) = state.waiting.get_mut(agent_id) else {
                return Vec::new();
            };
            let (calm, rest) = std::mem::take(waiting)
                .into_iter()
                .partition(Queued::is_calm);
            *waiting = rest;
            calm
        })
    }

    /// Takes what waits for the agent besides calm reports, which ride on its next turn, to
    /// hand to its turn at work. It counts as unread until that turn is over.
    pub(super) fn take_for_turn(&self, agent_id: &str) -> Vec<Queued> {
        if !self.has_urgent_waiting(agent_id) {
            return Vec::new();
        }
        self.change(|state| {
            let Some(waiting) = state.waiting.get_mut(agent_id) else {
                return Vec::new();
            };
            let (calm, urgent): (Vec<Queued>, Vec<Queued>) = std::mem::take(waiting)
                .into_iter()
                .partition(Queued::is_calm);
            *waiting = calm;
            let offered = state.offered.entry(agent_id.to_owned()).or_default();
            offered.push(urgent.clone());
            urgent
        })
    }

    pub(super) fn note_reported(&self, worker: &str, to: &str) {
        let now = epoch_ms();
        let mut state = self.state.locked();
        state
            .reported
            .retain(|_, (_, at)| now.saturating_sub(*at) < REPORTED_MS);
        state
            .reported
            .insert(worker.to_owned(), (to.to_owned(), now));
    }

    pub(super) fn missed_pastes(&self, agent_id: &str) -> u8 {
        self.state
            .locked()
            .missed_pastes
            .get(agent_id)
            .copied()
            .unwrap_or_default()
    }

    /// Answers how many wait now, and the earlier calm reports from the same
    /// worker this one replaces, so a lead holds only each worker's newest calm line.
    /// A report of how a turn ended is never replaced: each is that turn's only word
    /// to the lead.
    pub(super) fn wait(&self, agent_id: &str, queued: Queued) -> (usize, Vec<Queued>) {
        self.change(|state| {
            let waiting = state.waiting.entry(agent_id.to_owned()).or_default();
            let mut superseded = Vec::new();
            if let Some(Origin {
                from,
                kind: MessageKind::Report,
                ..
            }) = &queued.origin
            {
                let (older, kept) = std::mem::take(waiting).into_iter().partition(|waiting| {
                    waiting.is_calm()
                        && waiting.origin.as_ref().is_some_and(|origin| {
                            origin.kind == MessageKind::Report && &origin.from == from
                        })
                });
                *waiting = kept;
                superseded = older;
            }
            waiting.push(queued);
            (waiting.len(), superseded)
        })
    }

    pub(super) fn take(&self, agent_id: &str) -> Vec<Queued> {
        if !self.has_waiting(agent_id) {
            return Vec::new();
        }
        self.change(|state| state.waiting.remove(agent_id).unwrap_or_default())
    }

    /// Keeps `texts` waiting ahead of any that arrived meanwhile.
    pub(super) fn put_back(&self, agent_id: &str, texts: Vec<Queued>) {
        self.change(|state| put_back(state, agent_id, texts));
    }

    /// A message the agent was handed but dropped unread waits for its next
    /// turn, ahead of the rest, as what it was when it is known. The turn that dropped it is
    /// not offered it again.
    pub(crate) fn returned(&self, agent_id: &str, text: String) {
        self.change(|state| {
            state.unsteered.insert(agent_id.to_owned());
            let unread = state.take_offered(agent_id, &text).unwrap_or_else(|| {
                vec![Queued {
                    text,
                    origin: None,
                    mark: None,
                    paths: Vec::new(),
                }]
            });
            put_back(state, agent_id, unread);
        });
    }

    /// The task an agent in a terminal starts on waits for its CLI to take a prompt.
    pub(crate) fn task_waits(&self, agent_id: &str, text: String) {
        self.put_back(
            agent_id,
            vec![Queued {
                text,
                origin: None,
                mark: Some(TASK.to_owned()),
                paths: Vec::new(),
            }],
        );
    }
}

pub(super) fn put_back(state: &mut State, agent_id: &str, mut texts: Vec<Queued>) {
    let waiting = state.waiting.entry(agent_id.to_owned()).or_default();
    texts.append(waiting);
    *waiting = texts;
}
