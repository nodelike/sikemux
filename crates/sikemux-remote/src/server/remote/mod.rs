//! Paired devices reaching the core from other machines. The on/off switch
//! and the trusted devices live in `<socket>.remote.json`, so the dev and
//! release cores keep separate ones; the core's key and the digest of what
//! that file trusts live in the person's secret store.

use serde_json::{json, Value};
pub use sikemux_corekit::usage::remote_file as file_path;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use iroh::endpoint::{presets, Incoming};
use iroh::{Endpoint, RelayMode, SecretKey};
use serde::{Deserialize, Serialize};
use sikemux_process::clock::{epoch_ms, epoch_secs};
use sikemux_process::sync::Locked;
use tokio::sync::{oneshot, watch, Notify};
use tokio::task::JoinHandle;

use crate::accounts::live::{self, Link, Push, Want};
use crate::accounts::protocol::{AccountEvent, AccountEventType, DeviceRole, LiveApp, PushResult};
use crate::join::{self, Expected, Refusal, TrustedKeys};
use crate::push::NotificationKey;
use sikemux_client::accounts::network;
use sikemux_client::join::JOIN_ALPN;
use sikemux_client::remote::CORE_ALPN;
use sikemux_wire::accounts::protocol::{JoinTicket, Network, Relay, RevokeReason};
use sikemux_wire::protocol::{
    AccountLink, AccountLinkState, AllowedDevice, DeviceAccess, DeviceInfo, Event, HeldTrust,
    NotificationState, NotifyPrefs, NotifyWhen, PendingDevice, PhoneNotifications, RemoteStatus,
    UpdateRequired,
};

use super::access::{Peer, Setup, Standing};
use super::changed::Topic;
use super::connection::{blocking, serve_client};
use super::host_key::{secret_from_hex, HostKey, Keeper};
use super::{Core, CoreError, CoreResult};

pub mod account;
pub mod held;
mod links;
pub mod listening;
pub mod pairing;
pub use pairing::{ApprovalSetting, RemotePairing};
pub mod push;

pub use account::*;
pub use held::*;
pub use listening::*;
pub use pairing::*;
pub use push::*;

/// What a link between two hosts of one account speaks.
pub const LINK_ALPN: &[u8] = b"sikemux/link/1";

/// How long signing out waits for the account to confirm this host left.
#[cfg(not(any(test, feature = "testing")))]
const LEAVE_WAIT: Duration = Duration::from_secs(5);
#[cfg(any(test, feature = "testing"))]
const LEAVE_WAIT: Duration = Duration::from_millis(300);
const NETWORK_REFRESH: Duration = Duration::from_secs(4 * 60 * 60);
const NETWORK_RETRY: Duration = Duration::from_secs(5 * 60);
pub const SIGNED_OUT: &str = "sign in to use Sikemux on your phone";
const BOUND: &str = "this host belongs to another Sikemux account; to move it, sign in from Sikemux's app or run `sudo sikemux login`";
/// How long after a device joins the account the person still hears of it.
const NEW_DEVICE_NEWS_MS: u64 = 10 * 60_000;
/// How far a signed request's time may be from now; the server allows the same.
const HOST_REQUEST_SKEW_SECS: u64 = 60;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    /// Where the host's key lived before the secret store; read once to take
    /// that host off its account, never written again.
    #[serde(skip_serializing)]
    pub secret_key: Option<String>,
    pub enabled: bool,
    pub devices: Vec<DeviceInfo>,
    pub owner: Option<String>,
    #[serde(default)]
    pub account_event_id: i64,
    /// The account this host signed out of before the server heard, which
    /// it tells on its next connection.
    pub pending_leave: Option<String>,
    /// Why the account let this host go, until it signs in again.
    pub removed: Option<Removal>,
    /// The last network the accounts server described, for when it is out of
    /// reach. A copy this build cannot read is dropped and fetched again.
    #[serde(default, deserialize_with = "readable_or_none")]
    pub network: Option<Network>,
    /// Each phone's key for its notifications and what it wants to hear
    /// about, by device id.
    #[serde(default)]
    pub notifications: BTreeMap<String, PhoneNotify>,
    /// When the account lately revoked each phone, in milliseconds since the
    /// epoch, by device id. A join ticket issued before then is refused.
    #[serde(default)]
    pub revoked: BTreeMap<String, u64>,
    /// The chats this host shares, so they carry on when the core restarts.
    #[serde(default)]
    pub shares: Vec<StoredShare>,
    /// The account each device joined through, by device id. A device on
    /// the account this host is signed in to may answer for its owner.
    #[serde(default)]
    pub joined_through: BTreeMap<String, String>,
    /// Hosts on the account linked with this one, whose agents may start
    /// and reach agents here and the other way round.
    #[serde(default)]
    pub links: Vec<StoredLink>,
    /// The person asks to allow each new device and host, rather than let
    /// in whatever their account signed in.
    #[serde(default)]
    pub require_approval: bool,
    /// Hosts on the account whose person turned down linking with this one,
    /// which it does not ask again until the person here links them.
    #[serde(default)]
    pub declined_links: BTreeSet<String>,
    /// Devices the person removed from this host, with the most each may do
    /// if let back in. Each waits for the person to let it in again, even
    /// with approval off.
    #[serde(default)]
    pub removed_here: BTreeMap<String, DeviceAccess>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}

/// `value` with every null, false, zero, empty text, list and map left out
/// and every map's keys in order, or `None` when nothing is left.
fn without_defaults(value: serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    match value {
        Value::Null | Value::Bool(false) => None,
        Value::Number(number) if number.as_f64() == Some(0.0) => None,
        Value::String(text) if text.is_empty() => None,
        Value::Array(items) if items.is_empty() => None,
        Value::Array(items) => Some(Value::Array(
            items
                .into_iter()
                .map(|item| without_defaults(item).unwrap_or(Value::Null))
                .collect(),
        )),
        Value::Object(map) => {
            let mut entries: Vec<_> = map
                .into_iter()
                .filter_map(|(key, value)| Some((key, without_defaults(value)?)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            (!entries.is_empty()).then(|| Value::Object(entries.into_iter().collect()))
        }
        value => Some(value),
    }
}

/// The digest hosts kept before it was taken over a canonical form: the
/// trusted fields serialized whole, as `file` holds them. A host keeping one
/// is trusted once more when it first starts on this build, which then keeps
/// the canonical digest instead.
fn trusted_as_written(file: &serde_json::Value) -> CoreResult<String> {
    use serde_json::Value;
    let field = |name: &str, missing: Value| file.get(name).cloned().unwrap_or(missing);
    let unseen = |list: Value, cleared: &[(&str, Value)]| match list {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|mut item| {
                    if let Value::Object(fields) = &mut item {
                        for (name, value) in cleared {
                            fields.insert((*name).to_owned(), value.clone());
                        }
                    }
                    item
                })
                .collect(),
        ),
        other => other,
    };
    let mut trusted = serde_json::Map::new();
    trusted.insert("owner".into(), field("owner", Value::Null));
    trusted.insert(
        "devices".into(),
        unseen(
            field("devices", Value::Array(Vec::new())),
            &[("lastSeen", Value::Null)],
        ),
    );
    trusted.insert(
        "joined_through".into(),
        field("joinedThrough", Value::Object(Default::default())),
    );
    trusted.insert(
        "links".into(),
        unseen(
            field("links", Value::Array(Vec::new())),
            &[
                ("lastSeen", Value::Null),
                ("addresses", Value::Array(Vec::new())),
            ],
        ),
    );
    trusted.insert("shares".into(), field("shares", Value::Array(Vec::new())));
    trusted.insert(
        "revoked".into(),
        field("revoked", Value::Object(Default::default())),
    );
    if let Some(require_approval) = file.get("requireApproval") {
        trusted.insert("require_approval".into(), require_approval.clone());
    }
    Ok(sha256_hex(&serde_json::to_vec(&Value::Object(trusted))?))
}

/// The digest of what `file` trusts, taken over the file as written rather
/// than as this build reads it, so a field a later build no longer knows
/// still counts and the file stays trusted across that update.
fn trusted_in_file(file: &serde_json::Value) -> CoreResult<String> {
    use serde_json::Value;
    let field = |name: &str| file.get(name).cloned().unwrap_or(Value::Null);
    let unseen = |list: Value, cleared: &[&str]| match list {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|mut item| {
                    if let Value::Object(fields) = &mut item {
                        for name in cleared {
                            fields.remove(*name);
                        }
                    }
                    item
                })
                .collect(),
        ),
        other => other,
    };
    let mut trusted = serde_json::Map::new();
    trusted.insert("owner".into(), field("owner"));
    trusted.insert("devices".into(), unseen(field("devices"), &["lastSeen"]));
    trusted.insert("joined_through".into(), field("joinedThrough"));
    trusted.insert(
        "links".into(),
        unseen(field("links"), &["lastSeen", "addresses"]),
    );
    trusted.insert("shares".into(), field("shares"));
    trusted.insert("revoked".into(), field("revoked"));
    trusted.insert("require_approval".into(), field("requireApproval"));
    trusted.insert("removed_here".into(), field("removedHere"));
    let canonical = without_defaults(Value::Object(trusted)).unwrap_or_default();
    Ok(sha256_hex(&serde_json::to_vec(&canonical)?))
}

fn readable_or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredLink {
    /// Its key.
    pub id: String,
    /// What this host calls it, which agents address it by.
    pub name: String,
    pub platform: String,
    pub linked_at: u64,
    pub last_seen: Option<u64>,
    /// Where it was last reached directly, beside the relays.
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// A chat this host shares, and the key its link holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredShare {
    pub id: String,
    pub key: String,
    pub agent_id: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    /// What the app calls the chat, which its viewers see too.
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneNotify {
    key_id: u32,
    key: String,
    prefs: NotifyPrefs,
    since: u64,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Removal {
    reason: Option<RevokeReason>,
    at: u64,
}

impl Stored {
    /// What decides who may reach this host, leaving out what changes each
    /// time a device or host is seen, as the digest the secret store keeps.
    pub fn trusted(&self) -> CoreResult<String> {
        /// What the digest covers, taken over its canonical form, which
        /// leaves out every value at its default. A field added here, or to
        /// a device or link, therefore changes no host's digest until it is
        /// set. Removing a field changes the digest of every file holding it
        /// at another value, and the core then trusts nothing that file
        /// names: keep a field until no host can hold it set.
        #[derive(Serialize)]
        struct Trusted<'a> {
            owner: &'a Option<String>,
            devices: Vec<DeviceInfo>,
            joined_through: &'a BTreeMap<String, String>,
            links: Vec<StoredLink>,
            shares: &'a [StoredShare],
            revoked: &'a BTreeMap<String, u64>,
            require_approval: bool,
            removed_here: &'a BTreeMap<String, DeviceAccess>,
        }
        let trusted = Trusted {
            owner: &self.owner,
            devices: self
                .devices
                .iter()
                .map(|device| DeviceInfo {
                    last_seen: None,
                    ..device.clone()
                })
                .collect(),
            joined_through: &self.joined_through,
            links: self
                .links
                .iter()
                .map(|link| StoredLink {
                    last_seen: None,
                    addresses: Vec::new(),
                    ..link.clone()
                })
                .collect(),
            shares: &self.shares,
            revoked: &self.revoked,
            require_approval: self.require_approval,
            removed_here: &self.removed_here,
        };
        let canonical = without_defaults(serde_json::to_value(&trusted)?).unwrap_or_default();
        Ok(sha256_hex(&serde_json::to_vec(&canonical)?))
    }

    /// Forgets every device, host, share and account a file it did not write
    /// named, or that trusted a key this host no longer has.
    fn distrust(&mut self) {
        self.devices.clear();
        self.joined_through.clear();
        self.links.clear();
        self.shares.clear();
        self.notifications.clear();
        self.owner = None;
        self.pending_leave = None;
        self.enabled = false;
        self.account_event_id = 0;
    }

    /// Drops a device and everything kept for it, answering with the device.
    fn forget(&mut self, id: &str) -> Option<DeviceInfo> {
        self.links.retain(|link| link.id != id);
        self.notifications.remove(id);
        self.joined_through.remove(id);
        let index = self.devices.iter().position(|device| device.id == id)?;
        Some(self.devices.remove(index))
    }

    /// Forgets every device that did not join through `account`, answering
    /// their ids, so a host moved to another account keeps none of the old
    /// account's phones.
    fn forget_joined_elsewhere(&mut self, account: &str) -> Vec<String> {
        let elsewhere: Vec<String> = self
            .devices
            .iter()
            .filter(|device| {
                self.joined_through.get(&device.id).map(String::as_str) != Some(account)
            })
            .map(|device| device.id.clone())
            .collect();
        for id in &elsewhere {
            self.forget(id);
        }
        elsewhere
    }

    fn want(&self) -> Want {
        if self.owner.is_some() {
            Want::Stay
        } else if self.pending_leave.is_some() {
            Want::Leave
        } else {
            Want::Stop
        }
    }
}

pub struct LiveTask {
    task: JoinHandle<()>,
    changed: watch::Sender<()>,
}

pub struct Running {
    endpoint: Endpoint,
    accept: JoinHandle<()>,
}

pub struct Pending {
    device: PendingDevice,
    account: String,
    /// The host whose own app is asking, for an app that came through it.
    via: Option<String>,
    /// Where a host asking to link was reached; none for a phone.
    host: Option<Vec<String>>,
    answer: oneshot::Sender<Option<DeviceAccess>>,
}

#[derive(Default)]
pub struct Inner {
    pub path: Option<PathBuf>,
    pub keeper: Option<Keeper>,
    /// The account this host belongs to, signed in or not.
    pub bound: Option<String>,
    /// The digest and account the secret store holds.
    pub kept: (String, Option<String>),
    /// The digest of the file as last written.
    pub written: String,
    pub direct_only: bool,
    pub secret: Option<SecretKey>,
    /// A key this host moved off while it was on an account, until that
    /// account lets it go.
    pub retired: Option<SecretKey>,
    pub stored: Stored,
    pub running: Option<Running>,
    pub connected: HashMap<String, usize>,
    pub pending: Vec<Pending>,
    pub accounts_api: Option<String>,
    pub live: Option<LiveTask>,
    pub link: Option<(Link, u64)>,
    pub relays: Vec<Relay>,
    /// Dev builds are never too old for the accounts server.
    pub never_too_old: bool,
    pub update_required: Option<UpdateRequired>,
    pub network: Option<JoinHandle<()>>,
    pub outbox: Arc<live::Outbox>,
    /// What became of each phone's last notification, and when.
    pub pushed: HashMap<String, (NotificationState, u64)>,
    /// Phones the account will not pass notifications to.
    pub refused: HashSet<String>,
    /// The push token of the Live Activity each phone shows this host's agents in, by phone.
    pub activities: HashMap<String, String>,
    /// What the remote file trusted when it failed its check, until the
    /// person trusts it again or lets it go.
    pub held: Option<(Stored, u64)>,
    /// Each host on the account that failed to link lately: how many times
    /// in a row, and not before when it is asked again.
    pub relink_failures: HashMap<String, (u32, u64)>,
    #[cfg(any(test, feature = "testing"))]
    pub join_keys: Option<TrustedKeys>,
}

#[derive(Default)]
pub struct Remote {
    pub inner: Mutex<Inner>,
    left: Notify,
    /// Cuts the wait before the next read of the network short.
    refresh: Arc<Notify>,
}

pub fn read_stored(path: &Path) -> CoreResult<Stored> {
    Ok(read_file(path)?.1)
}

/// The remote file as written, beside what it says.
fn read_file(path: &Path) -> CoreResult<(serde_json::Value, Stored)> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let file: serde_json::Value = serde_json::from_slice(&bytes)?;
            let stored = serde_json::from_value(file.clone())?;
            Ok((file, stored))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((serde_json::Value::Null, Stored::default()))
        }
        Err(error) => Err(error.into()),
    }
}

pub fn write_stored(path: &Path, stored: &Stored) -> CoreResult<()> {
    sikemux_process::files::write_atomic(path, &serde_json::to_vec_pretty(stored)?, true)?;
    Ok(())
}

impl Remote {
    pub fn is_enabled(&self) -> bool {
        self.inner.locked().stored.enabled
    }

    pub fn owner(&self) -> Option<String> {
        self.inner.locked().stored.owner.clone()
    }

    /// This build is older than the accounts server works with.
    pub fn too_old(&self) -> bool {
        self.inner.locked().update_required.is_some()
    }

    /// What a paired device may do now, or `None` once it is unpaired or remote
    /// access is off.
    pub fn standing_of(&self, id: &str) -> Option<Standing> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return None;
        }
        let stored = &inner.stored;
        let device = stored.devices.iter().find(|device| device.id == id)?;
        Some(Standing {
            access: device.access,
            owners: stored.owner.is_some()
                && stored.joined_through.get(id) == stored.owner.as_ref(),
        })
    }

    pub fn is_paired(&self, id: &str) -> bool {
        let inner = self.inner.locked();
        inner.stored.devices.iter().any(|device| device.id == id)
    }

    pub fn device_platform(&self, id: &str) -> Option<String> {
        let inner = self.inner.locked();
        let device = inner.stored.devices.iter().find(|device| device.id == id)?;
        Some(device.platform.clone())
    }

    pub fn device_name(&self, id: &str) -> Option<String> {
        let inner = self.inner.locked();
        let device = inner.stored.devices.iter().find(|device| device.id == id)?;
        Some(device.name.clone())
    }

    pub fn setup(&self) -> Setup {
        let inner = self.inner.locked();
        Setup {
            signed_in: inner.stored.owner.is_some(),
            paired: !inner.stored.devices.is_empty(),
            app_signed: super::local_peer::app_is_signed(),
        }
    }

    pub fn status(&self) -> RemoteStatus {
        let inner = self.inner.locked();
        let mut connected: Vec<String> = inner.connected.keys().cloned().collect();
        connected.sort();
        let addresses = inner
            .running
            .as_ref()
            .map(|running| {
                running
                    .endpoint
                    .addr()
                    .ip_addrs()
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        RemoteStatus {
            enabled: inner.stored.enabled,
            core_id: inner
                .secret
                .as_ref()
                .map(|secret| secret.public().to_string())
                .unwrap_or_default(),
            addresses,
            devices: inner.stored.devices.clone(),
            connected,
            pending: inner
                .pending
                .iter()
                .filter(|pending| pending.host.is_none())
                .map(|pending| pending.device.clone())
                .collect(),
            owner: inner.stored.owner.clone(),
            account: account_link(&inner),
            update_required: inner.update_required.clone(),
            notifications: notifications(&inner),
            require_approval: inner.stored.require_approval,
            held: inner.held.as_ref().map(|(held, at)| HeldTrust {
                devices: held.devices.clone(),
                hosts: held.links.iter().map(|link| link.name.clone()).collect(),
                signed_in: held.owner.is_some(),
                at: *at,
            }),
        }
    }

    /// Lets a device in at full access as the person, with no question: its
    /// ticket shows it is signed in to this host's own account.
    pub fn admit(
        &self,
        device: &PendingDevice,
        account: String,
        via: Option<String>,
    ) -> CoreResult<()> {
        self.add_device(
            DeviceInfo {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                access: DeviceAccess::Full,
                paired_at: epoch_ms(),
                last_seen: None,
                via,
            },
            Some(account),
        )
    }

    /// A program here other than the app may sign this host in only to the
    /// account it belongs to. The app, root, or that account letting the host
    /// go moves it to another.
    pub fn may_join(&self, account: &str, peer: &Peer) -> CoreResult<()> {
        match &self.inner.locked().bound {
            Some(bound) if bound != account && *peer != Peer::App => Err(BOUND.into()),
            _ => Ok(()),
        }
    }

    /// The core's key and its signature over the registration text for
    /// `nonce` and `user_id`, which must be a challenge and an account.
    pub fn sign_registration(&self, nonce: &str, user_id: &str) -> CoreResult<(String, String)> {
        sikemux_client::accounts::check_registration(nonce, user_id).map_err(CoreError::from)?;
        self.sign_with_key(|key| {
            sikemux_client::accounts::registration_message(nonce, user_id, key)
        })
    }

    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        sign_live(self.inner.locked().secret.as_ref()?, nonce)
    }

    /// The core's key and its signature over the share text for `nonce`.
    pub fn sign_share(&self, nonce: &str) -> Option<(String, String)> {
        sikemux_client::accounts::check_live(nonce).ok()?;
        let inner = self.inner.locked();
        let secret = inner.secret.as_ref()?;
        let key = secret.public().to_string();
        let message = sikemux_client::accounts::share_message(nonce, &key);
        Some((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
    }

    /// The core's key and its signature over the webhook text for `nonce`.
    pub fn sign_hooks(&self, nonce: &str) -> Option<(String, String)> {
        sikemux_client::accounts::check_live(nonce).ok()?;
        self.sign_with_key(|key| sikemux_client::accounts::hooks_message(nonce, key))
            .ok()
    }

    /// The core's key and its signature over `request`, for calling the
    /// accounts API as this host. The time must be close to now, so no
    /// signature is made ahead for later use.
    pub fn sign_host_request(
        &self,
        request: &crate::accounts::HostRequest,
    ) -> CoreResult<(String, String)> {
        request.check().map_err(CoreError::from)?;
        let now = epoch_secs();
        if request.time.abs_diff(now) > HOST_REQUEST_SKEW_SECS {
            return Err("the request's time is not now".into());
        }
        self.sign_with_key(|_| request.message())
    }

    /// The core's key and its signature over the shared chat viewer's text
    /// for `nonce`, a share connection's challenge.
    pub fn sign_share_viewer(&self, nonce: &str) -> CoreResult<(String, String)> {
        sikemux_client::accounts::check_live(nonce).map_err(CoreError::from)?;
        self.sign_with_key(|key| sikemux_client::accounts::share_viewer_message(nonce, key))
    }

    pub fn sign_with_key(
        &self,
        message: impl FnOnce(&str) -> String,
    ) -> CoreResult<(String, String)> {
        let inner = self.inner.locked();
        let secret = inner
            .secret
            .as_ref()
            .ok_or_else(|| CoreError::from("the core's key has not loaded"))?;
        let key = secret.public().to_string();
        let message = message(&key);
        Ok((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
    }

    /// The accounts API, while the host is signed in to an account there.
    pub fn signed_in_api(&self) -> Option<String> {
        let inner = self.inner.locked();
        inner.stored.owner.as_ref()?;
        inner.accounts_api.clone()
    }

    pub fn shares(&self) -> Vec<StoredShare> {
        self.inner.locked().stored.shares.clone()
    }

    pub fn keep_shares(&self, shares: Vec<StoredShare>) -> CoreResult<()> {
        self.change(|stored| {
            stored.shares = shares;
            Ok(())
        })
    }

    /// The key this host signs with, once it has loaded.
    pub fn host_key(&self) -> Option<SecretKey> {
        self.inner.locked().secret.clone()
    }

    /// The keys of this host and the hosts linked with it.
    pub fn host_keys(&self) -> Vec<iroh::PublicKey> {
        let inner = self.inner.locked();
        let linked = inner
            .stored
            .links
            .iter()
            .filter_map(|link| link.id.parse().ok());
        inner
            .secret
            .iter()
            .map(SecretKey::public)
            .chain(linked)
            .collect()
    }

    pub fn core_id(&self) -> Option<String> {
        self.inner
            .locked()
            .secret
            .as_ref()
            .map(|secret| secret.public().to_string())
    }

    /// Waits for someone to answer `device`, which came with a ticket from
    /// `account`: a phone, or a host asking to link, reached at `host`.
    pub fn ask(
        &self,
        device: PendingDevice,
        account: String,
        host: Option<Vec<String>>,
        via: Option<String>,
    ) -> oneshot::Receiver<Option<DeviceAccess>> {
        let (answer, answered) = oneshot::channel();
        self.inner.locked().pending.push(Pending {
            device,
            account,
            via,
            host,
            answer,
        });
        answered
    }

    /// Adds an allowed device before telling it, so the answer the app gets
    /// back already lists it.
    pub fn answer(&self, id: &str, access: Option<DeviceAccess>) -> CoreResult<()> {
        let pending = {
            let mut inner = self.inner.locked();
            let index = inner
                .pending
                .iter()
                .position(|pending| pending.device.id == id)
                .ok_or_else(|| CoreError::from("that device is no longer waiting"))?;
            inner.pending.remove(index)
        };
        if let (Some(_), Some(addresses)) = (access, pending.host.clone()) {
            let device = &pending.device;
            let added = StoredLink {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                linked_at: epoch_ms(),
                last_seen: Some(epoch_ms()),
                addresses,
            };
            if let Err(error) = self.add_link(added) {
                drop(pending.answer);
                return Err(error);
            }
        } else if let Some(access) = access {
            let device = &pending.device;
            let added = DeviceInfo {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                access,
                paired_at: epoch_ms(),
                last_seen: None,
                via: pending.via.clone(),
            };
            if let Err(error) = self.add_device(added, Some(pending.account.clone())) {
                drop(pending.answer);
                return Err(error);
            }
        }
        let _ = pending.answer.send(access);
        Ok(())
    }

    /// Lets in phones the person chose when signing in. A phone already
    /// waiting is answered; the rest are paired now and join without asking.
    pub fn allow(&self, devices: Vec<AllowedDevice>) -> CoreResult<()> {
        for device in &devices {
            device
                .id
                .parse::<iroh::PublicKey>()
                .map_err(|_| CoreError::from("that is not a device key"))?;
        }
        for device in devices {
            let waiting = {
                let mut inner = self.inner.locked();
                let index = inner.pending.iter().position(|pending| {
                    pending.host.is_none() && pending.device.device_id == device.id
                });
                index.map(|index| inner.pending.remove(index))
            };
            let owner = self.inner.locked().stored.owner.clone();
            self.add_device(
                DeviceInfo {
                    id: device.id,
                    name: device.name,
                    platform: device.platform,
                    access: device.access,
                    paired_at: epoch_ms(),
                    last_seen: None,
                    via: waiting.as_ref().and_then(|waiting| waiting.via.clone()),
                },
                owner,
            )?;
            if let Some(waiting) = waiting {
                let _ = waiting.answer.send(Some(device.access));
            }
        }
        Ok(())
    }

    /// Checks a join ticket from the phone `phone` proved it is. Answers with
    /// the access the phone has if it is paired already.
    pub fn check_join(
        &self,
        ticket: &JoinTicket,
        phone: &str,
    ) -> Result<Option<DeviceAccess>, Refusal> {
        let keys = self.join_keys();
        let inner = self.inner.locked();
        let host = inner
            .secret
            .as_ref()
            .map(|secret| secret.public().to_string())
            .unwrap_or_default();
        join::check(
            ticket,
            &keys,
            &Expected {
                owner: inner.stored.owner.as_deref(),
                host: &host,
                phone,
                now: epoch_secs() as i64,
                revoked_at_ms: inner.stored.revoked.get(phone).copied(),
            },
        )?;
        Ok(inner
            .stored
            .devices
            .iter()
            .find(|device| device.id == phone)
            .map(|device| device.access))
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn trust_join_key(&self, id: &str, key: [u8; 32]) {
        let mut keys = TrustedKeys::default();
        keys.insert(id, key);
        self.inner.locked().join_keys = Some(keys);
    }

    /// Withdraws whatever `device_id` asked before, so a phone that tries
    /// again waits on one request.
    pub fn forget_device_requests(&self, device_id: &str) {
        self.inner
            .locked()
            .pending
            .retain(|pending| pending.device.device_id != device_id);
    }

    pub fn forget_pending(&self, id: &str) {
        self.inner
            .locked()
            .pending
            .retain(|pending| pending.device.id != id);
    }

    /// The secret store learns of a change to what the file trusts before the
    /// file does, keeping the file's digest as well until the file is written,
    /// so neither a failed write nor a stop between the two signs the host out.
    /// It forgets that digest once the file is written, so the file put back
    /// as it was is not trusted.
    fn save(&self, inner: &mut Inner) -> CoreResult<()> {
        let Some(path) = inner.path.clone() else {
            return Ok(());
        };
        let kept = (inner.stored.trusted()?, inner.bound.clone());
        if kept != inner.kept {
            keep(inner, &kept)?;
            inner.kept = kept;
        }
        write_stored(&path, &inner.stored)?;
        crate::server::runtime_file::write(
            &path,
            inner.stored.owner.clone(),
            inner
                .secret
                .as_ref()
                .map(|secret| secret.public().to_string()),
        );
        if inner.written != inner.kept.0 {
            inner.written = inner.kept.0.clone();
            keep(inner, &inner.kept)?;
        }
        Ok(())
    }

    fn forget_retired(&self) {
        let mut inner = self.inner.locked();
        if inner.retired.take().is_none() {
            return;
        }
        if let Err(error) = keep(&inner, &inner.kept) {
            eprintln!("sikemux core: could not forget the key this host moved off: {error}");
        }
    }

    pub fn change<T>(&self, edit: impl FnOnce(&mut Stored) -> CoreResult<T>) -> CoreResult<T> {
        let mut inner = self.inner.locked();
        let result = edit(&mut inner.stored)?;
        self.save(&mut inner)?;
        Ok(result)
    }

    fn note_connected(&self, id: &str, connected: bool) {
        let mut inner = self.inner.locked();
        if connected {
            *inner.connected.entry(id.to_owned()).or_default() += 1;
            let now = epoch_ms();
            if let Some(device) = inner
                .stored
                .devices
                .iter_mut()
                .find(|device| device.id == id)
            {
                device.last_seen = Some(now);
            }
            if let Err(error) = self.save(&mut inner) {
                eprintln!("sikemux core: could not save when a device was last seen: {error}");
            }
        } else if let Some(count) = inner.connected.get_mut(id) {
            *count -= 1;
            if *count == 0 {
                inner.connected.remove(id);
            }
        }
    }
}

fn keep(inner: &Inner, (trusted, account): &(String, Option<String>)) -> CoreResult<()> {
    if let (Some(keeper), Some(secret)) = (&inner.keeper, &inner.secret) {
        keeper.save(&HostKey {
            secret: secret.clone(),
            trusted: trusted.clone(),
            account: account.clone(),
            retired: inner.retired.clone(),
            written: (inner.written != *trusted).then(|| inner.written.clone()),
        })?;
    }
    Ok(())
}

fn sign_live(secret: &SecretKey, nonce: &str) -> Option<(String, String)> {
    sikemux_client::accounts::check_live(nonce).ok()?;
    let key = secret.public().to_string();
    let message = sikemux_client::accounts::live_message(nonce, &key);
    Some((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
}

/// Gives a core run without the secret store, as tests run one, the key
/// `key`, trusting the remote file beside `socket` as it now reads.
pub fn trust_remote_file(socket: &Path, key: &SecretKey) -> Result<(), String> {
    let trust = || {
        let stored = read_stored(&file_path(socket))?;
        Keeper::new(socket, false)?.save(&HostKey {
            secret: key.clone(),
            trusted: stored.trusted()?,
            account: stored.owner,
            retired: None,
            written: None,
        })
    };
    trust().map_err(|error: CoreError| error.to_string())
}

fn vouches(key: &HostKey, file: &serde_json::Value, trusted: &str) -> CoreResult<bool> {
    Ok(key.trusted == trusted
        || key.written.as_deref() == Some(trusted)
        || key.trusted == trusted_in_file(file)?
        || key.trusted == trusted_as_written(file)?)
}

/// Where a remote file that failed its check is kept until the person decides.
pub fn held_path(path: &Path) -> PathBuf {
    path.with_extension("held.json")
}

/// Copies the host's key from the secret store entry named for the socket's
/// file name to the one named for its whole path, when the old entry vouches
/// for this host's remote file. A core started with another HOME may have
/// written the old entry; that one is left behind. The old entry stays, so an
/// older core run again still finds it.
pub fn rename_host_key(socket: &Path) -> CoreResult<()> {
    let keeper = Keeper::new(socket, true)?;
    if keeper.load()?.is_some() {
        return Ok(());
    }
    let Some(key) = Keeper::named_by_file(socket)?.load()? else {
        return Ok(());
    };
    let (file, mut stored) = read_file(&file_path(socket))?;
    stored.secret_key.take();
    if !vouches(&key, &file, &stored.trusted()?)? {
        eprintln!(
            "sikemux core: the host key kept under the socket's name vouches for another core's devices, so it was left behind"
        );
        return Ok(());
    }
    keeper.save(&key)
}

/// Reads the remote file and the key that vouches for it, trusting none of
/// the devices, hosts or account the file names unless the key's digest is
/// the file's.
pub fn load(keeper: &Keeper, path: &Path) -> CoreResult<(Stored, HostKey, String)> {
    let (file, mut stored) = read_file(path)?;
    let file_key = stored.secret_key.take();
    let trusted = stored.trusted()?;
    let key = match keeper.load()? {
        Some(key) if vouches(&key, &file, &trusted)? => key,
        Some(key) => {
            eprintln!(
                "sikemux core: {} was changed by something other than this host, so remote access is off, each new device waits for approval, and the devices and hosts it names wait for the person to trust them again; the file is kept as {}",
                path.display(),
                held_path(path).display()
            );
            sikemux_process::files::write_atomic(
                &held_path(path),
                &serde_json::to_vec_pretty(&file)?,
                true,
            )?;
            stored.distrust();
            stored.require_approval = true;
            stored.owner.clone_from(&key.account);
            key
        }
        None => {
            let on_account = stored.owner.is_some() || stored.pending_leave.is_some();
            let account = stored.owner.clone();
            stored.distrust();
            HostKey {
                secret: SecretKey::generate(),
                trusted: String::new(),
                account,
                retired: file_key
                    .filter(|_| on_account)
                    .as_deref()
                    .and_then(secret_from_hex),
                written: None,
            }
        }
    };
    let trusted = stored.trusted()?;
    if trusted != key.trusted || key.written.is_some() {
        keeper.save(&HostKey {
            trusted: trusted.clone(),
            written: None,
            ..key.clone()
        })?;
    }
    write_stored(path, &stored)?;
    Ok((stored, key, trusted))
}

pub fn signed((key, signature): (String, String)) -> Value {
    json!({ "key": key, "signature": signature })
}

#[derive(Debug, Deserialize)]
pub struct NonceParams {
    nonce: String,
}

impl Remote {
    /// Signs what a program on this host, such as the CLI, asks the accounts server as this host.
    pub fn host_request_signature(
        &self,
        request: &crate::accounts::HostRequest,
    ) -> CoreResult<Value> {
        Ok(signed(self.sign_host_request(request)?))
    }

    pub fn share_viewer_signature(&self, params: NonceParams) -> CoreResult<Value> {
        Ok(signed(self.sign_share_viewer(&params.nonce)?))
    }
}
