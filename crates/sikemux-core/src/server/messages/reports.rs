use std::collections::HashMap;
use std::sync::Arc;

use sikemux_calls::messages::MessageKind;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{AgentLetter, LetterKind};
use sikemux_wire::protocol::ChatContext;

use super::delivery::deliver;
use super::format::{letter_from, CALM_HEADING};
use super::log::MessageOutcome;
use super::mailbox::{CalmWait, Queued};
use super::{called, called_now, log, CALM_CEILING_MS};
use crate::server::chat_state::ChatParts;
use crate::server::connection::blocking;
use crate::server::parts::Parts;
use crate::server::roster::{self, listed, Listed};
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// Hands `text` from `worker` to the agent that started it, as a message
/// about how its turn ended. A lead that has stopped gets it once it is
/// opened again; only one the person closed goes without.
pub(crate) async fn report(core: &Arc<Core>, worker: String, lead: String, text: String) {
    let name = core.names().name(&worker);
    report_as(core, worker, name, lead, text).await;
}

/// Hands `line` from `worker` to its lead as a calm report: one line read with the lead's next
/// turn, which starts a turn of its own once due, as [`calm_due`] says.
pub(crate) async fn report_calm(core: &Arc<Core>, worker: String, lead: String, line: String) {
    let name = core.names().name(&worker);
    report_noted(core, worker, name, lead, line, true, |_: &str| {
        String::new()
    })
    .await;
}

/// The calm reports waiting for `agent_id`, taken as what its turn starts with. Each also
/// shows in its chat as a message from its worker, since what the turn starts with is the
/// agent's alone.
pub(crate) fn calm_reports(core: &Core, agent_id: &str) -> Option<ChatContext> {
    let calm = core.harness().mailbox.take_calm(agent_id);
    if calm.is_empty() {
        return None;
    }
    log(core, agent_id, &calm, MessageOutcome::Delivered);
    let lines: Vec<&str> = calm.iter().map(|queued| queued.text.as_str()).collect();
    if let Some(chat) = core.chats().get(agent_id) {
        for queued in &calm {
            if let Some(letter) = calm_letter(core, queued) {
                crate::server::chat::tell_update(&chat, sikemux_fold::update::letter(&letter));
            }
        }
    }
    Some(crate::sessions::for_agent::sikemux_context(
        "reports",
        CALM_HEADING,
        &lines.join("\n"),
    ))
}

/// A calm report as its worker's message, without the name and open calls its lead reads it with.
pub(super) fn calm_letter(core: &Core, queued: &Queued) -> Option<AgentLetter> {
    let worker = &queued.origin.as_ref()?.from;
    let line = queued.text.lines().next()?;
    let (name, text) = line.strip_prefix("- ")?.split_once(": ")?;
    Some(AgentLetter {
        kind: LetterKind::Message,
        name: name.trim_start_matches('@').to_owned(),
        agent_id: worker.clone(),
        cli: (core.chats().get(worker))
            .map_or_else(|| "agent".to_owned(), |chat| chat.launch.provider.clone()),
        question: String::new(),
        text: text.to_owned(),
        secret: false,
        attachments: Vec::new(),
        tree: None,
        trigger: None,
    })
}

/// When `agent_id` last reported to `to`, within the last day.
pub(crate) fn reported_at(core: &Core, agent_id: &str, to: &str) -> Option<u64> {
    let state = core.harness().mailbox.state.locked();
    let (reached, at) = state.reported.get(agent_id)?;
    (reached == to).then_some(*at)
}

/// Hands the lead of `worker`, when it has one, a line on how a land of its went, which ends
/// after the turn that queued it. A calm one is a land that went in with nothing to fix.
pub(crate) fn report_land(core: &Arc<Core>, worker: &str, text: String, calm: bool) {
    let Some(lead) = core.names().reports_to(worker) else {
        return;
    };
    let name = core.names().name(worker);
    let (core, worker) = (core.clone(), worker.to_owned());
    tokio::spawn(async move {
        let note = |called: &str| {
            format!(
                "{called} is an agent you started, and this is how its land went, sent by Sikemux."
            )
        };
        let text = match calm {
            true => crate::text::one_line(&text),
            false => text,
        };
        report_noted(&core, worker.clone(), name, lead, text, calm, note).await;
        crate::server::now::asks::turn_reported(&core, &worker);
    });
}

/// Hands `to` the work an idle `worker` left behind: commits off the project's branch, or
/// changes it never committed.
pub(crate) fn report_left(core: &Arc<Core>, worker: String, to: String, text: String) {
    let name = core.names().name(&worker);
    let core = core.clone();
    tokio::spawn(async move {
        let note = |called: &str| {
            format!("{called} left work behind, and this is a note from Sikemux, sent once while it stays idle.")
        };
        report_noted(&core, worker, name, to, text, false, note).await;
    });
}

/// Hands `to` one note on a CI run that failed after the pushes of `workers`, all closed.
pub(crate) fn report_ci(core: &Arc<Core>, workers: Vec<String>, to: String, text: String) {
    let Some(worker) = workers.first().cloned() else {
        return;
    };
    let name = core.names().name(&worker);
    let others: Vec<String> = (workers.iter().skip(1))
        .map(|other| called_now(core, other))
        .collect();
    let core = core.clone();
    tokio::spawn(async move {
        let note = move |called: &str| {
            let closed = match others.is_empty() {
                true => format!("{called} has closed, and CI failed after its push"),
                false => format!(
                    "{called} and {} have closed, and CI failed after their pushes",
                    others.join(", ")
                ),
            };
            format!("{closed}. Sikemux watched it and sends this to you once in their place: fix it or hand it on.")
        };
        report_noted(&core, worker, name, to, text, false, note).await;
    });
}

/// [`report`] for a worker that may be gone, under the name it had.
pub(crate) async fn report_as(
    core: &Arc<Core>,
    worker: String,
    name: Option<String>,
    lead: String,
    text: String,
) {
    let note = |called: &str| {
        format!(
            "{called} is an agent you started, and this is how its turn ended, sent by Sikemux. Reply with agent_send to it if it needs more; once its work has landed and you need nothing more, close it with agent_close."
        )
    };
    report_noted(core, worker, name, lead, text, false, note).await;
}

async fn report_noted(
    core: &Arc<Core>,
    worker: String,
    name: Option<String>,
    lead: String,
    text: String,
    calm: bool,
    note: impl FnOnce(&str) -> String + Send + 'static,
) {
    let found = {
        let core = core.clone();
        blocking(move || {
            let agents = listed(&core);
            let find = |id: &str| agents.iter().find(|agent| agent.agent_id == id).cloned();
            let (target, text) = match find(&lead) {
                Some(target) => (Some(target), text),
                None => match roster::hears_of(&core, &worker) {
                    roster::Hears::Chief(chief) if calm => {
                        (find(&chief), format!("{text} (its lead has closed)"))
                    }
                    roster::Hears::Chief(chief) => (
                        find(&chief),
                        format!("Its lead has closed, so this comes to you as the project's chief.\n\n{text}"),
                    ),
                    _ => (None, text),
                },
            };
            let Some(target) = target else {
                return Ok(None);
            };
            let name = name.unwrap_or_else(|| worker.clone());
            let note = note(&called(&name, &worker));
            let mut letter = letter_from(&core, &agents, MessageKind::Report, worker, text, Some(note));
            letter.from_name = name;
            letter.calm = calm.then(|| calm_due(&core, &agents, &letter.from_id, &target.agent_id, letter.at));
            Ok(Some((target, letter)))
        })
        .await
    };
    let Ok(Some((target, letter))) = found else {
        return;
    };
    if let Err(error) = deliver(core, &target, letter).await {
        eprintln!("sikemux core: an agent's report did not reach its lead: {error}");
    }
}

/// A calm report is due at once when the person waits on the worker's work or no other worker
/// of its lead is still at work, and otherwise waits until siblings go idle or the fallback
/// ceiling passes.
fn calm_due(core: &Core, agents: &[Listed], worker: &str, lead: &str, at: u64) -> CalmWait {
    calm_due_in(
        core,
        &crate::server::turn_state::all(core),
        agents,
        worker,
        lead,
        at,
    )
}

fn has_busy_siblings(
    turns: &HashMap<String, sikemux_view::turn::TurnState>,
    agents: &[Listed],
    worker: &str,
    lead: &str,
) -> bool {
    agents.iter().any(|agent| {
        agent.agent_id != worker
            && agent.parent.as_deref() == Some(lead)
            && (turns.get(&agent.agent_id)).is_some_and(|turn| {
                matches!(
                    turn.state,
                    sikemux_view::turn::TurnPhase::Working | sikemux_view::turn::TurnPhase::Waiting
                )
            })
    })
}

pub(crate) fn siblings_busy(core: &Core, worker: &str, lead: &str) -> bool {
    let turns = crate::server::turn_state::all(core);
    let agents = roster::listed(core);
    has_busy_siblings(&turns, &agents, worker, lead)
}

/// [`calm_due`] with each agent's turn standing as `turns` says.
pub(super) fn calm_due_in(
    core: &Core,
    turns: &HashMap<String, sikemux_view::turn::TurnState>,
    agents: &[Listed],
    worker: &str,
    lead: &str,
    at: u64,
) -> CalmWait {
    if crate::server::now::asks::person_waits_on(core, worker) {
        return CalmWait::At { at };
    }
    match has_busy_siblings(turns, agents, worker, lead) {
        true => CalmWait::UntilSiblingsIdle {
            capped_at: at.saturating_add(CALM_CEILING_MS),
        },
        false => CalmWait::At { at },
    }
}

/// Re-evaluates calm reports waiting on `lead_id`'s children when a child stops, closes, or goes idle.
pub(crate) fn recheck_calm_for_lead(core: &Core, lead_id: &str) {
    let mailbox = &core.harness().mailbox;
    if !mailbox.has_calm_waiting_on_siblings(lead_id) {
        return;
    }
    let agents = roster::listed(core);
    let any_busy = agents.iter().any(|agent| {
        agent.parent.as_deref() == Some(lead_id)
            && crate::server::turn_state::now(core, &agent.agent_id).is_some_and(|turn| {
                matches!(
                    turn.state,
                    sikemux_view::turn::TurnPhase::Working | sikemux_view::turn::TurnPhase::Waiting
                )
            })
    });
    if !any_busy {
        mailbox.promote_calm_waiting(lead_id, epoch_ms());
    }
}

pub(crate) fn recheck_calm_for_child(core: &Core, child_id: &str) {
    let Some(lead_id) = core.names().reports_to(child_id) else {
        return;
    };
    recheck_calm_for_lead(core, &lead_id);
}
