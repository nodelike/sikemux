//! A project's board shared with one org the account is in. The project's home keeps the share in
//! the board's settings and tells the account server it is the host to dial for it. A share starts
//! only at the person's Share tap. Nothing is shared, registered or served for an org whose Team
//! plan is unpaid; the share stays in place.

use std::path::Path;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use sikemux_view::device::AccountOrg;
use sikemux_view::teammates::{OrgChoice, ProjectSharing, SharedOrg, SharedWith, Teammate};

use crate::accounts::protocol::{OrgShareList, ShareRegistration, ShareRegistry, ShareRemoval};
use crate::server::account::shares;
use crate::server::board;
use crate::server::board_state::BoardParts;
use crate::server::changed::{ChangedCore, What};
use crate::server::kept::Kept;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// What the account server knows a project by: never its remote's address.
pub(crate) fn key(identity: &str) -> String {
    hex::encode(Sha256::digest(identity.as_bytes()))
}

fn orgs(core: &Core) -> Vec<AccountOrg> {
    sikemux_corekit::accounts::orgs(core.account().shown().as_ref()).to_vec()
}

/// The name of org `org` the account is in.
pub(crate) fn org_name(core: &Core, org: &str) -> Option<String> {
    (orgs(core).into_iter())
        .find(|known| known.id == org)
        .map(|known| known.name)
}

/// The org whose key `route` names, among those the account is in.
pub(crate) fn org_of_route(core: &Core, route: &str) -> Option<AccountOrg> {
    (orgs(core).into_iter()).find(|org| crate::launch::providers::route_of_org(route, &org.id))
}

pub(crate) fn active(core: &Core, org: &str) -> bool {
    orgs(core)
        .iter()
        .any(|known| known.id == org && known.active)
}

/// The folder and repository of the project named by its folder or by its repository.
fn project(core: &Core, project: &str) -> Option<(String, String)> {
    let folder = core.workspaces().project_of(std::path::Path::new(project));
    (core.workspaces().projects().into_iter())
        .find(|known| {
            known.path.as_os_str() == folder.as_str() || known.identity.as_deref() == Some(project)
        })
        .and_then(|known| Some((known.path.to_string_lossy().into_owned(), known.identity?)))
}

pub(crate) fn shared_with(core: &Core, folder: &str) -> Option<String> {
    let state = core.boards().state.locked();
    state.of(folder)?.settings.shared_with.clone()
}

pub(crate) fn project_org(core: &Core, path: &Path) -> Option<String> {
    let folder = core.workspaces().project_of(path);
    shared_with(core, &folder).filter(|org| active(core, org))
}

/// The org the project's board is shared with, as both the rail and the Share control show it.
pub(crate) fn shared_org(core: &Core, folder: &str) -> Option<SharedOrg> {
    let org = shared_with(core, folder)?;
    let name = org_name(core, &org).unwrap_or_default();
    Some(SharedOrg { org, name })
}

/// One reconcile at a time, the renames still to move, and the shares it last acted on.
pub(crate) struct Reconciling {
    running: tokio::sync::Mutex<()>,
    /// What renamed projects were known by before and are known by now, kept across restarts so
    /// a rename never ends a share.
    renames: Mutex<Vec<(String, String)>>,
    renames_kept: Kept,
    held: Mutex<Vec<Held>>,
}

impl Reconciling {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let renames_kept = Kept::new(data_dir, "share-renames.json");
        Self {
            running: tokio::sync::Mutex::default(),
            renames: Mutex::new(renames_kept.load()),
            renames_kept,
            held: Mutex::default(),
        }
    }

    fn change_renames(&self, change: impl FnOnce(&mut Vec<(String, String)>)) {
        let mut renames = self.renames.locked();
        change(&mut renames);
        self.renames_kept.save(&*renames);
    }
}

/// A project this host keeps the record of: its key, the org its board is shared with, and
/// whether its chief is here.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    project: String,
    org: Option<String>,
    chief_here: bool,
}

fn held(core: &Core) -> Vec<Held> {
    (core.workspaces().projects().into_iter())
        .filter(|known| core.hosts().copies.home_of(core, known).is_none())
        .filter_map(|known| {
            let folder = known.path.to_string_lossy().into_owned();
            Some(Held {
                project: key(&known.identity?),
                org: shared_with(core, &folder),
                chief_here: known.chief.is_some(),
            })
        })
        .collect()
}

/// Whether this host acts for a registration `other` made: it holds the project's chief, or
/// `other` is no longer one of the person's linked hosts. While no host holds the chief, a linked
/// host's registration is left to that host.
fn takes_over(held: &Held, other: &str, linked: &[String]) -> bool {
    held.chief_here || !linked.iter().any(|link| link == other)
}

/// The keys renamed projects are known by now, while their shares have yet to move.
fn renaming(core: &Core) -> Vec<String> {
    (core.peer().shares.renames.locked().iter())
        .map(|(_, new)| key(new))
        .collect()
}

/// The account's registry or orgs were read again, with the registry if the account answered.
pub(crate) fn heard(core: &Arc<Core>, registry: Option<ShareRegistry>) {
    told(core);
    let Some(registry) = registry else {
        return;
    };
    let core = core.clone();
    tokio::spawn(async move { reconcile(&core, registry).await });
}

fn told(core: &Arc<Core>) {
    super::peer::heard(core);
    super::memory::shares_changed(core);
    super::org_routes::sweep(core);
    core.changed(What::View);
}

/// A board this host keeps was saved. Teammates look again when one of them is shared, and the
/// registry is read again only when what this host shares, or where, changed.
pub(crate) fn records_changed(core: &Arc<Core>, keys: &[String]) {
    super::org_routes::sweep(core);
    let state = core.boards().state.locked();
    let shared = (keys.iter())
        .any(|key| (state.of(key)).is_some_and(|board| board.settings.shared_with.is_some()));
    drop(state);
    if !shared {
        return;
    }
    super::peer::changed(core);
    told(core);
    let now = held(core);
    if *core.peer().shares.held.locked() == now {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        if let Some(registry) = shares::refresh(&core).await {
            reconcile(&core, registry).await;
        }
    });
}

/// What reconcile asks of the account, or of a board whose share has ended.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Register(ShareRegistration),
    Remove(ShareRemoval),
    End { project: String, org: String },
}

/// What this host does about the registry. It never registers a share: the person's Share tap
/// does. A share the registry no longer lists with an org it lists has ended, unless its project
/// was renamed and has yet to move; an org the registry leaves out, as it does one whose Team plan
/// is unpaid, keeps its share. It moves a registration to this host when this host is the
/// project's home, and takes back this host's own registrations for projects no longer shared
/// there. It never removes another host's.
fn plan(
    held: &[Held],
    registry: &ShareRegistry,
    host: &str,
    linked: &[String],
    renaming: &[String],
) -> Vec<Step> {
    let mut steps = Vec::new();
    for item in held {
        for list in &registry.orgs {
            let wanted = item.org.as_deref() == Some(list.org.as_str());
            let org = list.org.clone();
            let project = item.project.clone();
            match (list.mine.iter()).find(|mine| mine.project == item.project) {
                None if wanted && !renaming.contains(&item.project) => {
                    steps.push(Step::End { project, org })
                }
                Some(mine) if mine.host == host && !wanted => {
                    steps.push(Step::Remove(ShareRemoval { org, project }))
                }
                Some(mine)
                    if mine.host != host && wanted && takes_over(item, &mine.host, linked) =>
                {
                    let host = host.to_owned();
                    steps.push(Step::Register(ShareRegistration { org, project, host }))
                }
                _ => {}
            }
        }
    }
    steps
}

/// Acts on the registry as the account answered it, once renamed projects' shares have moved.
/// A registry read before the boards were may be stale, so before acting it is read once more.
async fn reconcile(core: &Arc<Core>, mut registry: ShareRegistry) {
    let Some(host) = core.remote().core_id() else {
        return;
    };
    let _running = core.peer().shares.running.lock().await;
    let linked: Vec<String> = (core.remote().links().into_iter())
        .map(|link| link.id)
        .collect();
    if move_renamed(core, &registry, &host, &linked).await {
        let Some(again) = shares::refresh(core).await else {
            return;
        };
        registry = again;
    }
    let held = held(core);
    *core.peer().shares.held.locked() = held.clone();
    if plan(&held, &registry, &host, &linked, &renaming(core)).is_empty() {
        return;
    }
    let Some(registry) = shares::refresh(core).await else {
        return;
    };
    let mut ended = false;
    for step in plan(&held, &registry, &host, &linked, &renaming(core)) {
        let outcome = match step {
            Step::Register(registration) => shares::register(core, &registration).await,
            Step::Remove(removal) => shares::remove(core, &removal).await,
            Step::End { project, org } => {
                ended = true;
                end_share(core, &project, org).await;
                continue;
            }
        };
        if let Err(error) = outcome {
            eprintln!(
                "sikemux core: a shared project's registration did not reach the account: {error}"
            );
        }
    }
    if ended {
        super::peer::changed(core);
        core.changed(What::View);
    }
}

/// Clears the board's setting of a share the account no longer lists, so a share ended on one of
/// the person's hosts ends on all of them.
async fn end_share(core: &Arc<Core>, project: &str, org: String) {
    let Some(folder) = (core.workspaces().projects().into_iter())
        .find(|known| known.identity.as_deref().map(key).as_deref() == Some(project))
        .map(|known| known.path.to_string_lossy().into_owned())
    else {
        return;
    };
    if let Err(error) = board::share_ended(core, folder, org).await {
        eprintln!("sikemux core: an ended share stayed on its board: {error}");
    }
}

/// A renamed project's registration moving to its new name: registered under the new name first,
/// when it is still shared here, then removed under the old.
#[derive(Debug, PartialEq, Eq)]
struct Move {
    register: Option<ShareRegistration>,
    remove: ShareRemoval,
}

/// How the registrations under a renamed project's old key move to `item`, its new key, and
/// whether any is left for another host to move. A registration moves only with an org the board
/// is shared with, and only by the host that acts for it, so a host that sees the rename first
/// never moves its home's share out from under it.
fn plan_move(
    old: &str,
    item: &Held,
    registry: &ShareRegistry,
    host: &str,
    linked: &[String],
) -> (Vec<Move>, bool) {
    let mut moves = Vec::new();
    let mut waiting = false;
    for list in &registry.orgs {
        let Some(mine) = (list.mine.iter()).find(|mine| mine.project == old) else {
            continue;
        };
        let remove = ShareRemoval {
            org: list.org.clone(),
            project: old.to_owned(),
        };
        let wanted = item.org.as_deref() == Some(list.org.as_str());
        if wanted && (mine.host == host || takes_over(item, &mine.host, linked)) {
            let register = ShareRegistration {
                org: list.org.clone(),
                project: item.project.clone(),
                host: host.to_owned(),
            };
            moves.push(Move {
                register: Some(register),
                remove,
            });
        } else if mine.host == host {
            moves.push(Move {
                register: None,
                remove,
            });
        } else {
            waiting = true;
        }
    }
    (moves, waiting)
}

/// Moves each renamed project's registrations to its new name. A rename stays to try again while
/// any move did not reach the account, or a registration under the old name waits for another
/// host to move it. Answers whether anything reached the account.
async fn move_renamed(
    core: &Arc<Core>,
    heard: &ShareRegistry,
    host: &str,
    linked: &[String],
) -> bool {
    let renames = core.peer().shares.renames.locked().clone();
    let mut registry = None;
    let mut reached = false;
    for (old, new) in renames {
        let held = held(core);
        let Some(item) = (held.iter()).find(|item| item.project == key(&new)) else {
            continue;
        };
        let (moves, waiting) = plan_move(&key(&old), item, heard, host, linked);
        if !moves.is_empty() && registry.is_none() {
            registry = shares::refresh(core).await;
        }
        let (moves, waiting) = match &registry {
            Some(fresh) if !moves.is_empty() => plan_move(&key(&old), item, fresh, host, linked),
            None if !moves.is_empty() => return reached,
            _ => (moves, waiting),
        };
        let mut moved = !waiting;
        for Move { register, remove } in moves {
            let registered = match register {
                Some(registration) => shares::register(core, &registration).await,
                None => Ok(()),
            };
            match registered.and(shares::remove(core, &remove).await) {
                Ok(()) => {
                    reached = true;
                    registry = None;
                }
                Err(error) => {
                    moved = false;
                    eprintln!(
                        "sikemux core: a renamed project's share did not move on the account: {error}"
                    );
                }
            }
        }
        if moved {
            (core.peer().shares).change_renames(|renames| {
                renames.retain(|rename| *rename != (old.clone(), new.clone()))
            });
        }
    }
    reached
}

/// Adds renames to the pending ones. A pending rename to a name now renamed again moves straight
/// to the newest name, so a project renamed twice before its share moved keeps it.
fn chain(pending: &mut Vec<(String, String)>, renamed: &[(String, String)]) {
    for (old, new) in renamed {
        for rename in pending.iter_mut().filter(|rename| rename.1 == *old) {
            rename.1 = new.clone();
        }
        pending.push((old.clone(), new.clone()));
    }
    let mut kept: Vec<(String, String)> = Vec::new();
    for rename in pending.drain(..) {
        if rename.0 != rename.1 && !kept.contains(&rename) {
            kept.push(rename);
        }
    }
    *pending = kept;
}

/// Projects are about to be known by new names: their shares move at the next reconcile, which
/// starts now.
pub(crate) fn renamed(core: &Core, renamed: &[(String, String)]) {
    (core.peer().shares).change_renames(|renames| chain(renames, renamed));
    let Some(core) = core.arc() else {
        return;
    };
    tokio::spawn(async move {
        if let Some(registry) = shares::refresh(&core).await {
            reconcile(&core, registry).await;
        }
    });
}

/// Shares the project's board with `org`, in place of any org it was shared with from any of the
/// person's hosts.
pub(crate) async fn share(
    core: &Arc<Core>,
    params: sikemux_calls::board::ShareProjectParams,
) -> Result<ProjectSharing, String> {
    if !active(core, &params.org) {
        return Err("the board can be shared only with an org whose Team plan is paid".into());
    }
    let (folder, identity) =
        project(core, &params.project).ok_or("the project has no repository to share it by")?;
    let host = (core.remote().core_id())
        .ok_or("this computer isn't signed in to the account, so the board can't be shared")?;
    let project = key(&identity);
    let running = core.peer().shares.running.lock().await;
    let registry = shares::refresh(core).await;
    let registration = ShareRegistration {
        org: params.org.clone(),
        project: project.clone(),
        host,
    };
    shares::register(core, &registration)
        .await
        .map_err(|error| {
            format!("the account didn't take the share, so the board isn't shared: {error}")
        })?;
    let before = board::person_share(core, folder.clone(), Some(params.org.clone())).await?;
    let account = orgs(core).into_iter().map(|org| org.id);
    let mut others = (registry.as_ref()).map_or_else(Vec::new, |registry| {
        removals(registry, std::slice::from_ref(&project), account)
    });
    if let Some(org) = before.filter(|before| !others.iter().any(|other| other.org == *before)) {
        others.push(ShareRemoval {
            org,
            project: project.clone(),
        });
    }
    others.retain(|removal| removal.org != params.org);
    for removal in others {
        if let Err(error) = shares::remove(core, &removal).await {
            eprintln!("sikemux core: a project shared elsewhere stayed on its old org: {error}");
        }
    }
    drop(running);
    shares::refresh(core).await;
    super::peer::changed(core);
    core.changed(What::View);
    Ok(sharing(core, &params.project).await)
}

/// What taking a share back removes from the account: the project under each of `keys` from every org the
/// registry lists it with, and from every org the registry leaves out, as it does one whose Team
/// plan is unpaid, since another host may have registered it there.
fn removals(
    registry: &ShareRegistry,
    keys: &[String],
    orgs: impl IntoIterator<Item = String>,
) -> Vec<ShareRemoval> {
    let mut removals: Vec<ShareRemoval> = (registry.orgs.iter())
        .flat_map(|list| {
            (list.mine.iter())
                .filter(|mine| keys.contains(&mine.project))
                .map(|mine| ShareRemoval {
                    org: list.org.clone(),
                    project: mine.project.clone(),
                })
        })
        .collect();
    let mut unlisted: Vec<String> = Vec::new();
    for org in orgs {
        if !(registry.orgs.iter()).any(|list| list.org == org) && !unlisted.contains(&org) {
            unlisted.push(org);
        }
    }
    for org in unlisted {
        removals.extend(keys.iter().map(|project| ShareRemoval {
            org: org.clone(),
            project: project.clone(),
        }));
    }
    removals
}

/// Stops sharing the project's board, at once and whether or not its org is paid. The account
/// drops the share first, under the project's name and any older name it has yet to move from;
/// the board's setting is cleared only once that took, so the board never shows "Only you" while
/// an org still holds it.
pub(crate) async fn unshare(
    core: &Arc<Core>,
    params: sikemux_calls::board::BoardParams,
) -> Result<ProjectSharing, String> {
    let (folder, identity) =
        project(core, &params.project).ok_or("the project has no repository to share it by")?;
    let running = core.peer().shares.running.lock().await;
    let registry = shares::refresh(core)
        .await
        .ok_or("the account couldn't be reached, so the board is still shared")?;
    let mut keys = vec![key(&identity)];
    keys.extend(
        (core.peer().shares.renames.locked().iter())
            .filter(|(_, new)| *new == identity)
            .map(|(old, _)| key(old)),
    );
    let orgs = (orgs(core).into_iter().map(|org| org.id)).chain(shared_with(core, &folder));
    for removal in removals(&registry, &keys, orgs) {
        shares::remove(core, &removal).await.map_err(|error| {
            format!("the account didn't drop the share, so the board is still shared: {error}")
        })?;
    }
    board::person_share(core, folder, None).await?;
    (core.peer().shares).change_renames(|renames| renames.retain(|(_, new)| *new != identity));
    drop(running);
    shares::refresh(core).await;
    super::peer::changed(core);
    core.changed(What::View);
    Ok(sharing(core, &params.project).await)
}

/// Who the project's board is shared with, as the account holds it whichever host registered it,
/// and the orgs it could be shared with. The board's setting stands in while the account is unread
/// or leaves the setting's org out, as it does one whose Team plan is unpaid.
pub(crate) async fn sharing(core: &Arc<Core>, project_named: &str) -> ProjectSharing {
    let registry = shares::read(core).await;
    let list = |org: &str| -> Option<&OrgShareList> {
        (registry.as_ref()?.orgs.iter()).find(|list| list.org == org)
    };
    let orgs = orgs(core);
    let shared = project(core, project_named).and_then(|(folder, identity)| {
        let setting = shared_with(core, &folder);
        let Some(registry) = &registry else {
            return setting;
        };
        let project = key(&identity);
        let held = (registry.orgs.iter())
            .find(|list| list.mine.iter().any(|mine| mine.project == project))
            .map(|list| list.org.clone());
        held.or(setting.filter(|org| list(org).is_none()))
    });
    let shared = shared.map(|org| SharedOrg {
        name: org_name(core, &org).unwrap_or_default(),
        org,
    });
    ProjectSharing {
        project: project_named.to_owned(),
        shared: shared.map(|SharedOrg { org, name }| SharedWith {
            name,
            on: active(core, &org),
            people: (list(&org).into_iter())
                .flat_map(|list| &list.members)
                .map(|member| Teammate {
                    member: member.id.clone(),
                    name: member.name.clone(),
                    picture: member.image_url.clone(),
                    you: member.you,
                })
                .collect(),
            org,
        }),
        orgs: (orgs.iter())
            .filter(|org| org.active)
            .map(|org| OrgChoice {
                org: org.id.clone(),
                name: org.name.clone(),
                people: list(&org.id).map_or(0, |list| list.members.len() as u32),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::protocol::SharedProject;

    fn registry(orgs: &[(&str, &[(&str, &str)])]) -> ShareRegistry {
        ShareRegistry {
            orgs: (orgs.iter())
                .map(|(org, mine)| OrgShareList {
                    org: (*org).into(),
                    members: Vec::new(),
                    shares: Vec::new(),
                    mine: (mine.iter())
                        .map(|(project, host)| SharedProject {
                            project: (*project).into(),
                            member: "member".into(),
                            host: (*host).into(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn held(project: &str, org: Option<&str>, chief_here: bool) -> Held {
        Held {
            project: project.into(),
            org: org.map(Into::into),
            chief_here,
        }
    }

    #[test]
    fn a_rename_seen_first_off_the_home_leaves_the_home_its_share() {
        let registry = registry(&[("org", &[("old", "home")])]);
        let item = held("new", Some("org"), false);
        let linked = ["home".to_owned()];
        assert_eq!(
            plan_move("old", &item, &registry, "other", &linked),
            (Vec::new(), true)
        );
    }

    #[test]
    fn sharing_with_another_org_takes_back_one_another_host_made() {
        let registry = registry(&[("a", &[("project", "other")]), ("b", &[])]);
        let keys = ["project".to_owned()];
        let orgs = ["a".to_owned(), "b".to_owned(), "unpaid".to_owned()];
        let orgs: Vec<String> = (removals(&registry, &keys, orgs).into_iter())
            .map(|removal| removal.org)
            .collect();
        assert_eq!(orgs, ["a", "unpaid"]);
    }

    #[test]
    fn unshare_reaches_the_old_name_of_a_rename_yet_to_move() {
        let registry = registry(&[("org", &[("old", "other")])]);
        let keys = ["new".to_owned(), "old".to_owned()];
        assert_eq!(
            removals(&registry, &keys, ["org".to_owned()]),
            [ShareRemoval {
                org: "org".into(),
                project: "old".into()
            }]
        );
    }

    #[test]
    fn a_project_renamed_twice_moves_from_its_first_name() {
        let mut pending = vec![("a".to_owned(), "b".to_owned())];
        chain(&mut pending, &[("b".to_owned(), "c".to_owned())]);
        assert_eq!(
            pending,
            [
                ("a".to_owned(), "c".to_owned()),
                ("b".to_owned(), "c".to_owned())
            ]
        );
    }

    #[test]
    fn a_share_whose_project_is_still_moving_does_not_end() {
        let registry = registry(&[("org", &[("old", "host")])]);
        let held = [held("new", Some("org"), true)];
        let renaming = ["new".to_owned()];
        assert_eq!(plan(&held, &registry, "host", &[], &renaming), []);
    }

    #[test]
    fn unshare_reaches_an_unpaid_org_another_host_registered_in() {
        let registry = registry(&[("paid", &[])]);
        let keys = ["project".to_owned()];
        let orgs = ["paid".to_owned(), "unpaid".to_owned()];
        assert_eq!(
            removals(&registry, &keys, orgs),
            [ShareRemoval {
                org: "unpaid".into(),
                project: "project".into()
            }]
        );
    }
}
