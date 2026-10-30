//! What runs on an SSH host, asked over the person's own SSH once a terminal on it connects
//! and again after it is set up (ADR 0018, ADR 0019): whether it runs Sikemux, on this host's
//! account or another, and which host it is, so the host shows as one row and every client
//! offers the one thing that sets it up. The question never prompts: with no key the SSH
//! config already offers, it finds nothing.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::super::changed::Topic;
use super::super::Core;
use super::install::Work;

/// Long enough to connect and ask; a host that takes longer is left as it was.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    running: bool,
    /// Missing from releases before the probe, and while the core is stopped.
    core_id: Option<String>,
    account: Option<String>,
    /// The host's own name, which it signed in to the account under.
    name: Option<String>,
}

/// What the SSH host's Sikemux is, beside this host's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Found {
    /// No Sikemux there, or none that answers.
    Nothing,
    Same,
    SignedOut,
    Other,
    Stopped,
    /// A release too old to say which host it is.
    Outdated,
}

impl Found {
    /// What setting the host up takes, if anything does.
    pub(crate) fn set_up(self) -> Option<Work> {
        match self {
            Found::Nothing => Some(Work::Install),
            Found::SignedOut => Some(Work::SignIn),
            Found::Stopped => Some(Work::Start),
            Found::Outdated => Some(Work::Update),
            Found::Same | Found::Other => None,
        }
    }
}

/// Asks the SSH host `alias` what it runs, unless that is already being asked or was asked
/// since this core started and `again` is not set.
pub(crate) fn ask(core: &Arc<Core>, alias: &str, again: bool) {
    let mut asking = core.hosts.probing.locked();
    if asking.contains(alias) || (!again && core.hosts.probes.locked().contains_key(alias)) {
        return;
    }
    asking.insert(alias.to_owned());
    drop(asking);
    let (core, alias) = (core.clone(), alias.to_owned());
    tokio::spawn(async move {
        let found = probe(&core, &alias).await;
        core.hosts.probing.locked().remove(&alias);
        if let Some(found) = found {
            core.hosts.probes.locked().insert(alias, found);
            core.changes.changed(Topic::Hosts, None, None);
        }
    });
}

/// What was found on the SSH host `alias` when it was last asked.
pub(crate) fn found(core: &Core, alias: &str) -> Option<Found> {
    core.hosts.probes.locked().get(alias).copied()
}

/// None when the host could not be asked at all, which leaves what was known.
async fn probe(core: &Arc<Core>, alias: &str) -> Option<Found> {
    let mut ssh = tokio::process::Command::from(sikemux_process::user_environment::command("ssh"));
    ssh.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ControlMaster=no",
        "-o",
        "ConnectTimeout=10",
        "--",
        alias,
        &super::install::over_ssh("status --json"),
    ])
    .stdin(Stdio::null())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, ssh.output())
        .await
        .ok()?
        .ok()?;
    // Any status that reads means Sikemux is there, running or not.
    let Ok(status) = serde_json::from_slice::<Status>(&output.stdout) else {
        return Some(Found::Nothing);
    };
    if !status.running {
        return Some(Found::Stopped);
    }
    let Some(key) = status.core_id else {
        return Some(Found::Outdated);
    };
    let found = match status.account {
        None => Found::SignedOut,
        Some(account) if Some(&account) == core.remote.owner().as_ref() => Found::Same,
        Some(_) => Found::Other,
    };
    if let Some(link) = core.remote.links().into_iter().find(|link| link.id == key) {
        super::listed::note_installed(core, key.clone(), alias.to_owned());
        // A host still under the name it signed in with takes the alias, the person's name for it.
        if status
            .name
            .is_some_and(|name| name == link.name && name != alias)
        {
            let renamed = serde_json::from_value(serde_json::json!({ "key": key, "name": alias }))
                .map(|params| super::account::rename_device(core, params));
            if let Ok(renaming) = renamed {
                if let Err(error) = renaming.await {
                    eprintln!("sikemux core: {alias} keeps its own name: {error}");
                }
            }
        }
    }
    Some(found)
}

/// The aliases being asked now, kept apart from what was found so a slow answer is asked once.
pub(crate) type Probing = HashSet<String>;
