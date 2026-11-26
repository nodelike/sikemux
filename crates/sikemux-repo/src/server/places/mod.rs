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
use crate::server::places::place::PlaceExt;
use crate::server::remote_state::RemoteParts;
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
pub use place::{Place, GITHUB_RELEASES};

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
/// `addresses` is what the action writes to, in full; none reads them from the action. `plain`
/// is the repository it changes when it is a push or merge through git with nothing else to it,
/// and `rewrites` says it is a push that may do more than send commits.
pub fn judge(
    core: &Core,
    project: &str,
    ship: &Ship,
    agent: &str,
    addresses: Option<&str>,
    plain: Option<&str>,
    rewrites: bool,
) -> Judged {
    deploys_pushes(core, project, Path::new(project));
    let _ = has_ci_push_deploys(core, project, Path::new(project));
    let (places, confirmed) = current(core, project);
    let addresses = addresses.unwrap_or(&ship.what);
    let judged = judge::judged_reading(&places, confirmed, ship, agent, addresses, plain, rewrites);
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

/// [`judge`], but by another of this host's projects where one of its production places takes
/// `ship` and asks more of it, so a change to that project's production waits as its own would.
/// Returns the project that decided.
pub fn judge_anywhere(
    core: &Core,
    project: &str,
    ship: &Ship,
    agent: &str,
    addresses: Option<&str>,
    plain: Option<&str>,
    rewrites: bool,
) -> (String, Judged) {
    let mut decided = (
        project.to_owned(),
        judge(core, project, ship, agent, addresses, plain, rewrites),
    );
    let others = (core.workspaces().project_paths().into_iter())
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|other| other != project);
    for other in others {
        let (places, _) = current(core, &other);
        if !(places.iter()).any(|place| place.shown.production && place.recognises(ship)) {
            continue;
        }
        let theirs = judge(core, &other, ship, agent, addresses, plain, rewrites);
        if holds(theirs.goes) > holds(decided.1.goes) {
            decided = (other, theirs);
        }
    }
    decided
}

/// How much a judgement holds an action back.
fn holds(goes: Goes) -> u8 {
    match goes {
        Goes::Freely => 0,
        Goes::Unplaced => 1,
        Goes::Tells => 2,
        Goes::Asks => 3,
    }
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

/// The person said on a call to just tell them of routine changes to `place` through
/// `repository` from now on.
pub fn just_tell(core: &Core, project: &str, place: Place, repository: &str, from: Option<String>) {
    let tell = |place: &mut Place| {
        place.shown.level = Level::TellMe;
        place.shown.said_by = Some(by_tap());
        place.shown.said_from = from.clone();
        place.routine_only = true;
        place.ordinary_only = false;
        if !place.just_tell.iter().any(|said| said == repository) {
            place.just_tell.push(repository.to_owned());
        }
    };
    let _ = change(core, project, |kept| {
        match kept
            .places
            .iter_mut()
            .find(|other| other.shown.id == place.shown.id)
        {
            Some(other) => {
                other.shown.was = Some(other.shown.level);
                tell(other);
            }
            None => {
                let mut place = place;
                tell(&mut place);
                keep(&mut kept.places, place);
            }
        }
        Ok(())
    });
}

/// The person said on a call to let agents make ordinary changes to the place `id` from now on
/// and tell them of each: a tag, a release or a push that does more than send commits still
/// waits for their yes. Only once they confirmed the list, and only for a place that still asks.
pub fn always_tell(core: &Core, project: &str, id: &str, from: Option<String>) {
    let _ = tapped(core, project, |kept| {
        let confirmed = kept.confirmed.is_some();
        let place = (kept.places.iter_mut())
            .find(|place| place.shown.id == id)
            .filter(|place| confirmed && place.asks(confirmed));
        if let Some(place) = place {
            place.shown.was = Some(Level::Mine);
            place.shown.level = Level::TellMe;
            place.routine_only = false;
            place.just_tell.clear();
            place.ordinary_only = true;
            place.shown.said_by = Some(by_tap());
            place.shown.said_from = from;
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
pub fn may_pass(core: &Core, call: &PersonCall, peer: &Peer, picked: bool) -> Result<(), String> {
    if call.kind != CallKind::Production {
        return Ok(());
    }
    if !matches!(peer, Peer::App | Peer::Device { .. }) {
        return Err(
            "only the person passes a production place, from Sikemux, its notch or a paired phone"
                .into(),
        );
    }
    core.remote()
        .may_sign(peer)
        .map_err(|error| error.to_string())?;
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

/// Whether a push to the branch of `project` deploys it to production on Vercel, as its files
/// at `root` say or once said. Agents can change those files, so what they said is kept.
pub fn deploys_pushes(core: &Core, project: &str, root: &Path) -> bool {
    if core.places().of(project).deploys_pushes {
        return true;
    }
    let deploys = found::vercel_deploys_pushes(root);
    if deploys {
        let _ = core.places().change(project, |kept| {
            kept.deploys_pushes = true;
            Ok(())
        });
    }
    deploys
}

/// Whether `project` (as its files at `root` say or once said) has any workflow that deploys
/// pushes to a branch. Captures seen push-deploying workflows and their places.
pub fn has_ci_push_deploys(core: &Core, project: &str, root: &Path) -> bool {
    let seen = found::workflow_push_deploys(root);
    if !seen.is_empty() {
        let _ = change(core, project, |kept| {
            for deploy in &seen {
                found::merge_push_deploy(&mut kept.push_workflows, deploy.clone());
                let words = Recognise::Command {
                    words: format!("gh workflow run {}", deploy.file),
                };
                let name = deploy
                    .environments
                    .iter()
                    .find(|env| place::says_production(env))
                    .map_or_else(
                        || format!("The {} workflow on GitHub", deploy.file),
                        |env| format!("{env} on GitHub"),
                    );
                keep(
                    &mut kept.places,
                    place::new_place(
                        &name,
                        true,
                        vec![words],
                        Some(format!(".github/workflows/{}", deploy.file)),
                    ),
                );
            }
            Ok(())
        });
        return true;
    }
    !core.places().of(project).push_workflows.is_empty()
}

/// A workflow of `project` (as its files at `root` say or once said) that deploys a push to
/// `branch` touching `changed` files. Keeps seen push-deploying workflows and their places.
pub fn ci_push_deploy(
    core: &Core,
    project: &str,
    root: &Path,
    branch: Option<&str>,
    changed: Option<&[String]>,
) -> Option<store::PushDeploy> {
    if !has_ci_push_deploys(core, project, root) {
        return None;
    }
    let all = core.places().of(project).push_workflows;
    all.into_iter()
        .find(|deploy| deploy.matches(branch, changed))
}

/// `project`'s places as clients show them.
pub fn of(core: &Core, project: &str) -> Places {
    deploys_pushes(core, project, Path::new(project));
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
    if place.shown.production {
        format!(
            "{} is production, so a change there waits for you",
            place.name()
        )
    } else {
        format!("{} isn't production, so agents deploy there", place.name())
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
    let signs = match lowered(&core, &params).await? {
        Some(name) => {
            let words = format!(
                "Lower the protection on {name}: agents will change it without waiting for your yes."
            );
            (core.remote().sign(peer, &words).await).map_err(|error| error.to_string())
        }
        None => Err("that place changed while you were answering; try again".to_owned()),
    };
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &params.project)?;
        tapped(&core, &project, |kept| {
            let confirmed = kept.confirmed.is_some();
            let place = (kept.places.iter_mut())
                .find(|place| place.shown.id == params.place)
                .ok_or("that place is no longer one of the project's")?;
            if place.shown.production && params.level < place.shown.level {
                signs?;
            }
            if place.shown.production && !confirmed && params.level != Level::Mine {
                return Err(
                    "Confirm the list of production places first; until then each waits for you"
                        .to_owned(),
                );
            }
            if place.shown.level != params.level {
                place.shown.was = Some(place.shown.level);
                place.shown.level = params.level;
                place.routine_only = false;
                place.just_tell.clear();
                place.ordinary_only = false;
                place.shown.said_by = Some(by_tap());
                place.shown.said_from = from.clone();
            }
            Ok(())
        })?;
        Ok(of(&core, &project))
    })
    .await
}

/// The name of the production place `params` would lower, if it would.
async fn lowered(core: &Arc<Core>, params: &PlaceLevelParams) -> CoreResult<Option<String>> {
    let (core, project, place, level) = (
        core.clone(),
        params.project.clone(),
        params.place.clone(),
        params.level,
    );
    blocking(move || {
        let project = (core.repo_above().board_project_at)(&core, &project)?;
        let (places, _) = current(&core, &project);
        Ok((places.into_iter())
            .find(|kept| kept.shown.id == place)
            .filter(|kept| kept.shown.production && level < kept.shown.level)
            .map(|kept| kept.shown.name))
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
        person: None,
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
    if !params.production {
        let words = format!(
            "Add {} as a place that is not production, so agents change it freely.",
            params.name
        );
        core.remote().sign(peer, &words).await?;
    }
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
