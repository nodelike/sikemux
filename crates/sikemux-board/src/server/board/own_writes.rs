//! What the board writes by itself, such as an agent's commits on its card. When another host
//! keeps a project's board, these go to that home to be written there, waiting while it is out
//! of reach.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_view::inbox::InboxRecording;

use super::{CardPull, Core};
use crate::server::board_state::BoardParts;

pub const METHOD: &str = "boardOwnWrite";

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "write",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OwnWrite {
    TriggerCard {
        trigger: u64,
        host: String,
        title: String,
        details: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<super::Subject>,
    },
    CheckPassed {
        agents: Vec<String>,
        sha: String,
    },
    Committed {
        agent: String,
        commits: Vec<(String, String)>,
        landed: bool,
    },
    PullSeen {
        card: u64,
        pull: CardPull,
    },
    Recording {
        agent: String,
        recording: InboxRecording,
    },
}

#[derive(Serialize, Deserialize)]
pub struct AtHome {
    project: String,
    #[serde(flatten)]
    write: OwnWrite,
}

/// Sends `write` to the home of the project at `project` when another host keeps its board, and
/// answers whether it went.
pub(super) fn sent_home(core: &Core, project: &str, write: OwnWrite) -> bool {
    let at_home = AtHome {
        project: project.to_owned(),
        write,
    };
    match serde_json::to_value(&at_home) {
        Ok(params) => (core.board_above().records_post)(core, project, METHOD, params),
        Err(_) => false,
    }
}

/// A linked host sent what the board there wrote by itself, to the project whose board this host
/// keeps. A trigger's card that nobody holds is taken up here, and its agent reopened here.
pub fn write_here(core: &Arc<Core>, at_home: AtHome) -> Result<(), String> {
    let project = super::project_at(core, &at_home.project)?;
    match at_home.write {
        OwnWrite::TriggerCard {
            trigger,
            host,
            title,
            details,
            subject,
        } => {
            let went = super::add_for_trigger(
                core,
                &project,
                (trigger, &host),
                &title,
                &details,
                subject,
            )?;
            tokio::spawn(super::hand_off::carry(core.clone(), project, went));
        }
        OwnWrite::CheckPassed { agents, sha } => super::check_passed(core, &project, &agents, &sha),
        OwnWrite::Committed {
            agent,
            commits,
            landed,
        } => super::committed(core, &project, &agent, &commits, landed),
        OwnWrite::PullSeen { card, pull } => super::pull_seen(core, &project, card, pull),
        OwnWrite::Recording { agent, recording } => {
            super::attach_recording(core, &agent, recording)
        }
    }
    Ok(())
}

pub fn read(params: Value) -> Result<AtHome, String> {
    serde_json::from_value(params).map_err(|error| error.to_string())
}
