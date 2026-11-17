//! Where a project's chief stands, kept on the other hosts that hold its repository. After each
//! of the chief's turns its home sends them the brief a fresh session would start from, so a host
//! that takes the chief over once its home is removed goes on from there rather than from
//! nothing. The project's rules reach those hosts with its record.

use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::ChatContext;

use super::super::chat::rotation;
use super::super::kept::file_for;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::{ask, ASK_WAIT};

const FOLDER: &str = "chief-copies";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChiefCopy {
    identity: String,
    chief: String,
    /// When its home sent it, in Unix milliseconds.
    at: u64,
    standing: ChatContext,
}

/// The chief `agent_id` ended a turn: the hosts that hold its repository get where it stands.
pub(crate) fn send(core: &Core, agent_id: &str) {
    let Some(identity) = (core.workspaces().projects().into_iter())
        .find(|project| project.chief.as_deref() == Some(agent_id))
        .and_then(|project| project.identity)
    else {
        return;
    };
    let mut holders = core.hosts().copies.holders(&identity);
    holders.retain(|host| core.remote().link(host).is_some());
    let (Some(core), Ok(chat), false) = (
        core.arc(),
        core.chats().running(agent_id),
        holders.is_empty(),
    ) else {
        return;
    };
    let brief = rotation::brief(&core, &chat);
    let params = params_of::<asks::ChiefCopy>(&ChiefCopy {
        identity,
        chief: agent_id.to_owned(),
        at: epoch_ms(),
        standing: brief.message,
    });
    for host in holders {
        let (core, params) = (core.clone(), params.clone());
        tokio::spawn(async move {
            let _ = ask(
                &core,
                &host,
                asks::ChiefCopy::METHOD,
                params,
                None,
                ASK_WAIT,
            )
            .await;
        });
    }
}

fn file(core: &Core, identity: &str) -> Option<PathBuf> {
    Some(
        core.data_dir()
            .as_ref()?
            .join(FOLDER)
            .join(file_for(identity)),
    )
}

/// A linked host sent where its chief of a repository this host holds stands.
pub(super) fn heard(core: &Core, copy: ChiefCopy) -> Answer {
    let holds = (core.workspaces().projects().iter())
        .any(|project| project.identity.as_deref() == Some(copy.identity.as_str()));
    let Some(path) = file(core, &copy.identity).filter(|_| holds) else {
        return Ok(Value::Null);
    };
    let text = serde_json::to_vec(&copy).map_err(|error| error.to_string())?;
    (path.parent().map_or(Ok(()), std::fs::create_dir_all))
        .and_then(|()| std::fs::write(&path, text))
        .map_err(|error| format!("the chief's copy was not kept: {error}"))?;
    Ok(Value::Null)
}

/// What this host kept of where `chief`, of the repository `identity`, stood: its brief as a
/// fresh session's first message, and when its home sent it.
pub(super) fn kept(core: &Core, identity: &str, chief: &str) -> Option<(Vec<ChatContext>, u64)> {
    let text = std::fs::read(file(core, identity)?).ok()?;
    let copy: ChiefCopy = serde_json::from_slice(&text).ok()?;
    if copy.chief != chief {
        return None;
    }
    Some((vec![copy.standing], copy.at))
}

/// Removes what was kept of a chief this host has taken over.
pub(super) fn forget(core: &Core, identity: &str) {
    if let Some(path) = file(core, identity) {
        let _ = std::fs::remove_file(path);
    }
}
