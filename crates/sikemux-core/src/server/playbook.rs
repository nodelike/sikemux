//! A project's Playbook in one read: the person's rules, what agents noted, who lands and what
//! runs with nobody asking.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_view::playbook::{Auto, AutoAgent, Landing, Lands, Playbook, RunsAlone};

use super::access::Peer;
use super::board::{auto, project_at};
use super::connection::blocking;
use super::Core;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybookParams {
    pub project: String,
}

pub(crate) async fn read(
    core: &Arc<Core>,
    peer: Peer,
    params: PlaybookParams,
) -> Result<Playbook, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        Ok(playbook(&core, &project, &peer))
    })
    .await
    .map_err(|error| error.to_string())
}

fn playbook(core: &Core, project: &str, peer: &Peer) -> Playbook {
    let (rules, notes) = super::memory::for_playbook(core, project, peer);
    let settings = super::board::settings_of(core, project);
    let lands = match settings.landing {
        auto::Landing::Core => Lands::Agents,
        auto::Landing::Person => Lands::Person,
    };
    let auto = settings.auto.then(|| {
        RunsAlone::Auto(Box::new(Auto {
            slots: settings.slots,
            ceiling: settings.spend_ceiling,
            builder: AutoAgent {
                launcher: settings.launcher.clone(),
                provider: settings.provider.clone(),
                model: settings.model.clone(),
            },
            reviewer: settings.reviewer.as_ref().map(|reviewer| AutoAgent {
                launcher: Some(reviewer.launcher.clone()),
                provider: reviewer.provider.clone(),
                model: reviewer.model.clone(),
            }),
        }))
    });
    let automation = auto
        .into_iter()
        .chain(super::triggers::for_playbook(core, project))
        .collect();
    Playbook {
        rules,
        notes,
        landing: Landing {
            lands,
            means: Some(super::landing::means_now(core, project)),
        },
        automation,
        places: super::places::of(core, project),
    }
}
