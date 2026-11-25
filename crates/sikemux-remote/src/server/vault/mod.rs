//! The account vault (ADR 0016): the account's connector sign-ins and database logins, sealed
//! under a key the accounts server never sees. This host's secret store keeps the local copy
//! the plugins read; this module keeps it in step with the vault, logs every use, and hands on
//! the access tokens of the sign-ins this host refreshes. Every host on the account uses every
//! entry: the account is the trust.

use crate::server::changed::ChangedCore;
use crate::server::remote_state::RemoteParts;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::vault::{Caller, Keeper, Refresh, SharedAccess, SignIn};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::{oneshot, Notify};

use super::changed::{Topic, What};
use super::Core;

mod floor;
mod local;
pub mod org;
mod seal;
pub mod shown;
mod sync;
mod trust;

pub use seal::{open_bound, open_bytes, random_key, seal_bound, seal_bytes, Key};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared: Option<SharedAccess>,
    /// When its token was last written, in milliseconds since the Unix epoch.
    written_at: u64,
    /// The hosts an entry made before every host took in every entry was limited to. A host
    /// left out does not take it in; the first host on the list to reach it writes it again
    /// without the list.
    #[serde(default, rename = "access", skip_serializing_if = "Option::is_none")]
    limited_to: Option<BTreeMap<String, serde_json::Value>>,
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
    /// Who refreshes each entry this host held when it last reached the account, until it
    /// reaches it again in this run.
    rules: BTreeMap<String, local::Rule>,
    /// Uses not yet sent to the account, by entry and who used it.
    uses: BTreeMap<(String, String), Tally>,
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
    /// Which sign-ins stay on this host, as it last read them.
    marks: local::Marks,
    /// The sign-ins this host's plugins held when it last compared them with the vault.
    here: Vec<local::Mark>,
    /// Entries the vault lists that this host leaves out, so each is logged once.
    left_out: BTreeSet<String>,
    /// The key is due to be replaced but this host holds off, as it last read the vault.
    rotation_held: bool,
    /// This host has opened or written an authenticated v2 key wrap for this vault.
    authenticated_wraps: bool,
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

    fn known(&self, service: &str, account: &str) -> Option<Known<'_>> {
        if let Some((id, entry)) = self.entry_of(service, account) {
            return Some(Known {
                id,
                label: &entry.body.sign_in.label,
                refresher: &entry.body.refresher,
                shared: entry.body.shared.as_ref(),
            });
        }
        let (id, rule) = self.rules.iter().find(|(_, rule)| {
            rule.secrets
                .iter()
                .any(|(kept, named)| kept == service && named == account)
        })?;
        Some(Known {
            id,
            label: &rule.label,
            refresher: &rule.refresher,
            shared: None,
        })
    }
}

/// What the keeper needs of an entry to log a use and refresh it.
struct Known<'a> {
    id: &'a String,
    label: &'a String,
    refresher: &'a String,
    shared: Option<&'a SharedAccess>,
}

#[derive(Default)]
pub struct Vault {
    state: Mutex<State>,
    wake: Arc<Notify>,
    /// A use was logged that the disk does not hold yet.
    used: Arc<Notify>,
    orgs: org::OrgVaults,
    /// Who waits for the next round to end, each told why when it failed.
    awaiting: Mutex<Vec<oneshot::Sender<Result<(), &'static str>>>>,
}

impl Vault {
    /// Starts a round and waits for it to end.
    async fn next_round(&self) -> Result<(), &'static str> {
        let (ended, end) = oneshot::channel();
        self.awaiting.locked().push(ended);
        self.wake();
        end.await.unwrap_or(Err("Sikemux is closing."))
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// The vaults of the orgs the account is in, apart from its own.
    pub fn orgs(&self) -> &org::OrgVaults {
        &self.orgs
    }

    /// The account, the key's generation and the key, while this host holds the vault's key, for
    /// what else the account keeps sealed under it.
    pub fn key(&self) -> Option<(String, i64, Key)> {
        let state = self.state.locked();
        Some((state.account.clone()?, state.generation, state.key?))
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

impl Keeper for HostKeeper {
    fn using(&self, service: &str, account: &str, caller: Option<&Caller>) {
        if caller.is_some_and(|caller| caller.checking) {
            return;
        }
        self.with((), |core, state| {
            if state.me.is_none() {
                return;
            }
            let Some(entry) = state.known(service, account) else {
                return;
            };
            let id = entry.id.clone();
            let actor = caller
                .and_then(|caller| caller.agent.as_deref())
                .unwrap_or("Sikemux")
                .to_owned();
            let now = epoch_ms();
            let tally = state.uses.entry((id, actor)).or_default();
            if tally.count == 0 {
                tally.first = now;
            }
            tally.count += 1;
            tally.last = now;
            core.vault().used.notify_one();
        });
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
            let Some(entry) = state.known(service, account) else {
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
    org::start(core);
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
    pub since: u64,
}

pub fn waits(core: &Core) -> Vec<Wait> {
    let state = core.vault().state.locked();
    if state.status != Status::Open || state.recovery_text.is_none() {
        return Vec::new();
    }
    vec![Wait {
        id: "vault:recovery".into(),
        title: "Account vault".into(),
        what: "Save the vault's recovery key".into(),
        since: state.opened_at,
    }]
}

/// Handing out and replacing the vaults' keys by themselves, without the Keychain a whole round
/// reads.
#[cfg(any(test, feature = "testing"))]
pub mod keys {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use sikemux_calls::vault::OrgLetInAnswer;
    use sikemux_view::device::AccountOrg;

    use super::trust::{self, Me};
    use super::{org, seal, sync, Core, Key};
    use crate::accounts::vault::Vault;
    use crate::server::remote_state::RemoteParts;
    use crate::server::{CoreError, CoreResult};

    pub async fn hand_out(
        core: &Arc<Core>,
        account: &str,
        vault: &Vault,
        key: &Key,
    ) -> CoreResult<()> {
        let host = core
            .remote()
            .host_key()
            .ok_or_else(|| CoreError::from("no host key"))?;
        let secret = seal::host_secret(&host);
        sync::add_keys(core, account, &secret, vault, key).await
    }

    pub async fn replace(
        core: &Arc<Core>,
        account: &str,
        me: &str,
        vault: &Vault,
        key: &Key,
    ) -> CoreResult<bool> {
        let host = core
            .remote()
            .host_key()
            .ok_or_else(|| CoreError::from("no host key"))?;
        let secret = seal::host_secret(&host);
        sync::rotate(core, account, me, &secret, vault, key, Default::default()).await
    }

    pub async fn read_org(core: &Core, org: &AccountOrg, host: &iroh::SecretKey) -> CoreResult<()> {
        org::sync_org(core, org, &Me::of(host)).await
    }

    pub fn org_full(core: &Core, org: &str) -> Option<String> {
        org::full(core, org)
    }

    pub async fn make_org(core: &Core, org: &str, host: &iroh::SecretKey) -> CoreResult<()> {
        org::make_vault(core, org, &Me::of(host)).await
    }

    /// Reads the vaults of orgs this host holds that the account's list, `orgs`, leaves out.
    pub async fn leave_unlisted(core: &Core, orgs: &[AccountOrg]) {
        org::leave_unlisted(core, orgs).await;
    }

    pub async fn let_in_to_org(
        core: &Core,
        org: &AccountOrg,
        host: &iroh::SecretKey,
        code: &str,
    ) -> CoreResult<OrgLetInAnswer> {
        org::let_in_to(core, org, &Me::of(host), code).await
    }

    /// An org's entry that holds `body`, as a host puts it at `revision`.
    pub fn org_entry(
        key: &Key,
        org: &str,
        (id, kind): (&str, &str),
        (generation, revision): (i64, i64),
        body: &serde_json::Value,
    ) -> CoreResult<String> {
        let plain = serde_json::to_vec(body)?;
        org::seal_entry(key, org, id, kind, generation, revision - 1, &plain)
    }

    /// Each entry of the org's vault this host opened: its revision and what it holds.
    pub fn org_entries(core: &Core, org: &str) -> BTreeMap<String, (i64, serde_json::Value)> {
        org::opened(core, org)
    }

    /// Whether this host writes to the org's vault as it last read it.
    pub fn open_to_writes(core: &Core, org: &AccountOrg) -> bool {
        org::open_to_writes(core, org).is_ok()
    }

    /// Why this host writes no project's entry to the org's vault, if it does not.
    pub async fn put_project_refused(core: &Core, org: &str) -> Option<String> {
        let at = org::At {
            revision: 0,
            made: None,
        };
        let body = serde_json::json!({});
        org::put_project(core, org, org::PROJECT_MEMORY, "id", &at, &body, &[])
            .await
            .err()
            .map(|error| error.to_string())
    }

    /// Has this host hold `key` as it kept it from before hosts were let in by their codes.
    pub fn keep_from_before(core: &Core, org: &str, key: Key, generation: i64) {
        org::keep_from_before(core, org, key, generation);
    }

    /// The org's key as `from` seals it for `to`.
    pub fn org_wrap(
        key: &Key,
        from: &iroh::SecretKey,
        to: &str,
        org: &str,
        generation: i64,
    ) -> String {
        let context = trust::wrap_context(org, generation, to);
        sealed(key, from, to, &context)
    }

    /// The org's key as `from` sealed it for `to` before hosts were let in by their codes.
    pub fn org_wrap_from_before(
        key: &Key,
        from: &iroh::SecretKey,
        to: &str,
        org: &str,
        generation: i64,
    ) -> String {
        let context = trust::wrap_context(org, generation, to).replace("|v2|", "|v1|");
        sealed(key, from, to, &context)
    }

    fn sealed(key: &Key, from: &iroh::SecretKey, to: &str, context: &str) -> String {
        seal::host_public(to)
            .and_then(|to| seal::wrap_from(key, &seal::host_secret(from), &to, context))
            .unwrap_or_default()
    }
}
