//! What the account's other hosts hold of its projects (ADR 0023). Each host tells every linked
//! host its projects when a link comes up and whenever they change, and keeps what the others
//! told it, so it can show one project for each repository, with a copy on each host that
//! holds one.

use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::workspace::{ChiefHome, ProjectCopy, ProjectElsewhere, ProjectGit, Workspace};

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
    /// The project's chief on that host, by agent id, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chief: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    chief_count: u32,
    /// When the person last wrote to that chief, in Unix milliseconds, or 0 if never.
    #[serde(default, skip_serializing_if = "never")]
    chief_wrote: u64,
}

fn never(at: &u64) -> bool {
    *at == 0
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// A host's chief of a project, as it counts.
struct Rank<'a> {
    count: u32,
    wrote: u64,
    host: &'a str,
}

impl Rank<'_> {
    /// The higher count wins. Between equal counts the one the person wrote to last wins, so the
    /// chief they are using stays, and only then the host with the lower key, so every host
    /// picks the same one.
    fn over(&self, other: &Rank) -> bool {
        fn key<'b>(rank: &Rank<'b>) -> (u32, u64, std::cmp::Reverse<&'b str>) {
            (rank.count, rank.wrote, std::cmp::Reverse(rank.host))
        }
        key(self) > key(other)
    }
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

    /// The highest chief count a linked host told for the repository `identity`.
    pub(crate) fn chief_count(&self, identity: &str) -> u32 {
        (self.heard.locked().values().flatten())
            .filter(|held| held.identity.as_deref() == Some(identity))
            .map(|held| held.chief_count)
            .max()
            .unwrap_or_default()
    }

    /// The chiefs `host` said it holds: each one's repository, agent id and count.
    pub(super) fn chiefs_on(&self, host: &str) -> Vec<(String, String, u32)> {
        (self.heard.locked().get(host).into_iter().flatten())
            .filter_map(|held| {
                Some((
                    held.identity.clone()?,
                    held.chief.clone()?,
                    held.chief_count,
                ))
            })
            .collect()
    }

    /// The linked hosts that said they hold the repository `identity`.
    pub(super) fn holders(&self, identity: &str) -> Vec<String> {
        (self.heard.locked().iter())
            .filter(|(_, held)| {
                (held.iter()).any(|held| held.identity.as_deref() == Some(identity))
            })
            .map(|(host, _)| host.clone())
            .collect()
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
    /// the same repository, else in a project of its own among those only elsewhere. A project
    /// here with no chief of its own takes the first linked host's that has one as its home.
    pub(crate) fn fold(&self, core: &Core, workspace: &mut Workspace) {
        let heard = self.heard.locked().clone();
        let me = core.remote().core_id().unwrap_or_default();
        let mut homes: BTreeMap<String, (u32, u64, String)> = BTreeMap::new();
        for link in core.remote().links() {
            let Some(held) = heard.get(&link.id) else {
                continue;
            };
            let away = core.hosts().connected(&link.id).is_none();
            let away_since = core.hosts().away_since(&link.id);
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
                    let theirs = Rank {
                        count: project.chief_count,
                        wrote: project.chief_wrote,
                        host: &link.id,
                    };
                    let ours = here.chief.as_ref().map(|chief| Rank {
                        count: here.chief_count,
                        wrote: crate::server::chief::person_wrote_at(core, chief).unwrap_or(0),
                        host: &me,
                    });
                    let best = homes.get(identity).map(|(count, wrote, host)| Rank {
                        count: *count,
                        wrote: *wrote,
                        host,
                    });
                    let wins = ours.is_none_or(|ours| theirs.over(&ours))
                        && best.is_none_or(|best| theirs.over(&best));
                    if let Some(chief) = project.chief.clone().filter(|_| wins) {
                        homes.insert(
                            identity.clone(),
                            (project.chief_count, project.chief_wrote, link.id.clone()),
                        );
                        here.chief_home = Some(ChiefHome {
                            host: link.id.clone(),
                            host_name: link.name.clone(),
                            agent_id: chief,
                            away_since,
                        });
                    }
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
            chief_count: project.chief_count,
            chief_wrote: (project.chief.as_deref())
                .and_then(|chief| crate::server::chief::person_wrote_at(core, chief))
                .unwrap_or(0),
            chief: project.chief,
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
        stand_down(core);
        core.changed(What::View);
    }
    Ok(serde_json::Value::Null)
}

/// A chief's turn ended, so it may stand down now if it was waiting to.
pub(crate) fn chief_rested(core: &Core, agent_id: &str) {
    if crate::server::chief::is_chief(core, agent_id) {
        stand_down(core);
    }
}

/// Stands this host's chiefs down where another host's outranks them, as when two hosts each
/// made one. Each closes once it is out of its turn, and its chat stays in Recent.
fn stand_down(core: &Core) {
    for project in core.workspace_view().projects {
        let (Some(chief), Some(home)) = (project.chief, project.chief_home) else {
            continue;
        };
        if core.chats().at_work(&chief) || core.agent_now(&chief).mid_turn() {
            continue;
        }
        if let Err(error) = core.workspaces().stand_down(&project.id) {
            eprintln!(
                "sikemux core: @Chief of {} did not stand down: {error}",
                project.name
            );
            continue;
        }
        if core.workspaces().agent(&chief).is_some() {
            if let Err(error) = super::super::workspace::close(core, &[chief]) {
                eprintln!("sikemux core: the stood-down @Chief did not close: {error}");
            }
        }
        eprintln!(
            "sikemux core: @Chief of {} stood down for {}'s",
            project.name, home.host_name
        );
    }
}
