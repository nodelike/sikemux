//! The Hosts group every client shows: the hosts linked with this one, and
//! the SSH hosts a terminal is open to, each with how it is reached.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_pty::shell_protocol::SshLink;

use super::super::changed::Topic;
use super::super::{Core, CoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Reach {
    Connected,
    Connecting,
    /// A linked host this one has not reached, which it keeps trying, or an
    /// SSH host whose terminals are not connected.
    Away,
    /// Only for an SSH host: ssh gave up connecting to it.
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostRow {
    /// The host's core key; none for an SSH host without Sikemux.
    id: Option<String>,
    name: String,
    platform: Option<String>,
    reach: Reach,
    /// The SSH host it was installed on from here, or the row's own alias.
    ssh: Option<String>,
    /// What is being done to an SSH host from here: installing, updating or signing in.
    #[serde(skip_serializing_if = "Option::is_none")]
    setup: Option<super::install::SetupRow>,
    /// What its SSH host was found to run when last asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    found: Option<super::probe::Found>,
    /// What setting its SSH host up takes: installing where nothing has said Sikemux is there.
    #[serde(skip_serializing_if = "Option::is_none")]
    set_up: Option<super::install::Work>,
}

/// Linked hosts in the order they were linked, then the SSH hosts a terminal
/// is open to, by alias, leaving out those a linked host was installed on.
pub(crate) fn rows(core: &Core) -> Vec<HostRow> {
    if !core.hosts.listed.swap(true, Ordering::AcqRel) {
        core.hosts.wake.now();
    }
    let installed = core.hosts.installed.locked().clone();
    let listening = core.remote.endpoint().is_some();
    let mut rows: Vec<HostRow> = core
        .remote
        .links()
        .into_iter()
        .map(|link| HostRow {
            reach: if listening {
                core.hosts.reach(&link.id)
            } else {
                Reach::Away
            },
            found: installed
                .get(&link.id)
                .and_then(|alias| super::probe::found(core, alias)),
            set_up: installed
                .get(&link.id)
                .and_then(|alias| super::probe::found(core, alias))
                .and_then(super::probe::Found::set_up),
            ssh: installed.get(&link.id).cloned(),
            setup: None,
            id: Some(link.id),
            name: link.name,
            platform: Some(link.platform),
        })
        .collect();
    let mut setups: BTreeMap<String, super::install::SetupRow> =
        super::install::rows(core).into_iter().collect();
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
    rows.extend(open.into_iter().map(|(alias, reach)| {
        let found = super::probe::found(core, &alias);
        HostRow {
            id: None,
            name: alias.clone(),
            platform: None,
            reach,
            setup: setups.remove(&alias),
            set_up: found.map_or(
                Some(super::install::Work::Install),
                super::probe::Found::set_up,
            ),
            found,
            ssh: Some(alias),
        }
    }));
    rows
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

pub(crate) fn note_installed(core: &Core, key: String, alias: String) {
    let mut map = core.hosts.installed.locked();
    map.insert(key, alias);
    core.hosts.installed_kept.save(&*map);
    drop(map);
    core.changes.changed(Topic::Hosts, None, None);
}
