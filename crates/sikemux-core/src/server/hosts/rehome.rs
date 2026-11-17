//! A host removed from the account never comes back, so a project whose chief lived there gets a
//! new home without the person asking: of the hosts still holding the repository, the one with
//! the lowest key takes the chief up under the same agent id and a higher count. Workers on other
//! hosts then report to it, and what waited for the removed host goes there instead. A host only
//! out of reach may come back, so there the person takes the chief over with a tap.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_view::workspace::ProjectInfo;
use sikemux_wire::protocol::AgentStatus;

use super::super::chief;
use super::super::messages::{self, Carried};
use super::super::{Core, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Refusal;
use super::outbox::Sending;
use super::team::{Foreign, Role};
use super::{name_of, post, record, route};
use crate::server::device_view::DeviceViewCore;
use sikemux_process::clock::epoch_ms;

/// Moves the chiefs that lived on `removed`, called `removed_name`, to their new homes. Runs
/// before this host forgets what `removed` told it.
pub(super) fn host_removed(core: &Core, removed: &str, removed_name: &str) {
    let me = core.remote().core_id().unwrap_or_default();
    let gone = "was removed from the account";
    for (identity, lead, count) in core.hosts().copies.chiefs_on(removed) {
        let here = (core.workspaces().projects().into_iter())
            .find(|project| project.identity.as_deref() == Some(identity.as_str()));
        let mut holders = core.hosts().copies.holders(&identity);
        holders.retain(|host| host != removed && core.remote().link(host).is_some());
        holders.extend(here.as_ref().map(|_| me.clone()));
        let Some(home) = holders.into_iter().min() else {
            continue;
        };
        if home == me {
            let (Some(project), Some(core)) =
                (here.filter(|project| project.chief.is_none()), core.arc())
            else {
                continue;
            };
            let waiting = waiting_for(&core, removed, &lead);
            let written = record::sent_to(&core, removed, &identity);
            core.hosts().team.remove(&lead);
            let removed_name = removed_name.to_owned();
            tokio::spawn(async move {
                match taken_up(&core, project, &lead, count, &removed_name, gone).await {
                    Ok(_) => {
                        record::send_to(&core, &home, written);
                        if let Some(waiting) = waiting {
                            send_on(&core, waiting, &removed_name, gone, &lead, &home, &identity);
                        }
                    }
                    Err(error) => eprintln!("sikemux core: @Chief was not taken up here: {error}"),
                }
            });
            continue;
        }
        move_workers(core, removed, removed_name, gone, &lead, &home, &identity);
    }
}

/// The person asked this host to take over the chief of `params.project`, whose home host is out
/// of reach. It opens here under the same agent id and a higher count, so the home stands down
/// once it is back, and workers on other hosts report here from then on.
pub(crate) async fn take_over_here(
    core: &Arc<Core>,
    params: chief::OpenChiefParams,
) -> CoreResult<chief::OpenedChief> {
    let project = (core.workspace_view().projects.into_iter())
        .find(|project| project.id == params.project)
        .ok_or("that project is not on this host")?;
    let home = project
        .chief_home
        .clone()
        .ok_or("this project's chief already lives here")?;
    if home.away_since.is_none() {
        return Err(format!("{} is in reach; open @Chief there", home.host_name).into());
    }
    let identity = project.identity.clone().unwrap_or_default();
    let count = core
        .hosts()
        .copies
        .chief_count(&identity)
        .max(project.chief_count);
    let gone = "is out of reach";
    let agent_id = taken_up(core, project, &home.agent_id, count, &home.host_name, gone).await?;
    move_workers(
        core,
        &home.host,
        &home.host_name,
        gone,
        &home.agent_id,
        &core.remote().core_id().unwrap_or_default(),
        &identity,
    );
    Ok(chief::OpenedChief { agent_id })
}

/// A linked host's chief outranks the host this host's workers of it report to, as when the
/// person took it over there: they report to that host from then on.
pub(super) fn follow(core: &Core, host: &str) {
    for (identity, lead, count) in core.hosts().copies.chiefs_on(host) {
        record::follow_home(core, host, &identity);
        let Some(was) = (core.hosts().team.get(&lead))
            .filter(|foreign| foreign.role == Role::Lead && foreign.host != host)
            .map(|foreign| foreign.host)
        else {
            continue;
        };
        let before = (core.hosts().copies.chiefs_on(&was).into_iter())
            .find(|(_, chief, _)| *chief == lead)
            .map_or(0, |(_, _, count)| count);
        if count > before {
            let name = name_of(core, &was);
            move_workers(core, &was, &name, "is out of reach", &lead, host, &identity);
        }
    }
}

/// Makes `lead` this host's chief of `project`, its `count + 1`th, and opens it with where it
/// stood when `home_name`, which `gone`, last sent it.
async fn taken_up(
    core: &Arc<Core>,
    mut project: ProjectInfo,
    lead: &str,
    count: u32,
    home_name: &str,
    gone: &str,
) -> CoreResult<String> {
    core.workspaces().set_chief(&project.id, lead, count + 1)?;
    core.hosts().copies.wake.now();
    let identity = project.identity.clone().unwrap_or_default();
    let (mut standing, at) = super::chief_copy::kept(core, &identity, lead).unwrap_or_default();
    let stood = match at {
        0 => format!("Nothing of where you stood reached this host from {home_name}, so ask the person what they need."),
        at => format!("What follows is where you stood when {home_name} last sent it, {} minutes ago.", epoch_ms().saturating_sub(at) / 60_000),
    };
    let moved = format!("{home_name}, where you ran as this project's chief, {gone}, so you run on this host now under the same name, in a fresh session. Your chat before the move stayed on {home_name}. The board is here as {home_name} last sent it, and this host keeps it now. Your workers on other hosts report here from now on, and their reports that waited for {home_name} come here too. {stood}");
    standing.insert(
        0,
        crate::sessions::for_agent::sikemux_context(
            "chief-moved",
            "Your home host changed",
            &moved,
        ),
    );
    project.chief = Some(lead.to_owned());
    let agent_id = chief::take_over(core, &project, standing).await?;
    super::chief_copy::forget(core, &identity);
    Ok(agent_id)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Moved {
    agent_id: String,
    name: String,
    cli: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LeadMovedParams {
    lead: String,
    identity: String,
    worker: Moved,
}

/// Points this host's workers of `lead` at `home`, and sends on what waited for `removed`.
fn move_workers(
    core: &Core,
    removed: &str,
    removed_name: &str,
    gone: &str,
    lead: &str,
    home: &str,
    identity: &str,
) {
    record::send_to(core, home, record::sent_to(core, removed, identity));
    if let Some(waiting) = waiting_for(core, removed, lead) {
        send_on(core, waiting, removed_name, gone, lead, home, identity);
    }
}

/// This host's workers of `lead`, which runs on `host`, and what waits to go to it from them.
struct Waiting {
    lead_name: String,
    workers: Vec<String>,
    posts: Vec<super::outbox::Post>,
}

fn waiting_for(core: &Core, host: &str, lead: &str) -> Option<Waiting> {
    let foreign = (core.hosts().team.get(lead))
        .filter(|foreign| foreign.role == Role::Lead && foreign.host == host)?;
    let workers = route::workers_of(core, lead);
    let posts = (core.hosts().outbox).take(host, |post| workers.contains(&post.lane));
    Some(Waiting {
        lead_name: foreign.name,
        workers,
        posts,
    })
}

fn send_on(
    core: &Core,
    waiting: Waiting,
    removed_name: &str,
    gone: &str,
    lead: &str,
    home: &str,
    identity: &str,
) {
    let Waiting {
        lead_name,
        workers,
        posts: waiting,
    } = waiting;
    let me = core.remote().core_id().unwrap_or_default();
    if home == me {
        core.hosts().team.remove(lead);
        deliver_here(core, waiting);
        for worker in workers {
            route::notice(core, worker, format!("{removed_name}, where your lead {} ran, {gone}, so your lead runs on this host now. What you sent it meanwhile has reached it; carry on with your task.", lead_name));
        }
        return;
    }
    core.hosts()
        .team
        .update(lead, |foreign| foreign.host = home.to_owned());
    for worker in &workers {
        let moved = Moved {
            agent_id: worker.clone(),
            name: core.names().name(worker).unwrap_or_else(|| worker.clone()),
            cli: (core.workspaces().agent(worker)).map_or_else(String::new, |agent| agent.provider),
        };
        post(
            core,
            home,
            Sending::new(
                asks::LeadMoved::METHOD,
                params_of::<asks::LeadMoved>(&LeadMovedParams {
                    lead: lead.to_owned(),
                    identity: identity.to_owned(),
                    worker: moved,
                }),
            )
            .kept()
            .about(worker),
        );
    }
    core.hosts().outbox.put(home, waiting);
    let home_name = name_of(core, home);
    for worker in workers {
        route::notice(core, worker, format!("{removed_name}, where your lead {} ran, {gone}, so your lead runs on {home_name} now. What you sent it meanwhile goes there; carry on with your task.", lead_name));
    }
}

/// Hands the reports that waited for the removed host to the lead, which now runs here.
fn deliver_here(core: &Core, waiting: Vec<super::outbox::Post>) {
    let Some(core) = core.arc() else {
        return;
    };
    let carried: Vec<Carried> = (waiting.into_iter())
        .filter(|post| post.method == "deliver")
        .filter_map(|post| serde_json::from_value(post.params).ok())
        .collect();
    tokio::spawn(async move {
        for carried in carried {
            if let Err(error) = messages::from_host(&core, None, carried, None).await {
                eprintln!(
                    "sikemux core: a report that waited for a removed host was lost: {error}"
                );
            }
        }
    });
}

/// `host` says its worker of `lead`, a chief that now lives here, report here from now on.
/// While this host has not taken the chief up yet, `host` asks again later.
pub(super) fn lead_moved_here(
    core: &Arc<Core>,
    host: &str,
    params: LeadMovedParams,
) -> Result<Value, Refusal> {
    let Some(project) = (core.workspaces().projects().into_iter())
        .find(|project| project.identity.as_deref() == Some(params.identity.as_str()))
    else {
        return Err("this host does not hold that project".to_owned().into());
    };
    if project.chief.as_deref() != Some(params.lead.as_str()) {
        super::account::relink(core);
        return Err(Refusal::retry("this host has not taken that chief up yet"));
    }
    let worker = params.worker;
    core.hosts().team.add(Foreign {
        agent_id: worker.agent_id,
        host: host.to_owned(),
        name: worker.name,
        cli: worker.cli,
        project: project.id,
        parent: Some(params.lead),
        role: Role::Worker,
        status: AgentStatus::Working,
        said: None,
        active_at: None,
        repo: Some(project.path),
        landed: None,
        landed_head: None,
        branch: None,
        pending: None,
    });
    route::pin(core);
    super::changed(core);
    Ok(Value::Null)
}
