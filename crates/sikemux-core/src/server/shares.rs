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

use crate::accounts::protocol::{
    OrgShareList, ShareMove, ShareRegistration, ShareRegistry, ShareRemoval, ShareRename,
};
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

/// The keys the account may list the project by: its name, and the old names of a rename whose
/// share has yet to move.
fn keys(core: &Core, identity: &str) -> Vec<String> {
    let mut keys = vec![key(identity)];
    keys.extend(
        (core.peer().shares.renames.locked().iter())
            .filter(|(_, new)| new == identity)
            .map(|(old, _)| key(old)),
    );
    keys
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

/// What reconcile asks of the account, or of a board this host keeps.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Move(ShareMove),
    /// The board's setting follows the account, unless it changed from `was` meanwhile.
    Setting {
        project: String,
        was: Option<String>,
        now: Option<String>,
    },
}

/// What this host does about the registry for the projects it keeps the boards of. The account
/// is the fact: a board takes the org the account lists its project under, whatever a board sent
/// late said, and the registration moves to this host. A share the registry no longer lists with
/// an org it lists has ended, unless its project was renamed and has yet to move; an org the
/// registry leaves out, as it does one whose Team plan is unpaid, keeps its share. It never starts
/// a share and never removes a registration.
fn plan(
    held: &[Held],
    registry: &ShareRegistry,
    host: &str,
    linked: &[String],
    renaming: &[String],
) -> Vec<Step> {
    let mut steps = Vec::new();
    for item in held {
        let listed: Vec<(&str, &str)> = (registry.orgs.iter())
            .filter_map(|list| {
                let mine = (list.mine.iter()).find(|mine| mine.project == item.project)?;
                Some((list.org.as_str(), mine.host.as_str()))
            })
            .collect();
        let account = (listed.iter())
            .find(|(org, _)| item.org.as_deref() == Some(*org))
            .or(listed.first());
        let now = match account {
            Some(&(org, at)) => {
                if at != host && takes_over(item, at, linked) {
                    steps.push(Step::Move(ShareMove {
                        org: org.to_owned(),
                        project: item.project.clone(),
                        host: host.to_owned(),
                    }));
                }
                Some(org.to_owned())
            }
            None => item.org.clone().filter(|org| {
                renaming.contains(&item.project)
                    || !(registry.orgs.iter()).any(|list| list.org == *org)
            }),
        };
        if now != item.org {
            steps.push(Step::Setting {
                project: item.project.clone(),
                was: item.org.clone(),
                now,
            });
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
    if move_renamed(core).await {
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
    let mut followed = false;
    for step in plan(&held, &registry, &host, &linked, &renaming(core)) {
        match step {
            Step::Move(share) => {
                if let Err(error) = shares::move_share(core, &share).await {
                    eprintln!("sikemux core: a shared project did not move here: {error}");
                }
            }
            Step::Setting { project, was, now } => {
                followed = true;
                follow_account(core, &project, was, now).await;
            }
        }
    }
    if followed {
        super::peer::changed(core);
        core.changed(What::View);
    }
}

/// Sets the board's share to what the account lists, so a share made or ended on one of the
/// person's hosts holds on all of them.
async fn follow_account(core: &Arc<Core>, project: &str, was: Option<String>, now: Option<String>) {
    let Some(folder) = (core.workspaces().projects().into_iter())
        .find(|known| known.identity.as_deref().map(key).as_deref() == Some(project))
        .map(|known| known.path.to_string_lossy().into_owned())
    else {
        return;
    };
    if let Err(error) = board::share_follows_account(core, folder, was, now).await {
        eprintln!("sikemux core: a board's share did not follow the account: {error}");
    }
}

/// Moves each renamed project's share to its new name. The account's answer settles a rename,
/// whether it had a share to move or none; only a call that failed leaves it for the next try.
/// Answers whether a share moved.
async fn move_renamed(core: &Arc<Core>) -> bool {
    let renames = core.peer().shares.renames.locked().clone();
    let mut moved = false;
    for (old, new) in renames {
        let rename = ShareRename {
            from: key(&old),
            project: key(&new),
        };
        match shares::rename_share(core, &rename).await {
            Ok(had) => {
                moved |= had;
                (core.peer().shares).change_renames(|renames| {
                    renames.retain(|pending| *pending != (old.clone(), new.clone()))
                });
            }
            Err(error) => eprintln!(
                "sikemux core: a renamed project's share did not move on the account: {error}"
            ),
        }
    }
    moved
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
    let running = core.peer().shares.running.lock().await;
    let registration = ShareRegistration {
        org: params.org.clone(),
        project: key(&identity),
        host,
    };
    shares::register(core, &registration)
        .await
        .map_err(|error| {
            format!("the account didn't take the share, so the board isn't shared: {error}")
        })?;
    board::person_share(core, folder, Some(params.org.clone())).await?;
    shares::refresh(core).await;
    drop(running);
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
    let orgs = (orgs(core).into_iter().map(|org| org.id)).chain(shared_with(core, &folder));
    for removal in removals(&registry, &keys(core, &identity), orgs) {
        shares::remove(core, &removal).await.map_err(|error| {
            format!("the account didn't drop the share, so the board is still shared: {error}")
        })?;
    }
    board::person_share(core, folder, None).await?;
    shares::refresh(core).await;
    drop(running);
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
        let keys = keys(core, &identity);
        let held = (registry.orgs.iter())
            .find(|list| list.mine.iter().any(|mine| keys.contains(&mine.project)))
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
    fn a_late_board_from_a_departed_home_does_not_undo_a_newer_share() {
        let registry = registry(&[("x", &[]), ("y", &[("project", "here")])]);
        let held = [held("project", Some("x"), true)];
        assert_eq!(
            plan(&held, &registry, "here", &["away".to_owned()], &[]),
            [Step::Setting {
                project: "project".into(),
                was: Some("x".into()),
                now: Some("y".into()),
            }]
        );
    }

    #[test]
    fn a_new_home_takes_the_share_a_departing_home_never_sent() {
        let registry = registry(&[("x", &[("project", "away")])]);
        let held = [held("project", None, true)];
        assert_eq!(
            plan(&held, &registry, "here", &["away".to_owned()], &[]),
            [
                Step::Move(ShareMove {
                    org: "x".into(),
                    project: "project".into(),
                    host: "here".into(),
                }),
                Step::Setting {
                    project: "project".into(),
                    was: None,
                    now: Some("x".into()),
                },
            ]
        );
    }

    #[test]
    fn a_share_with_an_org_the_registry_leaves_out_never_ends() {
        let registry = registry(&[("paid", &[])]);
        let held = [held("project", Some("unpaid"), true)];
        assert_eq!(plan(&held, &registry, "here", &[], &[]), []);
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
