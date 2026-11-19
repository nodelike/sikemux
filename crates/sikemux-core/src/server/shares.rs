//! A project's board shared with one org the account is in. The project's home keeps the share in
//! the board's settings and tells the account server it is the host to dial for it. A share starts
//! only at the person's Share tap. Nothing is shared, registered or served for an org whose Team
//! plan is unpaid; the share stays in place.

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

/// The org the project's board is shared with, as both the rail and the Share control show it.
pub(crate) fn shared_org(core: &Core, folder: &str) -> Option<SharedOrg> {
    let org = shared_with(core, folder)?;
    let name = org_name(core, &org).unwrap_or_default();
    Some(SharedOrg { org, name })
}

/// One reconcile at a time, the renames still to move, and the shares it last acted on.
#[derive(Default)]
pub(crate) struct Reconciling {
    running: tokio::sync::Mutex<()>,
    /// What renamed projects were known by before and are known by now.
    renames: Mutex<Vec<(String, String)>>,
    held: Mutex<Vec<Held>>,
}

/// A project this host keeps the record of: its key, the org its board is shared with, and
/// whether its chief is here.
#[derive(Clone, PartialEq, Eq)]
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
    core.changed(What::View);
}

/// A board this host keeps was saved. Teammates look again when one of them is shared, and the
/// registry is read again only when what this host shares, or where, changed.
pub(crate) fn records_changed(core: &Arc<Core>, keys: &[String]) {
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

/// Acts on the registry as the account just answered it. It never registers a share: the person's
/// Share tap does. A share the registry no longer lists with an org it lists has ended, and its
/// board's setting is cleared; an org the registry leaves out, as it does one whose Team plan is
/// unpaid, keeps its share. It moves a registration to this host when this host is the project's home,
/// moves the registration of a renamed project to its new name, and takes back this host's own
/// registrations for projects no longer shared there. While no host holds the project's chief,
/// another of the person's hosts can count as its home too, so a registration a linked host made
/// is left alone; the host holding the chief, the one home, takes it over, as it does one from a
/// host no longer linked. It never removes another host's.
async fn reconcile(core: &Arc<Core>, mut registry: ShareRegistry) {
    let Some(host) = core.remote().core_id() else {
        return;
    };
    let _running = core.peer().shares.running.lock().await;
    if move_renamed(core, &registry).await {
        let Some(again) = shares::refresh(core).await else {
            return;
        };
        registry = again;
    }
    let held = held(core);
    *core.peer().shares.held.locked() = held.clone();
    let linked: Vec<String> = (core.remote().links().into_iter())
        .map(|link| link.id)
        .collect();
    let mut ended = Vec::new();
    for Held {
        project,
        org,
        chief_here,
    } in held
    {
        for list in &registry.orgs {
            let wanted = org.as_deref() == Some(list.org.as_str());
            let Some(mine) = (list.mine.iter()).find(|mine| mine.project == project) else {
                if wanted {
                    ended.push((project.clone(), list.org.clone()));
                }
                continue;
            };
            let outcome = if mine.host == host && !wanted {
                let removal = ShareRemoval {
                    org: list.org.clone(),
                    project: project.clone(),
                };
                shares::remove(core, &removal).await
            } else if mine.host != host && wanted && (chief_here || !linked.contains(&mine.host)) {
                let registration = ShareRegistration {
                    org: list.org.clone(),
                    project: project.clone(),
                    host: host.clone(),
                };
                shares::register(core, &registration).await
            } else {
                continue;
            };
            if let Err(error) = outcome {
                eprintln!(
                    "sikemux core: a shared project's registration did not reach the account: {error}"
                );
            }
        }
    }
    if !ended.is_empty() {
        end_shares(core, ended).await;
    }
}

/// Clears the board's setting of each share the account no longer lists, so a share ended on one
/// of the person's hosts ends on all of them. The registry is read once more first: one read
/// before a Share tap saved its board's setting would not list the share yet. A renamed project's
/// share is left until it has moved.
async fn end_shares(core: &Arc<Core>, ended: Vec<(String, String)>) {
    let Some(registry) = shares::refresh(core).await else {
        return;
    };
    let renaming: Vec<String> = (core.peer().shares.renames.locked().iter())
        .map(|(_, new)| key(new))
        .collect();
    for (project, org) in ended {
        let Some(list) = (registry.orgs.iter()).find(|list| list.org == org) else {
            continue;
        };
        if renaming.contains(&project) || list.mine.iter().any(|mine| mine.project == project) {
            continue;
        }
        let Some(folder) = (core.workspaces().projects().into_iter())
            .find(|known| known.identity.as_deref().map(key).as_deref() == Some(project.as_str()))
            .map(|known| known.path.to_string_lossy().into_owned())
        else {
            continue;
        };
        if let Err(error) = board::share_ended(core, folder, org).await {
            eprintln!("sikemux core: an ended share stayed on its board: {error}");
        }
    }
    super::peer::changed(core);
    core.changed(What::View);
}

/// Moves each renamed project's registrations to its new name, registering the new name before
/// taking back the old. A rename moves only a share the registry still lists under the old name
/// with an org the board is shared with; a registration this host made under the old name for
/// any other org is taken back. A rename stays to try again while any of that did not reach the
/// account. Answers whether anything reached it.
async fn move_renamed(core: &Arc<Core>, registry: &ShareRegistry) -> bool {
    let renames = core.peer().shares.renames.locked().clone();
    let host = core.remote().core_id();
    let mut reached = false;
    for (old, new) in renames {
        let Some((folder, _)) = project(core, &new) else {
            continue;
        };
        let shared = shared_with(core, &folder);
        let (before, after) = (key(&old), key(&new));
        let mut moved = true;
        for list in &registry.orgs {
            let Some(mine) = (list.mine.iter()).find(|mine| mine.project == before) else {
                continue;
            };
            let removal = ShareRemoval {
                org: list.org.clone(),
                project: before.clone(),
            };
            let outcome = if shared.as_deref() == Some(list.org.as_str()) {
                let registration = ShareRegistration {
                    org: list.org.clone(),
                    project: after.clone(),
                    host: mine.host.clone(),
                };
                match shares::register(core, &registration).await {
                    Ok(()) => shares::remove(core, &removal).await,
                    Err(error) => Err(error),
                }
            } else if host.as_deref() == Some(mine.host.as_str()) {
                shares::remove(core, &removal).await
            } else {
                continue;
            };
            match outcome {
                Ok(()) => reached = true,
                Err(error) => {
                    moved = false;
                    eprintln!(
                        "sikemux core: a renamed project's share did not move on the account: {error}"
                    );
                }
            }
        }
        if moved {
            (core.peer().shares.renames.locked())
                .retain(|rename| *rename != (old.clone(), new.clone()));
        }
    }
    reached
}

/// Projects are about to be known by new names: their shares move at the next reconcile, which
/// starts now.
pub(crate) fn renamed(core: &Core, renamed: &[(String, String)]) {
    core.peer()
        .shares
        .renames
        .locked()
        .extend_from_slice(renamed);
    let Some(core) = core.arc() else {
        return;
    };
    tokio::spawn(async move {
        if let Some(registry) = shares::refresh(&core).await {
            reconcile(&core, registry).await;
        }
    });
}

/// Shares the project's board with `org`, in place of any org it was shared with.
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
    let before = board::person_share(core, folder.clone(), Some(params.org.clone())).await?;
    if let Some(org) = before.filter(|before| *before != params.org) {
        take_back(core, org, &identity).await;
    }
    shares::refresh(core).await;
    super::peer::changed(core);
    core.changed(What::View);
    Ok(sharing(core, &params.project).await)
}

/// Stops sharing the project's board, at once and whether or not its org is paid.
pub(crate) async fn unshare(
    core: &Arc<Core>,
    params: sikemux_calls::board::BoardParams,
) -> Result<ProjectSharing, String> {
    let (folder, identity) =
        project(core, &params.project).ok_or("the project has no repository to share it by")?;
    if let Some(org) = board::person_share(core, folder, None).await? {
        take_back(core, org, &identity).await;
    }
    super::peer::changed(core);
    core.changed(What::View);
    Ok(sharing(core, &params.project).await)
}

/// Ends the project's share with `org` at the person's tap, whichever of their hosts registered it.
async fn take_back(core: &Arc<Core>, org: String, identity: &str) {
    let removal = ShareRemoval {
        org,
        project: key(identity),
    };
    if let Err(error) = shares::remove(core, &removal).await {
        eprintln!("sikemux core: an unshared project stayed on the account's registry: {error}");
    }
}

/// Who the project's board is shared with, and the orgs it could be shared with.
pub(crate) async fn sharing(core: &Arc<Core>, project_named: &str) -> ProjectSharing {
    let registry = shares::read(core).await;
    let list = |org: &str| -> Option<&OrgShareList> {
        (registry.as_ref()?.orgs.iter()).find(|list| list.org == org)
    };
    let orgs = orgs(core);
    let folder = project(core, project_named).map(|(folder, _)| folder);
    let shared = (folder.as_deref()).and_then(|folder| shared_org(core, folder));
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
