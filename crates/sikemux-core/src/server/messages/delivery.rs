use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_calls::messages::MessageKind;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::format::{joined, read_as, Letter, Sender};
use super::log::MessageOutcome;
use super::mailbox::{
    awaited, by_person, capped, in_charge, put_back, settle_ms, Admitted, Held, Mailbox, Origin,
    Pasted, Queued, State,
};
use super::{
    at_name, carry, log, ENTER_AFTER_PASTE, MAX_AUTO_TURNS, MAX_MISSED_PASTES, MESSAGING_TOOLS,
    PAIR_WINDOW_MS, TURN_OUTPUT_MS, WAKE_RETRY_MS,
};
use crate::server::changed::{ChangedCore, What};
use crate::server::chat::{self, Delivery};
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::roster::{Listed, Reach};
use crate::server::serving::{ServeCore, ServeParts};
use crate::server::session::{self, Session};
use crate::server::wake::sooner;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{agent, agent_stop, finished, hosts, launching, Core, CoreResult};

pub(super) async fn deliver(
    core: &Arc<Core>,
    target: &Listed,
    mut letter: Letter,
) -> Result<Value, String> {
    let text = read_as(core, &target.agent_id, &mut letter);
    Box::pin(deliver_as(core, target, letter, text)).await
}

/// Delivers `text` in place of `letter`'s own, such as several answers read
/// as one message.
pub(super) async fn deliver_as(
    core: &Arc<Core>,
    target: &Listed,
    mut letter: Letter,
    text: String,
) -> Result<Value, String> {
    if target.reach == Reach::Remote {
        return carry(core, target, letter, None).await;
    }
    letter.awaited |= agent_stop::may_start(core, &letter.from_id, &target.agent_id);
    let queued = Queued {
        text,
        origin: Some(Origin {
            from: letter.from_id.clone(),
            kind: letter.kind,
            awaited: letter.awaited,
            calm: letter.calm,
        }),
        mark: letter.puts.clone(),
        paths: letter.paths.clone(),
    };
    let handed = handed(core, target, letter, &queued).await;
    let outcome = handed
        .as_ref()
        .map_or(MessageOutcome::Refused, |(_, outcome)| *outcome);
    log(core, &target.agent_id, [&queued], outcome);
    handed.map(|(answer, _)| answer)
}

/// Hands `letter` to its agent, or leaves it waiting for the agent, and says
/// what became of it.
async fn handed(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    queued: &Queued,
) -> Result<(Value, MessageOutcome), String> {
    let name = target
        .name
        .as_deref()
        .map(at_name)
        .unwrap_or_else(|| target.agent_id.clone());
    let lead_hint = if letter.kind == MessageKind::Message
        && core.names().reports_to(&letter.from_id).as_deref() == Some(target.agent_id.as_str())
    {
        " For a final report, end your turn with it instead of calling agent_send."
    } else {
        ""
    };
    let answer = |delivery: &str, note: String| json!({ "to": name, "agentId": target.agent_id, "delivery": delivery, "note": note });
    let mailbox = &core.harness().mailbox;
    match mailbox.admit(&letter, &target.agent_id, &name)? {
        Admitted::Yes => {}
        Admitted::Repeat { seconds_ago } => {
            return Ok((
                answer(
                    "repeat",
                    format!(
                    "You sent {name} the same text {seconds_ago} s ago, so it was not sent again."
                ),
                ),
                MessageOutcome::Repeat,
            ))
        }
        Admitted::Held => {
            let dropped = mailbox.hold(&letter.from_id, &target.agent_id, queued.clone());
            log(core, &target.agent_id, &dropped, MessageOutcome::Dropped);
            return Ok((
                answer(
                    "held",
                    format!("You and {name} keep messaging each other without either doing other work, so Sikemux holds this and has asked the person. It goes to {name} once one of you does other work or the person writes to one of you; carry on with your own work."),
                ),
                MessageOutcome::Waiting,
            ));
        }
    }
    if Sender::of(&letter.from_id) == Some(Sender::Person) {
        person_wrote(core, &target.agent_id);
    } else if agent_stop::may_start(core, &letter.from_id, &target.agent_id) {
        core.inbox().may_start(&target.agent_id);
    }
    if in_charge(core, &letter.from_id, &target.agent_id) {
        let_go(core, &target.agent_id);
    }
    if letter.kind == MessageKind::Report {
        mailbox.note_reported(&letter.from_id, &target.agent_id);
    }
    let stopped = capped(core, &target.agent_id, queued) && mailbox.stopped(&target.agent_id);
    let mut woke = "";
    let not_woken;
    let mut held = None;
    let mut reach = target.reach;
    let stopped_by = core.inbox().stopped_by(&target.agent_id);
    let notice = letter.kind == MessageKind::Notice && stopped_by.is_none();
    let halted = stopped_by.map(|by| {
        let by = core.names().name_or_id(&by);
        format!("was stopped by @{by} and starts again only for it, a lead above it or the person")
    });
    match target.reach {
        Reach::Chat | Reach::Terminal | Reach::Asleep if halted.is_some() => {
            held = halted.as_deref()
        }
        Reach::Chat | Reach::Terminal | Reach::Asleep if stopped => held = Some(STOPPED_NOTE),
        Reach::Chat | Reach::Terminal | Reach::Asleep if letter.calm.is_some() => {
            held = Some("reads calm reports with its next turn")
        }
        Reach::Chat | Reach::Terminal => {}
        Reach::Asleep if notice => held = Some("is asleep"),
        Reach::Asleep
            if !core.window().is_open() && core.hosts().team.get(&letter.from_id).is_none() =>
        {
            held = Some("is asleep while Sikemux is closed")
        }
        Reach::Asleep if core.workspaces().held(&target.agent_id) && !lifts_hold(core, &letter) => {
            held = Some("was put to sleep when the person stopped everything")
        }
        Reach::Asleep => {
            let woken = match launching::wake(core, target.agent_id.clone()).await {
                Ok(()) => Ok(()),
                Err(_) => launching::wake(core, target.agent_id.clone()).await,
            };
            match woken {
                Ok(()) => {
                    mailbox.state.locked().wake_failed.remove(&target.agent_id);
                    woke = " Sikemux woke it first.";
                    if core.agent_session(&target.agent_id).is_some() {
                        reach = Reach::Terminal;
                    }
                }
                Err(error) => {
                    mailbox
                        .state
                        .locked()
                        .wake_failed
                        .insert(target.agent_id.clone(), epoch_ms());
                    not_woken = format!("is asleep and did not wake twice ({error}); Sikemux tries again within a minute");
                    held = Some(not_woken.as_str());
                }
            }
        }
        Reach::Stopped
            if letter.answers.is_some() || letter.kind == MessageKind::Report || letter.waits =>
        {
            held = Some("has stopped")
        }
        Reach::Stopped => {
            return Err(format!(
                "{name} has stopped; it gets messages again once the person opens it"
            ))
        }
        Reach::Remote => return Err(format!("{name} runs on another host")),
    }
    if !notice {
        mailbox.note_sent(&letter, &target.agent_id);
    }
    struct DeliveringGuard<'a> {
        mailbox: &'a Mailbox,
        agent_id: String,
    }
    impl Drop for DeliveringGuard<'_> {
        fn drop(&mut self) {
            self.mailbox
                .state
                .locked()
                .delivering
                .remove(&self.agent_id);
            self.mailbox.wake.now();
        }
    }

    let mut delivering_guard = None;
    if held.is_none() {
        let mut state = mailbox.state.locked();
        if state
            .waiting
            .get(&target.agent_id)
            .is_some_and(|waiting| !waiting.iter().all(Queued::is_calm))
        {
            held = Some("already has messages waiting");
        } else if reach == Reach::Terminal {
            if state.delivering.contains(&target.agent_id) {
                held = Some("is taking another message");
            }
        } else if state.delivering.insert(target.agent_id.clone()) {
            state
                .offered
                .entry(target.agent_id.clone())
                .or_default()
                .push(vec![queued.clone()]);
            delivering_guard = Some(DeliveringGuard {
                mailbox,
                agent_id: target.agent_id.clone(),
            });
        } else {
            held = Some("is taking another message");
        }
    }
    if held.is_none() {
        let delivered = match reach {
            Reach::Terminal => {
                ready_terminal(core, &target.agent_id, settle_ms([queued])).map(|terminal| {
                    paste(core, &target.agent_id, terminal, vec![queued.clone()]);
                    MessageOutcome::Typed
                })
            }
            _ => {
                let one = std::slice::from_ref(queued);
                let handed = hand(core, &target.agent_id, one).await;
                if !matches!(handed, Ok(Delivery::Steered)) {
                    let mut state = mailbox.state.locked();
                    state.take_offered(&target.agent_id, &joined(one));
                }
                drop(delivering_guard);
                handed.map(|delivered| match delivered {
                    Delivery::Steered => MessageOutcome::Steered,
                    _ => MessageOutcome::NewTurn,
                })
            }
        };
        match delivered {
            Ok(MessageOutcome::Steered) => {
                return Ok((
                    answer(
                        "steered",
                        format!(
                            "{name} is working and reads this at its next step.{woke}{lead_hint}"
                        ),
                    ),
                    MessageOutcome::Steered,
                ))
            }
            Ok(outcome) => {
                return Ok((
                    answer(
                        "newTurn",
                        format!(
                            "{name} was idle and started a turn to read this.{woke}{lead_hint}"
                        ),
                    ),
                    outcome,
                ))
            }
            Err(busy) => held = Some(busy),
        }
    }
    if notice {
        let busy = held.unwrap_or_default();
        return Ok((
            answer(
                "dropped",
                format!(
                    "{name} {busy}, and a notice is only worth reading at once, so it was dropped."
                ),
            ),
            MessageOutcome::Dropped,
        ));
    }
    let (waiting, superseded) = match stopped {
        true => (mailbox.cap(&target.agent_id, queued.clone()), Vec::new()),
        false => mailbox.wait(&target.agent_id, queued.clone()),
    };
    log(core, &target.agent_id, &superseded, MessageOutcome::Dropped);
    if stopped {
        core.changed(What::View);
    }
    let busy = held.unwrap_or_default();
    Ok((
        answer(
            "waiting",
            format!("{name} {busy}, so it gets this once it is free, with any others waiting ({waiting} now).{woke}{lead_hint}"),
        ),
        MessageOutcome::Waiting,
    ))
}

/// Whether this letter may wake an agent held since the person stopped everything: the person
/// wrote it, or an agent they have set going again did. Mail left from the stop never may.
fn lifts_hold(core: &Core, letter: &Letter) -> bool {
    match Sender::of(&letter.from_id) {
        Some(sender) => sender == Sender::Person,
        None => {
            core.workspaces().agent(&letter.from_id).is_some()
                && !core.workspaces().held(&letter.from_id)
        }
    }
}

/// When the agent will have rested `settle_ms` since its last turn, unless it has.
fn rests_until(core: &Core, agent_id: &str, settle_ms: u64) -> Option<u64> {
    core.seen()
        .rested_at(agent_id)
        .map(|at| at + settle_ms)
        .filter(|until| *until > epoch_ms())
}

fn rested(core: &Core, agent_id: &str, settle_ms: u64) -> bool {
    rests_until(core, agent_id, settle_ms).is_none()
}

/// Hands `queued` to the chat as one message, or says why it cannot take it
/// now.
async fn hand(core: &Core, agent_id: &str, queued: &[Queued]) -> Result<Delivery, &'static str> {
    let chat = core.chats().get(agent_id).ok_or("is not running")?;
    if !chat.is_ready() {
        chat.start_agent();
        return Err("is still starting");
    }
    if !chat.turn_running() && !rested(core, agent_id, settle_ms(queued)) {
        return Err("has just finished a turn");
    }
    let paths: Vec<String> = queued
        .iter()
        .flat_map(|queued| queued.paths.clone())
        .collect();
    match chat::letter(core, agent_id, joined(queued), paths, by_person(queued)).await {
        Ok(Delivery::NewTurn) => {
            took_turn(core, agent_id, queued);
            Ok(Delivery::NewTurn)
        }
        Ok(Delivery::Steered) => Ok(Delivery::Steered),
        Ok(Delivery::TurnEnded) => Err("has just finished a turn"),
        Ok(Delivery::Refused) | Err(_) => Err("is working and cannot read messages mid-turn"),
    }
}

/// The terminal of an agent that waits for a prompt, or why it cannot take
/// one now.
fn ready_terminal(
    core: &Core,
    agent_id: &str,
    settle_ms: u64,
) -> Result<Arc<Session>, &'static str> {
    let terminal = core
        .agent_session(agent_id)
        .ok_or("no longer has a terminal")?;
    if !agent::takes_prompt(core, &terminal) {
        let idle = terminal
            .agent
            .as_ref()
            .is_some_and(agent::AgentActivity::idle);
        return Err(if idle {
            "is still starting in its terminal"
        } else {
            "is working in its terminal"
        });
    }
    if !rested(core, agent_id, settle_ms) {
        return Err("has just finished a turn");
    }
    Ok(terminal)
}

/// Types `texts` into the agent's terminal as one pasted prompt. They count
/// as delivered only once the agent is seen to take them, and wait again if
/// it does not; Enter is held back from a permission prompt that came up
/// meanwhile.
fn paste(core: &Arc<Core>, agent_id: &str, terminal: Arc<Session>, texts: Vec<Queued>) {
    let typed = session::pasted(&terminal, &joined(&texts));
    took_turn(core, agent_id, &texts);
    core.harness().mailbox.change(|state| {
        state.delivering.insert(agent_id.to_owned());
        state.pasted.insert(
            agent_id.to_owned(),
            Pasted {
                texts,
                entered_at: None,
            },
        );
    });
    agent::note_input(core, &terminal, b"\r");
    session::queue_input(&terminal, typed, session::Typist::Other, Box::new(drop));
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(ENTER_AFTER_PASTE).await;
        if agent::asks_now(&core, &terminal) {
            unpasted(&core, &agent_id);
            return;
        }
        session::queue_input(
            &terminal,
            b"\r".to_vec(),
            session::Typist::Other,
            Box::new(drop),
        );
        if let Some(pasted) = core
            .harness()
            .mailbox
            .state
            .locked()
            .pasted
            .get_mut(&agent_id)
        {
            pasted.entered_at = Some(epoch_ms());
        }
    });
}

/// The paste the agent did not take waits again, ahead of the rest.
fn unpasted(core: &Core, agent_id: &str) {
    let returned = core.harness().mailbox.change(|state| {
        state.delivering.remove(agent_id);
        let pasted = state.pasted.remove(agent_id)?;
        put_back(state, agent_id, pasted.texts.clone());
        *state.missed_pastes.entry(agent_id.to_owned()).or_default() += 1;
        Some(pasted.texts)
    });
    log(
        core,
        agent_id,
        returned.iter().flatten(),
        MessageOutcome::Returned,
    );
}

/// Types what the person wrote to a terminal agent into its terminal once its CLI takes a
/// prompt, waking it on its saved session first when its terminal has ended.
pub(crate) fn from_person(core: &Arc<Core>, agent_id: &str, text: String) -> CoreResult<()> {
    let text = session::typable(&text).ok_or("there is nothing to send")?;
    worked(core, agent_id);
    person_wrote(core, agent_id);
    crate::server::now::asks::asked(core, agent_id, &text, &[], None);
    let text = match core.harness().questions.open_calls_note(agent_id) {
        Some(note) => format!("{text}\n\n{note}"),
        None => text,
    };
    let mailbox = &core.harness().mailbox;
    mailbox.wait(
        agent_id,
        Queued {
            text,
            origin: None,
            mark: None,
            paths: Vec::new(),
        },
    );
    mailbox.wake.now();
    Ok(())
}

const STOPPED_NOTE: &str =
    "has taken turn after turn on triggers and other agents' messages, and waits for the person, its lead or its chief to write to it";

/// The agent started a turn on `queued`. One nobody waits on counts toward [`MAX_AUTO_TURNS`].
fn took_turn(core: &Core, agent_id: &str, queued: &[Queued]) {
    if awaited(queued) || !queued.iter().any(|queued| capped(core, agent_id, queued)) {
        return;
    }
    let stopped = {
        let mut state = core.harness().mailbox.state.locked();
        let turns = state.auto_turns.entry(agent_id.to_owned()).or_default();
        turns.taken += 1;
        let stops = turns.taken >= MAX_AUTO_TURNS && turns.stopped_at.is_none();
        if stops {
            turns.stopped_at = Some(epoch_ms());
        }
        stops
    };
    if stopped {
        core.changed(What::View);
    }
}

/// The person wrote to the agent, so it may take turns of its own again, and what waited for
/// them goes on.
pub(crate) fn person_wrote(core: &Core, agent_id: &str) {
    core.seen().person_wrote(agent_id);
    core.inbox().may_start(agent_id);
    core.harness().questions.steered(agent_id);
    let_go(core, agent_id);
}

/// Someone wrote to the agent, so it may take turns of its own again, and what waited goes on.
fn let_go(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let turns = mailbox.state.locked().auto_turns.remove(agent_id);
    if turns.is_some_and(|turns| turns.stopped_at.is_some()) {
        mailbox.change(|state| {
            let capped = state.capped.remove(agent_id).unwrap_or_default();
            state
                .waiting
                .entry(agent_id.to_owned())
                .or_default()
                .extend(capped);
        });
        core.changed(What::View);
    }
}

/// The agent did something besides messaging other agents, which ends any
/// loop it was in: the messages held between it and another agent go on.
pub(crate) fn worked(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let now = epoch_ms();
    let looped = {
        let mut state = mailbox.state.locked();
        state
            .worked
            .retain(|_, at| now.saturating_sub(*at) < PAIR_WINDOW_MS);
        state.worked.insert(agent_id.to_owned(), now);
        state
            .held
            .iter()
            .any(|held| held.from == agent_id || held.to == agent_id)
    };
    if !looped {
        return;
    }
    mailbox.change(|state| {
        let (released, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut state.held)
            .into_iter()
            .partition(|held| held.from == agent_id || held.to == agent_id);
        state.held = kept;
        for held in released {
            state.waiting.entry(held.to).or_default().push(held.queued);
        }
    });
}

/// The agent called a tool, by the name its CLI gives it.
pub(crate) fn used_tool(core: &Core, agent_id: &str, tool: &str) {
    core.seen().used_tool(agent_id);
    core.stalls().worked(core, agent_id);
    if !MESSAGING_TOOLS
        .iter()
        .any(|messaging| tool.ends_with(messaging))
    {
        worked(core, agent_id);
        hosts::route::worked(core, agent_id);
    }
}

/// The terminal agent was seen at work on a turn or asking the person
/// during one, so it took what was pasted, if anything was.
pub(crate) fn prompt_taken(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let taken = {
        let mut state = mailbox.state.locked();
        state.missed_pastes.remove(agent_id);
        let entered = state
            .pasted
            .get(agent_id)
            .is_some_and(|pasted| pasted.entered_at.is_some());
        if !entered {
            return;
        }
        state.delivering.remove(agent_id);
        let taken = state.pasted.remove(agent_id);
        mailbox.kept.save_soon(&state.record());
        taken
    };
    mailbox.wake.now();
    log(
        core,
        agent_id,
        taken.iter().flat_map(|pasted| &pasted.texts),
        MessageOutcome::Delivered,
    );
}

/// The terminal agent came to rest, or stopped, after a turn. When the turn
/// followed a paste, it took the paste if it went on printing a while after
/// Enter, as a turn does; a swallowed Enter only redraws the prompt box. A
/// turn of its own makes it worth typing into again.
pub(crate) fn prompt_settled(core: &Core, agent_id: &str, last_output_ms: u64) {
    let entered_at = {
        let mut state = core.harness().mailbox.state.locked();
        match state.pasted.get(agent_id) {
            Some(pasted) => pasted.entered_at,
            None => {
                state.missed_pastes.remove(agent_id);
                return;
            }
        }
    };
    match entered_at {
        Some(at) if last_output_ms >= at + TURN_OUTPUT_MS => prompt_taken(core, agent_id),
        Some(_) => unpasted(core, agent_id),
        None => {}
    }
}

/// Gives each agent that has rested since its turn ended the messages
/// waiting for it, wakes an asleep chat that has some, and drops those for
/// agents that are gone. Runs when an agent's state or its messages change,
/// and again when an agent still resting from a turn is due.
pub(crate) fn settle(core: &Arc<Core>) {
    let mailbox = &core.harness().mailbox;
    show(core);
    let capped: Vec<String> = mailbox.state.locked().capped.keys().cloned().collect();
    for agent_id in capped
        .into_iter()
        .filter(|agent_id| core.workspaces().closed(agent_id))
    {
        let dropped = mailbox.change(|state| {
            state.auto_turns.remove(&agent_id);
            state.capped.remove(&agent_id).unwrap_or_default()
        });
        log(core, &agent_id, &dropped, MessageOutcome::Dropped);
    }
    let pasted: Vec<String> = mailbox.state.locked().pasted.keys().cloned().collect();
    for agent_id in pasted {
        if core.agent_session(&agent_id).is_none() {
            unpasted(core, &agent_id);
        }
    }
    let due: Vec<String> = {
        let mut state = mailbox.state.locked();
        let State {
            waiting,
            wake_failed,
            missed_pastes,
            delivering,
            unsteered,
            ..
        } = &mut *state;
        unsteered.retain(|agent_id| waiting.contains_key(agent_id));
        wake_failed.retain(|agent_id, _| waiting.contains_key(agent_id));
        missed_pastes.retain(|agent_id, _| waiting.contains_key(agent_id));
        waiting
            .keys()
            .filter(|agent_id| !delivering.contains(*agent_id))
            .cloned()
            .collect()
    };
    let last: Vec<String> = mailbox
        .state
        .locked()
        .offered_last
        .keys()
        .cloned()
        .collect();
    for agent_id in last
        .into_iter()
        .filter(|agent_id| core.workspaces().closed(agent_id))
    {
        mailbox.state.locked().offered_last.remove(&agent_id);
    }
    let offered: Vec<String> = {
        let state = mailbox.state.locked();
        (state.offered.keys())
            .filter(|agent_id| !state.delivering.contains(*agent_id))
            .cloned()
            .collect()
    };
    for agent_id in offered {
        let at_work = (core.chats().get(&agent_id)).is_some_and(|chat| chat.turn_running());
        if !at_work {
            let mut state = mailbox.state.locked();
            if let Some(batches) = state.offered.remove(&agent_id) {
                state.offered_last.insert(agent_id, batches);
            }
            drop(state);
            finished::moved(core);
        }
    }
    let mut next = None;
    for agent_id in due {
        if core.inbox().stopped_by(&agent_id).is_some() && !core.workspaces().closed(&agent_id) {
            continue;
        }
        if let Some(until) = mailbox
            .calm_until(&agent_id)
            .filter(|until| *until > epoch_ms())
        {
            next = sooner(next, Some(until));
            continue;
        }
        let rest = mailbox.rest_for(&agent_id);
        if let Some(chat) = core.chats().get(&agent_id) {
            if !chat.is_ready() {
                chat.start_agent();
                continue;
            }
            if chat.turn_running() {
                // A turn someone else started reads what waits at its next step, as it
                // would a message sent now, rather than only once it has ended and rested.
                let offer = mailbox.has_urgent_waiting(&agent_id)
                    && !mailbox.state.locked().unsteered.contains(&agent_id);
                if offer {
                    deliver_later(core, agent_id, Pending::Turn);
                }
                continue;
            }
            mailbox.state.locked().unsteered.remove(&agent_id);
            match rests_until(core, &agent_id, rest) {
                Some(until) => next = sooner(next, Some(until)),
                None => deliver_later(core, agent_id, Pending::Chat),
            }
        } else if let Some(terminal) = core.agent_session(&agent_id) {
            if mailbox.missed_pastes(&agent_id) >= MAX_MISSED_PASTES
                || !agent::takes_prompt(core, &terminal)
            {
                continue;
            }
            match rests_until(core, &agent_id, rest) {
                Some(until) => next = sooner(next, Some(until)),
                None => {
                    let texts = mailbox.take(&agent_id);
                    if !texts.is_empty() {
                        paste(core, &agent_id, terminal, texts);
                    }
                }
            }
        } else if core
            .workspaces()
            .agent(&agent_id)
            .is_some_and(|agent| agent.asleep || agent.terminal)
        {
            if !core.window().is_open() || core.workspaces().held(&agent_id) {
                continue;
            }
            let failed = mailbox.state.locked().wake_failed.get(&agent_id).copied();
            match failed.map(|at| at + WAKE_RETRY_MS) {
                Some(retry) if retry > epoch_ms() => next = sooner(next, Some(retry)),
                _ => deliver_later(core, agent_id, Pending::Asleep),
            }
        } else if core.workspaces().closed(&agent_id) {
            log(
                core,
                &agent_id,
                &mailbox.take(&agent_id),
                MessageOutcome::Dropped,
            );
        }
    }
    if let Some(next) = next {
        mailbox.wake.at(next);
    }
}

/// Puts the messages waiting for the chat in its feed, as it starts or wakes.
pub(crate) fn seed(core: &Core, feed: &crate::server::chat::feed::Feed, agent_id: &str) {
    feed.set_mail(waiting_texts(core, agent_id));
}

pub(super) fn waiting_texts(core: &Core, agent_id: &str) -> Vec<String> {
    let state = core.harness().mailbox.state.locked();
    (state.waiting.get(agent_id).into_iter().flatten())
        .map(|queued| queued.text.clone())
        .collect()
}

/// Tells each chat whose waiting messages changed what waits for it now.
pub(super) fn show(core: &Core) {
    let agents: Vec<String> = {
        let mut state = core.harness().mailbox.state.locked();
        let now: HashSet<String> = state.waiting.keys().cloned().collect();
        let agents = now.union(&state.shown).cloned().collect();
        state.shown = now;
        agents
    };
    for agent_id in agents {
        if let Some(chat) = core.chats().get(&agent_id) {
            chat.feed.set_mail(waiting_texts(core, &agent_id));
        }
    }
}

enum Pending {
    Chat,
    /// The chat is at work on a turn, which may take what waits.
    Turn,
    Asleep,
}

fn deliver_later(core: &Arc<Core>, agent_id: String, pending: Pending) {
    let mailbox = &core.harness().mailbox;
    if !mailbox.state.locked().delivering.insert(agent_id.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let mailbox = &core.harness().mailbox;
        match pending {
            Pending::Chat => {
                let texts = mailbox.take(&agent_id);
                if !texts.is_empty() {
                    match hand(&core, &agent_id, &texts).await {
                        Ok(_) => log(&core, &agent_id, &texts, MessageOutcome::Delivered),
                        Err(_) => mailbox.put_back(&agent_id, texts),
                    }
                }
            }
            Pending::Turn => {
                let texts = mailbox.take_for_turn(&agent_id);
                if !texts.is_empty() {
                    let handed = hand(&core, &agent_id, &texts).await;
                    if !matches!(handed, Ok(Delivery::Steered)) {
                        let mut state = mailbox.state.locked();
                        state.take_offered(&agent_id, &joined(&texts));
                    }
                    match handed {
                        Ok(_) => log(&core, &agent_id, &texts, MessageOutcome::Delivered),
                        Err(_) => {
                            mailbox.state.locked().unsteered.insert(agent_id.clone());
                            mailbox.put_back(&agent_id, texts);
                        }
                    }
                }
            }
            Pending::Asleep if core.workspaces().held(&agent_id) => {}
            Pending::Asleep => {
                let woke = launching::wake(&core, agent_id.clone()).await;
                let mut state = mailbox.state.locked();
                match woke {
                    Ok(_) => state.wake_failed.remove(&agent_id),
                    Err(_) => state.wake_failed.insert(agent_id.clone(), epoch_ms()),
                };
            }
        }
        mailbox.state.locked().delivering.remove(&agent_id);
        mailbox.wake.now();
    });
}
