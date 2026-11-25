//! Every agent the core runs or the app holds, merged once: chats, terminals, those asleep and
//! those on linked hosts. What the agents' tools list, read and close comes from here.

use crate::server::agent_now::AgentNowCore;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_view::turn::TurnPhase;
use sikemux_view::workspace::WorkspaceAgent;
use sikemux_wire::protocol::{AgentStatus, ChatInfo, ChatState, SessionInfo};

use crate::cli::protocol::HarnessRequest;

use super::connection::blocking;
use super::workspace::project_of;
use super::Core;

/// The terminals of agents that have no chat here, the agent's id with each.
pub(crate) fn terminals<'a>(
    chats: &'a [ChatInfo],
    sessions: &'a [SessionInfo],
) -> impl Iterator<Item = (&'a SessionInfo, &'a str)> {
    sessions.iter().filter_map(|session| {
        let agent_id = session.agent_id.as_deref()?;
        (!chats.iter().any(|chat| chat.agent_id == agent_id)).then_some((session, agent_id))
    })
}

/// The folder of the project an agent's terminal was opened in.
pub(crate) fn terminal_folder(core: &Core, session: &SessionInfo) -> Option<std::path::PathBuf> {
    let id = session.project.as_deref()?;
    Some(core.workspaces().project_info(id)?.path)
}

pub(crate) use super::listed::{Listed, Reach};

/// Who hears what the core has to say about an agent: a stall, work it left behind, a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Hears {
    /// The agent that started it, while that one is open, asleep or not.
    Lead(String),
    /// Its project's chief, with no lead open.
    Chief(String),
    /// Nobody but the person, in a project with no chief.
    Person,
}

impl Hears {
    pub(crate) fn agent(&self) -> Option<&str> {
        match self {
            Hears::Lead(agent) | Hears::Chief(agent) => Some(agent),
            Hears::Person => None,
        }
    }
}

/// Who hears of `agent_id`, open or closed.
pub(crate) fn hears_of(core: &Core, agent_id: &str) -> Hears {
    let agent =
        (core.workspaces().agent(agent_id)).or_else(|| core.workspaces().closed_agent(agent_id));
    let named = core.names().get(agent_id);
    let lead = (agent.as_ref().and_then(|agent| agent.lead.clone()))
        .or_else(|| named.as_ref().and_then(|named| named.parent.clone()));
    if let Some(lead) = lead.filter(|lead| core.open_agents().contains(lead)) {
        return Hears::Lead(lead);
    }
    let chief = match agent {
        Some(agent) => super::chief::seated(core).remove(&agent.project),
        None => named.and_then(|named| super::chief::chief_at(core, &named.project)),
    };
    (chief.filter(|chief| chief != agent_id && core.workspaces().agent(chief).is_some()))
        .map_or(Hears::Person, Hears::Chief)
}

/// An agent's status as other agents are told it.
pub(crate) fn standing(agent: &Listed, core: &Core) -> AgentStatus {
    let unstarted = agent.status == AgentStatus::Starting
        && (core.chats().get(&agent.agent_id)).is_some_and(|chat| chat.unstarted());
    let has_mail = core.harness().mailbox.has_waiting(&agent.agent_id)
        && core.inbox().stopped_by(&agent.agent_id).is_none();
    stands(agent.status, unstarted, has_mail)
}

/// A chat whose agent starts only once it is written to is asleep, not starting, since
/// nothing of it is on its way up. An agent at rest that has a task or messages waiting
/// for it counts as about to start its next turn.
fn stands(status: AgentStatus, unstarted: bool, has_mail: bool) -> AgentStatus {
    let status = match status {
        AgentStatus::Starting if unstarted => AgentStatus::Asleep,
        status => status,
    };
    let resting = matches!(
        status,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Asleep
    );
    if resting && has_mail {
        AgentStatus::Starting
    } else {
        status
    }
}

/// Every agent the core runs or the app holds, in the order the device view
/// puts them. Reads the disk to place agents in projects.
pub(crate) fn listed(core: &Core) -> Vec<Listed> {
    let (chats, sessions) = core.ranked_infos();
    let projects = core.workspaces().project_paths();
    let place = |agent_id: &str, cwd: Option<&std::path::Path>| {
        core.names()
            .get(agent_id)
            .map(|named| (named.project, named.parent))
            .unwrap_or_else(|| {
                let project = cwd.map_or_else(String::new, |cwd| project_of(cwd, &projects));
                (project, None)
            })
    };
    let mut agents: Vec<Listed> = chats
        .iter()
        .map(|chat| {
            let chat = chat.clone();
            let (project, parent) = place(&chat.agent_id, Some(&chat.cwd));
            let reach = match chat.state {
                ChatState::Asleep => Reach::Asleep,
                ChatState::Stopped => Reach::Stopped,
                ChatState::Starting | ChatState::Ready => Reach::Chat,
            };
            Listed {
                name: chat.name,
                project,
                cli: chat.provider,
                title: chat.title,
                status: chat.status,
                parent,
                active_at: chat.active_at,
                rank: chat.attention_rank,
                reach,
                agent_id: chat.agent_id,
            }
        })
        .collect();
    for (session, agent_id) in terminals(&chats, &sessions) {
        let (Some(rank), Some(status)) = (session.attention_rank, session.status) else {
            continue;
        };
        let folder = terminal_folder(core, session);
        let (project, parent) = place(agent_id, folder.as_deref());
        agents.push(Listed {
            name: session.agent_name.clone(),
            project,
            cli: session
                .agent_type
                .clone()
                .unwrap_or_else(|| "terminal".into()),
            title: session.title.clone(),
            status,
            parent,
            active_at: session.active_at,
            rank,
            reach: Reach::Terminal,
            agent_id: agent_id.to_owned(),
        });
    }
    let away: Vec<WorkspaceAgent> = core
        .workspaces()
        .agent_ids()
        .into_iter()
        .filter(|agent_id| !agents.iter().any(|listed| &listed.agent_id == agent_id))
        .filter_map(|agent_id| core.workspaces().agent(&agent_id))
        .filter(|agent| agent.terminal)
        .collect();
    for agent in away {
        let (project, parent) = place(&agent.id, Some(&agent.cwd));
        agents.push(Listed {
            name: core.names().get(&agent.id).map(|named| named.name),
            project,
            title: core.workspaces().title(&agent.id),
            cli: agent.provider,
            status: AgentStatus::Asleep,
            parent,
            active_at: None,
            rank: u32::MAX,
            reach: Reach::Asleep,
            agent_id: agent.id,
        });
    }
    agents.sort_by_key(|agent| agent.rank);
    agents.extend(core.hosts().team.all().into_iter().map(|foreign| {
        let shown = super::inbox::foreign_shown(core, &foreign);
        Listed {
            name: shown.name,
            project: foreign.project,
            cli: foreign.cli,
            title: shown.titled.then_some(shown.title),
            status: foreign.status,
            parent: foreign.parent,
            active_at: shown.active_at,
            rank: u32::MAX,
            reach: Reach::Remote,
            agent_id: foreign.agent_id,
        }
    }));
    let peers = core.hosts().peers.all().into_iter();
    agents.extend(peers.filter(|peer| core.hosts().team.get(&peer.agent_id).is_none()));
    agents
}

/// The host an agent on a linked host runs on, as this host calls it.
pub(crate) fn host_of(core: &Core, agent: &Listed) -> Option<String> {
    if agent.reach != Reach::Remote {
        return None;
    }
    let host = (core
        .hosts()
        .team
        .get(&agent.agent_id)
        .map(|foreign| foreign.host))
    .or_else(|| core.hosts().peers.host(&agent.agent_id))?;
    Some(super::hosts::name_of(core, &host))
}

fn since(at: Option<u64>, now: u64) -> Option<String> {
    let seconds = now.saturating_sub(at?) / 1000;
    Some(match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    })
}

pub(crate) fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "waiting on the person",
        AgentStatus::Working => "working",
        AgentStatus::Done => "done",
        AgentStatus::Idle => "idle",
        AgentStatus::Asleep => "asleep",
        AgentStatus::Stopped => "stopped",
        AgentStatus::Starting => "starting",
    }
}

pub(crate) async fn list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    if let Some(models) = super::caller::text_param(&request.params, "models", 64)? {
        let (core, request, models) = (core.clone(), request.clone(), models.to_owned());
        return blocking(move || {
            let project = super::caller::project_of(&core, &request);
            super::can_start::for_tool(&core, std::path::Path::new(&project), &models)
                .map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string());
    }
    let all = match request.params.get("allProjects") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(all)) => *all,
        Some(_) => return Err("allProjects must be a boolean".into()),
    };
    let full = matches!(request.params.get("full"), Some(Value::Bool(true)));
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = super::caller::project_of(&core, &request);
        let caller = request.agent_id.clone().unwrap_or_default();
        let rows = rows(&core, (!all).then_some(project.as_str()), &caller);
        let now = super::now::text_of_path(&core, &project, full);
        if !full {
            let lines: Vec<String> = rows.iter().map(line).collect();
            return Ok(json!({ "project": project, "now": now, "agents": lines }));
        }
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|mut row| {
                if let Some(fields) = row.as_object_mut() {
                    for key in ["name", "startedBy"] {
                        if let Some(name) = fields.get(key).and_then(Value::as_str) {
                            fields.insert(key.into(), super::names::at_name(name).into());
                        }
                    }
                }
                row
            })
            .collect();
        let updates = core.updates().status();
        Ok(json!({
            "project": project,
            "now": now,
            "core": {
                "version": updates.current,
                "commit": updates.commit,
                "tookOver": since(Some(updates.running_since).filter(|at| *at != 0), epoch_ms()),
            },
            "agents": rows,
        }))
    })
    .await
    .map_err(|error| error.to_string())
}

/// One line for an agent's row: who it is, how it stands and what it last did.
fn line(row: &Value) -> String {
    let field = |key: &str| row.get(key).and_then(Value::as_str);
    let mut line = format!(
        "@{} ({}) {}, {}",
        field("name").unwrap_or("?"),
        field("agentId").unwrap_or_default(),
        field("cli").unwrap_or_default(),
        field("status").unwrap_or_default(),
    );
    for (label, key) in [
        ("", "turn"),
        ("started by @", "startedBy"),
        ("on ", "host"),
        ("project ", "project"),
    ] {
        if let Some(value) = field(key).filter(|value| !value.is_empty()) {
            line.push_str(&format!("; {label}{value}"));
        }
    }
    for ask in row
        .get("asksPerson")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        line.push_str(&format!("; asks the person {ask}"));
    }
    if row.get("you").is_some() {
        line.push_str("; you");
    } else if row.get("canMessage") == Some(&Value::Bool(false)) {
        line.push_str("; can't message");
    }
    line
}

/// The agents in `project`, or in every project without one, as `agent_list` answers
/// `caller`.
pub(crate) fn rows(core: &Core, project: Option<&str>, caller: &str) -> Vec<Value> {
    let agents = listed(core);
    let names: HashMap<String, String> = agents
        .iter()
        .filter_map(|agent| Some((agent.agent_id.clone(), agent.name.clone()?)))
        .collect();
    let now = epoch_ms();
    let turns = super::turn_state::all(core);
    agents
        .iter()
        .filter(|agent| project.is_none_or(|project| agent.project == project))
        .map(|agent| {
            let host = host_of(core, agent);
            let reachable = core
                .hosts()
                .team
                .get(&agent.agent_id)
                .is_none_or(|foreign| core.hosts().reachable(&foreign.host));
            let status = if reachable {
                status_word(standing(agent, core))
            } else {
                "out of reach"
            };
            let mut row = json!({
                "name": agent.name,
                "agentId": agent.agent_id,
                "cli": agent.cli,
                "title": agent.title,
                "status": status,
                "turn": turns.get(&agent.agent_id).and_then(|turn| {
                    let answer = (turn.state == TurnPhase::Done && agent.parent.is_none())
                        .then(|| super::follow::last_answer(core, &agent.agent_id))
                        .flatten();
                    super::turn_state::listed_line(turn, answer.as_deref(), agent.parent.is_some())
                }),
                "lastActive": since(agent.active_at, now),
                "canMessage": agent.reach != Reach::Stopped && agent.agent_id != caller,
            });
            if let Some(fields) = row.as_object_mut() {
                if let Some(parent) = &agent.parent {
                    fields.insert(
                        "startedBy".into(),
                        (names.get(parent).cloned())
                            .or_else(|| core.names().name(parent))
                            .unwrap_or_else(|| parent.clone())
                            .into(),
                    );
                }
                let asks = core.harness().questions.asks_person(&agent.agent_id);
                if !asks.is_empty() {
                    fields.insert("asksPerson".into(), asks.into());
                }
                if agent.agent_id == caller {
                    fields.insert("you".into(), true.into());
                }
                let (model, effort) = super::follow::runs_on(core, &agent.agent_id);
                if let Some(model) = model {
                    fields.insert("model".into(), model.into());
                }
                if let Some(effort) = effort {
                    fields.insert("effort".into(), effort.into());
                }
                if let Some(host) = host {
                    fields.insert("host".into(), host.into());
                }
                if project.is_none() {
                    fields.insert("project".into(), agent.project.clone().into());
                }
            }
            row
        })
        .collect()
}

/// The agents the call lists, as it names them.
pub(crate) fn wanted(request: &HarnessRequest, to: &str) -> Result<Vec<String>, String> {
    match request.params.get("agents") {
        Some(Value::Array(agents)) if !agents.is_empty() => agents
            .iter()
            .map(|agent| agent.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| "agents must be names or ids".into()),
        _ => Err(format!("agents must list the agents to {to}")),
    }
}

/// Whether `caller` started the agent, directly or down the line.
pub(crate) fn started_by(core: &Core, agent_id: &str, caller: &str) -> bool {
    core.names()
        .leads_of(agent_id)
        .iter()
        .any(|lead| lead == caller)
}

/// Closes agents the caller started, or that one of its workers started, or one that no open agent
/// leads, as the person closing their tabs would, on whichever host each runs. A project's chief may
/// also close any finished agent of its project. Their chats stay in Recent. Each agent it may not
/// close is refused with its reason, and the rest still close.
pub(crate) async fn close(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_close needs the calling agent's id")?;
    let wanted = wanted(request, "close")?;
    let (here, request, caller_id) = (core.clone(), request.clone(), caller.clone());
    let (mut closed, left, remote, mut refused) = blocking(move || {
        let (core, caller) = (here, caller_id);
        let project = super::caller::project_of(&core, &request);
        let agents = listed(&core);
        let waited_on = waited_on(&core, &caller);
        let mut closing = Vec::new();
        let mut remote = Vec::new();
        let mut refused = Vec::new();
        for to in &wanted {
            match closable(&core, &agents, &waited_on, to, &project, &caller) {
                Ok(target) if target.reach == Reach::Remote => remote.push(target.agent_id),
                Ok(target) => {
                    let name = target
                        .name
                        .as_deref()
                        .map(super::names::at_name)
                        .unwrap_or_else(|| target.agent_id.clone());
                    closing.push((target.agent_id, name));
                }
                Err(why) => {
                    refused.push(json!({ "agent": super::names::at_name(to), "reason": why }))
                }
            }
        }
        let ids: Vec<String> = closing.iter().map(|(id, _)| id.clone()).collect();
        let left: Vec<String> = (closing.iter())
            .filter_map(|(id, name)| super::left_behind::left_by_close(&core, id, name))
            .collect();
        if !ids.is_empty() {
            super::workspace::close(&core, &ids)?;
        }
        let closed: Vec<String> = closing.into_iter().map(|(_, name)| name).collect();
        Ok((closed, left, remote, refused))
    })
    .await
    .map_err(|error| error.to_string())?;
    for agent_id in remote {
        match super::hosts::route::close(core, &caller, &agent_id).await {
            Ok(name) => closed.push(super::names::at_name(&name)),
            Err(why) => refused.push(json!({ "agent": agent_id, "reason": why })),
        }
    }
    let mut result = serde_json::Map::new();
    result.insert("closed".to_owned(), closed.into());
    if !left.is_empty() {
        result.insert("left".to_owned(), left.into());
    }
    if !refused.is_empty() {
        result.insert("refused".to_owned(), refused.into());
    }
    Ok(Value::Object(result))
}

/// Moves agents' chats to another account of their agent, named by its name or id, where each
/// carries on; lists those it refused, with why.
pub(crate) async fn move_account(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_account needs the calling agent's id")?;
    let account = (request.params.get("account").and_then(Value::as_str))
        .map(str::trim)
        .filter(|account| !account.is_empty())
        .ok_or("account must name the account to move to")?
        .to_owned();
    let wanted = wanted(request, "move")?;
    let (here, request) = (core.clone(), request.clone());
    let (moving, mut refused) = blocking(move || {
        let project = super::caller::project_of(&here, &request);
        let agents = listed(&here);
        let mut moving: Vec<(Listed, String)> = Vec::new();
        let mut refused = Vec::new();
        for to in &wanted {
            let target = match resolve(&here, &agents, to, &project, &caller) {
                Ok(target) if target.reach == Reach::Remote => {
                    Err("it runs on another host".to_owned())
                }
                found => found,
            };
            let chosen = target.and_then(|target| {
                let kind = crate::agent_catalog::agent(&target.cli);
                if let Some(kind) = kind.filter(|kind| kind.accounts_by_hand) {
                    return Err(format!(
                        "only the person moves {}'s chats to another account",
                        kind.label
                    ));
                }
                let takers = super::chat::accounts(&here, &target.agent_id)
                    .map_err(|error| error.to_string())?;
                let named = |other: &&sikemux_wire::protocol::ChatAccount| {
                    other.id == account || other.label.eq_ignore_ascii_case(&account)
                };
                match takers.others.iter().find(named) {
                    Some(other) => Ok((target, other.id.clone())),
                    None if takers.current.as_ref().is_some_and(|now| named(&now)) => {
                        Err("it is on that account already".into())
                    }
                    None => Err(takers.none_because.unwrap_or_else(|| {
                        format!("{account} is not an account it can carry on on")
                    })),
                }
            });
            match chosen {
                Ok((target, account_id)) => moving.push((target, account_id)),
                Err(why) => {
                    refused.push(json!({ "agent": super::names::at_name(to), "reason": why }))
                }
            }
        }
        Ok((moving, refused))
    })
    .await
    .map_err(|error| error.to_string())?;
    let mut moved = Vec::new();
    for (target, account_id) in moving {
        let name = target
            .name
            .as_deref()
            .map(super::names::at_name)
            .unwrap_or_else(|| target.agent_id.clone());
        match super::chat::move_to_account(core, &target.agent_id, &account_id).await {
            Ok(()) => moved.push(name),
            Err(why) => refused.push(json!({ "agent": name, "reason": why.to_string() })),
        }
    }
    let mut result = serde_json::Map::new();
    result.insert("moved".into(), moved.into());
    if !refused.is_empty() {
        result.insert("refused".into(), refused.into());
    }
    Ok(Value::Object(result))
}

/// The open agents other than `caller` that wait on another agent, by the agent each waits on.
fn waited_on(core: &Core, caller: &str) -> HashMap<String, String> {
    (super::turn_state::all(core).into_iter())
        .filter(|(waiter, _)| waiter != caller)
        .filter_map(|(waiter, turn)| Some((turn.wait?.agent?, core.names().name_or_id(&waiter))))
        .collect()
}

/// The agent `to` names, when the caller may close it, else why not.
fn closable(
    core: &Core,
    agents: &[Listed],
    waited_on: &HashMap<String, String>,
    to: &str,
    project: &str,
    caller: &str,
) -> Result<Listed, String> {
    let target = resolve(core, agents, to, project, caller)?;
    if target.reach == Reach::Remote {
        return Ok(target);
    }
    if let Some(waiter) = waited_on.get(&target.agent_id) {
        let name = target
            .name
            .as_deref()
            .map(super::names::at_name)
            .unwrap_or_else(|| target.agent_id.clone());
        return Err(format!(
            "{} is waiting on {name}, so closing it is not yours to do yet",
            super::names::at_name(waiter)
        ));
    }
    if started_by(core, &target.agent_id, caller) {
        return Ok(target);
    }
    let led = core.names().leads_of(&target.agent_id);
    let orphan = !led.is_empty() && !led.iter().any(|lead| core.agent_now(lead).open());
    if orphan {
        return Ok(target);
    }
    let name = target
        .name
        .as_deref()
        .map(super::names::at_name)
        .unwrap_or_else(|| target.agent_id.clone());
    Err(format!(
        "{name} is not one you started, so Sikemux closes it once it is finished"
    ))
}

/// The agent `to` names: an agent id, or a name held in the caller's project
/// or on its team, preferring the caller's project. `Name on host` names the
/// agent of that name on that host.
pub(crate) fn resolve(
    core: &Core,
    agents: &[Listed],
    to: &str,
    project: &str,
    caller: &str,
) -> Result<Listed, String> {
    let wanted = to.trim().trim_start_matches('@');
    if let Some(agent) = agents.iter().find(|agent| agent.agent_id == wanted) {
        return Ok(agent.clone());
    }
    let (wanted, on) = super::messages::on_host(wanted);
    let named = |agent: &&Listed| {
        agent
            .name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            && on.is_none_or(|on| {
                host_of(core, agent).is_some_and(|host| host.eq_ignore_ascii_case(on))
            })
    };
    let (yours, called): (Vec<&Listed>, Vec<&Listed>) = agents
        .iter()
        .filter(named)
        .partition(|agent| agent.agent_id == caller);
    if called.is_empty() && !yours.is_empty() {
        return Err("That agent is you".into());
    }
    let reachable: Vec<&Listed> = called
        .iter()
        .copied()
        .filter(|agent| on.is_some() || reaches(core, agent, project, caller))
        .collect();
    let here: Vec<&Listed> = reachable
        .iter()
        .copied()
        .filter(|agent| agent.project == project)
        .collect();
    match (here.as_slice(), reachable.as_slice(), called.as_slice()) {
        ([agent], _, _) | ([], [agent], _) => Ok((*agent).clone()),
        ([], [], []) => Err(teammates_work(core, project, wanted).unwrap_or_else(|| format!(
            "No open agent is called {wanted}; agent_list shows the agents you can message. One you started that closed is read by agent_read and reopened by agent_send; any other is found by agent_sessions, or who_wrote for its commit, and reopened by agent_spawn with resume set to its sessionId"
        ))),
        ([], [], elsewhere) => Err(format!(
            "No agent in your project or on your team is called {}. {}; pass its agent id if you mean it",
            crate::server::names::at_name(wanted),
            elsewhere
                .iter()
                .map(|agent| format!(
                    "{} ({}) is in {}",
                    crate::server::names::at_name(wanted),
                    agent.agent_id,
                    agent.project
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        (_, many, _) => Err(format!(
            "More than one agent is called {}: {}. Pass the agent id instead",
            crate::server::names::at_name(wanted),
            many.iter()
                .map(|agent| match host_of(core, agent) {
                    Some(host) => format!("{} on {host}", agent.agent_id),
                    None => format!("{} in {}", agent.agent_id, agent.project),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Why `wanted` can't be reached when it names a teammate's task or their agent on it.
fn teammates_work(core: &Core, project: &str, wanted: &str) -> Option<String> {
    if super::peer::teammate_task(core, project, wanted).is_some() {
        return Some(format!(
            "{wanted} is a teammate's task; agent_read with it and a question asks about it"
        ));
    }
    let holder = super::peer::held_by_teammate(core, project, wanted)?;
    Some(format!(
        "{holder}. Another person's agents can't be messaged, read, woken or closed; agent_read with their task and a question asks about it"
    ))
}

/// Whether a bare name from `caller` may mean `agent`: it works in the
/// caller's project, or on the caller's team on any host.
pub(crate) fn reaches(core: &Core, agent: &Listed, project: &str, caller: &str) -> bool {
    let top = match (&agent.reach, &agent.parent) {
        (Reach::Remote, Some(parent)) => core.names().team_of(parent),
        _ => core.names().team_of(&agent.agent_id),
    };
    agent.project == project || top == core.names().team_of(caller)
}
