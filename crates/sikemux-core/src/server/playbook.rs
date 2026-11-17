//! A project's Playbook in one read: the person's rules, what agents noted, who lands and what
//! runs with nobody asking.

pub use sikemux_calls::playbook::PlaybookParams;
use std::sync::Arc;

use sikemux_view::playbook::{Landing, Lands, Playbook};

use super::access::Peer;
use super::board::project_at;
use super::connection::blocking;
use super::Core;

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
        sikemux_calls::board::Landing::Core => Lands::Agents,
        sikemux_calls::board::Landing::Person => Lands::Person,
    };
    Playbook {
        rules,
        notes,
        landing: Landing {
            lands,
            means: Some(super::landing::means_now(core, project)),
        },
        automation: super::triggers::for_playbook(core, project),
        places: super::places::of(core, project),
    }
}
