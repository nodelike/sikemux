//! The account vault (ADR 0016): the account's connector sign-ins and database logins, sealed
//! under a key the accounts server never sees. This host's secret store keeps the local copy
//! the plugins read; this module keeps it in step with the vault, decides who may use each
//! entry, logs every use, and hands on the access tokens of the sign-ins this host refreshes.

use crate::server::changed::ChangedCore;
use crate::server::remote_state::RemoteParts;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::vault::{Caller, Keeper, Refresh, SharedAccess, SignIn};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::changed::{Topic, What};
use super::Core;

mod local;
mod seal;
pub mod shown;
mod sync;

/// What an entry holds, sealed under the vault key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    plugin: String,
    sign_in: SignIn,
    /// The host the sign-in was made on.
    origin: String,
    /// The one host that refreshes it and writes the new tokens back.
    refresher: String,
    /// The hosts that may use it, by key.
    access: BTreeMap<String, Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared: Option<SharedAccess>,
    /// When its token was last written, in milliseconds since the Unix epoch.
    written_at: u64,
}

/// Which agents on a host may use an entry: all of them, or those in the projects listed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
}

#[derive(Clone)]
struct Entry {
    revision: i64,
    body: Body,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// This host is on no account, or has not reached it yet.
    #[default]
    Off,
    /// The account has a vault this host holds no key to yet.
    Waiting,
    Open,
    Failed(String),
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Tally {
    count: i64,
    first: u64,
    last: u64,
}

#[derive(Default)]
pub struct State {
    account: Option<String>,
    me: Option<String>,
    status: Status,
    generation: i64,
    key: Option<seal::Key>,
    recovery_key: Option<String>,
    recovery_wrap: Option<String>,
    holders: Vec<String>,
    waiting: Vec<String>,
    /// What the account calls each of its hosts, by key.
    hosts: HashMap<String, String>,
    entries: BTreeMap<String, Entry>,
    /// Each secret's entry, by service and account.
    index: HashMap<(String, String), String>,
    /// Who may use each entry this host held when it last reached the account, until it reaches
    /// it again in this run.
    rules: BTreeMap<String, local::Rule>,
    /// Uses not yet sent to the account, by entry and who used it.
    uses: BTreeMap<(String, String), Tally>,
    /// The latest use of each entry here, for Settings.
    latest: BTreeMap<String, (String, u64)>,
    /// Entries whose body changed here and wait to be written to the vault.
    pending: BTreeSet<String>,
    /// Rounds that failed in a row since the last that synced.
    failures: u32,
    /// A plugin wrote or deleted a secret, or the person changed a connector, since this
    /// host last compared its store with the vault.
    dirty: bool,
    /// This host compared its store with the vault since it started.
    compared: bool,
    /// The recovery key of a vault this host just made, until the person has saved it.
    recovery_text: Option<String>,
    /// When this host first read the vault in this run.
    opened_at: u64,
    /// Entries this host failed to renew, and when, so it waits before trying again.
    renew_failed: HashMap<String, u64>,
}

impl State {
    fn reindex(&mut self) {
        self.index = self
            .entries
            .iter()
            .flat_map(|(id, entry)| {
                entry
                    .body
                    .sign_in
                    .secrets
                    .iter()
                    .map(move |kept| ((kept.service.clone(), kept.account.clone()), id.clone()))
            })
            .collect();
    }

    fn entry_of(&self, service: &str, account: &str) -> Option<(&String, &Entry)> {
        let id = self.index.get(&(service.to_owned(), account.to_owned()))?;
        Some((id, self.entries.get(id)?))
    }

    fn allowed(&self, service: &str, account: &str) -> Option<Allowed<'_>> {
        if let Some((id, entry)) = self.entry_of(service, account) {
            return Some(Allowed {
                id,
                label: &entry.body.sign_in.label,
                refresher: &entry.body.refresher,
                access: &entry.body.access,
                shared: entry.body.shared.as_ref(),
            });
        }
        let (id, rule) = self.rules.iter().find(|(_, rule)| {
            rule.secrets
                .iter()
                .any(|(kept, named)| kept == service && named == account)
        })?;
        Some(Allowed {
            id,
            label: &rule.label,
            refresher: &rule.refresher,
            access: &rule.access,
            shared: None,
        })
    }
}

/// What the keeper needs of an entry to decide a use.
struct Allowed<'a> {
    id: &'a String,
    label: &'a String,
    refresher: &'a String,
    access: &'a BTreeMap<String, Scope>,
    shared: Option<&'a SharedAccess>,
}

#[derive(Default)]
pub struct Vault {
    state: Mutex<State>,
    wake: Arc<Notify>,
    /// A use was logged that the disk does not hold yet.
    used: Arc<Notify>,
}

impl Vault {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// The person changed a connector, so this host's sign-ins are compared with the vault's.
    pub fn touched(&self) {
        self.state.locked().dirty = true;
        self.wake();
    }
}

/// The keeper the plugins ask, which reads the vault of the core it was made for.
struct HostKeeper(Weak<Core>);

impl HostKeeper {
    fn with<T>(&self, absent: T, read: impl FnOnce(&Core, &mut State) -> T) -> T {
        match self.0.upgrade() {
            Some(core) => {
                let mut state = core.vault().state.locked();
                read(&core, &mut state)
            }
            None => absent,
        }
    }
}

fn not_allowed(label: &str) -> String {
    format!(
        "the account vault does not let this host use {label}; allow it in Settings → Connectors"
    )
}

impl Keeper for HostKeeper {
    fn using(&self, service: &str, account: &str, caller: Option<&Caller>) -> Result<(), String> {
        self.with(Ok(()), |core, state| {
            let Some(me) = state.me.clone() else {
                return Ok(());
            };
            let Some(entry) = state.allowed(service, account) else {
                return Ok(());
            };
            let id = entry.id.clone();
            let label = entry.label.clone();
            let Some(scope) = entry.access.get(&me).cloned() else {
                return Err(not_allowed(&label));
            };
            let agent = caller.and_then(|caller| caller.agent.as_deref());
            if let (Some(projects), Some(_)) = (&scope.projects, agent) {
                let project = caller.and_then(|caller| caller.project.as_deref());
                if !project.is_some_and(|project| projects.iter().any(|kept| kept == project)) {
                    return Err(format!(
                        "the account vault lets agents use {label} only in other projects; allow this one in Settings → Connectors"
                    ));
                }
            }
            let actor = agent.unwrap_or("Sikemux").to_owned();
            let now = epoch_ms();
            let tally = state.uses.entry((id.clone(), actor.clone())).or_default();
            if tally.count == 0 {
                tally.first = now;
            }
            tally.count += 1;
            tally.last = now;
            state.latest.insert(id, (actor, now));
            core.vault().used.notify_one();
            Ok(())
        })
    }

    fn changed(&self, _service: &str, _account: &str) {
        if let Some(core) = self.0.upgrade() {
            core.vault().state.locked().dirty = true;
            core.vault().wake();
        }
    }

    fn refresh(&self, service: &str, account: &str) -> Refresh {
        self.with(Refresh::Here, |core, state| {
            let Some(me) = state.me.as_deref() else {
                return Refresh::Here;
            };
            let Some(entry) = state.allowed(service, account) else {
                return Refresh::Here;
            };
            if entry.refresher == me {
                return Refresh::Here;
            }
            match entry.shared {
                Some(shared) if shared.expires_at > epoch_ms() + 60_000 => {
                    Refresh::Use(shared.clone())
                }
                _ => Refresh::Wait(format!(
                    "{} refreshes {} for your account and has not handed on a fresh token; it may be offline",
                    shown::named(core, state, entry.refresher).0,
                    entry.label
                )),
            }
        })
    }

    fn refreshed(&self, service: &str, account: &str, access: SharedAccess) {
        let Some(core) = self.0.upgrade() else { return };
        {
            let mut state = core.vault().state.locked();
            let Some(me) = state.me.clone() else { return };
            let Some(id) = state
                .entry_of(service, account)
                .filter(|(_, entry)| entry.body.refresher == me)
                .map(|(id, _)| id.clone())
            else {
                return;
            };
            if let Some(entry) = state.entries.get_mut(&id) {
                entry.body.shared = Some(access);
            }
            state.renew_failed.remove(&id);
            state.pending.insert(id);
        }
        core.vault().wake();
    }
}

/// Lets the plugins ask this core's vault, and keeps it in step with the account's.
pub fn start(core: &Arc<Core>) {
    let keeper: &'static HostKeeper = Box::leak(Box::new(HostKeeper(Arc::downgrade(core))));
    sikemux_plugin_api::vault::keep(keeper);
    tokio::spawn(sync::run(Arc::downgrade(core)));
    tokio::spawn(sync::keep_uses(Arc::downgrade(core)));
}

fn announce(core: &Core) {
    core.changed(What::Read(Topic::Plugins));
}

fn inbox_changed(core: &Core) {
    core.changed(What::View);
}

/// Something about the vault the person has to do.
pub struct Wait {
    pub id: String,
    pub title: String,
    pub what: String,
    pub host: Option<String>,
    pub since: u64,
}

pub fn waits(core: &Core) -> Vec<Wait> {
    let (recovery, idle_hosts, count, since) = {
        let state = core.vault().state.locked();
        if state.status != Status::Open {
            return Vec::new();
        }
        let me = state.me.clone().unwrap_or_default();
        let hosts: BTreeSet<&String> = state.holders.iter().chain(&state.waiting).collect();
        let idle: Vec<String> = hosts
            .into_iter()
            .filter(|host| **host != me)
            .filter(|host| {
                !state
                    .entries
                    .values()
                    .any(|entry| entry.body.access.contains_key(*host))
            })
            .cloned()
            .collect();
        (
            state.recovery_text.is_some(),
            idle,
            state.entries.len(),
            state.opened_at,
        )
    };
    let mut waits = Vec::new();
    if recovery {
        waits.push(Wait {
            id: "vault:recovery".into(),
            title: "Account vault".into(),
            what: "Save the vault's recovery key".into(),
            host: None,
            since,
        });
    }
    if count == 0 {
        return waits;
    }
    let plural = if count == 1 { "" } else { "s" };
    for host in idle_hosts {
        waits.push(Wait {
            id: format!("vault:host:{host}"),
            title: shown::host_name(core, &host),
            what: format!("Can't use your {count} sign-in{plural} until you allow it"),
            host: Some(host),
            since,
        });
    }
    waits
}
