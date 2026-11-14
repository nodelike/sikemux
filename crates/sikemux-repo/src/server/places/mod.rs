//! A project's places, the person's facts about where agents' actions land outside this
//! machine. The core lists them from hard facts: the project's deploy config, what a
//! connector marks as production and what agents ran. The person confirms the list once and
//! sets each production place's level; only their own tap in the app, the notch or a paired
//! phone changes either. What agents send out that no place covers runs, and is listed once.

pub mod found;
pub mod judge;
pub mod place;
pub mod push;
pub mod store;
pub mod work;

use crate::server::changed::ChangedCore;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::places::{AddPlaceParams, PlaceLevelParams, PlacesParams, SeenParams};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_view::calls::{CallKind, PersonCall};
use sikemux_view::memory::Learned;
use sikemux_view::places::{Level, Places, Recognise, Seen};

pub use judge::{Goes, Judged};
pub use place::{Passes, Place, GITHUB_RELEASES};

use super::access::Peer;
use super::changed::{Topic, What};
use super::connection::blocking;
use super::{Core, CoreResult};
use crate::ship::Ship;

/// Actions seen that no place covers, past which the one heard of longest ago goes.
const MAX_SEEN: usize = 64;

/// Something agents sent out that no place covers, as the core keeps it: what clients
/// show, and the fact that would make it a place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unplaced {
    #[serde(flatten)]
    pub seen: Seen,
    pub fact: Recognise,
}

/// Changes what `project` keeps of its places, has its slices' ladders read the result, and
/// open Playbooks read it again.
pub fn change<R>(
    core: &Core,
    project: &str,
    change: impl FnOnce(&mut store::Kept) -> Result<R, String>,
) -> Result<R, String> {
    let (answer, kept, changed) = core.places().change(project, change)?;
    (core.repo_above().board_mirror_places)(core, project, kept.places, kept.confirmed);
    if changed {
        core.changed(What::Read(Topic::Playbook));
    }
    Ok(answer)
}

/// `project`'s places with those its files name that it doesn't keep yet, and whether the
/// person confirmed the list. Nothing is written.
pub fn current(core: &Core, project: &str) -> (Vec<Place>, bool) {
    let kept = core.places().of(project);
    let mut places = kept.places;
    for found in found::in_files(Path::new(project)) {
        keep(&mut places, found);
    }
    (places, kept.confirmed.is_some())
}

/// Adds `place` to `places`, or the facts it brings to the one of the same id, which then
/// counts as production if either does.
pub fn keep(places: &mut Vec<Place>, place: Place) {
    if let Some(kept) = places
        .iter_mut()
        .find(|kept| kept.shown.id == place.shown.id)
    {
        if place.shown.production && !kept.shown.production {
            kept.shown.production = true;
            kept.shown.level = Level::Mine;
        }
        for fact in place.recognise {
            if !kept.recognise.contains(&fact) {
                kept.recognise.push(fact);
            }
        }
        return;
    }
    if places.len() >= place::MAX_PLACES {
        eprintln!(
            "sikemux core: {} was not kept, as a project keeps at most {} places",
            place.name(),
            place::MAX_PLACES
        );
        return;
    }
    places.push(place);
}

/// Where `ship`, an action of the agent called `agent`, lands in `project`. The places its
/// files name and a production place the action itself names are kept from then on.
pub fn judge(core: &Core, project: &str, ship: &Ship, agent: &str) -> Judged {
    let (places, confirmed) = current(core, project);
    let judged = judge::judged(&places, confirmed, ship, agent);
    let kept = core.places().of(project).places;
    let found = places.len() != kept.len();
    if found || judged.keep {
        let action = judged.place.clone().filter(|_| judged.keep);
        let _ = change(core, project, |kept| {
            for place in places.into_iter().chain(action) {
                keep(&mut kept.places, place);
            }
            Ok(())
        });
    }
    judged
}

/// Lists an action no place covers under seen, once however often it runs. The list never
/// drops an entry the person hasn't looked at: once full it counts what more it saw. Only a
/// new entry is written out at once.
pub fn saw(core: &Core, project: &str, ship: &Ship, agent_id: &str) {
    let (id, what) = judge::seen_key(ship);
    let name = core.names().get(agent_id).map(|named| named.name);
    let at = epoch_ms();
    let listed = core
        .places()
        .of(project)
        .unplaced
        .iter()
        .any(|kept| kept.seen.id == id);
    let full = core.places().of(project).unplaced.len() >= MAX_SEEN;
    if listed || full {
        core.places().touch(project, |kept| {
            match kept.unplaced.iter_mut().find(|kept| kept.seen.id == id) {
                Some(kept) => {
                    kept.seen.times += 1;
                    kept.seen.last_by = agent_id.to_owned();
                    kept.seen.last_name = name;
                    kept.seen.last_at = at;
                }
                None => kept.more_seen += 1,
            }
        });
        core.changed(What::Read(Topic::Playbook));
        return;
    }
    let fact = judge::fact_of_seen(ship);
    let _ = change(core, project, |kept| {
        kept.unplaced.push(Unplaced {
            seen: Seen {
                id,
                what,
                from: None,
                times: 1,
                last_by: agent_id.to_owned(),
                last_name: name,
                last_at: at,
            },
            fact,
        });
        Ok(())
    });
}

/// Places an agent proposed that the person accepted: kept as the project's, the stricter
/// of the two where one has the same id as a place already kept.
pub fn accepted(core: &Core, project: &str, proposed: Vec<Place>) -> Result<(), String> {
    change(core, project, |kept| {
        for place in proposed {
            match kept
                .places
                .iter()
                .any(|other| other.shown.id == place.shown.id)
            {
                true => keep(&mut kept.places, place),
                false => {
                    let place = place::checked(place, &kept.places)?;
                    kept.places.push(place);
                }
            }
        }
        Ok(())
    })
}

/// Keeps the place the call `id` offers to keep on the person's answer, with the project's
/// places, so it outlives a restart.
pub fn offer(core: &Core, project: &str, id: &str, place: Place) {
    let _ = change(core, project, |kept| {
        let open: Vec<String> = (super::person_calls::shown(core).into_iter())
            .filter(|call| call.state == sikemux_view::calls::CallState::Open)
            .map(|call| call.id)
            .collect();
        while kept.offered.len() >= MAX_SEEN {
            let settled = (kept.offered.keys())
                .find(|id| !open.iter().any(|call| call == *id))
                .cloned();
            let Some(settled) = settled else {
                break;
            };
            kept.offered.remove(&settled);
        }
        kept.offered.insert(id.to_owned(), place);
        Ok(())
    });
}

/// The place the call `id` offered to keep, taken off the offers.
pub fn take_offer(core: &Core, project: &str, id: &str) -> Option<Place> {
    change(core, project, |kept| Ok(kept.offered.remove(id)))
        .ok()
        .flatten()
}

/// Keeps `place` by the person's answer on a call: production or not, at `level` when they
/// set one, recorded as theirs, from where they tapped.
pub fn kept_by_tap(
    core: &Core,
    project: &str,
    mut place: Place,
    production: bool,
    level: Option<Level>,
    from: Option<String>,
) {
    place.shown.production = production;
    place.shown.level = level.unwrap_or(match production {
        true => Level::Mine,
        false => Level::Yours,
    });
    place.shown.said_by = Some(by_tap());
    place.shown.said_from = from;
    let _ = change(core, project, |kept| {
        match kept
            .places
            .iter_mut()
            .find(|other| other.shown.id == place.shown.id)
        {
            Some(other) => {
                other.shown.was = Some(other.shown.level);
                other.shown.production = place.shown.production;
                other.shown.level = place.shown.level;
                other.shown.said_by = place.shown.said_by.clone();
                other.shown.said_from = place.shown.said_from.clone();
            }
            None => keep(&mut kept.places, place),
        }
        Ok(())
    });
}

/// A production call the gate raised was answered: the place counts it.
pub fn call_answered(core: &Core, call: &PersonCall) {
    let (Some(place), Some(answer)) = (&call.place, &call.answer) else {
        return;
    };
    let as_picked = match (&call.pick, &answer.option) {
        (Some(pick), Some(option)) => Some(pick == option),
        _ => None,
    };
    let _ = change(core, &call.project, |kept| {
        if let Some(kept) = kept
            .places
            .iter_mut()
            .find(|kept| kept.shown.id == place.id)
        {
            let stats = &mut kept.shown.stats;
            stats.asked += 1;
            match as_picked {
                Some(true) => stats.as_picked += 1,
                Some(false) => stats.overruled += 1,
                None => {}
            }
        }
        Ok(())
    });
}

/// Whether the person's answer to a production call may open its place: it came from the
/// app, the notch or a paired phone, an option or options were picked there, and the work is still the
/// work the call showed.
pub fn may_pass(_core: &Core, call: &PersonCall, peer: &Peer, picked: bool) -> Result<(), String> {
    if call.kind != CallKind::Production {
        return Ok(());
    }
    if !matches!(peer, Peer::App | Peer::Device { .. }) {
        return Err(
            "only the person passes a production place, from Sikemux, its notch or a paired phone"
                .into(),
        );
    }
    match picked {
        true => Ok(()),
        false => Err("a production place passes only on the call's button".into()),
    }
}

/// The sites agents in `project` write to without asking: those of places that don't wait.
pub fn allowed_sites(core: &Core, project: &str) -> Vec<String> {
    let kept = core.places().of(project);
    let (places, confirmed) = (kept.places, kept.confirmed.is_some());
    (places.iter())
        .filter(|place| !place.asks(confirmed))
        .flat_map(|place| place.recognise.iter())
        .filter_map(|fact| match fact {
            Recognise::Site { origin } => Some(origin.clone()),
            _ => None,
        })
        .collect()
}

/// `project`'s places as clients show them.
pub fn of(core: &Core, project: &str) -> Places {
    let (places, _) = current(core, project);
    let kept = core.places().of(project);
    Places {
        places: places.into_iter().map(|place| place.shown).collect(),
        seen: kept.unplaced.into_iter().map(|kept| kept.seen).collect(),
        confirmed: kept.confirmed,
        more_seen: kept.more_seen,
    }
}

/// What a place an agent proposed would mean, as the person reads it in Needs you.
pub fn proposed_words(place: &Place) -> String {
    match (place.shown.production, place.step) {
        (true, _) => format!(
            "{} is production, so a change there waits for you",
            place.name()
        ),
        (false, Some(place::StepPass::Check)) => {
            format!("{} goes once the project's check passes", place.name())
        }
        (false, _) => format!("{} isn't production, so agents deploy there", place.name()),
    }
}

/// The person reads a project's places.
pub async fn read(core: &Arc<Core>, params: PlacesParams) -> CoreResult<Places> {
    let core = core.clone();
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        Ok(of(&core, &project))
    })
    .await
}

/// The person sets a place's level by their own tap. A production place loosens only once
/// they confirmed the list.
pub async fn set_level(
    core: &Arc<Core>,
    peer: &Peer,
    params: PlaceLevelParams,
) -> CoreResult<Places> {
    let core = core.clone();
    let from = tapped_on(peer);
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        tapped(&core, &project, |kept| {
            let confirmed = kept.confirmed.is_some();
            let place = (kept.places.iter_mut())
                .find(|place| place.shown.id == params.place)
                .ok_or("that place is no longer one of the project's")?;
            if place.shown.production && !confirmed && params.level != Level::Mine {
                return Err(
                    "Confirm the list of production places first; until then each waits for you"
                        .to_owned(),
                );
            }
            if place.shown.level != params.level {
                place.shown.was = Some(place.shown.level);
                place.shown.level = params.level;
                place.shown.said_by = Some(by_tap());
                place.shown.said_from = from.clone();
            }
            Ok(())
        })?;
        Ok(of(&core, &project))
    })
    .await
}

/// A change the person made by their own tap, to the places the project keeps and those its
/// files name.
fn tapped(
    core: &Core,
    project: &str,
    tap: impl FnOnce(&mut store::Kept) -> Result<(), String>,
) -> Result<(), String> {
    let (places, _) = current(core, project);
    change(core, project, |kept| {
        for place in places {
            keep(&mut kept.places, place);
        }
        tap(kept)
    })
}

/// Where the person tapped, as a Playbook line names it.
fn tapped_on(peer: &Peer) -> Option<String> {
    match peer {
        Peer::App => Some("Sikemux".to_owned()),
        Peer::Device { .. } => Some("a paired phone".to_owned()),
        Peer::Local | Peer::Share { .. } => None,
    }
}

fn by_tap() -> Learned {
    Learned {
        agent: None,
        name: None,
        feed: None,
        turn: None,
        at: epoch_ms(),
        with: None,
    }
}

/// The person confirms the list of production places: "Looks right".
pub async fn confirm(core: &Arc<Core>, params: PlacesParams) -> CoreResult<Places> {
    let core = core.clone();
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        tapped(&core, &project, |kept| {
            kept.confirmed = Some(epoch_ms());
            Ok(())
        })?;
        Ok(of(&core, &project))
    })
    .await
}

/// The person adds a place the core didn't find, at `mine` when it is production.
pub async fn add(core: &Arc<Core>, peer: &Peer, params: AddPlaceParams) -> CoreResult<Places> {
    let core = core.clone();
    let from = tapped_on(peer);
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        tapped(&core, &project, |kept| {
            let mut place =
                place::new_place(&params.name, params.production, params.recognise, None);
            place.shown.said_by = Some(by_tap());
            place.shown.said_from = from.clone();
            let place = place::checked(place, &kept.places)?;
            kept.places.push(place);
            Ok(())
        })?;
        Ok(of(&core, &project))
    })
    .await
}

/// The person counts something seen as production: it becomes a production place at `mine`.
pub async fn count_as_production(
    core: &Arc<Core>,
    peer: &Peer,
    params: SeenParams,
) -> CoreResult<Places> {
    let core = core.clone();
    let from = tapped_on(peer);
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        tapped(&core, &project, |kept| {
            let at = (kept.unplaced.iter())
                .position(|unplaced| unplaced.seen.id == params.seen)
                .ok_or("that is no longer listed as seen")?;
            let unplaced = kept.unplaced.remove(at);
            let found_in = match &unplaced.seen.last_name {
                Some(name) => format!("what @{name} ran"),
                None => "what an agent ran".to_owned(),
            };
            let name = unplaced.seen.what.clone();
            let mut place = place::new_place(&name, true, vec![unplaced.fact], Some(found_in));
            place.shown.said_by = Some(by_tap());
            place.shown.said_from = from.clone();
            keep(&mut kept.places, place);
            Ok(())
        })?;
        Ok(of(&core, &project))
    })
    .await
}
