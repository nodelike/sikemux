//! A host removed from the account never comes back, so a project whose chief lived there gets a
//! new home without the person asking: of the hosts still holding the repository, the one with
//! the lowest key takes the chief up under the same agent id and a higher count. Workers on other
//! hosts then report to it, and what waited for the removed host goes there instead.

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
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Refusal;
use super::outbox::Sending;
use super::team::{Foreign, Role};
use super::{name_of, post, route};

/// Moves the chiefs that lived on `removed`, called `removed_name`, to their new homes. Runs
/// before this host forgets what `removed` told it.
pub(super) fn host_removed(core: &Core, removed: &str, removed_name: &str) {
    let me = core.remote().core_id().unwrap_or_default();
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
            let Some(project) = here.filter(|project| project.chief.is_none()) else {
                continue;
            };
            take_up(core, &project, &lead, count, removed_name);
        }
        move_workers(core, removed, removed_name, &lead, &home, &identity);
    }
}

/// Makes `lead` this host's chief of `project` and opens it.
fn take_up(core: &Core, project: &ProjectInfo, lead: &str, count: u32, removed_name: &str) {
    if let Err(error) = core.workspaces().set_chief(&project.id, lead, count + 1) {
        eprintln!(
            "sikemux core: @Chief of {} was not taken up here: {error}",
            project.name
        );
        return;
    }
    core.hosts().copies.wake.now();
    let Some(core) = core.arc() else {
        return;
    };
    let identity = project.identity.clone().unwrap_or_default();
    let (mut standing, at) = super::chief_copy::kept(&core, &identity, lead).unwrap_or_default();
    let stood = match at {
        0 => format!("Nothing of where you stood reached this host before {removed_name} went, so ask the person what they need."),
        at => format!("What follows is where you stood when {removed_name} last sent it, {} minutes ago.", sikemux_process::clock::epoch_ms().saturating_sub(at) / 60_000),
    };
    let moved = format!("{removed_name}, where you ran as this project's chief, was removed from the account, so you run on this host now under the same name, in a fresh session. Your chat before the move and the board stayed on {removed_name}. Your workers on other hosts report here from now on, and their reports that waited for {removed_name} come here too. {stood}");
    standing.insert(
        0,
        crate::sessions::for_agent::sikemux_context(
            "chief-moved",
            "Your home host was removed",
            &moved,
        ),
    );
    let mut project = project.clone();
    project.chief = Some(lead.to_owned());
    tokio::spawn(async move {
        match chief::take_over(&core, &project, standing).await {
            Ok(_) => super::chief_copy::forget(&core, &identity),
            Err(error) => eprintln!(
                "sikemux core: @Chief of {} did not open here: {error}",
                project.name
            ),
        }
    });
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
    lead: &str,
    home: &str,
    identity: &str,
) {
    let Some(foreign) = (core.hosts().team.get(lead))
        .filter(|foreign| foreign.role == Role::Lead && foreign.host == removed)
    else {
        return;
    };
    let workers = route::workers_of(core, lead);
    let waiting = (core.hosts().outbox).take(removed, |post| workers.contains(&post.lane));
    let me = core.remote().core_id().unwrap_or_default();
    if home == me {
        core.hosts().team.remove(lead);
        deliver_here(core, waiting);
        for worker in workers {
            route::notice(core, worker, format!("{removed_name}, where your lead {} ran, was removed from the account, so your lead runs on this host now. What you sent it meanwhile has reached it; carry on with your task.", foreign.name));
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
        route::notice(core, worker, format!("{removed_name}, where your lead {} ran, was removed from the account, so your lead runs on {home_name} now. What you sent it meanwhile goes there; carry on with your task.", foreign.name));
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
