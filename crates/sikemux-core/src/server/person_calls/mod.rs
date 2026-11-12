//! Calls: every decision put to the person, whatever raised it. A question an agent's `ask`
//! takes to the person is one, and so is an action held at a production place. Each is asked
//! once and waits until the person answers it; nothing times it out. Every client draws the same
//! card from the device view and answers it with `answerCall`, and the answer reaches whatever
//! raised the call. A production call passes only on a client that drew it, by its button.

use std::sync::{Arc, Mutex};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    AnswerCallParams, CallAnswer, CallAsker, CallBrief, CallClient, CallEvidence, CallKind,
    CallOption, CallOutcome, CallParams, CallResult, CallState, PersonCall,
};

use super::access::Peer;
use super::changed::What;
use super::questions::AnswerParams;
use super::{messages, questions, ship, Core, CoreResult};

/// How long an answered or closed call stays in the device view.
const SHOWN_AFTER_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_SHOWN_CLOSED: usize = 64;

/// What raised a call, which its answer goes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// A question an agent put to the person, by the question's id.
    Question(String),
    /// An action held for the person, by the hold's id.
    Hold(String),
    /// Raised by [`raise`], which hears the answer through its `key`.
    Raised,
}

struct Kept {
    call: PersonCall,
    source: Source,
    /// Calls raised with the same key are joined into one.
    key: Option<String>,
    /// The clients that drew it, and when each first did.
    seen: Vec<(CallClient, u64)>,
}

#[derive(Default)]
pub(crate) struct PersonCalls {
    calls: Mutex<Vec<Kept>>,
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
    pub place: Option<String>,
    /// What the call is about, such as one piece of work at one place. Agents that raise a call
    /// with the key of one still open join it.
    pub key: Option<String>,
}

impl Raise {
    fn into_call(self, id: String) -> (PersonCall, Option<String>) {
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
        (call, self.key)
    }
}

/// The open call an agent waits on longest, with how many it waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "the turn state shows an agent's top call through it"
)]
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

impl PersonCalls {
    fn changed(&self, core: &Core) {
        core.changed(What::View);
    }

    /// Every call the device view shows: those still open, oldest first, then those answered or
    /// closed lately, newest first.
    pub(crate) fn shown(&self) -> Vec<PersonCall> {
        let now = epoch_ms();
        let calls = self.calls.locked();
        let open = (calls.iter())
            .filter(|kept| kept.call.state == CallState::Open)
            .map(|kept| kept.call.clone());
        let mut closed: Vec<PersonCall> = (calls.iter())
            .filter(|kept| kept.call.state != CallState::Open)
            .filter(|kept| now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS)
            .map(|kept| kept.call.clone())
            .collect();
        closed.sort_by_key(|call| std::cmp::Reverse(settled_at(call)));
        closed.truncate(MAX_SHOWN_CLOSED);
        open.chain(closed).collect()
    }

    /// The open call `agent_id` has waited on longest, and how many it waits on.
    #[expect(
        dead_code,
        reason = "the turn state shows an agent's top call through it"
    )]
    pub(crate) fn of_agent(&self, agent_id: &str) -> Option<AgentCall> {
        let calls = self.calls.locked();
        let open: Vec<&PersonCall> = (calls.iter())
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
        (self.calls.locked().iter())
            .map(|kept| &kept.call)
            .find(|call| call.project == project && matches(call))
            .cloned()
    }

    fn insert(&self, call: PersonCall, source: Source, key: Option<String>) {
        let mut calls = self.calls.locked();
        let now = epoch_ms();
        calls.retain(|kept| {
            kept.call.state == CallState::Open
                || now.saturating_sub(settled_at(&kept.call)) < SHOWN_AFTER_MS
        });
        calls.push(Kept {
            call,
            source,
            key,
            seen: Vec::new(),
        });
    }

    /// Puts the calls `source_of` names among `wanted` to the person, and closes the open calls
    /// it names that are no longer wanted. A call already open keeps what it was raised with.
    fn sync(&self, core: &Core, owns: impl Fn(&Source) -> bool, wanted: Vec<(Source, PersonCall)>) {
        let now = epoch_ms();
        let mut moved = false;
        {
            let mut calls = self.calls.locked();
            for kept in calls.iter_mut() {
                let still = wanted.iter().any(|(source, _)| *source == kept.source);
                if kept.call.state == CallState::Open && owns(&kept.source) && !still {
                    kept.call.state = CallState::Closed;
                    kept.call.outcome = Some(CallOutcome {
                        result: CallResult::Withdrawn,
                        healthy: None,
                        roll_back: false,
                        at: now,
                    });
                    moved = true;
                }
            }
            drop(calls);
            for (source, call) in wanted {
                let mut calls = self.calls.locked();
                match calls.iter_mut().find(|kept| kept.source == source) {
                    Some(kept) if kept.call.state == CallState::Closed => {
                        kept.call.state = CallState::Open;
                        kept.call.outcome = None;
                        moved = true;
                    }
                    Some(_) => {}
                    None => {
                        drop(calls);
                        self.insert(call, source, None);
                        moved = true;
                    }
                }
            }
        }
        if moved {
            self.changed(core);
        }
    }

    fn seen_by(&self, id: &str, client: &CallClient) -> bool {
        (self.calls.locked().iter())
            .find(|kept| kept.call.id == id)
            .is_some_and(|kept| kept.seen.iter().any(|(seer, _)| seer == client))
    }

    fn source_of(&self, id: &str) -> Option<Source> {
        (self.calls.locked().iter())
            .find(|kept| kept.call.id == id)
            .map(|kept| kept.source.clone())
    }

    fn id_of(&self, source: &Source) -> Option<String> {
        (self.calls.locked().iter())
            .find(|kept| kept.source == *source && kept.call.state == CallState::Open)
            .map(|kept| kept.call.id.clone())
    }

    /// Takes the person's answer to the open call `id`, which waits for what came of it.
    fn take_answer(&self, id: &str, answer: CallAnswer) -> CoreResult<(PersonCall, Source)> {
        let mut calls = self.calls.locked();
        let kept = (calls.iter_mut())
            .find(|kept| kept.call.id == id && kept.call.state == CallState::Open)
            .ok_or(GONE)?;
        kept.call.state = CallState::Answered;
        kept.call.answer = Some(answer);
        Ok((kept.call.clone(), kept.source.clone()))
    }

    /// The answer could not reach what raised the call, so it waits on the person again.
    fn reopen(&self, id: &str) {
        let mut calls = self.calls.locked();
        if let Some(kept) = calls.iter_mut().find(|kept| kept.call.id == id) {
            kept.call.state = CallState::Open;
            kept.call.answer = None;
        }
    }

    fn settle(&self, id: &str, result: CallResult) {
        let mut calls = self.calls.locked();
        if let Some(kept) = calls.iter_mut().find(|kept| kept.call.id == id) {
            kept.call.outcome = Some(CallOutcome {
                result,
                healthy: None,
                roll_back: false,
                at: epoch_ms(),
            });
        }
    }
}

fn settled_at(call: &PersonCall) -> u64 {
    (call.outcome.as_ref().map(|outcome| outcome.at))
        .or(call.answer.as_ref().map(|answer| answer.at))
        .unwrap_or(call.raised_at)
}

/// Puts a call to the person, or joins `raise.asker` to the open call of the project with the
/// same key. Answers the call's id.
#[expect(
    dead_code,
    reason = "the gates by place raise production calls through it"
)]
pub(crate) fn raise(core: &Core, raise: Raise) -> String {
    let joined = {
        let mut calls = core.person_calls.calls.locked();
        let same = (calls.iter_mut()).find(|kept| {
            kept.call.state == CallState::Open
                && kept.call.project == raise.project
                && kept.key.is_some()
                && kept.key == raise.key
        });
        same.map(|kept| {
            if !waits_on(&kept.call, &raise.asker.agent_id) {
                kept.call.joined.push(raise.asker.clone());
            }
            kept.call.id.clone()
        })
    };
    let id = joined.unwrap_or_else(|| {
        let (call, key) = raise.into_call(uuid::Uuid::new_v4().simple().to_string());
        let id = call.id.clone();
        core.person_calls.insert(call, Source::Raised, key);
        id
    });
    core.person_calls.changed(core);
    id
}

/// Says what came of an answered call, such as that it shipped and its place stayed healthy.
#[expect(
    dead_code,
    reason = "the gates by place raise production calls through it"
)]
pub(crate) fn set_outcome(core: &Core, id: &str, outcome: CallOutcome) {
    let mut calls = core.person_calls.calls.locked();
    if let Some(kept) = calls.iter_mut().find(|kept| kept.call.id == id) {
        kept.call.outcome = Some(outcome);
        drop(calls);
        core.person_calls.changed(core);
    }
}

/// The questions now put to the person, as calls.
pub(crate) fn sync_questions(core: &Core, wanted: Vec<(String, PersonCall)>) {
    let wanted = (wanted.into_iter())
        .map(|(id, call)| (Source::Question(id), call))
        .collect();
    (core.person_calls).sync(core, |source| matches!(source, Source::Question(_)), wanted);
}

/// The actions now held for the person, as calls.
pub(crate) fn sync_holds(core: &Core, wanted: Vec<(String, PersonCall)>) {
    let wanted = (wanted.into_iter())
        .map(|(id, call)| (Source::Hold(id), call))
        .collect();
    (core.person_calls).sync(core, |source| matches!(source, Source::Hold(_)), wanted);
}

/// A client drew the call.
pub(crate) fn seen(core: &Core, peer: &Peer, params: CallParams) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let mut calls = core.person_calls.calls.locked();
    let kept = (calls.iter_mut())
        .find(|kept| kept.call.id == params.id)
        .ok_or(GONE)?;
    if !kept.seen.iter().any(|(seer, _)| *seer == client) {
        kept.seen.push((client, epoch_ms()));
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
    let answer = CallAnswer {
        option: params.option.clone(),
        options: params.options.clone(),
        note: note.filter(|_| !call.secret),
        words: words.filter(|_| !call.secret),
        by: client,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    Ok((answer, typed))
}

fn call_of(core: &Core, id: &str) -> CoreResult<PersonCall> {
    Ok((core.person_calls.calls.locked().iter())
        .find(|kept| kept.call.id == id && kept.call.state == CallState::Open)
        .map(|kept| kept.call.clone())
        .ok_or(GONE)?)
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
        Source::Question(id) => {
            let index = |option: &String| call.options.iter().position(|known| known.id == *option);
            let params = AnswerParams {
                id: id.clone(),
                answer: typed,
                choice: answer.option.as_ref().and_then(index),
                choices: (!answer.options.is_empty())
                    .then(|| answer.options.iter().filter_map(index).collect()),
                ..AnswerParams::default()
            };
            questions::answer(core, params).await?;
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
        Source::Raised => {
            let label = (answer.option.as_ref())
                .and_then(|id| call.options.iter().find(|option| option.id == *id))
                .map(|option| option.label.clone());
            let said = [label, typed]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n\n");
            let text = format!("The person answered \"{}\":\n{said}", call.question);
            for asker in std::iter::once(&call.asker).chain(&call.joined) {
                let _ = messages::for_person(core, asker.agent_id.clone(), text.clone()).await;
            }
        }
    }
    Ok(())
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
        return Ok(None);
    };
    let call = call_of(core, &id)?;
    let given = answer(&call, client);
    if call.kind == CallKind::Production {
        may_pass(core, &call, peer, given.option.as_deref())?;
    }
    core.person_calls.take_answer(&id, given)?;
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
    let mut taken = Vec::new();
    for params in &given.answers {
        let taken_now = answered_before(
            core,
            peer,
            &Source::Question(params.id.clone()),
            |call, by| {
                let option = |index: usize| call.options.get(index).map(|option| option.id.clone());
                let typed =
                    (params.answer.clone()).filter(|text| !text.trim().is_empty() && !call.secret);
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
        Source::Question(id) => questions::hand_back(core, id).await,
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

/// Every call the device view shows.
pub(crate) fn shown(core: &Core) -> Vec<PersonCall> {
    core.person_calls.shown()
}
