//! Agents on the person's other linked hosts that are on no team with an agent here. A host
//! asks the others for one when a message, a read or a mention names an agent it doesn't know,
//! and keeps those it heard of until it stops, so an answer finds its way back.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::AgentStatus;
use tokio::task::JoinSet;

use super::super::connection::blocking;
use super::super::roster::{self, Listed, Reach};
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::{ask, name_of};

/// The longest a host waits for the others to say whether they run the agent asked for.
const FIND_WAIT: Duration = Duration::from_secs(3);

#[derive(Serialize, Deserialize)]
pub(crate) struct AgentParams {
    /// The agent's id or name.
    agent: String,
}

/// One of a host's own agents, as it tells another host that asked for it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    agent_id: String,
    name: Option<String>,
    cli: String,
    project: String,
    title: Option<String>,
    status: AgentStatus,
}

/// Each agent heard of on a linked host, by its id, with that host's key.
#[derive(Default)]
pub(crate) struct Peers(Mutex<HashMap<String, (String, Listed)>>);

impl Peers {
    pub(crate) fn host(&self, agent_id: &str) -> Option<String> {
        let peers = self.0.locked();
        peers.get(agent_id).map(|(host, _)| host.clone())
    }

    pub(crate) fn get(&self, agent_id: &str) -> Option<(String, Listed)> {
        self.0.locked().get(agent_id).cloned()
    }

    pub(crate) fn all(&self) -> Vec<Listed> {
        let peers = self.0.locked();
        peers.values().map(|(_, agent)| agent.clone()).collect()
    }

    pub(crate) fn forget_host(&self, host: &str) {
        self.0.locked().retain(|_, (on, _)| on != host);
    }

    fn put(&self, host: &str, agent: Listed) {
        let mut peers = self.0.locked();
        peers.insert(agent.agent_id.clone(), (host.to_owned(), agent));
    }
}

/// Keeps the agent on `host` that sent a letter here, so the agent it wrote to can answer it.
pub(crate) fn heard_from(core: &Core, host: &str, agent_id: &str, name: &str, cli: &str) {
    if core.hosts().team.get(agent_id).is_some() {
        return;
    }
    let mut agent = core.hosts().peers.get(agent_id).map_or_else(
        || Listed {
            agent_id: agent_id.to_owned(),
            name: None,
            project: String::new(),
            cli: cli.to_owned(),
            title: None,
            status: AgentStatus::Idle,
            parent: None,
            active_at: None,
            rank: u32::MAX,
            reach: Reach::Remote,
        },
        |(_, agent)| agent,
    );
    agent.name = Some(name.to_owned());
    core.hosts().peers.put(host, agent);
}

/// Asks the linked hosts for the agent `wanted` names, by id, name or `Name on host`, when
/// this host knows none by it, and keeps each one they run.
pub(crate) async fn find(core: &Arc<Core>, wanted: &str) {
    let wanted = wanted.trim().trim_start_matches('@').to_owned();
    let hosts = {
        let (core, wanted) = (core.clone(), wanted.clone());
        blocking(move || Ok(unknown_on(&core, &wanted)))
            .await
            .unwrap_or_default()
    };
    if hosts.is_empty() {
        return;
    }
    let name = super::super::messages::on_host(&wanted).0.to_owned();
    let mut asking = JoinSet::new();
    for host in hosts {
        let params = params_of::<asks::Agent>(&AgentParams {
            agent: name.clone(),
        });
        let core = core.clone();
        asking.spawn(async move {
            let asked = ask(&core, &host, asks::Agent::METHOD, params, None, FIND_WAIT);
            let found = tokio::time::timeout(FIND_WAIT, asked).await.ok()?.ok()?;
            Some((host, serde_json::from_value::<Vec<Found>>(found).ok()?))
        });
    }
    while let Some(done) = asking.join_next().await {
        let Ok(Some((host, found))) = done else {
            continue;
        };
        for found in found {
            core.hosts().peers.put(
                &host,
                Listed {
                    agent_id: found.agent_id,
                    name: found.name,
                    project: found.project,
                    cli: found.cli,
                    title: found.title,
                    status: found.status,
                    parent: None,
                    active_at: None,
                    rank: u32::MAX,
                    reach: Reach::Remote,
                },
            );
        }
    }
}

/// The reachable linked hosts to ask for `wanted`, none when an agent this host knows goes by it.
fn unknown_on(core: &Core, wanted: &str) -> Vec<String> {
    let (name, on) = super::super::messages::on_host(wanted);
    let known = roster::listed(core).iter().any(|agent| {
        agent.agent_id == wanted
            || agent.name.as_deref().is_some_and(|known| {
                known.eq_ignore_ascii_case(name)
                    && on.is_none_or(|on| {
                        roster::host_of(core, agent)
                            .is_some_and(|host| host.eq_ignore_ascii_case(on))
                    })
            })
    });
    if known {
        return Vec::new();
    }
    let on = match on.map(|on| super::find(core, on)) {
        Some(Ok(link)) => Some(link.id),
        Some(Err(_)) => return Vec::new(),
        None => None,
    };
    (core.remote().links().into_iter())
        .map(|link| link.id)
        .filter(|host| on.as_ref().is_none_or(|on| on == host))
        .filter(|host| core.hosts().reachable(host))
        .collect()
}

/// This host's own agents a linked host asked for, by id or name.
pub(super) fn here(core: &Core, params: AgentParams) -> Answer {
    let projects = core.workspaces().project_names();
    let found: Vec<Found> = (roster::listed(core).into_iter())
        .filter(|agent| agent.reach != Reach::Remote)
        .filter(|agent| {
            agent.agent_id == params.agent
                || (agent.name.as_deref())
                    .is_some_and(|name| name.eq_ignore_ascii_case(&params.agent))
        })
        .map(|agent| Found {
            project: projects
                .get(&agent.project)
                .cloned()
                .unwrap_or(agent.project),
            agent_id: agent.agent_id,
            name: agent.name,
            cli: agent.cli,
            title: agent.title,
            status: agent.status,
        })
        .collect();
    serde_json::to_value(found).map_err(|error| error.to_string())
}

/// The note for an agent on a linked host that a mention names.
pub(crate) fn note(core: &Core, id: &str) -> Option<String> {
    let (host, agent) = core.hosts().peers.get(id)?;
    let host = name_of(core, &host);
    let name = agent.name.as_deref().unwrap_or(id);
    let to = format!("{name} on {host}");
    let cli =
        crate::agent_catalog::agent(&agent.cli).map_or(agent.cli.as_str(), |entry| entry.label);
    let mut lines = vec![
        format!("@{to} is an agent on another of the person's hosts, {host}, not the person."),
        format!("Agent id: {id}"),
        format!("CLI: {cli}"),
        format!(
            "Status: {}",
            super::super::display::status_label(agent.status)
        ),
    ];
    if let Some(title) = &agent.title {
        lines.push(format!("Working on: {title}"));
    }
    lines.push(format!("To message it, call agent_send with to \"{to}\"."));
    lines.push(format!(
        "To read what it said, call agent_read with agent \"{to}\"."
    ));
    Some(lines.join("\n"))
}
