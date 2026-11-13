//! What the account's other hosts hold of its projects (ADR 0023). Each host tells every linked
//! host its projects when a link comes up and whenever they change, and keeps what the others
//! told it, so it can show one project for each repository, with a copy on each host that
//! holds one.

use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::workspace::{ProjectCopy, ProjectElsewhere, ProjectGit, Workspace};

use super::super::changed::What;
use super::super::kept::Kept;
use super::super::wake::Wake;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::{ask, ASK_WAIT};

const KEPT_FILE: &str = "host-copies.json";

/// One project of a host, as it tells the others.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Held {
    id: String,
    name: String,
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git: Option<ProjectGit>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct HeldParams {
    projects: Vec<Held>,
}

pub(crate) struct Copies {
    /// What each linked host last said it holds, by its key.
    heard: Mutex<BTreeMap<String, Vec<Held>>>,
    kept: Kept,
    /// What this host last told each linked host, by its key.
    told: Mutex<BTreeMap<String, Vec<Held>>>,
    /// This host's projects may have changed since the linked hosts were told.
    pub(crate) wake: Wake,
}

impl Copies {
    pub(super) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, KEPT_FILE);
        Self {
            heard: Mutex::new(kept.load()),
            kept,
            told: Mutex::default(),
            wake: Wake::default(),
        }
    }

    /// Whether `host` said it holds any project.
    pub(super) fn holds(&self, host: &str) -> bool {
        self.heard
            .locked()
            .get(host)
            .is_some_and(|held| !held.is_empty())
    }

    /// Whether `host` has said what it holds since it was linked.
    pub(super) fn heard_from(&self, host: &str) -> bool {
        self.heard.locked().contains_key(host)
    }

    /// `host` is reached again, so it is told this host's projects afresh.
    pub(super) fn reached(&self, host: &str) {
        self.told.locked().remove(host);
        self.wake.now();
    }

    pub(super) fn forget(&self, host: &str) {
        self.told.locked().remove(host);
        let mut heard = self.heard.locked();
        if heard.remove(host).is_some() {
            self.kept.save(&*heard);
        }
    }

    /// Adds the copies the linked hosts hold to `workspace`: each under this host's project of
    /// the same repository, else in a project of its own among those only elsewhere.
    pub(crate) fn fold(&self, core: &Core, workspace: &mut Workspace) {
        let heard = self.heard.locked().clone();
        for link in core.remote().links() {
            let Some(held) = heard.get(&link.id) else {
                continue;
            };
            let away = core.hosts().connected(&link.id).is_none();
            for project in held {
                let copy = ProjectCopy {
                    host: link.id.clone(),
                    host_name: link.name.clone(),
                    project: project.id.clone(),
                    name: project.name.clone(),
                    path: project.path.clone(),
                    git: project.git.clone(),
                    away,
                };
                let Some(identity) = &project.identity else {
                    workspace.elsewhere.push(ProjectElsewhere {
                        name: project.name.clone(),
                        identity: None,
                        copies: vec![copy],
                    });
                    continue;
                };
                let same = |known: &Option<String>| known.as_ref() == Some(identity);
                if let Some(here) =
                    (workspace.projects.iter_mut()).find(|here| same(&here.identity))
                {
                    here.copies.push(copy);
                } else if let Some(there) =
                    (workspace.elsewhere.iter_mut()).find(|there| same(&there.identity))
                {
                    there.copies.push(copy);
                } else {
                    workspace.elsewhere.push(ProjectElsewhere {
                        name: project.name.clone(),
                        identity: Some(identity.clone()),
                        copies: vec![copy],
                    });
                }
            }
        }
    }
}

/// This host's projects, as it tells the others.
fn held_here(core: &Core) -> Vec<Held> {
    (core.workspaces().projects().into_iter())
        .map(|project| Held {
            git: core.git().shown(&project.path.to_string_lossy()).1,
            id: project.id,
            name: project.name,
            path: project.path,
            identity: project.identity,
        })
        .collect()
}

/// Tells each linked host in reach this host's projects, when they changed since it was last
/// told.
pub(crate) fn share(core: &Arc<Core>) {
    let reached: Vec<String> = core
        .hosts()
        .state
        .locked()
        .connections
        .keys()
        .cloned()
        .collect();
    if reached.is_empty() {
        return;
    }
    let held = held_here(core);
    let told = core.hosts().copies.told.locked().clone();
    for host in reached {
        if told.get(&host) == Some(&held) {
            continue;
        }
        core.hosts()
            .copies
            .told
            .locked()
            .insert(host.clone(), held.clone());
        let (core, held) = (core.clone(), held.clone());
        tokio::spawn(async move {
            let params = params_of::<asks::Copies>(&HeldParams {
                projects: held.clone(),
            });
            let sent = ask(&core, &host, asks::Copies::METHOD, params, None, ASK_WAIT).await;
            if sent.is_err_and(|failed| failed.passing()) {
                let mut told = core.hosts().copies.told.locked();
                if told.get(&host) == Some(&held) {
                    told.remove(&host);
                }
            }
        });
    }
}

/// `host` told this one what it holds.
pub(super) fn heard(core: &Core, host: &str, params: HeldParams) -> Answer {
    let copies = &core.hosts().copies;
    let mut heard = copies.heard.locked();
    if heard.get(host) != Some(&params.projects) {
        heard.insert(host.to_owned(), params.projects);
        copies.kept.save(&*heard);
        drop(heard);
        core.changed(What::View);
    }
    Ok(serde_json::Value::Null)
}
