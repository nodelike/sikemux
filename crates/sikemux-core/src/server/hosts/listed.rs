//! The Hosts group every client shows: the hosts linked with this one, and
//! the SSH hosts a terminal is open to, each with how it is reached.

use crate::server::changed::What;
use crate::server::parts::Parts;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_pty::shell_protocol::SshLink;

use super::super::changed::Topic;
use super::super::{Core, CoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    Connected,
    Connecting,
    /// A linked host this one has not reached, which it keeps trying, or an
    /// SSH host whose terminals are not connected.
    Away,
    /// Only for an SSH host: ssh gave up connecting to it.
    Failed,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostRow {
    /// The host's core key; none for an SSH host not linked with this one.
    pub id: Option<String>,
    pub name: String,
    pub platform: Option<String>,
    pub reach: Reach,
    /// The SSH host it was installed on or found at from here, or the row's own alias.
    pub ssh: Option<String>,
    /// What is being done to its SSH host from here: installing, updating or signing in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<super::install::SetupRow>,
    /// What its SSH host was found to run when last asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<super::probe::Found>,
    /// Why its SSH host has no answer yet; nothing is offered meanwhile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unanswered: Option<super::probe::Unanswered>,
    /// What setting its SSH host up takes, once it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_up: Option<super::install::SetUpWork>,
}

/// One row per host: the hosts linked with this one in the order they were linked, each with
/// the SSH host it is reached at from here, then the SSH hosts a terminal is open to or a set-up
/// runs on, by alias, one row for each host they reach.
pub(crate) fn rows(core: &Core) -> Vec<HostRow> {
    if !core.hosts().listed.swap(true, Ordering::AcqRel) {
        core.hosts().wake.now();
    }
    let links = core.remote().links();
    let links_moved = forget_unlinked(core, &links);
    let installed = core.hosts().installed.locked().clone();
    let listening = core.remote().endpoint().is_some();
    let mut setups: BTreeMap<String, super::install::SetupRow> =
        super::install::rows(core).into_iter().collect();
    let mut asking: Vec<String> = Vec::new();
    let mut rows: Vec<HostRow> = links
        .into_iter()
        .map(|link| {
            let reach = if listening {
                core.hosts().reach(&link.id)
            } else {
                Reach::Away
            };
            let ssh = installed.get(&link.id).cloned().filter(|alias| {
                super::probe::reached(core, alias).is_none_or(|reached| reached == link.id)
            });
            let mut row = HostRow {
                setup: ssh.as_ref().and_then(|alias| setups.remove(alias)),
                ssh: ssh.clone(),
                id: Some(link.id),
                name: link.name,
                platform: Some(link.platform),
                reach,
                found: None,
                unanswered: None,
                set_up: None,
            };
            if let Some(alias) = ssh.filter(|_| reach != Reach::Connected) {
                answered(&mut row, super::probe::answer(core, &alias));
                asking.push(alias);
            }
            row
        })
        .collect();
    let mut open: BTreeMap<String, Reach> = BTreeMap::new();
    for (alias, setup) in &setups {
        open.insert(alias.clone(), setup.reach());
    }
    for (alias, link) in core.ssh_links() {
        if rows.iter().any(|row| row.ssh.as_deref() == Some(&alias)) {
            continue;
        }
        let reach = match link {
            Some(SshLink::Connected) => Reach::Connected,
            Some(SshLink::Connecting) => Reach::Connecting,
            Some(SshLink::Disconnected) => Reach::Failed,
            None => Reach::Away,
        };
        let best = open.entry(alias).or_insert(reach);
        if rank(reach) > rank(*best) {
            *best = reach;
        }
    }
    // Each alias that reaches a host already listed folds into its row, with its set-up.
    let mut reached: BTreeMap<String, usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(at, row)| Some((row.id.clone()?, at)))
        .collect();
    for (alias, reach) in open {
        let key = super::probe::reached(core, &alias);
        if let Some(&at) = key.as_ref().and_then(|key| reached.get(key)) {
            if rows[at].setup.is_none() {
                rows[at].setup = setups.remove(&alias);
            }
            continue;
        }
        if let Some(key) = key {
            reached.insert(key, rows.len());
        }
        let mut row = HostRow {
            id: None,
            name: alias.clone(),
            platform: None,
            reach,
            setup: setups.remove(&alias),
            found: None,
            unanswered: None,
            set_up: None,
            ssh: Some(alias.clone()),
        };
        answered(&mut row, super::probe::answer(core, &alias));
        asking.push(alias);
        rows.push(row);
    }
    if let Some(core) = core.arc() {
        let why = if links_moved {
            super::probe::Why::Changed
        } else {
            super::probe::Why::Shown
        };
        for alias in asking {
            super::probe::ask(&core, &alias, why);
        }
    }
    rows
}

fn answered(row: &mut HostRow, answer: super::probe::Answer) {
    match answer {
        super::probe::Answer::Found(found) => {
            row.found = Some(found);
            row.set_up = found.set_up();
        }
        super::probe::Answer::Unanswered(unanswered) => row.unanswered = Some(unanswered),
    }
}

/// Drops the SSH hosts of hosts no longer linked, and says whether the links moved since the
/// hosts were last listed.
fn forget_unlinked(core: &Core, links: &[super::super::remote::StoredLink]) -> bool {
    let now: Vec<String> = links.iter().map(|link| link.id.clone()).collect();
    let before = core.hosts().listed_links.locked().replace(now.clone());
    let Some(before) = before else {
        return false;
    };
    if before == now {
        return false;
    }
    let mut installed = core.hosts().installed.locked();
    let count = installed.len();
    installed.retain(|key, _| now.contains(key) || !before.contains(key));
    if installed.len() != count {
        core.hosts().installed_kept.save(&*installed);
    }
    true
}

fn rank(reach: Reach) -> u8 {
    match reach {
        Reach::Away => 0,
        Reach::Failed => 1,
        Reach::Connecting => 2,
        Reach::Connected => 3,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledFrom {
    key: String,
    alias: String,
}

/// Notes that the host with `key` was installed on the SSH host `alias` from
/// here, so the two show as one row.
pub(crate) fn installed_from(core: &Core, installed: InstalledFrom) -> CoreResult<()> {
    if installed.alias.is_empty() || installed.alias.starts_with('-') {
        return Err("that is not an SSH host".into());
    }
    note_installed(core, installed.key, installed.alias);
    Ok(())
}

/// Notes that `alias` reaches the host `key`, unless another alias of it still does.
pub(crate) fn note_reached(core: &Core, key: String, alias: String) {
    let current = core.hosts().installed.locked().get(&key).cloned();
    let keeps = current.is_some_and(|current| {
        current == alias
            || super::probe::reached(core, &current).is_none_or(|reached| reached == key)
    });
    if !keeps {
        note_installed(core, key, alias);
    }
}

/// Notes that `alias` reaches the host `key`, which no other host's row is reached at then.
pub(crate) fn note_installed(core: &Core, key: String, alias: String) {
    let mut map = core.hosts().installed.locked();
    map.retain(|_, held| *held != alias);
    map.insert(key, alias);
    core.hosts().installed_kept.save(&*map);
    drop(map);
    core.changed(What::Read(Topic::Hosts));
}
