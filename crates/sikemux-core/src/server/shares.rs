//! A project's board shared with one org the account is in. The project's home keeps the share in
//! the board's settings and tells the account server it is the host to dial for it. Nothing is
//! shared, registered or served for an org whose Team plan is unpaid; the share stays in place.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use sikemux_view::device::AccountOrg;
use sikemux_view::teammates::{OrgChoice, ProjectSharing, SharedOrg, SharedWith, Teammate};

use crate::accounts::protocol::{OrgShareList, ShareRegistration, ShareRemoval};
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

/// The registry's account answered again, or a board this host keeps was saved.
pub(crate) fn heard(core: &Arc<Core>) {
    super::peer::heard(core);
    super::memory::shares_changed(core);
    core.changed(What::View);
    let core = core.clone();
    tokio::spawn(async move { reconcile(&core).await });
}

/// A board this host keeps was saved: the registry is read again only when one of them is shared.
pub(crate) fn records_changed(core: &Arc<Core>, keys: &[String]) {
    let state = core.boards().state.locked();
    let shared = (keys.iter())
        .any(|key| (state.of(key)).is_some_and(|board| board.settings.shared_with.is_some()));
    drop(state);
    if shared {
        super::peer::changed(core);
        heard(core);
    }
}

/// Registers each project this host keeps the record of with the org it is shared with, and takes
/// back this host's own registrations for projects no longer shared there. While no host holds
/// the project's chief, another of the person's hosts can count as its home too, so a
/// registration a linked host made is left alone; the host holding the chief, the one home, takes
/// it over, as it does one from a host no longer linked. It never removes another host's.
async fn reconcile(core: &Arc<Core>) {
    let (Some(host), Some(registry)) = (core.remote().core_id(), shares::read(core).await) else {
        return;
    };
    let kept: Vec<(String, Option<String>, bool)> = (core.workspaces().projects().into_iter())
        .filter(|known| core.hosts().copies.home_of(core, known).is_none())
        .filter_map(|known| {
            let folder = known.path.to_string_lossy().into_owned();
            let chief_here = known.chief.is_some();
            Some((
                key(&known.identity?),
                shared_with(core, &folder),
                chief_here,
            ))
        })
        .collect();
    let linked: Vec<String> = (core.remote().links().into_iter())
        .map(|link| link.id)
        .collect();
    for (project, org, chief_here) in kept {
        for list in &registry.orgs {
            let wanted = org.as_deref() == Some(list.org.as_str());
            let listed = (list.mine.iter()).find(|mine| mine.project == project);
            let outcome = match listed {
                Some(mine) if mine.host == host && wanted => continue,
                Some(mine) if mine.host == host => {
                    let removal = ShareRemoval {
                        org: list.org.clone(),
                        project: project.clone(),
                    };
                    shares::remove(core, &removal).await
                }
                Some(mine) if !wanted || (!chief_here && linked.contains(&mine.host)) => continue,
                _ if wanted && active(core, &list.org) => {
                    let registration = ShareRegistration {
                        org: list.org.clone(),
                        project: project.clone(),
                        host: host.clone(),
                    };
                    shares::register(core, &registration).await
                }
                _ => continue,
            };
            if let Err(error) = outcome {
                eprintln!(
                    "sikemux core: a shared project's registration did not reach the account: {error}"
                );
            }
        }
    }
}

/// Takes back what this host registered under what renamed projects were known by before, and
/// registers them under what they are known by now.
pub(crate) fn renamed(core: &Core, renamed: &[(String, String)]) {
    let Some(core) = core.arc() else {
        return;
    };
    let before: Vec<String> = renamed.iter().map(|(old, _)| key(old)).collect();
    tokio::spawn(async move {
        let (Some(host), Some(registry)) = (core.remote().core_id(), shares::read(&core).await)
        else {
            return;
        };
        for list in &registry.orgs {
            for mine in (list.mine.iter())
                .filter(|mine| mine.host == host && before.contains(&mine.project))
            {
                let removal = ShareRemoval {
                    org: list.org.clone(),
                    project: mine.project.clone(),
                };
                if let Err(error) = shares::remove(&core, &removal).await {
                    eprintln!(
                        "sikemux core: a renamed project stayed on the account's registry: {error}"
                    );
                }
            }
        }
        reconcile(&core).await;
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
    let before = board::person_share(core, folder.clone(), Some(params.org.clone())).await?;
    if let Some(org) = before.filter(|before| *before != params.org) {
        take_back(core, org, &identity).await;
    }
    reconcile(core).await;
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
