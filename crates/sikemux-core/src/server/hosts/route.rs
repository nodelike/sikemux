//! Agents working across a link. A message, a report, a question or an
//! answer for an agent on another host goes to that host, which hands it on
//! under the same rules as its own agents' messages. What each agent of a
//! team does is told to the hosts its teammates run on, which journal it for
//! `agent_wait` and show it.

use crate::server::changed::What;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

use sikemux_wire::protocol::AgentStatus;

use super::super::connection::blocking;
use super::super::follow::{self, Happened};
use super::super::messages::{self, Carried, MessageKind};
use super::super::names::Worker;
use super::super::roster::{self, Reach};
use super::super::{questions, Core, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::team::{Foreign, Role};
use super::{ask, call, call_id, goes_now, name_of, post, Failed, Sending};

const ASK_WAIT: Duration = Duration::from_secs(30);
/// How often a host hears that an agent of a team did other work than
/// messaging, which lets messages held between it and a teammate go.
const WORKED_EVERY_MS: u64 = 30_000;
/// How far up a chain of leads to look.
const MAX_CHAIN: usize = 8;

/// Names this host keeps for agents on linked hosts while they run there.
pub(crate) fn pin(core: &Core) {
    core.names.pin(core.hosts.team.ids());
}

/// The hosts that know `agent_id`, an agent here, as one of a team's: the one
/// its lead runs on, and those its workers run on.
fn watchers(core: &Core, agent_id: &str) -> HashSet<String> {
    let mut hosts = HashSet::new();
    let team = core.hosts.team.all();
    if team.is_empty() {
        return hosts;
    }
    let mut current = agent_id.to_owned();
    for _ in 0..MAX_CHAIN {
        let Some(parent) = core.names.get(&current).and_then(|named| named.parent) else {
            break;
        };
        if let Some(foreign) = team.iter().find(|agent| agent.agent_id == parent) {
            hosts.insert(foreign.host.clone());
            break;
        }
        current = parent;
    }
    hosts.extend(
        team.iter()
            .filter(|agent| agent.parent.as_deref() == Some(agent_id))
            .map(|agent| agent.host.clone()),
    );
    hosts
}

/// The host the lead at the top of `agent_id`'s team runs on, when that is
/// another host.
pub(crate) fn lead_host(core: &Core, agent_id: &str) -> Option<String> {
    let mut current = agent_id.to_owned();
    for _ in 0..MAX_CHAIN {
        let parent = core.names.get(&current)?.parent?;
        if let Some(foreign) = core.hosts.team.get(&parent) {
            return (foreign.role == Role::Lead).then_some(foreign.host);
        }
        current = parent;
    }
    None
}

/// The agent here as a teammate on another host shows it.
fn shown(foreign: &Foreign, core: &Core) -> String {
    format!("{} on {}", foreign.name, name_of(core, &foreign.host))
}

fn status_of(happened: Happened) -> AgentStatus {
    match happened {
        Happened::Working => AgentStatus::Working,
        Happened::Done => AgentStatus::Done,
        Happened::Stopped | Happened::Failed => AgentStatus::Idle,
        Happened::NeedsYou => AgentStatus::Blocked,
        Happened::Gone => AgentStatus::Stopped,
    }
}

/// What a teammate on another host hears when an agent here stopped running but may run
/// again, such as after a crash. A host that does not know it only takes the status.
const ENDED: &str = "agent.ended";
/// What it hears when the agent's chat went to sleep, to wake when it is written to.
const ASLEEP: &str = "agent.asleep";

/// Something happened to an agent here: the hosts its teammates run on hear. An agent that
/// stopped running stays their teammate until it is closed.
pub(crate) fn happened(core: &Core, agent_id: &str, happened: Happened) {
    let kind = match happened {
        Happened::Gone => ENDED,
        happened => happened.kind(),
    };
    tell_watchers(core, agent_id, kind, status_of(happened), false);
}

/// An agent here went to sleep.
pub(crate) fn asleep(core: &Core, agent_id: &str) {
    tell_watchers(core, agent_id, ASLEEP, AgentStatus::Asleep, false);
}

/// The person closed these agents here: their teammates elsewhere let them go, and so does
/// this host.
pub(crate) fn closed(core: &Core, agent_ids: &[&String]) {
    for agent_id in agent_ids {
        tell_watchers(
            core,
            agent_id,
            Happened::Gone.kind(),
            AgentStatus::Stopped,
            true,
        );
        local_gone(core, agent_id);
    }
}

fn tell_watchers(core: &Core, agent_id: &str, kind: &str, status: AgentStatus, keep: bool) {
    for host in watchers(core, agent_id) {
        post(core, &host, {
            let sending = Sending::new(
                asks::Happened::METHOD,
                params_of::<asks::Happened>(&HappenedParams {
                    agent_id: agent_id.to_owned(),
                    kind: kind.to_owned(),
                    status,
                }),
            )
            .keyed(format!("happened:{agent_id}"))
            .about(agent_id);
            if keep {
                sending.kept()
            } else {
                sending
            }
        });
    }
}

/// An agent here was closed: its checkout's record goes, and so does a lead on another host
/// that has no other worker here.
fn local_gone(core: &Core, agent_id: &str) {
    core.hosts.team.remove_tree(agent_id);
    let Some(lead) = core.names.get(agent_id).and_then(|named| named.parent) else {
        return;
    };
    let Some(foreign) = core
        .hosts
        .team
        .get(&lead)
        .filter(|lead| lead.role == Role::Lead)
    else {
        return;
    };
    let open = core.open_agents();
    let others = open.iter().any(|other| {
        other != agent_id
            && core
                .names
                .get(other)
                .is_some_and(|named| named.parent.as_deref() == Some(&foreign.agent_id))
    });
    if !others {
        core.hosts.team.remove(&foreign.agent_id);
        pin(core);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HappenedParams {
    agent_id: String,
    kind: String,
    status: AgentStatus,
}

pub(super) fn happened_here(core: &Core, host: &str, params: HappenedParams) -> CoreResult<Value> {
    let Some(foreign) = core
        .hosts
        .team
        .get(&params.agent_id)
        .filter(|foreign| foreign.host == host)
    else {
        return Ok(Value::Null);
    };
    let happened = match params.kind.as_str() {
        ENDED => Some(Happened::Gone),
        ASLEEP => None,
        kind => Happened::of(kind),
    };
    let closed = params.kind == Happened::Gone.kind();
    core.hosts.team.update(&foreign.agent_id, |foreign| {
        foreign.status = params.status;
        foreign.active_at = Some(epoch_ms());
    });
    if let Some(happened) = happened {
        core.harness
            .note_agent(&foreign.project, happened.kind(), &foreign.agent_id);
        if foreign.role == Role::Lead
            && matches!(
                happened,
                Happened::Done | Happened::Stopped | Happened::Failed
            )
        {
            core.note_linked_turn_ended(&foreign.agent_id);
        }
        if closed && !super::super::board::elsewhere::holds(core, &foreign.agent_id) {
            core.hosts.team.remove(&foreign.agent_id);
            pin(core);
        }
    }
    core.changed(What::Agents);
    super::changed(core);
    Ok(Value::Null)
}

/// The agents here that `host` knows as teammates, with what each is doing,
/// so it can tell which of them still run after the link was down.
pub(crate) fn resync(core: &Arc<Core>, host: &str) {
    let foreign = core.hosts.team.on(host);
    if foreign.is_empty() {
        return;
    }
    let (core, host) = (core.clone(), host.to_owned());
    tokio::spawn(async move {
        let listed = {
            let core = core.clone();
            blocking(move || Ok(roster::listed(&core))).await
        };
        let Ok(listed) = listed else {
            return;
        };
        let mut watched: HashSet<String> = HashSet::new();
        for agent in &foreign {
            match agent.role {
                Role::Worker => watched.extend(agent.parent.clone()),
                Role::Lead => watched.extend(
                    listed
                        .iter()
                        .filter(|local| local.reach != Reach::Remote)
                        .filter(|local| lead_host(&core, &local.agent_id).as_deref() == Some(&host))
                        .map(|local| local.agent_id.clone()),
                ),
            }
        }
        let agents: Vec<Standing> = watched
            .iter()
            .filter_map(|agent_id| {
                let status = match listed.iter().find(|local| &local.agent_id == agent_id) {
                    Some(local) => local.standing(&core),
                    None if super::super::board::elsewhere::leads_a_run(&core, agent_id) => {
                        AgentStatus::Working
                    }
                    None => {
                        core.names.get(agent_id)?;
                        AgentStatus::Asleep
                    }
                };
                Some(Standing {
                    agent_id: agent_id.clone(),
                    status,
                })
            })
            .collect();
        post(
            &core,
            &host,
            Sending::new(
                asks::Team::METHOD,
                params_of::<asks::Team>(&TeamParams { agents }),
            )
            .keyed("team".to_owned()),
        );
    });
}

/// Back in reach of `host`, the workers this host started there are asked after, so commits
/// they sent while this host was away and that never landed are sent again. A host too old to
/// be asked only pushes them.
pub(crate) fn ask_after_workers(core: &Arc<Core>, host: &str) {
    let workers: Vec<Held> = (core.hosts.team.on(host).into_iter())
        .filter(|foreign| foreign.role == Role::Worker && foreign.pending.is_none())
        .map(|foreign| Held {
            agent_id: foreign.agent_id,
            landed: foreign.landed_head,
        })
        .collect();
    if workers.is_empty() {
        return;
    }
    let (core, host) = (core.clone(), host.to_owned());
    tokio::spawn(async move {
        let params = params_of::<asks::Workers>(&WorkersParams { workers });
        if let Err(error) = ask(&core, &host, asks::Workers::METHOD, params, None, ASK_WAIT).await {
            eprintln!(
                "sikemux core: could not ask {} after its workers: {error}",
                name_of(&core, &host)
            );
        }
    });
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Held {
    agent_id: String,
    landed: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct WorkersParams {
    workers: Vec<Held>,
}

/// The host that leads workers here says which of their commits it has: any it lacks, with
/// none waiting to go, are sent again.
pub(super) fn workers_here(
    core: &Core,
    host: &str,
    params: WorkersParams,
) -> Result<Value, String> {
    let Some(core) = core.arc() else {
        return Ok(Value::Null);
    };
    for held in params.workers {
        if lead_host(&core, &held.agent_id).as_deref() != Some(host) {
            continue;
        }
        let Some(tree) = core.hosts.team.tree(&held.agent_id) else {
            continue;
        };
        let waiting = (core.hosts.outbox)
            .pulled_body(host, |post| {
                post.params["agentId"] == held.agent_id.as_str()
            })
            .is_some();
        if waiting || tree.sent.is_none() || tree.sent == held.landed {
            continue;
        }
        core.hosts.team.tree_unsent(&held.agent_id);
        let core = core.clone();
        tokio::spawn(async move { super::spawn::send_commits(&core, &held.agent_id).await });
    }
    Ok(Value::Null)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Standing {
    agent_id: String,
    status: AgentStatus,
}

#[derive(Serialize, Deserialize)]
pub(super) struct TeamParams {
    agents: Vec<Standing>,
}

/// What `host` says its agents of shared teams are doing. Those it no longer
/// lists have stopped running there.
pub(super) fn team_here(core: &Core, host: &str, params: TeamParams) -> CoreResult<Value> {
    let present: HashMap<String, AgentStatus> = params
        .agents
        .into_iter()
        .map(|standing| (standing.agent_id, standing.status))
        .collect();
    for foreign in core.hosts.team.on(host) {
        match present.get(&foreign.agent_id) {
            None if foreign.pending.is_some() => {}
            Some(status) => {
                core.hosts
                    .team
                    .update(&foreign.agent_id, |foreign| foreign.status = *status);
            }
            None => {
                core.harness
                    .note_agent(&foreign.project, Happened::Gone.kind(), &foreign.agent_id);
                core.hosts.team.remove(&foreign.agent_id);
                if let (Role::Worker, Some(lead)) = (foreign.role, foreign.parent.clone()) {
                    let host = name_of(core, host);
                    notice(core, lead, format!("{} no longer runs on {host}: it stopped while {host} was out of reach, so it reports no more. What it committed and did not report yet stays in its checkout there; start another agent if the work is not done.", foreign.name));
                }
            }
        }
    }
    pin(core);
    core.changed(What::Agents);
    super::changed(core);
    Ok(Value::Null)
}

pub(super) fn notice(core: &Core, to: String, text: String) {
    let Some(core) = core.arc() else {
        return;
    };
    tokio::spawn(async move {
        let _ = messages::from_hosts(&core, to, text).await;
    });
}

/// The agents here whose lead is `lead`, an agent on another host.
fn workers_of(core: &Core, lead: &str) -> Vec<String> {
    core.running_agents()
        .into_iter()
        .filter(|agent_id| {
            core.names
                .get(agent_id)
                .is_some_and(|named| named.parent.as_deref() == Some(lead))
        })
        .collect()
}

/// `host` has been out of reach for a while: the leads of its workers, and
/// the workers here of its leads, hear of it once.
pub(super) fn out_of_reach(core: &Core, host: &str) {
    let name = name_of(core, host);
    for foreign in core.hosts.team.on(host) {
        match foreign.role {
            Role::Worker => {
                if let Some(lead) = foreign.parent.clone() {
                    notice(core, lead, format!("{} has been out of reach for a few minutes, so {} cannot hear you now. What you send it waits and goes once {name} is back, and so does its report to you. Its work goes on there while {name} keeps running; carry on with your own.", name, foreign.name));
                }
            }
            Role::Lead => {
                for worker in workers_of(core, &foreign.agent_id) {
                    notice(core, worker, format!("{name}, where your lead {} runs, has been out of reach for a few minutes. What you send it, and your report, wait and go once {name} is back; carry on with your task.", foreign.name));
                }
            }
        }
    }
}

/// `host` came back after its teammates here heard it was out of reach.
pub(super) fn back_in_reach(core: &Core, host: &str) {
    let name = name_of(core, host);
    for foreign in core.hosts.team.on(host) {
        let to = match foreign.role {
            Role::Worker => foreign.parent.clone().into_iter().collect(),
            Role::Lead => workers_of(core, &foreign.agent_id),
        };
        for agent in to {
            notice(
                core,
                agent,
                format!(
                    "{name} is back in reach, and what waited for {} has gone to it.",
                    shown(&foreign, core)
                ),
            );
        }
    }
}

/// `host` is no longer linked: its agents are gone from this host's teams,
/// and their teammates here hear why.
pub(super) fn host_gone(core: &Core, host: &str, why: &str) {
    let name = name_of(core, host);
    let gone = core.hosts.team.remove_host(host);
    if gone.is_empty() {
        return;
    }
    pin(core);
    for foreign in gone {
        core.harness
            .note_agent(&foreign.project, Happened::Gone.kind(), &foreign.agent_id);
        match foreign.role {
            Role::Worker => {
                if let Some(lead) = foreign.parent.clone() {
                    notice(core, lead, format!("{} ran on {name}, which is no longer linked with this host ({why}), so Sikemux can no longer reach it. Its work there goes on without you, and what it has not reported or committed back since its last report stays there.", foreign.name));
                }
            }
            Role::Lead => {
                for worker in workers_of(core, &foreign.agent_id) {
                    notice(core, worker, format!("Your lead {} ran on {name}, which is no longer linked with this host ({why}), so your answers can no longer reach it. Finish your task and stop, or ask the person with ask.", foreign.name));
                }
            }
        }
    }
    core.changed(What::Agents);
}

/// Hands a letter to its agent on the host it runs on, or leaves it waiting
/// for that host to come back.
pub(crate) async fn deliver(
    core: &Arc<Core>,
    agent_id: &str,
    carried: Carried,
) -> Result<Value, String> {
    let foreign = core
        .hosts
        .team
        .get(agent_id)
        .ok_or("Sikemux no longer knows which host that agent runs on")?;
    let host_name = name_of(core, &foreign.host);
    let shown = shown(&foreign, core);
    let keep = carried.kind == MessageKind::Report;
    let from = carried.from_id.clone();
    let params = serde_json::to_value(&carried).map_err(|error| error.to_string())?;
    let id = call_id();
    if goes_now(core, &foreign.host, &from) {
        match call(
            core,
            &foreign.host,
            "deliver",
            params.clone(),
            None,
            ASK_WAIT,
            &id,
        )
        .await
        {
            Ok(mut answer) => {
                answer["to"] = shown.into();
                answer["host"] = host_name.into();
                return Ok(answer);
            }
            Err(error) if !error.passing() => return Err(error.to_string()),
            Err(_) => {}
        }
    }
    let reachable = core.hosts.reachable(&foreign.host);
    post(core, &foreign.host, {
        let sending = Sending::new("deliver", params)
            .from(&from)
            .about(&from)
            .call(id);
        if keep {
            sending.kept()
        } else {
            sending
        }
    });
    let note = if reachable {
        format!("This goes to {shown} after what was sent to {host_name} before it.")
    } else {
        format!("{host_name} is out of reach now, so this waits and goes to {shown} once {host_name} is back.")
    };
    Ok(json!({
        "to": shown,
        "agentId": agent_id,
        "host": host_name,
        "delivery": "waiting",
        "note": note,
    }))
}

pub(super) async fn deliver_here(
    core: &Arc<Core>,
    host: &str,
    params: Value,
) -> Result<Value, String> {
    let carried: Carried = serde_json::from_value(params).map_err(|error| error.to_string())?;
    let teammate = core
        .hosts
        .team
        .get(&carried.from_id)
        .is_some_and(|foreign| foreign.host == host);
    let answer_copy = carried.answered.is_some()
        && core.hosts.team.on(host).iter().any(|foreign| {
            foreign.role == Role::Worker && foreign.parent.as_deref() == Some(carried.to.as_str())
        });
    if !teammate && !answer_copy {
        return Err("that agent is not one of a team this host shares with yours".into());
    }
    let landed = if carried.kind == MessageKind::Report {
        let mut landed = None;
        core.hosts
            .team
            .update(&carried.from_id, |foreign| landed = foreign.landed.take());
        landed
    } else {
        None
    };
    let host = teammate.then(|| name_of(core, host));
    messages::from_host(core, host.as_deref(), carried, landed).await
}

/// What `caller` reads of `agent_id`, an agent on another host.
pub(crate) async fn read(
    core: &Arc<Core>,
    caller: &str,
    agent_id: &str,
    cursor: Option<String>,
) -> Result<Value, String> {
    let foreign = core
        .hosts
        .team
        .get(agent_id)
        .ok_or("Sikemux no longer knows which host that agent runs on")?;
    let params = params_of::<asks::Read>(&ReadParams {
        caller: caller.to_owned(),
        agent_id: agent_id.to_owned(),
        cursor,
    });
    let mut answer = ask(
        core,
        &foreign.host,
        asks::Read::METHOD,
        params,
        None,
        ASK_WAIT,
    )
    .await
    .map_err(|error| match error {
        Failed::Unreachable(why) | Failed::Unknown(why) | Failed::Retry(why) => {
            format!("{} cannot be read now: {why}", shown(&foreign, core))
        }
        Failed::Refused(why) | Failed::NotLinked(why) => why,
    })?;
    answer["name"] = shown(&foreign, core).into();
    answer["host"] = name_of(core, &foreign.host).into();
    Ok(answer)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadParams {
    caller: String,
    agent_id: String,
    cursor: Option<String>,
}

pub(super) async fn read_here(
    core: &Arc<Core>,
    host: &str,
    params: ReadParams,
) -> Result<Value, String> {
    if core
        .hosts
        .team
        .get(&params.caller)
        .is_none_or(|foreign| foreign.host != host)
    {
        return Err(
            "only an agent of a team this host shares with yours may read its agents".into(),
        );
    }
    follow::read_here(core, params.agent_id, params.cursor).await
}

/// Asks the host a team's lead runs on to name a worker `lead` starts here,
/// counting the whole team there against the caps.
pub(crate) async fn hire(
    core: &Arc<Core>,
    host: &str,
    agent_id: &str,
    lead: &str,
    provider: &str,
    reports: bool,
) -> Result<String, String> {
    let params = params_of::<asks::Hire>(&HireParams {
        agent_id: agent_id.to_owned(),
        lead: lead.to_owned(),
        provider: provider.to_owned(),
        reports,
    });
    let id = call_id();
    let answer = call(core, host, asks::Hire::METHOD, params, None, ASK_WAIT, &id)
        .await
        .map_err(|error| {
            if let Failed::Unknown(_) = error {
                forget_hired(core, host, agent_id);
            }
            match error {
                error if error.passing() => format!("{}, where your team's lead runs, did not answer, and Sikemux counts your team there; try again once it is back", name_of(core, host)),
                error => error.to_string(),
            }
        })?;
    answer["name"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the lead's host named no worker".into())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HireParams {
    agent_id: String,
    lead: String,
    provider: String,
    reports: bool,
}

pub(super) fn hire_here(core: &Core, host: &str, params: HireParams) -> CoreResult<Value> {
    let lead = core
        .hosts
        .team
        .get(&params.lead)
        .filter(|lead| lead.host == host)
        .ok_or("that lead is not one of a team this host shares with yours")?;
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    let named = core
        .names
        .hire(
            Worker {
                agent_id: &params.agent_id,
                lead: &params.lead,
                project: None,
                provider: &params.provider,
                session: None,
                reports: params.reports,
            },
            core.open_agents(),
        )
        .map_err(String::from)?;
    core.hosts.team.add(Foreign {
        agent_id: params.agent_id,
        host: host.to_owned(),
        name: named.name.clone(),
        cli: params.provider,
        project: named.project,
        parent: Some(params.lead),
        role: Role::Worker,
        status: AgentStatus::Starting,
        active_at: Some(epoch_ms()),
        repo: lead.repo,
        landed: None,
        landed_head: None,
        branch: None,
        pending: None,
    });
    pin(core);
    super::changed(core);
    Ok(json!({ "name": named.name }))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AboutAgent {
    pub(super) agent_id: String,
}

/// A worker `host` may have named for an agent here, which will not start.
pub(crate) fn forget_hired(core: &Core, host: &str, agent_id: &str) {
    post(
        core,
        host,
        Sending::new(
            asks::Forget::METHOD,
            params_of::<asks::Forget>(&AboutAgent {
                agent_id: agent_id.to_owned(),
            }),
        )
        .about(agent_id),
    );
}

/// A worker this host named for another host never started there.
pub(super) fn forget_here(core: &Core, host: &str, params: AboutAgent) -> CoreResult<Value> {
    let agent_id = params.agent_id.as_str();
    if core
        .hosts
        .team
        .get(agent_id)
        .is_some_and(|foreign| foreign.host == host)
    {
        core.hosts.team.remove(agent_id);
        core.names.forget(agent_id);
        pin(core);
        super::changed(core);
    }
    Ok(Value::Null)
}

/// Stops a worker this host started on another, whenever that host next hears, and
/// forgets it here. Whatever it sends after this is not taken.
pub(crate) fn stop_worker(core: &Core, agent_id: &str) {
    let Some(worker) = core
        .hosts
        .team
        .get(agent_id)
        .filter(|foreign| foreign.role == Role::Worker)
    else {
        return;
    };
    core.hosts.team.remove(agent_id);
    core.harness
        .note_agent(&worker.project, Happened::Gone.kind(), agent_id);
    pin(core);
    post(
        core,
        &worker.host,
        Sending::new(
            asks::Stop::METHOD,
            params_of::<asks::Stop>(&AboutAgent {
                agent_id: agent_id.to_owned(),
            }),
        )
        .kept()
        .about(agent_id),
    );
    super::changed(core);
}

pub(super) fn stop_here(core: &Core, host: &str, params: AboutAgent) -> CoreResult<Value> {
    let agent_id = params.agent_id.as_str();
    if lead_host(core, agent_id).as_deref() != Some(host) {
        return Ok(Value::Null);
    }
    if core.chats.stop(agent_id) {
        core.note_agent_gone(agent_id);
    }
    local_gone(core, agent_id);
    Ok(Value::Null)
}

/// An agent here did work besides messaging: hosts where its teammates run
/// let go of messages held between them, at most every so often.
pub(crate) fn worked(core: &Core, agent_id: &str) {
    if core.hosts.team.ids().is_empty() || !core.hosts.worked_due(agent_id, WORKED_EVERY_MS) {
        return;
    }
    for host in watchers(core, agent_id) {
        post(
            core,
            &host,
            Sending::new(
                asks::Worked::METHOD,
                params_of::<asks::Worked>(&AboutAgent {
                    agent_id: agent_id.to_owned(),
                }),
            )
            .keyed(format!("worked:{agent_id}"))
            .about(agent_id),
        );
    }
}

pub(super) fn worked_here(core: &Core, host: &str, params: AboutAgent) -> CoreResult<Value> {
    let agent_id = params.agent_id.as_str();
    if core
        .hosts
        .team
        .get(agent_id)
        .is_some_and(|foreign| foreign.host == host)
    {
        messages::worked(core, agent_id);
    }
    Ok(Value::Null)
}

/// Puts a question an agent here asked to the person on the host its team's
/// lead runs on.
pub(crate) fn relay_question(core: &Core, host: &str, question: Value) {
    let asker = question["agentId"].as_str().unwrap_or_default().to_owned();
    post(
        core,
        host,
        Sending::new("question", question).kept().about(&asker),
    );
}

/// Sends the person's answer to a question back to the host its asker runs on.
pub(crate) fn relay_answer(core: &Core, host: &str, answer: Value, asked: Value) {
    post(
        core,
        host,
        Sending::new("answer", answer).kept().back(asked),
    );
}

#[derive(Serialize, Deserialize)]
pub(super) struct ClosedParams {
    pub(super) id: String,
}

/// A question put to the person elsewhere no longer waits.
pub(crate) fn relay_closed(core: &Core, host: &str, id: &str) {
    post(
        core,
        host,
        Sending::new(
            asks::QuestionClosed::METHOD,
            params_of::<asks::QuestionClosed>(&ClosedParams { id: id.to_owned() }),
        ),
    );
}

/// A lead here passes a question its worker on another host asked it on to
/// the person.
pub(crate) async fn pass_on(
    core: &Arc<Core>,
    lead: &str,
    worker: &Foreign,
    text: String,
    choices: Value,
    raise: questions::Raise,
) -> Result<Value, String> {
    let params = params_of::<asks::PassOn>(&PassOnParams {
        lead: lead.to_owned(),
        worker: worker.agent_id.clone(),
        text,
        choices,
        raise,
    });
    ask(
        core,
        &worker.host,
        asks::PassOn::METHOD,
        params,
        None,
        ASK_WAIT,
    )
    .await
    .map_err(|error| error.to_string())
}

#[derive(Serialize, Deserialize)]
pub(super) struct PassOnParams {
    lead: String,
    worker: String,
    text: String,
    choices: Value,
    raise: questions::Raise,
}

pub(super) fn pass_on_here(core: &Core, host: &str, params: PassOnParams) -> Result<Value, String> {
    let lead = params.lead.as_str();
    if core
        .hosts
        .team
        .get(lead)
        .is_none_or(|foreign| foreign.host != host)
    {
        return Err("that lead is not one of a team this host shares with yours".into());
    }
    questions::pass_on_from(
        core,
        lead,
        &params.worker,
        &params.text,
        &params.choices,
        params.raise,
    )
}
