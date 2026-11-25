//! What `agent_list` and `agent_send` answer: the agents running here, and
//! messages between them. A message reaches an agent as text marked as coming
//! from the agent that sent it, so it can never answer a permission request.
//! A busy chat takes it into its turn when it can; an agent that cannot gets
//! the messages waiting for it together once its turn ends.

pub(crate) use super::names::{at_name, at_name_now, called, called_now};
use crate::server::changed::{ChangedCore, What};
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
pub use sikemux_calls::messages::{AgentMessageRows, AgentMessagesParams, MessageKind};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::cli::protocol::HarnessRequest;

use self::log::{MessageEntry, MessageOutcome};
use super::chat::{self, Delivery};
use super::connection::blocking;
use super::roster::{listed, resolve, Listed, Reach};
use super::{launching, Core, CoreResult};

mod delivery;
mod format;
pub mod log;
mod mailbox;
mod reports;
#[cfg(test)]
mod tests;

pub(crate) use delivery::{
    from_person, person_wrote, prompt_settled, prompt_taken, seed, settle, used_tool, worked,
};
pub(crate) use format::{task, Answered, Answering, Carried, Sender};
pub(crate) use mailbox::Mailbox;
pub(crate) use reports::{
    calm_reports, recheck_calm_for_child, report, report_as, report_calm, report_ci, report_land,
    report_left, reported_at, siblings_busy,
};

use delivery::{deliver, deliver_as};
use format::{letter_from, note_open_calls, wrapped, Letter};
#[cfg(test)]
use mailbox::CalmWait;
use mailbox::{Origin, Queued};

pub(super) const FILE: &str = "agent-mailbox.json";
/// The mark on the task an agent in a terminal waits to take.
pub(crate) const TASK: &str = "task";
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_WHY_BYTES: usize = 8 * 1024;
pub(super) const MAX_WAITING: usize = 10;
const HOSTS_NOTE: &str =
    "Sent by Sikemux about a teammate on another host, not by the person or another agent.";
/// The same text to the same agent again within this long is dropped.
pub(super) const REPEAT_WINDOW_MS: u64 = 2 * 60 * 1000;
/// Two agents answering each other forever stop here.
pub(super) const PAIR_WINDOW_MS: u64 = 10 * 60 * 1000;
pub(super) const PAIR_LIMIT: usize = 30;
/// Two agents that have exchanged this many messages, both ways, with
/// neither doing anything else in between, have their messages to each other
/// held for the person.
pub(super) const LOOP_MESSAGES: usize = 4;
/// The messages held between one pair of agents.
pub(super) const MAX_HELD: usize = 10;
/// The tools that only talk to other agents, so calling them is no other work.
pub(super) const MESSAGING_TOOLS: [&str; 4] =
    ["agent_send", "agent_list", "agent_read", "agent_wait"];
/// How long an agent rests after a turn before a message starts another, so a
/// message the person queued in the app goes first.
pub(super) const SETTLE_MS: u64 = 1500;
/// How long an agent rests after a turn before messages nobody waits on, such as a trigger's or a
/// teammate's report, start another, so those that come close together are read in one turn.
pub(super) const AUTO_SETTLE_MS: u64 = 30_000;
/// How long a calm report waiting on busy siblings waits at most before waking its lead anyway.
pub(super) const CALM_CEILING_MS: u64 = 10 * 60 * 1000;
/// How long after a worker's turn ends its report to its lead is taken to be on its way,
/// before the report has begun.
pub(super) const REPORT_DUE_MS: u64 = 10_000;
/// How long Sikemux remembers that an agent reported, so the chief's digest does not repeat it.
pub(super) const REPORTED_MS: u64 = 24 * 60 * 60_000;
/// Turns in a row an agent may start on messages nobody waits on before the rest wait for the
/// person, its lead or its chief to write to it.
pub(super) const MAX_AUTO_TURNS: u32 = 20;
/// How long after waking an asleep chat failed before it is tried again.
pub(super) const WAKE_RETRY_MS: u64 = 60_000;
/// A terminal agent takes the Enter that sends a pasted message only once it
/// has taken in the paste.
pub(super) const ENTER_AFTER_PASTE: Duration = Duration::from_millis(150);
/// A terminal agent that keeps printing this long after the Enter that sends
/// a paste has started a turn on it.
pub(super) const TURN_OUTPUT_MS: u64 = 1000;
/// Pastes a terminal agent may leave untaken in a row before Sikemux stops
/// typing into it until it starts a turn of its own.
pub(super) const MAX_MISSED_PASTES: u8 = 3;

/// Hands a new agent its task: into a turn, or ahead of whatever waits for
/// it when it is already busy with something else.
pub(crate) async fn hand_task(
    core: &Arc<Core>,
    agent_id: &str,
    task: String,
) -> Result<(), String> {
    let mailbox = &core.harness().mailbox;
    let from = core
        .names()
        .get(agent_id)
        .and_then(|named| named.parent)
        .unwrap_or_else(|| Sender::Auto.id().to_owned());
    let queued = Queued {
        text: task.clone(),
        origin: Some(Origin {
            from,
            kind: MessageKind::Message,
            awaited: false,
            calm: None,
        }),
        mark: None,
        paths: Vec::new(),
    };
    if mailbox.has_waiting(agent_id) {
        log(core, agent_id, [&queued], MessageOutcome::Waiting);
        mailbox.put_back(agent_id, vec![queued]);
        return Ok(());
    }
    match chat::letter(core, agent_id, task, Vec::new(), false).await {
        Ok(delivered @ (Delivery::Steered | Delivery::NewTurn)) => {
            let outcome = match delivered {
                Delivery::Steered => MessageOutcome::Steered,
                _ => MessageOutcome::NewTurn,
            };
            log(core, agent_id, [&queued], outcome);
            Ok(())
        }
        Ok(Delivery::TurnEnded | Delivery::Refused) => {
            log(core, agent_id, [&queued], MessageOutcome::Waiting);
            mailbox.put_back(agent_id, vec![queued]);
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The latest messages between agents in `project`, newest first.
pub(crate) fn logged(core: &Core, project: &str, limit: Option<usize>) -> Vec<MessageEntry> {
    let limit = limit.unwrap_or(log::MAX_READ).min(log::MAX_READ);
    core.harness().mailbox.log.read(project, limit)
}

/// The project's messages between agents, one row each, newest first.
pub(crate) fn logged_rows(core: &Core, project: &str) -> Vec<log::MessageRow> {
    log::rows(&logged(core, project, None), |id| core.names().name(id))
}

/// Notes in the log of the project of agent `to` what became of messages
/// for it.
fn log<'a>(
    core: &Core,
    to: &str,
    queued: impl IntoIterator<Item = &'a Queued>,
    outcome: MessageOutcome,
) {
    let queued: Vec<&Queued> = queued.into_iter().collect();
    let answers = queued.iter().any(|queued| {
        queued
            .origin
            .as_ref()
            .is_some_and(|origin| origin.kind == MessageKind::Answer)
    });
    if answers && !matches!(outcome, MessageOutcome::Waiting | MessageOutcome::Returned) {
        core.harness().questions.delivered(to);
    }
    let Some(project) = core.names().get(to).map(|named| named.project) else {
        return;
    };
    let at = epoch_ms();
    let to_name = core.names().name(to);
    let mut logged = false;
    for queued in queued {
        let from = queued.origin.as_ref().map(|origin| origin.from.clone());
        core.harness().mailbox.log.append(
            &project,
            at,
            &MessageEntry {
                at,
                from_name: from.as_deref().and_then(|from| core.names().name(from)),
                from,
                kind: queued.origin.as_ref().map(|origin| origin.kind),
                to: to.to_owned(),
                to_name: to_name.clone(),
                outcome,
            },
        );
        logged = true;
    }
    if logged {
        core.changed(What::View);
    }
}

fn message_text(core: &Core, params: &Value) -> Result<String, String> {
    match params.get("text") {
        Some(Value::String(text)) if !text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES => {
            Ok(core.harness().questions.hide_secrets(text))
        }
        Some(Value::String(text)) if !text.trim().is_empty() => Err(format!(
            "text is {} bytes; a message holds at most {MAX_TEXT_BYTES}, so put long material in a file and send its path",
            text.len(),
        )),
        _ => Err("text must be the message, as nonempty text".into()),
    }
}

/// An agent's name and the host it is said to run on, as in "Otter on
/// studio" or "Otter@studio".
pub(super) fn on_host(wanted: &str) -> (&str, Option<&str>) {
    if let Some((name, host)) = wanted.split_once('@') {
        return (name.trim(), Some(host.trim()));
    }
    let lower = wanted.to_ascii_lowercase();
    let split = lower
        .find(" on ")
        .and_then(|at| Some((wanted.get(..at)?, wanted.get(at + 4..)?)));
    match split {
        Some((name, host)) => (name.trim(), Some(host.trim())),
        None => (wanted, None),
    }
}

/// What `agent_send` takes as `to` for every other agent of the caller's team.
const TEAM: &str = "team";
const MAX_TO_BYTES: usize = 2000;
const MAX_SWITCH_BYTES: usize = 200;

/// Sends one message to every other agent of `from`'s team: the agent that
/// leads it and every agent it started, each delivered as a message of its own.
async fn send_team(
    core: &Arc<Core>,
    request: &HarnessRequest,
    from: &str,
) -> Result<Value, String> {
    let top = core.names().team_of(from);
    let members: Vec<String> = core
        .names()
        .team(&top)
        .into_iter()
        .filter(|member| member != from)
        .collect();
    if members.is_empty() {
        return Err("You have no team: no agent leads you and you started none".into());
    }
    let named = members
        .into_iter()
        .map(|member| (at_name_now(core, &member), member))
        .collect();
    Ok(send_each(core, request, named).await)
}

/// Sends one message to each of `to`, its shown name and its address, all at once, each
/// delivered as a message of its own, and answers with each one's delivery in order.
async fn send_each(core: &Arc<Core>, request: &HarnessRequest, to: Vec<(String, String)>) -> Value {
    let sending = to.into_iter().map(|(name, address)| {
        let mut one = request.clone();
        if let Some(params) = one.params.as_object_mut() {
            params.insert("to".into(), Value::String(address));
        }
        async move { (name, Box::pin(send(core, &one)).await) }
    });
    let answers = futures_util::future::join_all(sending).await;
    let (mut sent, mut failed) = (Vec::new(), Vec::new());
    for (name, answer) in answers {
        let shown = at_name(&name);
        match answer {
            Ok(answer) => sent.push(json!({
                "to": answer.get("to").cloned().unwrap_or_else(|| shown.into()),
                "agentId": answer.get("agentId"),
                "delivery": answer.get("delivery"),
            })),
            Err(why) => failed.push(json!({ "to": shown, "error": why })),
        }
    }
    json!({ "sent": sent, "failed": failed })
}

pub(crate) async fn send(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let from = request
        .agent_id
        .clone()
        .ok_or("agent_send needs the calling agent's id")?;
    let to = match request.params.get("to") {
        Some(Value::String(to)) if !to.trim().is_empty() && to.len() <= MAX_TO_BYTES => to.clone(),
        _ => {
            return Err("to must be the agent's name or id, or several separated by commas".into())
        }
    };
    let text = message_text(core, &request.params)?;
    let why = super::caller::text_param(&request.params, "why", MAX_WHY_BYTES)?.map(str::to_owned);
    let answers = match request.params.get("answers") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.trim().is_empty() => Some(Answering {
            id: id.trim().to_owned(),
            why,
        }),
        Some(_) => return Err("answers must be the id of the question this answers".into()),
    };
    let run_on = super::caller::text_param(&request.params, "runOn", MAX_SWITCH_BYTES)?;
    let switching = run_on.is_some();
    if switching && (to.contains(',') || to.trim().eq_ignore_ascii_case(TEAM)) {
        return Err("runOn switches one agent at a time".into());
    }
    if to.trim().eq_ignore_ascii_case(TEAM) {
        if answers.is_some() {
            return Err("answers goes to one agent, not the team".into());
        }
        return Box::pin(send_team(core, request, &from)).await;
    }
    if to.contains(',') {
        let mut several: Vec<String> = Vec::new();
        for name in to.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            if !several.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                several.push(name.to_owned());
            }
        }
        if answers.is_some() {
            return Err("answers goes to one agent, not several".into());
        }
        if several.iter().any(|name| name.eq_ignore_ascii_case(TEAM)) {
            return Err("team goes alone in to, not among other names".into());
        }
        let named = several
            .into_iter()
            .map(|name| (name.clone(), name))
            .collect();
        return Ok(send_each(core, request, named).await);
    }
    super::hosts::peers::find(core, &to).await;
    let found = {
        let (core, request, answers) = (core.clone(), request.clone(), answers.clone());
        blocking(move || {
            let project = super::caller::project_of(&core, &request);
            let agents = listed(&core);
            let target = match resolve(&core, &agents, &to, &project, &from) {
                Ok(target) => target,
                Err(why) => {
                    let (wanted, on) = on_host(to.trim().trim_start_matches('@'));
                    if on.is_some() {
                        return Err(why.into());
                    }
                    let named = |agent: &Listed| {
                        agent.agent_id == wanted
                            || (agent.name.as_deref())
                                .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                                && super::roster::reaches(&core, agent, &project, &from)
                    };
                    if agents.iter().any(named) {
                        return Err(why.into());
                    }
                    if let Some((_, ran)) = core.names().closed_worker(&from, wanted) {
                        return Ok(Found::Closed(wanted.to_owned(), ran));
                    }
                    return match super::hosts::reopen::closed_elsewhere(&core, &from, wanted) {
                        Some(worker) => Ok(Found::ClosedElsewhere(worker.name)),
                        None => Err(why.into()),
                    };
                }
            };
            if target.agent_id == from {
                return Err("That agent is you".into());
            }
            let mut letter = letter_from(&core, &agents, MessageKind::Message, from, text, None);
            if let Some(Answering { id, .. }) =
                answers.as_ref().filter(|_| target.reach != Reach::Remote)
            {
                let name = target.name.as_deref().unwrap_or(&target.agent_id);
                let asked = core
                    .harness()
                    .questions
                    .asked_of(&letter.from_id, &target.agent_id, id)
                    .ok_or_else(|| {
                        core.harness()
                            .questions
                            .not_asked(&letter.from_id, name, id)
                    })?;
                letter.kind = MessageKind::Answer;
                letter.note = Some(lead_answer_note(&letter.from_name, &asked));
            }
            Ok(Found::Open(target, Box::new(letter)))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (target, letter) = match found {
        Found::Open(target, letter) => (target, *letter),
        Found::Closed(name, _) | Found::ClosedElsewhere(name) if switching => {
            return Err(format!(
                "{} had closed; agent_spawn it again with the cli and model it should run on",
                at_name(&name)
            ));
        }
        Found::Closed(name, ran) if calls_off(&message_text(core, &request.params)?) => {
            return Ok(called_off(
                &name,
                &format!("{}, cli {}", ran.session_id, ran.provider),
            ));
        }
        Found::ClosedElsewhere(name) if calls_off(&message_text(core, &request.params)?) => {
            return Ok(called_off(&name, &name));
        }
        Found::Closed(name, ran) => return reopen(core, request, &name, ran).await,
        Found::ClosedElsewhere(name) => return reopen_elsewhere(core, request, &name).await,
    };
    let from = letter.from_id.clone();
    if target.reach == Reach::Remote && switching {
        return Err("it runs on another host, so runOn cannot switch it from here".into());
    }
    if target.reach == Reach::Remote {
        return carry(core, &target, letter, answers).await;
    }
    let switched = match run_on {
        Some(run_on) => {
            let shown = target
                .name
                .as_deref()
                .map(at_name)
                .unwrap_or_else(|| target.agent_id.clone());
            let switched = super::chat::switch::switch(core, &target.agent_id, run_on).await;
            Some(switched.map_err(|why| format!("{shown} was not switched or sent this: {why}"))?)
        }
        None => None,
    };
    let answer = letter.text.clone();
    let mut sent = deliver(core, &target, letter).await?;
    if let (Some(switched), Some(fields)) = (switched, sent.as_object_mut()) {
        fields.insert("runsOn".into(), switched.into());
    }
    core.seen().called(&target.agent_id);
    core.changed(What::Agents);
    match answers {
        Some(answering) => {
            settle_answer(core, from, target.agent_id.clone(), answering, answer).await;
        }
        None => {
            if let (Some(decided), Some(fields)) =
                (decided_for(core, &target, &from), sent.as_object_mut())
            {
                fields.insert("decided".into(), decided.into());
            }
        }
    }
    Ok(sent)
}

/// What `author` decided in the last day by answering `target`'s questions that still stands, and how to
/// correct it, so a message that changes course does not leave the old answer on record.
fn decided_for(core: &Core, target: &Listed, author: &str) -> Option<String> {
    let since = epoch_ms().saturating_sub(24 * 60 * 60_000);
    let decided: Vec<String> = super::board::goal::standing_since(core, &target.project, since, 20)
        .into_iter()
        .filter(|decision| {
            decision.holder.as_deref() == Some(target.agent_id.as_str())
                && decision.learned.agent.as_deref() == Some(author)
        })
        .map(|decision| format!("#{} \"{}\"", decision.id, decision.text))
        .collect();
    (!decided.is_empty()).then(|| {
        format!(
            "Your answers to it stand as decisions {}. If this message changes one, correct it with decide, decision: its number.",
            decided.join("; ")
        )
    })
}

enum Found {
    Open(Listed, Box<Letter>),
    /// A closed agent the sender led, by the name it was sent to.
    Closed(String, super::names::RanSession),
    /// A worker of the sender's that closed on a linked host, by its name.
    ClosedElsewhere(String),
}

/// Reopens on its host a worker of the sender's that closed on a linked host, with the message
/// as its task.
async fn reopen_elsewhere(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
) -> Result<Value, String> {
    let shown = at_name(name);
    let text = message_text(core, &request.params)?;
    let opened = super::hosts::reopen::reopen(core, request, None, name, text)
        .await
        .map_err(|why| format!("{shown} had closed, and reopening it failed: {why}"))?;
    Ok(json!({
        "to": opened.get("name").and_then(Value::as_str).map(at_name),
        "agentId": opened.get("agentId"),
        "delivery": "reopened",
        "note": format!("{shown} had closed, so Sikemux reopened its session on {}; it gets this as its task once it is ready.", opened.get("host").and_then(Value::as_str).unwrap_or("its host")),
    }))
}

/// Takes up again the session of a closed agent the sender led, with the message as its task.
/// The answer to a message that calls off the work of `name`, a worker that had closed, which
/// agent_spawn reopens with `resume`.
fn called_off(name: &str, resume: &str) -> Value {
    let shown = at_name(name);
    json!({
        "to": shown,
        "delivery": "notSent",
        "note": format!("{shown} had closed, so its work had already stopped, and Sikemux did not reopen it for a message that calls the work off. To reopen it with this message anyway, agent_spawn with resume {resume}."),
    })
}

/// Whether `text` only calls work off, as a short message that opens with stop or cancel does.
fn calls_off(text: &str) -> bool {
    const OPENINGS: [&str; 12] = [
        "stop",
        "cancel",
        "abort",
        "halt",
        "drop it",
        "never mind",
        "nevermind",
        "stand down",
        "no need",
        "hold off",
        "scrap",
        "forget it",
    ];
    let text = text.trim().trim_start_matches('@').to_lowercase();
    let opens = |text: &str| {
        OPENINGS.iter().any(|opening| {
            text.strip_prefix(opening).is_some_and(|rest| {
                rest.chars()
                    .next()
                    .is_none_or(|next| !next.is_alphanumeric())
            })
        })
    };
    let after_name = (text.split_once(char::is_whitespace))
        .filter(|(first, _)| first.ends_with([',', ':']))
        .map(|(_, rest)| rest.trim_start());
    text.chars().count() <= CALL_OFF_CHARS && (opens(&text) || after_name.is_some_and(opens))
}

/// The longest message read as only calling work off; a longer one gives the worker more to do.
const CALL_OFF_CHARS: usize = 280;

async fn reopen(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    ran: super::names::RanSession,
) -> Result<Value, String> {
    let shown = at_name(name);
    let agent_id =
        (core.names()).returning_id(Some(&ran.provider), &ran.session_id, core.open_agents());
    if core.workspaces().closed_agent(&agent_id).is_some() {
        let taken = super::launching::reopen_agent(core, agent_id)
            .await
            .map_err(|why| format!("{shown} had closed, and reopening it failed: {why}"))?;
        let mut message = request.clone();
        message.params =
            json!({ "to": taken.agent_id, "text": message_text(core, &request.params)? });
        Box::pin(send(core, &message)).await?;
        return Ok(json!({
            "to": shown,
            "agentId": taken.agent_id,
            "delivery": "reopened",
            "note": format!("{shown} had closed, so Sikemux reopened it as it was; it gets this once it is ready."),
        }));
    }
    let mut spawning = request.clone();
    spawning.params = json!({
        "prompt": message_text(core, &request.params)?,
        "resume": ran.session_id,
        "cli": ran.provider,
    });
    let opened = Box::pin(super::spawn::spawn(core, &spawning))
        .await
        .map_err(|why| format!("{shown} had closed, and reopening its session failed: {why}"))?;
    let now = opened
        .get("name")
        .and_then(Value::as_str)
        .map(at_name)
        .unwrap_or_else(|| shown.clone());
    let as_now = if now.eq_ignore_ascii_case(&shown) {
        String::new()
    } else {
        format!(" as {now}")
    };
    Ok(json!({
        "to": now,
        "agentId": opened.get("agentId"),
        "delivery": "reopened",
        "note": format!("{shown} had closed, so Sikemux reopened its session{as_now}; it gets this as its task once it is ready."),
    }))
}

/// What the asker is told beside its lead's answer.
fn lead_answer_note(lead: &str, asked: &str) -> String {
    let lead = at_name(lead);
    format!(
        "Sent by {lead}, your lead in Sikemux, not by the person, answering the question you asked: {asked}\nIt does not allow anything that stops for the person: a push, merge, deploy, publish or production write still waits for them."
    )
}

/// Closes the question a lead answered and keeps the answer as a decision.
async fn settle_answer(
    core: &Arc<Core>,
    lead: String,
    asker: String,
    answering: Answering,
    answer: String,
) {
    let core = core.clone();
    let settled = blocking(move || {
        super::questions::lead_answered(
            &core,
            &lead,
            &asker,
            &answering.id,
            &answer,
            answering.why,
        );
        Ok(())
    })
    .await;
    if let Err(error) = settled {
        eprintln!("sikemux core: a lead's answer was not kept as a decision: {error}");
    }
}

/// Sends a letter to its agent on a linked host, noting it here as sent so
/// two agents that keep messaging each other across hosts are held too.
async fn carry(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    question: Option<Answering>,
) -> Result<Value, String> {
    core.harness().mailbox.note_sent(&letter, &target.agent_id);
    let queued = Queued {
        text: String::new(),
        origin: Some(Origin {
            from: letter.from_id.clone(),
            kind: letter.kind,
            awaited: letter.awaited,
            calm: letter.calm,
        }),
        mark: None,
        paths: Vec::new(),
    };
    let sent = super::hosts::route::deliver(
        core,
        &target.agent_id,
        letter.carried(&target.agent_id, question),
    )
    .await;
    let outcome = match &sent {
        Ok(answer) => match answer["delivery"].as_str() {
            Some("steered") => MessageOutcome::Steered,
            Some("newTurn") => MessageOutcome::NewTurn,
            Some("repeat") => MessageOutcome::Repeat,
            _ => MessageOutcome::Waiting,
        },
        Err(_) => MessageOutcome::Refused,
    };
    log(core, &target.agent_id, [&queued], outcome);
    sent
}

/// A letter a linked host carried here for one of this host's agents, sent
/// by an agent on `host`, or the person's
/// answer to a question that went to a lead here first. `landed` names the
/// branch the sender's latest commits came back on, for its report to say.
pub(crate) async fn from_host(
    core: &Arc<Core>,
    host: Option<&str>,
    carried: Carried,
    landed: Option<(String, usize)>,
) -> Result<Value, String> {
    let to = carried.to.clone();
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to && agent.reach != Reach::Remote)
                .ok_or_else(|| "that agent is not on this host".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target.agent_id.clone());
    let mut letter = Letter {
        kind: carried.kind,
        from_id: carried.from_id.clone(),
        from_name: match host {
            Some(host) => format!("{} on {host}", carried.from_name),
            None => carried.from_name,
        },
        cli: carried.cli,
        text: carried.text,
        at: epoch_ms(),
        note: carried.note,
        answers: carried.answered,
        puts: None,
        paths: Vec::new(),
        waits: false,
        awaited: false,
        calm: None,
    };
    if let Some((branch, count)) = landed {
        letter.text.push_str(&format!(
            "\n\nIts commits came back to the branch {branch} in your checkout: {count} on top of the commit you started it from, the first holding your uncommitted changes as they were then. Merge what you want of them."
        ));
    }
    if let Some(Answering { id, .. }) = &carried.question {
        let asked = core
            .harness()
            .questions
            .asked_of(&letter.from_id, &target.agent_id, id)
            .ok_or_else(|| {
                core.harness()
                    .questions
                    .not_asked(&letter.from_id, &name, id)
            })?;
        letter.kind = MessageKind::Answer;
        letter.note = Some(lead_answer_note(&letter.from_name, &asked));
    }
    let from = letter.from_id.clone();
    let answer = letter.text.clone();
    let sent = deliver(core, &target, letter).await?;
    if let Some(answering) = carried.question {
        settle_answer(core, from, target.agent_id.clone(), answering, answer).await;
    }
    Ok(sent)
}

/// Tells an agent here something about a teammate on another host. Fails
/// when the agent has stopped or is gone.
pub(crate) async fn from_hosts(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| {
                    agent.agent_id == to && !matches!(agent.reach, Reach::Stopped | Reach::Remote)
                })
                .ok_or_else(|| "that agent is not running".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let letter = Letter::from_sender(
        Sender::Hosts,
        MessageKind::Message,
        text,
        Some(HOSTS_NOTE.to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Puts the question `id`, worded `text`, from one agent to its lead, with `note` in
/// place of the usual one. Fails when the lead has stopped or is gone.
pub(crate) async fn question(
    core: &Arc<Core>,
    id: String,
    from: String,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let (target, letter) = {
        let core = core.clone();
        blocking(move || {
            let agents = listed(&core);
            let target = agents
                .iter()
                .find(|agent| agent.agent_id == to && agent.reach != Reach::Stopped)
                .cloned()
                .ok_or("that agent is not running")?;
            let mut letter = letter_from(
                &core,
                &agents,
                MessageKind::Question,
                from,
                text,
                Some(note),
            );
            letter.puts = Some(id);
            Ok((target, letter))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    deliver(core, &target, letter).await
}

/// Hands an agent the person's answer to the question it asked.
pub(crate) async fn answer(
    core: &Arc<Core>,
    to: String,
    answers: Vec<(Answered, String, String, Vec<String>)>,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        let to = to.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to)
                .ok_or_else(|| "that agent is gone".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let chat = core.chats().get(&target.agent_id).is_some();
    let mut letters = answers.into_iter().map(|(answered, text, note, paths)| {
        let (text, paths) = match chat && !paths.is_empty() {
            true => (super::questions::without_attached(&text, &paths), paths),
            false => (text, Vec::new()),
        };
        Letter {
            kind: MessageKind::Answer,
            from_id: Sender::Person.id().to_owned(),
            from_name: Sender::Person.name().to_owned(),
            cli: "person".to_owned(),
            text,
            at: epoch_ms(),
            note: Some(note),
            answers: Some(answered),
            puts: None,
            paths,
            waits: false,
            awaited: false,
            calm: None,
        }
    });
    let mut letter = letters.next().ok_or("there is no answer to send")?;
    let rest: Vec<Letter> = letters.collect();
    if rest.is_empty() {
        return Box::pin(deliver(core, &target, letter)).await;
    }
    if target.reach == Reach::Remote {
        let mut delivered = carry(core, &target, letter, None).await?;
        for letter in rest {
            delivered = carry(core, &target, letter, None).await?;
        }
        return Ok(delivered);
    }
    note_open_calls(core, &target.agent_id, &mut letter);
    let text = std::iter::once(&letter)
        .chain(&rest)
        .map(wrapped)
        .collect::<Vec<_>>()
        .join("\n\n");
    letter
        .paths
        .extend(rest.into_iter().flat_map(|rest| rest.paths));
    Box::pin(deliver_as(core, &target, letter, text)).await
}

/// The agent `to`, unless it has stopped or is gone.
async fn running(core: &Arc<Core>, to: String) -> Result<Listed, String> {
    let core = core.clone();
    blocking(move || {
        listed(&core)
            .into_iter()
            .find(|agent| agent.agent_id == to && agent.reach != Reach::Stopped)
            .ok_or_else(|| "that agent is not running".into())
    })
    .await
    .map_err(|error| error.to_string())
}

/// Whether a message to `to` would reach a running agent.
pub(crate) async fn reachable(core: &Arc<Core>, to: &str) -> bool {
    running(core, to.to_owned()).await.is_ok()
}

/// The id a message reaches `to` by now, reopening it as it was when it had closed here.
pub(crate) async fn reopened(core: &Arc<Core>, to: &str) -> Option<String> {
    if reachable(core, to).await {
        return Some(to.to_owned());
    }
    core.workspaces().closed_agent(to)?;
    match super::launching::reopen_agent(core, to.to_owned()).await {
        Ok(taken) => reachable(core, &taken.agent_id)
            .await
            .then_some(taken.agent_id),
        Err(error) => {
            eprintln!("sikemux core: {to} had closed and did not reopen: {error}");
            None
        }
    }
}

/// Hands an agent a note from Sikemux's board about its cards. Fails when the
/// agent has stopped or is gone.
pub(crate) async fn from_board(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(Sender::Board, MessageKind::Message, text, Some(note));
    deliver(core, &target, letter).await
}

/// Hands an agent the prompt of a trigger it set, waking it first when it sleeps, unless it
/// sleeps since the person stopped everything. Fails when the agent has stopped or is gone.
pub(crate) async fn from_trigger(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let mut target = running(core, to.clone()).await?;
    if target.reach == Reach::Asleep && !core.workspaces().held(&to) {
        launching::wake(core, to.clone())
            .await
            .map_err(|error| format!("it is asleep and did not wake: {error}"))?;
        target = running(core, to).await?;
    }
    let letter = Letter::from_sender(Sender::Trigger, MessageKind::Message, text, Some(note));
    deliver(core, &target, letter).await
}

/// Hands an agent a note from Sikemux itself, about work it shares with
/// other agents. Fails when the agent has stopped or is gone.
pub(crate) async fn from_sikemux(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Notice,
        text,
        Some("Sent by Sikemux itself, not by the person or another agent.".to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Gives an agent work from Sikemux itself, starting a turn when it is at rest, where a notice
/// would be dropped; `why` says to the agent what Sikemux sends it for. Fails when the agent has
/// stopped or is gone.
pub(crate) async fn work_from_sikemux(
    core: &Arc<Core>,
    to: String,
    text: String,
    why: &str,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(format!(
            "Sent by Sikemux itself {why}, not by the person or another agent."
        )),
    );
    deliver(core, &target, letter).await
}

/// Hands an agent the step the person took on its turn's line with one click. It starts a turn
/// when the agent is at rest. The words are Sikemux's for that step, not the person's, so they
/// answer no call and are not kept as what the person said.
pub(crate) async fn step_from_person(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    person_wrote(core, &target.agent_id);
    let mut letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some("Sent by Sikemux because the person took this step on your turn's line with one click; the words are Sikemux's, not theirs.".to_owned()),
    );
    letter.awaited = true;
    deliver(core, &target, letter).await
}

/// Sends an agent back to work its turn ended on, saying `why`. It starts a turn when the agent is
/// at rest, where a notice would be dropped. Fails when the agent has stopped or is gone.
pub(crate) async fn send_back(
    core: &Arc<Core>,
    to: String,
    text: String,
    why: &str,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(why.to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Tells an agent what came of something it waited on the person for, as [`for_person`] does,
/// but waits for it when it has stopped. Fails only when it is gone.
pub(crate) async fn for_person_waiting(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to)
                .ok_or_else(|| "that agent is gone".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let mut letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(
            "Sent by Sikemux with the person's answer about an action it held for them.".to_owned(),
        ),
    );
    letter.waits = true;
    letter.awaited = true;
    deliver(core, &target, letter).await
}

/// Tells an agent what the person decided about something of its that
/// Sikemux held for them, which starts a turn when it is at rest. Fails when
/// the agent has stopped or is gone.
pub(crate) async fn for_person(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let mut letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(
            "Sent by Sikemux with the person's answer about an action it held for them.".to_owned(),
        ),
    );
    letter.awaited = true;
    deliver(core, &target, letter).await
}

/// Drops the messages holding `mark` that still wait for the agent.
pub(crate) fn withdraw(core: &Core, agent_id: &str, mark: &str) {
    if !is_waiting(core, agent_id, mark) {
        return;
    }
    let withdrawn = core.harness().mailbox.change(|state| {
        let Some(waiting) = state.waiting.get_mut(agent_id) else {
            return Vec::new();
        };
        let (withdrawn, kept) = std::mem::take(waiting)
            .into_iter()
            .partition(|queued| queued.mark.as_deref() == Some(mark));
        *waiting = kept;
        withdrawn
    });
    log(core, agent_id, &withdrawn, MessageOutcome::Dropped);
}

/// Whether the message known by `mark` still waits for the agent's turn to end.
pub(crate) fn is_waiting(core: &Core, agent_id: &str, mark: &str) -> bool {
    core.harness()
        .mailbox
        .state
        .locked()
        .waiting
        .get(agent_id)
        .is_some_and(|waiting| {
            waiting
                .iter()
                .any(|queued| queued.mark.as_deref() == Some(mark))
        })
}

/// Runs `read` off the async threads on the project at `project`, which must be an absolute
/// path, under the project's own path.
async fn in_project<T: Send + 'static>(
    core: &Arc<Core>,
    project: &str,
    read: impl FnOnce(&Core, String) -> T + Send + 'static,
) -> CoreResult<T> {
    let path = Path::new(project);
    if !path.is_absolute() {
        return Err("project must be the project's absolute path".into());
    }
    let (core, path) = (core.clone(), path.to_owned());
    blocking(move || {
        let project = core.workspaces().project_of(&path);
        Ok(read(&core, project))
    })
    .await
}

/// The project's log of messages between agents, newest first, under the
/// project's own path, which keys it in the inbox's `agentMessages`.
pub(crate) async fn agent_messages(
    core: &Arc<Core>,
    params: AgentMessagesParams,
) -> CoreResult<Value> {
    let limit = params.limit;
    in_project(core, &params.project, move |core, project| {
        let entries = logged(core, &project, limit);
        json!({ "project": project, "entries": entries })
    })
    .await
}

/// The project's messages between agents folded into one row each, newest first.
pub(crate) async fn agent_message_rows(
    core: &Arc<Core>,
    params: AgentMessagesParams,
) -> CoreResult<AgentMessageRows> {
    in_project(core, &params.project, |core, project| {
        let rows = logged_rows(core, &project);
        AgentMessageRows { project, rows }
    })
    .await
}
