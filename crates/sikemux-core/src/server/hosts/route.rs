//! Agents working across a link. A message, a report, a question or an
//! answer for an agent on another host goes to that host, which hands it on
//! under the same rules as its own agents' messages. What each agent of a
//! team does is told to the hosts its teammates run on, which journal it for
//! `agent_wait` and show it.

use crate::server::activity::ActivityCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use sikemux_view::inbox::InboxKind;
use sikemux_wire::protocol::AgentStatus;

use super::super::connection::blocking;
use super::super::follow::{self, Happened};
use super::super::messages::{self, Carried, MessageKind};
use super::super::names::Worker;
use super::super::roster::{self, Reach};
use super::super::{inbox, questions, Core, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::team::{Foreign, Role, Said};
use super::{ask, call, call_id, goes_now, name_of, post, Failed, Sending};

const ASK_WAIT: Duration = Duration::from_secs(30);
/// How long changes to the agents here gather before the hosts their teammates run on hear.
const ROWS_GATHER: Duration = Duration::from_millis(200);
/// How often a host hears that an agent of a team did other work than
/// messaging, which lets messages held between it and a teammate go.
const WORKED_EVERY_MS: u64 = 30_000;
/// How far up a chain of leads to look.
const MAX_CHAIN: usize = 8;

/// Names this host keeps for agents on linked hosts while they run there.
pub(crate) fn pin(core: &Core) {
    core.names().pin(core.hosts().team.ids());
}

/// The hosts that know `agent_id`, an agent here, as one of a team's: the one
/// its lead runs on, and those its workers run on.
fn watchers(core: &Core, agent_id: &str) -> HashSet<String> {
    let mut hosts = HashSet::new();
    let team = core.hosts().team.all();
    if team.is_empty() {
        return hosts;
    }
    let mut current = agent_id.to_owned();
    for _ in 0..MAX_CHAIN {
        let Some(parent) = core.names().get(&current).and_then(|named| named.parent) else {
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
        let parent = core.names().get(&current)?.parent?;
        if let Some(foreign) = core.hosts().team.get(&parent) {
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
/// What it hears when the agent's row among the agents here changed. A host that does not
/// know it only takes the status.
const SHOWN: &str = "agent.shown";

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

/// Something the agents here show changed: the hosts their teammates run on hear each row
/// that differs from what they were last told.
pub(crate) fn rows_moved(core: &Core) {
    core.hosts().rows.now();
}

/// Tells the changed rows once the changes have gathered.
pub(super) fn tell_rows_soon(core: &Arc<Core>) {
    if core.hosts().team.is_empty() || core.hosts().rows_due.swap(true, Ordering::AcqRel) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(ROWS_GATHER).await;
        core.hosts().rows_due.store(false, Ordering::Release);
        let _ = blocking(move || {
            tell_rows(&core);
            Ok(())
        })
        .await;
    });
}

/// What an inbox holds about an agent that only the host it runs on can tell: what it waits
/// on the person for there and how its turn went. Its questions and held actions reach its
/// lead's host as calls of their own.
const TOLD: [InboxKind; 6] = [
    InboxKind::Permission,
    InboxKind::Input,
    InboxKind::Changes,
    InboxKind::Failed,
    InboxKind::Finished,
    InboxKind::Working,
];

/// What this host shows of every agent here, by its id, for the hosts its teammates run on.
fn said(core: &Core) -> HashMap<String, Said> {
    let (chats, sessions) = core.ranked_infos();
    let listed = inbox::view(core, &chats, &sessions);
    let attentions = core.chats().attentions();
    (listed.agents.into_iter())
        .chain(listed.kept)
        .map(|shown| {
            let about = |agent: Option<&String>| agent == Some(&shown.agent_id);
            let said = Said {
                items: (listed.items.iter())
                    .filter(|item| about(item.agent_id.as_ref()) && TOLD.contains(&item.kind))
                    .cloned()
                    .collect(),
                attentions: (attentions.iter())
                    .filter(|attention| about(Some(&attention.agent_id)))
                    .cloned()
                    .collect(),
                shown,
            };
            (said.shown.agent_id.clone(), said)
        })
        .collect()
}

fn tell_rows(core: &Core) {
    let mut told = core.hosts().rows_told.locked();
    let mut watched = HashSet::new();
    for (agent_id, said) in said(core) {
        for host in watchers(core, &agent_id) {
            let key = (host, agent_id.clone());
            if told.get(&key) != Some(&said) {
                told.insert(key.clone(), said.clone());
                let status = said.shown.status;
                tell(core, &key.0, &agent_id, SHOWN, status, Some(&said), false);
            }
            watched.insert(key);
        }
    }
    told.retain(|key, _| watched.contains(key));
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
        tell(core, &host, agent_id, kind, status, None, keep);
    }
}

fn tell(
    core: &Core,
    host: &str,
    agent_id: &str,
    kind: &str,
    status: AgentStatus,
    said: Option<&Said>,
    keep: bool,
) {
    let key = match kind {
        SHOWN => "shown",
        _ => "happened",
    };
    post(core, host, {
        let sending = Sending::new(
            asks::Happened::METHOD,
            params_of::<asks::Happened>(&HappenedParams {
                agent_id: agent_id.to_owned(),
                kind: kind.to_owned(),
                status,
                said: said.cloned(),
            }),
        )
        .keyed(format!("{key}:{agent_id}"))
        .about(agent_id);
        if keep {
            sending.kept()
        } else {
            sending
        }
    });
}

/// An agent here was closed: a lead on another host that has no other worker here goes. Its
/// checkout's record stays while this host keeps the session it ran, so its lead can reopen it.
fn local_gone(core: &Core, agent_id: &str) {
    core.hosts()
        .team
        .retain_trees(|tree| core.names().ran_session(&tree.agent_id).is_some());
    let Some(lead) = core.names().get(agent_id).and_then(|named| named.parent) else {
        return;
    };
    let Some(foreign) = core
        .hosts()
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
                .names()
                .get(other)
                .is_some_and(|named| named.parent.as_deref() == Some(&foreign.agent_id))
    });
    if !others {
        core.hosts().team.remove(&foreign.agent_id);
        pin(core);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HappenedParams {
    agent_id: String,
    kind: String,
    status: AgentStatus,
    #[serde(flatten)]
    said: Option<Said>,
}

/// What the host a teammate runs on said of it: its status, and all it shows of it when it
/// sent that, with the name it goes by there once the person renamed it.
fn heard(foreign: &mut Foreign, status: AgentStatus, said: Option<Said>) {
    foreign.status = status;
    let renamed = (said.as_ref())
        .and_then(|said| said.shown.name.as_deref())
        .filter(|name| *name != foreign.name);
    if let Some(name) = renamed {
        foreign.name = name.to_owned();
    }
    if said.is_some() {
        foreign.said = said;
    }
}

/// What the host a teammate runs on said of it, and the name this host calls it by follows
/// its name there.
fn hear(core: &Core, agent_id: &str, status: AgentStatus, said: Option<Said>) {
    let mut name = None;
    core.hosts().team.update(agent_id, |foreign| {
        heard(foreign, status, said);
        name = Some(foreign.name.clone());
    });
    if let Some(name) = name.filter(|name| core.names().name(agent_id).as_ref() != Some(name)) {
        core.names().set_name(agent_id, &name);
    }
}

pub(super) fn happened_here(core: &Core, host: &str, params: HappenedParams) -> CoreResult<Value> {
    let Some(foreign) = core
        .hosts()
        .team
        .get(&params.agent_id)
        .filter(|foreign| foreign.host == host)
    else {
        return Ok(Value::Null);
    };
    let happened = match params.kind.as_str() {
        ENDED => Some(Happened::Gone),
        ASLEEP | SHOWN => None,
        kind => Happened::of(kind),
    };
    let closed = params.kind == Happened::Gone.kind();
    if params.said.is_none() {
        core.hosts().team.update(&foreign.agent_id, |foreign| {
            foreign.active_at = Some(epoch_ms());
        });
    }
    hear(core, &foreign.agent_id, params.status, params.said);
    if let Some(happened) = happened {
        core.harness()
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
            core.hosts().team.close(&foreign.agent_id);
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
    let foreign = core.hosts().team.on(host);
    if foreign.is_empty() {
        return;
    }
    let (core, host) = (core.clone(), host.to_owned());
    tokio::spawn(async move {
        let listed = {
            let core = core.clone();
            blocking(move || Ok((roster::listed(&core), said(&core)))).await
        };
        let Ok((listed, said)) = listed else {
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
                    Some(local) => crate::server::roster::standing(local, &core),
                    None if super::super::board::elsewhere::leads_a_run(&core, agent_id) => {
                        AgentStatus::Working
                    }
                    None => {
                        core.names().get(agent_id)?;
                        AgentStatus::Asleep
                    }
                };
                Some(Standing {
                    agent_id: agent_id.clone(),
                    status,
                    said: said.get(agent_id).cloned(),
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
    let workers: Vec<Held> = (core.hosts().team.on(host).into_iter())
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
        let Some(tree) = core.hosts().team.tree(&held.agent_id) else {
            continue;
        };
        let waiting = (core.hosts().outbox)
            .pulled_body(host, |post| {
                post.params["agentId"] == held.agent_id.as_str()
            })
            .is_some();
        if waiting || tree.sent.is_none() || tree.sent == held.landed {
            continue;
        }
        core.hosts().team.tree_unsent(&held.agent_id);
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
    #[serde(flatten)]
    said: Option<Said>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct TeamParams {
    agents: Vec<Standing>,
}

/// What `host` says its agents of shared teams are doing. Those it no longer
/// lists have stopped running there.
pub(super) fn team_here(core: &Core, host: &str, params: TeamParams) -> CoreResult<Value> {
    let present: HashMap<String, (AgentStatus, Option<Said>)> = params
        .agents
        .into_iter()
        .map(|standing| (standing.agent_id, (standing.status, standing.said)))
        .collect();
    let was_away = core.hosts().take_told_away(host);
    for foreign in core.hosts().team.on(host) {
        match present.get(&foreign.agent_id) {
            None if foreign.pending.is_some() => {}
            Some((status, said)) => {
                hear(core, &foreign.agent_id, *status, said.clone());
            }
            None => {
                core.harness().note_agent(
                    &foreign.project,
                    Happened::Gone.kind(),
                    &foreign.agent_id,
                );
                core.hosts().team.close(&foreign.agent_id);
                if let (Role::Worker, Some(lead)) = (foreign.role, foreign.parent.clone()) {
                    let host = name_of(core, host);
                    let why = match was_away {
                        true => format!("it stopped while {host} was out of reach"),
                        false => format!("it was closed there, or {host}'s Sikemux restarted"),
                    };
                    notice(core, lead, format!("{} is no longer open on {host} ({why}), so it reports no more. What it committed and did not report yet stays in its checkout there; start another agent if the work is not done.", foreign.name));
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
            core.names()
                .get(agent_id)
                .is_some_and(|named| named.parent.as_deref() == Some(lead))
        })
        .collect()
}

/// `host` has been out of reach for a while: the leads of its workers, and
/// the workers here of its leads, hear of it once.
pub(super) fn out_of_reach(core: &Core, host: &str) {
    let name = name_of(core, host);
    for foreign in core.hosts().team.on(host) {
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
    for foreign in core.hosts().team.on(host) {
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
    let gone = core.hosts().team.remove_host(host);
    if gone.is_empty() {
        return;
    }
    pin(core);
    for foreign in gone {
        core.harness()
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
        .hosts()
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
    let reachable = core.hosts().reachable(&foreign.host);
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
        .hosts()
        .team
        .get(&carried.from_id)
        .is_some_and(|foreign| foreign.host == host);
    let answer_copy = carried.answered.is_some()
        && core.hosts().team.on(host).iter().any(|foreign| {
            foreign.role == Role::Worker && foreign.parent.as_deref() == Some(carried.to.as_str())
        });
    if !teammate && !answer_copy {
        return Err("that agent is not one of a team this host shares with yours".into());
    }
    let landed = if carried.kind == MessageKind::Report {
        let mut landed = None;
        core.hosts()
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
        .hosts()
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
        .hosts()
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

/// Closes `agent_id`, an agent on another host that `caller` leads, as the person
/// there closing its tab would. Answers the agent as the caller knows it.
pub(crate) async fn close(
    core: &Arc<Core>,
    caller: &str,
    agent_id: &str,
) -> Result<String, String> {
    let foreign = core
        .hosts()
        .team
        .get(agent_id)
        .ok_or("Sikemux no longer knows which host that agent runs on")?;
    let params = params_of::<asks::Close>(&CloseParams {
        caller: caller.to_owned(),
        agent_id: agent_id.to_owned(),
    });
    ask(
        core,
        &foreign.host,
        asks::Close::METHOD,
        params,
        None,
        ASK_WAIT,
    )
    .await
    .map_err(|error| match error {
        Failed::Unreachable(why) | Failed::Unknown(why) | Failed::Retry(why) => {
            format!("{} cannot be closed now: {why}", shown(&foreign, core))
        }
        Failed::Refused(why) | Failed::NotLinked(why) => why,
    })?;
    Ok(shown(&foreign, core))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloseParams {
    caller: String,
    agent_id: String,
}

pub(super) fn close_here(core: &Core, host: &str, params: CloseParams) -> CoreResult<Value> {
    let from_there = core
        .hosts()
        .team
        .get(&params.caller)
        .is_some_and(|foreign| foreign.host == host);
    if !from_there
        || !core
            .names()
            .leads_of(&params.agent_id)
            .contains(&params.caller)
    {
        return Err("only the agent's lead may close it".into());
    }
    super::super::workspace::close(core, &[params.agent_id])?;
    Ok(Value::Null)
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
        .hosts()
        .team
        .get(&params.lead)
        .filter(|lead| lead.host == host)
        .ok_or("that lead is not one of a team this host shares with yours")?;
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    let named = core
        .names()
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
    core.hosts().team.add(Foreign {
        agent_id: params.agent_id,
        host: host.to_owned(),
        name: named.name.clone(),
        cli: params.provider,
        project: named.project,
        parent: Some(params.lead),
        role: Role::Worker,
        status: AgentStatus::Starting,
        said: None,
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
        .hosts()
        .team
        .get(agent_id)
        .is_some_and(|foreign| foreign.host == host)
    {
        core.hosts().team.remove(agent_id);
        core.names().forget(agent_id);
        pin(core);
        super::changed(core);
    }
    Ok(Value::Null)
}

/// Stops a worker this host started on another, whenever that host next hears, and
/// forgets it here. Whatever it sends after this is not taken.
pub(crate) fn stop_worker(core: &Core, agent_id: &str) {
    let Some(worker) = core
        .hosts()
        .team
        .get(agent_id)
        .filter(|foreign| foreign.role == Role::Worker)
    else {
        return;
    };
    core.hosts().team.remove(agent_id);
    core.harness()
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
    if core.chats().stop(agent_id) {
        core.note_agent_gone(agent_id);
    }
    local_gone(core, agent_id);
    Ok(Value::Null)
}

/// An agent here did work besides messaging: hosts where its teammates run
/// let go of messages held between them, at most every so often.
pub(crate) fn worked(core: &Core, agent_id: &str) {
    if core.hosts().team.ids().is_empty() || !core.hosts().worked_due(agent_id, WORKED_EVERY_MS) {
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
        .hosts()
        .team
        .get(agent_id)
        .is_some_and(|foreign| foreign.host == host)
    {
        messages::worked(core, agent_id);
    }
    Ok(Value::Null)
}

#[derive(Serialize, Deserialize)]
pub(super) struct ActParams {
    method: String,
    params: Value,
}

/// The agents a row's call names.
fn named(params: &Value) -> Vec<String> {
    let one = params["agentId"].as_str();
    let several = params["agentIds"].as_array().into_iter().flatten();
    (one.into_iter())
        .chain(several.filter_map(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// A call a row makes about its agents, split by who answers it: each linked host for the
/// workers that run there, with the params naming only those, and this host for the rest.
pub(crate) fn elsewhere(
    core: &Core,
    method: &str,
    params: Value,
) -> (Vec<(String, Value)>, Option<Value>) {
    if !crate::server::calls::ROW_ACTIONS.contains(&method) || core.hosts().team.is_empty() {
        return (Vec::new(), Some(params));
    }
    let mut there: Vec<(String, Vec<String>)> = Vec::new();
    let mut here = Vec::new();
    for agent_id in named(&params) {
        let worker =
            (core.hosts().team.get(&agent_id)).filter(|worker| worker.role == Role::Worker);
        match worker {
            Some(worker) => match there.iter_mut().find(|(host, _)| *host == worker.host) {
                Some((_, workers)) => workers.push(agent_id),
                None => there.push((worker.host, vec![agent_id])),
            },
            None => here.push(agent_id),
        }
    }
    if there.is_empty() {
        return (Vec::new(), Some(params));
    }
    let naming = |agents: Vec<String>| {
        let mut params = params.clone();
        if params["agentIds"].is_array() {
            params["agentIds"] = json!(agents);
        }
        params
    };
    let local = (!here.is_empty()).then(|| naming(here));
    let there = (there.into_iter())
        .map(|(host, workers)| (host, naming(workers)))
        .collect();
    (there, local)
}

/// Has `host` do what a row here asked of workers that run there.
pub(crate) async fn act(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
) -> CoreResult<Value> {
    let asked = params_of::<asks::Act>(&ActParams {
        method: method.to_owned(),
        params,
    });
    ask(core, host, asks::Act::METHOD, asked, None, ASK_WAIT)
        .await
        .map_err(|error| match error {
            Failed::Unreachable(why) | Failed::Unknown(why) | Failed::Retry(why) => {
                format!("{} is out of reach: {why}", name_of(core, host)).into()
            }
            Failed::Refused(why) | Failed::NotLinked(why) => why.into(),
        })
}

/// The host that leads agents here asks for what a row there does to them.
pub(super) async fn act_here(core: &Core, host: &str, asked: ActParams) -> CoreResult<Value> {
    let agents = named(&asked.params);
    let led = |agent: &String| lead_host(core, agent).as_deref() == Some(host);
    if agents.is_empty() || !agents.iter().all(led) {
        return Err("only the host an agent's lead runs on may do that to it".into());
    }
    let core = core.arc().ok_or("the core is stopping")?;
    crate::server::calls::row_action(&core, &asked.method, asked.params).await
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermitParams {
    agent_id: String,
    request_id: String,
    option_id: Option<String>,
}

/// The person here answered a permission request of `worker`, which runs on a linked host:
/// that host hands its agent the answer.
pub(crate) fn permit(core: &Core, worker: &Foreign, request_id: &str, option_id: Option<String>) {
    let params = PermitParams {
        agent_id: worker.agent_id.clone(),
        request_id: request_id.to_owned(),
        option_id,
    };
    post(
        core,
        &worker.host,
        Sending::new(asks::Permit::METHOD, params_of::<asks::Permit>(&params))
            .kept()
            .about(&worker.agent_id),
    );
    core.hosts().team.update(&worker.agent_id, |foreign| {
        if let Some(said) = &mut foreign.said {
            said.attentions
                .retain(|attention| attention.id != request_id);
            (said.items).retain(|item| item.request_id.as_deref() != Some(request_id));
        }
    });
    core.changed(What::Agents);
}

/// The host that leads an agent here passes on its person's answer to a permission request.
pub(super) fn permit_here(core: &Core, host: &str, params: PermitParams) -> CoreResult<Value> {
    if lead_host(core, &params.agent_id).as_deref() != Some(host) {
        return Err("only the host an agent's lead runs on may answer for it".into());
    }
    super::super::chat::reply_permission(
        core,
        &params.agent_id,
        &params.request_id,
        params.option_id,
        &super::super::access::Peer::App,
    )?;
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
        .hosts()
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

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sikemux_view::inbox::{AgentShown, RemoteAgent, TeamLink};
    use sikemux_view::workspace::AgentFace;
    use sikemux_wire::protocol::{AgentGroup, AgentStatus};

    use super::{heard, Foreign, HappenedParams, Role, Said, Standing, SHOWN};
    use crate::server::inbox::shown_here;

    /// What a row says only of the host that made it, so the host showing a teammate's row
    /// says its own.
    const HOST_OWN: [&str; 4] = ["project", "projectName", "session", "team"];

    /// A row with every field set to something a client would draw. A field added to
    /// `AgentShown` has to be given here, and then has to cross hosts below.
    fn local_row() -> AgentShown {
        AgentShown {
            agent_id: "agent-1".into(),
            chief: true,
            name: Some("Otter".into()),
            cli: "claude".into(),
            title: "Fix the login flake".into(),
            titled: true,
            session: Some(7),
            status: AgentStatus::Working,
            group: AgentGroup::Working,
            label: "Working".into(),
            turn: Some(
                serde_json::from_value(json!({ "state": "working", "since": 5, "on": "Tests" }))
                    .expect("a turn"),
            ),
            unread: true,
            subagents: 2,
            tasks: 3,
            busy: true,
            failed: true,
            background: true,
            active_at: Some(11),
            next_trigger: Some(12),
            triggers: 4,
            line: Some(sikemux_view::inbox::InLine {
                what: "cargo test".into(),
                place: 2,
                expected_ms: Some(13),
                by: None,
            }),
            called_at: Some(14),
            face: Some(AgentFace {
                pack: "buddies".into(),
                shape: "mochi".into(),
                color: 1,
                view_box: "0 0 24 24".into(),
            }),
            team: Some(TeamLink::Head),
            project: Some("/there/widget".into()),
            project_name: Some("widget there".into()),
            reconnecting: Some(2),
            snoozed_until: Some(15),
            settled_at: Some(16),
            auto_settled: true,
            tags: vec!["urgent".into()],
        }
    }

    fn worker() -> Foreign {
        Foreign {
            agent_id: "agent-1".into(),
            host: "host-key".into(),
            name: "Otter".into(),
            cli: "claude".into(),
            project: "widget".into(),
            parent: Some("lead".into()),
            role: Role::Worker,
            status: AgentStatus::Starting,
            said: None,
            active_at: None,
            repo: None,
            landed: None,
            landed_head: None,
            branch: None,
            pending: None,
        }
    }

    fn said(shown: &AgentShown) -> Said {
        Said {
            shown: shown.clone(),
            items: Vec::new(),
            attentions: Vec::new(),
        }
    }

    fn fields(value: &impl serde::Serialize) -> serde_json::Map<String, Value> {
        match serde_json::to_value(value).expect("json") {
            Value::Object(fields) => fields,
            other => panic!("not an object: {other}"),
        }
    }

    /// The row of `foreign` as this host's clients get it.
    fn remote_row(foreign: &Foreign) -> serde_json::Map<String, Value> {
        fields(&RemoteAgent {
            shown: shown_here(foreign, Some("widget".into()), || "Claude".into()),
            host: "studio".into(),
            host_id: foreign.host.clone(),
            lead: foreign.parent.clone(),
            reachable: true,
        })
    }

    fn assert_shows_all(local: &AgentShown, remote: &serde_json::Map<String, Value>) {
        let local = fields(local);
        for (field, value) in &local {
            assert!(
                !matches!(value, Value::Null | Value::Bool(false))
                    && value.as_u64() != Some(0)
                    && value.as_array().is_none_or(|list| !list.is_empty()),
                "the test's row leaves `{field}` empty, so it does not show whether it crosses hosts"
            );
            if HOST_OWN.contains(&field.as_str()) {
                continue;
            }
            assert_eq!(
                remote.get(field),
                Some(value),
                "a local agent's row shows `{field}`, and the row of the same agent on a linked host does not"
            );
        }
    }

    #[test]
    fn an_agent_on_a_linked_host_shows_every_field_a_local_one_does() {
        let local = local_row();

        let sent = serde_json::to_value(HappenedParams {
            agent_id: local.agent_id.clone(),
            kind: SHOWN.into(),
            status: local.status,
            said: Some(said(&local)),
        })
        .expect("json");
        let got: HappenedParams = serde_json::from_value(sent).expect("the report");
        let mut foreign = worker();
        heard(&mut foreign, got.status, got.said);
        assert_shows_all(&local, &remote_row(&foreign));

        let sent = serde_json::to_value(Standing {
            agent_id: local.agent_id.clone(),
            status: local.status,
            said: Some(said(&local)),
        })
        .expect("json");
        let got: Standing = serde_json::from_value(sent).expect("the standing");
        let mut foreign = worker();
        heard(&mut foreign, got.status, got.said);
        assert_shows_all(&local, &remote_row(&foreign));
    }

    #[test]
    fn its_row_is_in_the_project_here_and_off_its_hosts_terminal_and_team() {
        let mut foreign = worker();
        heard(&mut foreign, AgentStatus::Working, Some(said(&local_row())));
        let shown = shown_here(&foreign, Some("widget".into()), || "Claude".into());
        assert_eq!(shown.project.as_deref(), Some("widget"));
        assert_eq!(shown.project_name.as_deref(), Some("widget"));
        assert_eq!(shown.session, None);
        assert_eq!(shown.team, None);
    }

    #[test]
    fn a_worker_its_host_has_not_shown_yet_stands_in_as_a_row() {
        let shown = shown_here(&worker(), Some("widget".into()), || "Claude".into());
        assert_eq!(shown.name.as_deref(), Some("Otter"));
        assert_eq!(shown.title, "Claude");
        assert_eq!(shown.status, AgentStatus::Starting);
    }
}
