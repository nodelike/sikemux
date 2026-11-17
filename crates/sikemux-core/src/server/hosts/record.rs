//! A project's record, its board with the decisions on it, kept by its home: the host of its
//! chief (ADR 0023). The home sends the record to every linked host that holds the repository,
//! and those hosts send their writes to the home rather than keep them, so every host sees one
//! record. A host out of reach of the home refuses a write, naming the home.

use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;

use crate::cli::protocol::HarnessRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_view::workspace::{ChiefHome, ProjectInfo};

use super::super::board::record as board_record;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::{ask, name_of, Failed, ASK_WAIT};

/// The calls from the app that change a project's record.
pub(crate) const APP_WRITES: [&str; 10] = [
    "boardAdd",
    "boardAddTicket",
    "boardUpdate",
    "boardRemove",
    "boardSettings",
    "boardAuditEvery",
    "boardAuditNow",
    "overruleDecision",
    "undoOverrule",
    "boardDecide",
];

/// The tools an agent changes a project's record with.
pub(crate) const TOOL_WRITES: [&str; 4] =
    ["task.add", "task.claim", "task.update", "decision.keep"];

#[derive(Serialize, Deserialize)]
pub(super) struct RecordCopy {
    identity: String,
    board: Value,
}

#[derive(Serialize, Deserialize)]
pub(super) struct RecordAct {
    identity: String,
    method: String,
    params: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

fn project_at(core: &Core, folder: &str) -> Option<ProjectInfo> {
    (core.workspaces().projects().into_iter()).find(|project| project.path.as_os_str() == folder)
}

fn project_of(core: &Core, identity: &str) -> Option<ProjectInfo> {
    (core.workspaces().projects().into_iter())
        .find(|project| project.identity.as_deref() == Some(identity))
}

/// Whether the project at `folder` keeps its record on this host.
fn keeps_record(core: &Core, folder: &str) -> bool {
    project_at(core, folder)
        .is_none_or(|project| core.hosts().copies.home_of(core, &project).is_none())
}

/// The folders of this host's projects whose record another host keeps.
pub(crate) fn elsewhere(core: &Core) -> Vec<String> {
    (core.workspaces().projects().into_iter())
        .filter(|project| core.hosts().copies.home_of(core, project).is_some())
        .map(|project| project.path.to_string_lossy().into_owned())
        .collect()
}

/// The project at `folder`'s home and its repository, when that is another host.
fn away(core: &Core, folder: &str) -> Option<(ChiefHome, String)> {
    let project = project_at(core, folder)?;
    let home = core.hosts().copies.home_of(core, &project)?;
    Some((home, project.identity?))
}

/// Has the project's home do the app's `method`, when its record is kept on another host.
pub(crate) async fn app_write(
    core: &Arc<Core>,
    method: &str,
    params: &Value,
) -> Option<Result<Value, String>> {
    if !APP_WRITES.contains(&method) {
        return None;
    }
    let folder = (core.workspaces()).project_of(std::path::Path::new(
        params.get("project").and_then(Value::as_str)?,
    ));
    let (home, identity) = away(core, &folder)?;
    let mut params = params.clone();
    put_project(&mut params, identity.clone());
    let asked = RecordAct {
        identity,
        method: method.to_owned(),
        params,
        agent_id: None,
    };
    Some(act(core, &home, asked).await.map(|mut answer| {
        if answer.get("project").is_some_and(Value::is_string) {
            put_project(&mut answer, folder.clone());
        }
        answer
    }))
}

/// Has the project's home do an agent's tool call, when its record is kept on another host.
pub(crate) async fn tool_write(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Option<Result<Value, String>> {
    let closes_an_ask = request.params.get("asked").is_some();
    if !TOOL_WRITES.contains(&request.method.as_str()) || closes_an_ask {
        return None;
    }
    let folder = crate::server::caller::project_of(core, request);
    let (home, identity) = away(core, &folder)?;
    let asked = RecordAct {
        identity,
        method: request.method.clone(),
        params: request.params.clone(),
        agent_id: request.agent_id.clone(),
    };
    Some(act(core, &home, asked).await)
}

/// Has the home of the project at `folder` do the record's `method`, when another host keeps it.
/// Runs on a thread that may wait, never on the async workers.
pub(crate) fn act_waiting(
    core: &Arc<Core>,
    folder: &str,
    method: &str,
    mut params: Value,
) -> Option<Result<Value, String>> {
    let (home, identity) = away(core, folder)?;
    put_project(&mut params, identity.clone());
    let asked = RecordAct {
        identity,
        method: method.to_owned(),
        params,
        agent_id: None,
    };
    Some(tokio::runtime::Handle::current().block_on(act(core, &home, asked)))
}

async fn act(core: &Arc<Core>, home: &ChiefHome, asked: RecordAct) -> Result<Value, String> {
    let out_of_reach = || {
        format!(
            "{} keeps this project's board and is out of reach, so nothing was changed; try again once it is back",
            home.host_name
        )
    };
    if core.hosts().connected(&home.host).is_none() {
        return Err(out_of_reach());
    }
    let params = params_of::<asks::RecordAct>(&asked);
    ask(
        core,
        &home.host,
        asks::RecordAct::METHOD,
        params,
        None,
        ASK_WAIT,
    )
    .await
    .map_err(|error| match error {
        Failed::Unreachable(_) | Failed::Retry(_) | Failed::Unknown(_) => out_of_reach(),
        Failed::Refused(why) | Failed::NotLinked(why) => why,
    })
}

/// A linked host sent a write to a project whose record this host keeps.
pub(super) async fn act_here(core: &Arc<Core>, asked: RecordAct) -> Answer {
    let folder = project_of(core, &asked.identity)
        .map(|project| project.path.to_string_lossy().into_owned())
        .filter(|folder| keeps_record(core, folder))
        .ok_or("this host does not keep that project's board")?;
    if !asked.method.contains('.') {
        let mut params = asked.params;
        put_project(&mut params, folder);
        return crate::server::calls::record_write(core, &asked.method, params)
            .await
            .map_err(|error| error.to_string());
    }
    let request = HarnessRequest {
        id: super::call_id(),
        project: folder,
        agent_id: asked.agent_id,
        method: asked.method,
        params: asked.params,
    };
    crate::server::harness::record_write(core, &request).await
}

/// The boards kept under `keys` changed: each this host keeps goes to the linked hosts in reach
/// that hold its repository.
pub(crate) fn changed(core: &Arc<Core>, keys: Vec<String>) {
    for key in keys {
        let Some(project) = project_of(core, &key) else {
            continue;
        };
        if core.hosts().copies.home_of(core, &project).is_none() {
            send(core, &key, None);
        }
    }
}

/// `host` told this host what it holds: it gets the record of each repository it holds that this
/// host keeps.
pub(super) fn heard_holds(core: &Arc<Core>, host: &str) {
    for project in core.workspaces().projects() {
        let Some(identity) = &project.identity else {
            continue;
        };
        if core
            .hosts()
            .copies
            .holders(identity)
            .iter()
            .any(|holder| holder == host)
            && core.hosts().copies.home_of(core, &project).is_none()
        {
            send(core, identity, Some(host));
        }
    }
}

fn send(core: &Arc<Core>, identity: &str, only: Option<&str>) {
    let mut holders = core.hosts().copies.holders(identity);
    holders.retain(|host| {
        only.is_none_or(|only| only == host) && core.hosts().connected(host).is_some()
    });
    if holders.is_empty() {
        return;
    }
    let Some(board) = board_record::record(core, identity) else {
        return;
    };
    let params = params_of::<asks::RecordCopy>(&RecordCopy {
        identity: identity.to_owned(),
        board,
    });
    for host in holders {
        let (core, params, identity) = (core.clone(), params.clone(), identity.to_owned());
        tokio::spawn(async move {
            let sent = ask(
                &core,
                &host,
                asks::RecordCopy::METHOD,
                params,
                None,
                ASK_WAIT,
            )
            .await;
            match sent {
                Ok(Value::Null) => {}
                Ok(theirs) => {
                    if let Err(error) = board_record::fold_in(&core, &identity, theirs) {
                        eprintln!(
                            "sikemux core: {}'s own cards were not taken in: {error}",
                            name_of(&core, &host)
                        );
                    }
                }
                Err(error) => eprintln!(
                    "sikemux core: the board did not reach {}: {error}",
                    name_of(&core, &host)
                ),
            }
        });
    }
}

/// The project's home sent its record.
pub(super) fn heard(core: &Core, host: &str, copy: RecordCopy) -> Answer {
    let project =
        project_of(core, &copy.identity).ok_or("this host does not hold that repository")?;
    let home = core.hosts().copies.home_of(core, &project);
    if home.is_none_or(|home| home.host != host) {
        return Err(format!(
            "this host keeps that project's board with another home than {}",
            name_of(core, host)
        ));
    }
    let unshared = board_record::mirror(core, &copy.identity, copy.board)?;
    Ok(unshared.unwrap_or(Value::Null))
}

fn put_project(params: &mut Value, project: String) {
    if let Some(fields) = params.as_object_mut() {
        fields.insert("project".into(), project.into());
    }
}
