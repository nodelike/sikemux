//! What a chat is told of the team around it: what is open in its project and where its
//! workers stand, and who a teammate on another host is.

use std::collections::HashMap;

use sikemux_process::clock::epoch_ms;
use sikemux_view::calls::CallState;
use sikemux_view::turn::TurnState;

use super::chat::cap::{ago, standing, waited, LONGEST_OPEN};
use super::device_view::DeviceViewCore;
use super::messages::called_now;
use super::parts::Parts;
use super::workspace_state::WorkspaceParts;
use super::{display, hosts, inbox, person_calls, roster, Core};

/// The calls open to the person in the chat's project, what the chat waits on, and where each
/// of its workers stands.
pub(super) fn open_work(core: &Core, agent_id: &str) -> Vec<String> {
    let listed = roster::listed(core);
    let Some(own) = listed.iter().find(|agent| agent.agent_id == agent_id) else {
        return Vec::new();
    };
    let (chats, sessions) = core.ranked_infos();
    let turns: HashMap<String, TurnState> = inbox::view(core, &chats, &sessions)
        .agents
        .into_iter()
        .filter_map(|agent| Some((agent.agent_id, agent.turn?)))
        .collect();
    let now = epoch_ms();
    let mut lines: Vec<String> = person_calls::shown(core)
        .into_iter()
        .filter(|call| call.state == CallState::Open && call.project == own.project)
        .map(|call| {
            format!(
                "- A call to the person from @{}, open {}: {}",
                call.asker.name,
                ago(now, call.raised_at),
                call.question
            )
        })
        .collect();
    let turn = |agent: &roster::Listed| turns.get(&agent.agent_id);
    if let Some(wait) = turn(own).and_then(|turn| turn.wait.as_ref()) {
        lines.push(format!("- You wait on {}", waited(wait, now)));
    }
    let leads = core.workspaces().agent_leads();
    let mut workers: Vec<String> = (listed.iter())
        .filter(|agent| leads.get(&agent.agent_id).map(String::as_str) == Some(agent_id))
        .map(|worker| {
            let title = worker.title.as_deref().unwrap_or("no title yet");
            let stands =
                turn(worker).map_or_else(|| "not started".to_owned(), |turn| standing(turn, now));
            format!(
                "- Your worker {}: {title}. {stands}",
                called_now(core, &worker.agent_id)
            )
        })
        .collect();
    workers.sort();
    lines.extend(workers);
    for line in &mut lines {
        if line.chars().count() > LONGEST_OPEN {
            *line = line
                .chars()
                .take(LONGEST_OPEN)
                .chain(" [cut]".chars())
                .collect();
        }
    }
    lines
}

/// The note for a teammate on a linked host, which messages and reads reach by its name and
/// that host's.
pub(super) fn note_elsewhere(core: &Core, id: &str) -> Option<String> {
    let mate = core.hosts().team.get(id)?;
    let host = hosts::name_of(core, &mate.host);
    let to = format!("{} on {host}", mate.name);
    let shown = inbox::foreign_shown(core, &mate);
    let cli = crate::agent_catalog::agent(&mate.cli).map_or(mate.cli.as_str(), |entry| entry.label);
    Some(
        [
            format!("@{to} is an agent of your team on another host, {host}, not the person."),
            format!("Agent id: {id}"),
            format!("CLI: {cli}"),
            format!("Status: {}", display::status_label(mate.status)),
            format!("Working on: {}", shown.title),
            format!("To message it, call agent_send with to \"{to}\"."),
            format!("To read what it said, call agent_read with agent \"{to}\"."),
        ]
        .join("\n"),
    )
}
