//! Every agent the core runs or the app holds, merged once: chats, terminals, those asleep and
//! those on linked hosts. What the agents' tools list, read and close comes from here.

use crate::server::agent_now::AgentNowCore;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_view::turn::{TurnPhase, WorkAt};
use sikemux_view::workspace::WorkspaceAgent;
use sikemux_wire::protocol::{AgentStatus, ChatInfo, ChatState, SessionInfo};

use crate::cli::protocol::HarnessRequest;
use crate::git::Git;

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

/// An agent's status, with one at rest that has a task or messages waiting
/// for it counted as about to start its next turn.
pub(crate) fn standing(agent: &Listed, core: &Core) -> AgentStatus {
    let resting = matches!(
        agent.status,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Asleep
    );
    if resting && core.harness().mailbox.has_waiting(&agent.agent_id) {
        AgentStatus::Starting
    } else {
        agent.status
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
                model: chat.model,
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
            model: None,
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
            model: None,
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
            model: None,
            status: foreign.status,
            parent: foreign.parent,
            active_at: shown.active_at,
            rank: u32::MAX,
            reach: Reach::Remote,
            agent_id: foreign.agent_id,
        }
    }));
    agents
}

/// The host an agent on a linked host runs on, as this host calls it.
fn host_of(core: &Core, agent: &Listed) -> Option<String> {
    (agent.reach == Reach::Remote)
        .then(|| core.hosts().team.get(&agent.agent_id))
        .flatten()
        .map(|foreign| super::hosts::name_of(core, &foreign.host))
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
    let all = match request.params.get("allProjects") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(all)) => *all,
        Some(_) => return Err("allProjects must be a boolean".into()),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = super::caller::project_of(&core, &request);
        let caller = request.agent_id.clone().unwrap_or_default();
        let rows = rows(&core, (!all).then_some(project.as_str()), &caller);
        Ok(json!({ "project": project, "agents": rows }))
    })
    .await
    .map_err(|error| error.to_string())
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
                    let answer = (core.chats().get(&agent.agent_id))
                        .and_then(|chat| chat.feed.last_answer());
                    super::turn_state::listed_line(turn, answer.as_deref(), agent.parent.is_some())
                }),
                "lastActive": since(agent.active_at, now),
                "canMessage": agent.reach != Reach::Stopped
                    && agent.agent_id != caller
                    && super::messages::blind_between(core, caller, &agent.agent_id, "").is_ok(),
            });
            let fields = row.as_object_mut().expect("a row is an object");
            if let Some(parent) = &agent.parent {
                fields.insert(
                    "startedBy".into(),
                    names
                        .get(parent)
                        .cloned()
                        .unwrap_or_else(|| parent.clone())
                        .into(),
                );
            }
            if agent.agent_id == caller {
                fields.insert("you".into(), true.into());
            }
            if let Some(model) = &agent.model {
                fields.insert("model".into(), model.clone().into());
            }
            if let Some(host) = host {
                fields.insert("host".into(), host.into());
            }
            if project.is_none() {
                fields.insert("project".into(), agent.project.clone().into());
            }
            row
        })
        .collect()
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
    let wanted: Vec<String> = match request.params.get("agents") {
        Some(Value::Array(agents)) if !agents.is_empty() => agents
            .iter()
            .map(|agent| agent.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or("agents must be names or ids")?,
        _ => return Err("agents must list the agents to close".into()),
    };
    let (here, request, caller_id) = (core.clone(), request.clone(), caller.clone());
    let (mut closed, remote, mut refused) = blocking(move || {
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
                    let name = target.name.unwrap_or_else(|| target.agent_id.clone());
                    closing.push((target.agent_id, name));
                }
                Err(why) => refused.push(json!({ "agent": to, "reason": why })),
            }
        }
        let ids: Vec<String> = closing.iter().map(|(id, _)| id.clone()).collect();
        if !ids.is_empty() {
            super::workspace::close(&core, &ids)?;
        }
        let closed: Vec<String> = closing.into_iter().map(|(_, name)| name).collect();
        Ok((closed, remote, refused))
    })
    .await
    .map_err(|error| error.to_string())?;
    for agent_id in remote {
        match super::hosts::route::close(core, &caller, &agent_id).await {
            Ok(name) => closed.push(name),
            Err(why) => refused.push(json!({ "agent": agent_id, "reason": why })),
        }
    }
    let mut result = json!({ "closed": closed });
    if !refused.is_empty() {
        result["refused"] = refused.into();
    }
    Ok(result)
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
        let name = target.name.clone().unwrap_or(target.agent_id.clone());
        return Err(format!(
            "@{waiter} is waiting on {name}, so closing it is not yours to do yet"
        ));
    }
    let led = core.names().leads_of(&target.agent_id);
    if led.iter().any(|lead| lead == caller) {
        return Ok(target);
    }
    let orphan = !led.is_empty() && !led.iter().any(|lead| core.agent_now(lead).open());
    if orphan {
        return Ok(target);
    }
    let name = target.name.clone().unwrap_or(target.agent_id.clone());
    if !(super::chief::is_chief(core, caller) && target.project == project) {
        return Err(format!(
            "{name} is the person's, or its lead or Auto still runs, so closing it is theirs to do"
        ));
    }
    if let Some(why) = kept_from_chief(core, &target.agent_id) {
        return Err(format!(
            "{name} {why}, so closing it is not yours to do yet"
        ));
    }
    Ok(target)
}

/// Why a project's chief may not close `agent_id`, an agent of its project it does not lead: it is
/// still at work or waiting on something, it holds work not yet committed, landed or pushed, or the
/// person wrote to it within the hour. None when it is finished and free to close.
fn kept_from_chief(core: &Core, agent_id: &str) -> Option<String> {
    let now = core.agent_now(agent_id);
    if now.at_work() {
        return Some("is still working".into());
    }
    if now.waits_on_something() {
        return Some("waits on an answer or a message".into());
    }
    if let Some(turn) = super::turn_state::now(core, agent_id) {
        let why = match turn.state {
            TurnPhase::Done | TurnPhase::Stopped => None,
            TurnPhase::Working => Some("is still working".to_owned()),
            TurnPhase::NeedsYou => Some("waits on the person".to_owned()),
            TurnPhase::Failed => Some("ended its turn on a failure".to_owned()),
            TurnPhase::Waiting => Some(match turn.wait {
                Some(wait) => format!("waits on {}", wait.what),
                None => "waits on something set to wake it".to_owned(),
            }),
            TurnPhase::Unknown => Some("is in a state this core does not know".to_owned()),
        };
        if why.is_some() {
            return why;
        }
        match turn.done.map(|done| done.changes) {
            Some(WorkAt::Uncommitted) => return Some("has uncommitted changes".into()),
            Some(WorkAt::Committed) => return Some("has unlanded commits".into()),
            _ => {}
        }
    }
    if let Some(why) = super::spawn::held_work(core, agent_id) {
        return Some(why.into());
    }
    if unpushed(core, agent_id) {
        return Some("has unpushed commits".into());
    }
    let wrote = core.seen().person_at(agent_id)?;
    let now = epoch_ms();
    (now.saturating_sub(wrote) < PERSON_KEEPS_MS).then(|| {
        format!(
            "was written to by the person {}",
            since(Some(wrote), now).unwrap_or_default()
        )
    })
}

/// How long an agent the person wrote to stays theirs alone to close.
const PERSON_KEEPS_MS: u64 = 60 * 60 * 1000;

/// Whether a commit naming the agent is on its project's branch here but not on that branch's
/// remote yet.
fn unpushed(core: &Core, agent_id: &str) -> bool {
    let Some(agent) = core.workspaces().agent(agent_id) else {
        return false;
    };
    let repo = (agent.worktree.as_ref()).map_or(Path::new(&agent.project), |worktree| {
        worktree.repo.as_path()
    });
    let Some(branch) = super::landing::project_branch(repo) else {
        return false;
    };
    let Some(remote) = super::landing::remote_of(repo, &branch) else {
        return false;
    };
    let range = format!("{remote}..refs/heads/{branch}");
    let grep = format!("--grep={agent_id}");
    Git::quick(repo)
        .text(&[
            "log",
            "-n",
            "1",
            "--fixed-strings",
            &grep,
            "--format=%H",
            &range,
        ])
        .is_ok_and(|found| !found.trim().is_empty())
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
        ([], [], []) => Err(format!(
            "No open agent is called {wanted}; agent_list shows the agents you can message. One you started that closed is reopened by agent_send; any other is found by agent_sessions, or who_wrote for its commit, and reopened by agent_spawn with resume set to its sessionId"
        )),
        ([], [], elsewhere) => Err(format!(
            "No agent in your project or on your team is called {wanted}. {}; pass its agent id if you mean it",
            elsewhere
                .iter()
                .map(|agent| format!("{wanted} ({}) is in {}", agent.agent_id, agent.project))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        (_, many, _) => Err(format!(
            "More than one agent is called {wanted}: {}. Pass the agent id instead",
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

/// Whether a bare name from `caller` may mean `agent`: it works in the
/// caller's project, or on the caller's team on any host.
pub(crate) fn reaches(core: &Core, agent: &Listed, project: &str, caller: &str) -> bool {
    let top = match (&agent.reach, &agent.parent) {
        (Reach::Remote, Some(parent)) => core.names().team_of(parent),
        _ => core.names().team_of(&agent.agent_id),
    };
    agent.project == project || top == core.names().team_of(caller)
}
