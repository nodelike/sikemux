//! Paired devices reaching the core from other machines. The on/off switch
//! and the trusted devices live in `<socket>.remote.json`, so the dev and
//! release cores keep separate ones; the core's key and the digest of what
//! that file trusts live in the person's secret store.

use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::accounts::network;
use crate::accounts::protocol::{
    AccountEvent, AccountEventType, DeviceRole, JoinTicket, LiveApp, Network, PushResult, Relay,
    RevokeReason,
};
use crate::join::{self, Expected, Refusal, TrustedKeys, JOIN_ALPN};
use crate::protocol::{
    AccountLink, AccountLinkState, AllowedDevice, DeviceAccess, DeviceInfo, Event,
    NotificationState, NotifyPrefs, NotifyWhen, PendingDevice, PhoneNotifications, RemoteStatus,
    UpdateRequired,
};
use crate::push::NotificationKey;
use crate::remote::CORE_ALPN;

use super::access::{Peer, Setup, Standing};
use super::changed::Topic;
use super::connection::{blocking, serve_client};
use super::host_key::{secret_from_hex, HostKey, Keeper};
use super::{Core, CoreError, CoreResult, ServerConfig};

/// How long signing out waits for the account to confirm this host left.
#[cfg(not(test))]
const LEAVE_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const LEAVE_WAIT: Duration = Duration::from_millis(300);
const NETWORK_REFRESH: Duration = Duration::from_secs(4 * 60 * 60);
const NETWORK_RETRY: Duration = Duration::from_secs(5 * 60);
const SIGNED_OUT: &str = "sign in to use Sikemux on your phone";
const BOUND: &str = "this host belongs to another Sikemux account; to move it, sign in from Sikemux's app or run `sudo sikemux login`";
/// How long after a device joins the account the person still hears of it.
const NEW_DEVICE_NEWS_MS: u64 = 10 * 60_000;
/// How far a signed request's time may be from now; the server allows the same.
const HOST_REQUEST_SKEW_SECS: u64 = 60;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    /// Where the host's key lived before the secret store; read once to take
    /// that host off its account, never written again.
    #[serde(skip_serializing)]
    secret_key: Option<String>,
    enabled: bool,
    devices: Vec<DeviceInfo>,
    owner: Option<String>,
    /// The last account event applied.
    #[serde(default)]
    account_event_id: i64,
    /// The account this host signed out of before the server heard, which
    /// it tells on its next connection.
    pending_leave: Option<String>,
    /// Why the account let this host go, until it signs in again.
    removed: Option<Removal>,
    /// The last network the accounts server described, for when it is out of
    /// reach. A copy this build cannot read is dropped and fetched again.
    #[serde(default, deserialize_with = "readable_or_none")]
    network: Option<Network>,
    /// Each phone's key for its notifications and what it wants to hear
    /// about, by device id.
    #[serde(default)]
    notifications: BTreeMap<String, PhoneNotify>,
    /// When the account lately revoked each phone, in milliseconds since the
    /// epoch, by device id. A join ticket issued before then is refused.
    #[serde(default)]
    revoked: BTreeMap<String, u64>,
    /// The chats this host shares, so they carry on when the core restarts.
    #[serde(default)]
    shares: Vec<StoredShare>,
    /// The account each device joined through, by device id. A device on
    /// the account this host is signed in to may answer for its owner.
    #[serde(default)]
    joined_through: BTreeMap<String, String>,
    /// Hosts on the account linked with this one, whose agents may start
    /// and reach agents here and the other way round.
    #[serde(default)]
    links: Vec<LinkedHost>,
    /// The person asks to allow each new device and host, rather than let
    /// in whatever their account signed in.
    #[serde(default)]
    require_approval: bool,
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

fn readable_or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// Another host this one is linked with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkedHost {
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
pub(crate) struct StoredShare {
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
struct PhoneNotify {
    key_id: u32,
    key: String,
    prefs: NotifyPrefs,
    since: u64,
}

const MAX_MUTES: usize = 256;
const MAX_AGENT_ID: usize = 200;

/// A phone to notify, with its key and what it asked for.
#[derive(Clone, Debug)]
pub(crate) struct NotifyTarget {
    pub device_id: String,
    pub key: NotificationKey,
    pub prefs: NotifyPrefs,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Removal {
    reason: Option<RevokeReason>,
    at: u64,
}

impl Stored {
    /// What decides who may reach this host, leaving out what changes each
    /// time a device or host is seen, as the digest the secret store keeps.
    fn trusted(&self) -> CoreResult<String> {
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
            links: Vec<LinkedHost>,
            shares: &'a [StoredShare],
            revoked: &'a BTreeMap<String, u64>,
            require_approval: bool,
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
                .map(|link| LinkedHost {
                    last_seen: None,
                    addresses: Vec::new(),
                    ..link.clone()
                })
                .collect(),
            shares: &self.shares,
            revoked: &self.revoked,
            require_approval: self.require_approval,
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

struct LiveTask {
    task: JoinHandle<()>,
    changed: watch::Sender<()>,
}

struct Running {
    endpoint: Endpoint,
    accept: JoinHandle<()>,
}

struct Pending {
    device: PendingDevice,
    account: String,
    /// Where a host asking to link was reached; none for a phone.
    host: Option<Vec<String>>,
    answer: oneshot::Sender<Option<DeviceAccess>>,
}

#[derive(Default)]
struct Inner {
    path: Option<PathBuf>,
    keeper: Option<Keeper>,
    /// The account this host belongs to, signed in or not.
    bound: Option<String>,
    /// The digest and account the secret store holds.
    kept: (String, Option<String>),
    /// The digest of the file as last written.
    written: String,
    direct_only: bool,
    secret: Option<SecretKey>,
    /// A key this host moved off while it was on an account, until that
    /// account lets it go.
    retired: Option<SecretKey>,
    stored: Stored,
    running: Option<Running>,
    connected: HashMap<String, usize>,
    pending: Vec<Pending>,
    accounts_api: Option<String>,
    live: Option<LiveTask>,
    link: Option<(Link, u64)>,
    relays: Vec<Relay>,
    /// Dev builds are never too old for the accounts server.
    never_too_old: bool,
    update_required: Option<UpdateRequired>,
    network: Option<JoinHandle<()>>,
    outbox: Arc<live::Outbox>,
    /// What became of each phone's last notification, and when.
    pushed: HashMap<String, (NotificationState, u64)>,
    /// Phones the account will not pass notifications to.
    refused: HashSet<String>,
    #[cfg(test)]
    join_keys: Option<TrustedKeys>,
}

#[derive(Default)]
pub(crate) struct Remote {
    inner: Mutex<Inner>,
    left: Notify,
    /// Cuts the wait before the next read of the network short.
    refresh: Arc<Notify>,
}

pub(crate) fn file_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

fn read_stored(path: &Path) -> CoreResult<Stored> {
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

fn write_stored(path: &Path, stored: &Stored) -> CoreResult<()> {
    sikemux_process::files::write_atomic(path, &serde_json::to_vec_pretty(stored)?, true)?;
    Ok(())
}

impl Remote {
    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.locked().stored.enabled
    }

    pub(crate) fn owner(&self) -> Option<String> {
        self.inner.locked().stored.owner.clone()
    }

    /// This build is older than the accounts server works with.
    pub(crate) fn too_old(&self) -> bool {
        self.inner.locked().update_required.is_some()
    }

    /// What a paired device may do now, or `None` once it is unpaired or remote
    /// access is off.
    pub(crate) fn standing_of(&self, id: &str) -> Option<Standing> {
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

    pub(crate) fn device_platform(&self, id: &str) -> Option<String> {
        let inner = self.inner.locked();
        let device = inner.stored.devices.iter().find(|device| device.id == id)?;
        Some(device.platform.clone())
    }

    pub(crate) fn device_name(&self, id: &str) -> Option<String> {
        let inner = self.inner.locked();
        let device = inner.stored.devices.iter().find(|device| device.id == id)?;
        Some(device.name.clone())
    }

    pub(crate) fn setup(&self) -> Setup {
        let inner = self.inner.locked();
        Setup {
            signed_in: inner.stored.owner.is_some(),
            paired: !inner.stored.devices.is_empty(),
            app_signed: super::local_peer::app_is_signed(),
        }
    }

    pub(crate) fn status(&self) -> RemoteStatus {
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
        }
    }

    /// Whether a device or host with a good ticket waits for the person.
    pub(crate) fn requires_approval(&self) -> bool {
        self.inner.locked().stored.require_approval
    }

    /// Lets a device in at full access as the person, with no question: its
    /// ticket shows it is signed in to this host's own account.
    pub(super) fn admit(&self, device: &PendingDevice, account: String) -> CoreResult<()> {
        self.add_device(
            DeviceInfo {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                access: DeviceAccess::Full,
                paired_at: epoch_ms(),
                last_seen: None,
            },
            Some(account),
        )
    }

    /// Keeps a paired phone's notification key and preferences, replacing
    /// what it sent before.
    pub(crate) fn set_notifications(
        &self,
        id: &str,
        key_id: u32,
        key: &str,
        prefs: NotifyPrefs,
    ) -> CoreResult<()> {
        NotificationKey::from_hex(key_id, key)
            .ok_or_else(|| CoreError::from("the notification key is 32 bytes in hex"))?;
        if prefs.muted.len() > MAX_MUTES
            || prefs
                .muted
                .iter()
                .any(|mute| mute.agent_id.len() > MAX_AGENT_ID)
        {
            return Err("more muted agents than the host keeps".into());
        }
        self.change(|stored| {
            if !stored.devices.iter().any(|device| device.id == id) {
                return Err(CoreError::from(super::access::UNPAIRED));
            }
            let since = match stored.notifications.get(id) {
                Some(known) if known.key_id == key_id && known.key == key => known.since,
                _ => epoch_ms(),
            };
            stored.notifications.insert(
                id.to_owned(),
                PhoneNotify {
                    key_id,
                    key: key.to_ascii_lowercase(),
                    prefs,
                    since,
                },
            );
            Ok(())
        })?;
        let mut inner = self.inner.locked();
        inner.refused.remove(id);
        Ok(())
    }

    pub(crate) fn clear_notifications(&self, id: &str) -> CoreResult<()> {
        self.change(|stored| {
            stored.notifications.remove(id);
            Ok(())
        })?;
        let mut inner = self.inner.locked();
        inner.pushed.remove(id);
        inner.refused.remove(id);
        Ok(())
    }

    /// This host's key and the phones it may notify now: none while it is
    /// signed out, since notifications go through the account.
    pub(crate) fn notify_targets(&self) -> Option<(String, Vec<NotifyTarget>)> {
        let inner = self.inner.locked();
        if inner.stored.owner.is_none() || inner.update_required.is_some() {
            return None;
        }
        let host = inner.secret.as_ref()?.public().to_string();
        let targets = inner
            .stored
            .notifications
            .iter()
            .filter(|(id, _)| !inner.refused.contains(*id))
            .filter(|(id, _)| inner.stored.devices.iter().any(|device| &device.id == *id))
            .filter_map(|(id, notify)| {
                Some(NotifyTarget {
                    device_id: id.clone(),
                    key: NotificationKey::from_hex(notify.key_id, &notify.key)?,
                    prefs: notify.prefs.clone(),
                })
            })
            .collect();
        Some((host, targets))
    }

    /// A host with this key, signed in to `owner`, with these phones paired.
    #[cfg(test)]
    pub(crate) fn stand_in(
        &self,
        secret: SecretKey,
        owner: Option<&str>,
        devices: Vec<DeviceInfo>,
    ) {
        let mut inner = self.inner.locked();
        inner.secret = Some(secret);
        inner.stored.owner = owner.map(str::to_owned);
        inner.stored.devices = devices;
    }

    #[cfg(test)]
    pub(crate) fn listen_on_loopback(&self) {
        self.inner.locked().direct_only = true;
    }

    pub(crate) fn outbox(&self) -> Arc<live::Outbox> {
        self.inner.locked().outbox.clone()
    }

    /// Notes what the server did with a push. True when the phone's state
    /// changed, so the app should hear.
    #[cfg(test)]
    pub(crate) fn set_accounts_api(&self, base: &str) {
        self.inner.locked().accounts_api = Some(base.into());
    }

    pub(crate) fn note_pushed(&self, push: &Push, result: PushResult) -> bool {
        let state = match result {
            PushResult::Sent => NotificationState::On,
            PushResult::NoToken => NotificationState::PhoneOff,
            PushResult::NotAllowed => NotificationState::OtherAccount,
            PushResult::Failed | PushResult::NotSetUp => NotificationState::NotReaching,
            PushResult::Throttled | PushResult::Expired | PushResult::Unknown => return false,
        };
        let mut inner = self.inner.locked();
        if !inner.stored.notifications.contains_key(&push.to) {
            return false;
        }
        if state == NotificationState::OtherAccount {
            inner.refused.insert(push.to.clone());
        }
        let changed = inner.pushed.get(&push.to).map(|(known, _)| *known) != Some(state);
        if changed {
            inner.pushed.insert(push.to.clone(), (state, epoch_ms()));
        }
        changed
    }

    /// A program here other than the app may sign this host in only to the
    /// account it belongs to. The app, root, or that account letting the host
    /// go moves it to another.
    pub(super) fn may_join(&self, account: &str, peer: &Peer) -> CoreResult<()> {
        match &self.inner.locked().bound {
            Some(bound) if bound != account && *peer != Peer::App => Err(BOUND.into()),
            _ => Ok(()),
        }
    }

    fn bind(&self, account: Option<String>) -> CoreResult<()> {
        let mut inner = self.inner.locked();
        inner.bound = account;
        self.save(&mut inner)
    }

    /// The core's key and its signature over the registration text for
    /// `nonce` and `user_id`, which must be a challenge and an account.
    pub(super) fn sign_registration(
        &self,
        nonce: &str,
        user_id: &str,
    ) -> CoreResult<(String, String)> {
        crate::accounts::check_registration(nonce, user_id).map_err(CoreError::from)?;
        self.sign_with_key(|key| crate::accounts::registration_message(nonce, user_id, key))
    }

    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        sign_live(self.inner.locked().secret.as_ref()?, nonce)
    }

    /// The core's key and its signature over the share text for `nonce`.
    pub(super) fn sign_share(&self, nonce: &str) -> Option<(String, String)> {
        crate::accounts::check_live(nonce).ok()?;
        let inner = self.inner.locked();
        let secret = inner.secret.as_ref()?;
        let key = secret.public().to_string();
        let message = crate::accounts::share_message(nonce, &key);
        Some((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
    }

    /// The core's key and its signature over `request`, for calling the
    /// accounts API as this host. The time must be close to now, so no
    /// signature is made ahead for later use.
    pub(super) fn sign_host_request(
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
    pub(super) fn sign_share_viewer(&self, nonce: &str) -> CoreResult<(String, String)> {
        crate::accounts::check_live(nonce).map_err(CoreError::from)?;
        self.sign_with_key(|key| crate::accounts::share_viewer_message(nonce, key))
    }

    fn sign_with_key(&self, message: impl FnOnce(&str) -> String) -> CoreResult<(String, String)> {
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
    pub(super) fn signed_in_api(&self) -> Option<String> {
        let inner = self.inner.locked();
        inner.stored.owner.as_ref()?;
        inner.accounts_api.clone()
    }

    pub(super) fn shares(&self) -> Vec<StoredShare> {
        self.inner.locked().stored.shares.clone()
    }

    pub(super) fn keep_shares(&self, shares: Vec<StoredShare>) -> CoreResult<()> {
        self.change(|stored| {
            stored.shares = shares;
            Ok(())
        })
    }

    /// The key this host signs with, once it has loaded.
    pub(crate) fn host_key(&self) -> Option<SecretKey> {
        self.inner.locked().secret.clone()
    }

    /// The keys of this host and the hosts linked with it.
    pub(crate) fn host_keys(&self) -> Vec<iroh::PublicKey> {
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

    pub(super) fn core_id(&self) -> Option<String> {
        self.inner
            .locked()
            .secret
            .as_ref()
            .map(|secret| secret.public().to_string())
    }

    /// Waits for someone to answer `device`, which came with a ticket from
    /// `account`: a phone, or a host asking to link, reached at `host`.
    pub(super) fn ask(
        &self,
        device: PendingDevice,
        account: String,
        host: Option<Vec<String>>,
    ) -> oneshot::Receiver<Option<DeviceAccess>> {
        let (answer, answered) = oneshot::channel();
        self.inner.locked().pending.push(Pending {
            device,
            account,
            host,
            answer,
        });
        answered
    }

    /// The hosts asking to link with this one.
    pub(crate) fn pending_links(&self) -> Vec<PendingDevice> {
        self.inner
            .locked()
            .pending
            .iter()
            .filter(|pending| pending.host.is_some())
            .map(|pending| pending.device.clone())
            .collect()
    }

    pub(crate) fn links(&self) -> Vec<LinkedHost> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return Vec::new();
        }
        inner.stored.links.clone()
    }

    /// The linked host with this key, while remote access is on.
    pub(crate) fn link(&self, id: &str) -> Option<LinkedHost> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return None;
        }
        inner
            .stored
            .links
            .iter()
            .find(|link| link.id == id)
            .cloned()
    }

    /// Calls the linked hosts what the account calls them, answering whether
    /// any name changed.
    pub(crate) fn name_links(&self, names: &[(String, String)]) -> CoreResult<bool> {
        self.change(|stored| {
            let mut changed = false;
            for link in &mut stored.links {
                if let Some((_, name)) = names.iter().find(|(id, _)| *id == link.id) {
                    if !name.is_empty() && link.name != *name {
                        link.name.clone_from(name);
                        changed = true;
                    }
                }
            }
            Ok(changed)
        })
    }

    /// Keeps a host this one is now linked with, replacing what it knew of it.
    pub(crate) fn add_link(&self, link: LinkedHost) -> CoreResult<()> {
        self.change(|stored| {
            stored.forget(&link.id);
            stored.links.push(link);
            Ok(())
        })
    }

    /// Forgets a linked host, and whether it was one.
    pub(crate) fn forget_link(&self, id: &str) -> bool {
        let forgotten = self.change(|stored| {
            let before = stored.links.len();
            stored.links.retain(|link| link.id != id);
            Ok(stored.links.len() != before)
        });
        forgotten.unwrap_or_else(|error| {
            eprintln!("sikemux core: could not save forgetting a host: {error}");
            false
        })
    }

    /// Notes that a linked host was reached, and where.
    pub(crate) fn link_seen(&self, id: &str, addresses: Option<Vec<String>>) {
        let mut inner = self.inner.locked();
        let Some(link) = inner.stored.links.iter_mut().find(|link| link.id == id) else {
            return;
        };
        link.last_seen = Some(epoch_ms());
        if let Some(addresses) = addresses.filter(|addresses| !addresses.is_empty()) {
            link.addresses = addresses;
        }
        if let Err(error) = self.save(&mut inner) {
            eprintln!("sikemux core: could not save when a host was last seen: {error}");
        }
    }

    /// The endpoint remote access listens on, while it does.
    pub(crate) fn endpoint(&self) -> Option<Endpoint> {
        self.inner
            .locked()
            .running
            .as_ref()
            .map(|running| running.endpoint.clone())
    }

    /// The relays this host listens on, which a linked host listens on too.
    /// None while it listens only on loopback.
    pub(crate) fn relays(&self) -> Vec<Relay> {
        let inner = self.inner.locked();
        if inner.direct_only {
            return Vec::new();
        }
        inner.relays.clone()
    }

    /// Adds an allowed device before telling it, so the answer the app gets
    /// back already lists it.
    pub(super) fn answer(&self, id: &str, access: Option<DeviceAccess>) -> CoreResult<()> {
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
            let added = LinkedHost {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                linked_at: epoch_ms(),
                last_seen: Some(epoch_ms()),
                addresses,
            };
            if let Err(error) = self.add_link(added) {
                let _ = pending.answer.send(None);
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
            };
            if let Err(error) = self.add_device(added, Some(pending.account.clone())) {
                let _ = pending.answer.send(None);
                return Err(error);
            }
        }
        let _ = pending.answer.send(access);
        Ok(())
    }

    /// Lets in phones the person chose when signing in. A phone already
    /// waiting is answered; the rest are paired now and join without asking.
    pub(super) fn allow(&self, devices: Vec<AllowedDevice>) -> CoreResult<()> {
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
    pub(super) fn check_join(
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

    fn join_keys(&self) -> TrustedKeys {
        #[cfg(test)]
        if let Some(keys) = self.inner.locked().join_keys.clone() {
            return keys;
        }
        TrustedKeys::for_this_build()
    }

    #[cfg(test)]
    pub(super) fn trust_join_key(&self, id: &str, key: [u8; 32]) {
        let mut keys = TrustedKeys::default();
        keys.insert(id, key);
        self.inner.locked().join_keys = Some(keys);
    }

    /// Withdraws whatever `device_id` asked before, so a phone that tries
    /// again waits on one request.
    pub(super) fn forget_device_requests(&self, device_id: &str) {
        self.inner
            .locked()
            .pending
            .retain(|pending| pending.device.device_id != device_id);
    }

    pub(super) fn forget_pending(&self, id: &str) {
        self.inner
            .locked()
            .pending
            .retain(|pending| pending.device.id != id);
    }

    fn add_device(&self, device: DeviceInfo, account: Option<String>) -> CoreResult<()> {
        self.change(|stored| {
            stored.forget(&device.id);
            if let Some(account) = account {
                stored.joined_through.insert(device.id.clone(), account);
            }
            stored.devices.push(device);
            Ok(())
        })
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

    fn change<T>(&self, edit: impl FnOnce(&mut Stored) -> CoreResult<T>) -> CoreResult<T> {
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
    crate::accounts::check_live(nonce).ok()?;
    let key = secret.public().to_string();
    let message = crate::accounts::live_message(nonce, &key);
    Some((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
}

fn notifications(inner: &Inner) -> Vec<PhoneNotifications> {
    let signed_out = inner.stored.owner.is_none();
    inner
        .stored
        .notifications
        .iter()
        .filter(|(id, _)| inner.stored.devices.iter().any(|device| &device.id == *id))
        .map(|(id, notify)| {
            let pushed = inner.pushed.get(id).copied();
            let (state, since) = if notify.prefs.when == NotifyWhen::Off {
                (NotificationState::Off, notify.since)
            } else if signed_out {
                (NotificationState::SignedOut, notify.since)
            } else {
                pushed.unwrap_or((NotificationState::On, notify.since))
            };
            PhoneNotifications {
                device_id: id.clone(),
                state,
                since,
            }
        })
        .collect()
}

fn account_link(inner: &Inner) -> Option<AccountLink> {
    if inner.stored.owner.is_some() {
        let (link, since) = inner.link?;
        let state = match link {
            Link::Connecting => AccountLinkState::Connecting,
            Link::Live => AccountLinkState::Live,
            Link::Offline => AccountLinkState::Offline,
        };
        return Some(AccountLink {
            state,
            reason: None,
            since,
        });
    }
    inner.stored.removed.map(|removal| AccountLink {
        state: AccountLinkState::Removed,
        reason: removal.reason,
        since: removal.at,
    })
}

/// Gives a core run without the secret store, as tests run one, the key
/// `key`, trusting the remote file beside `socket` as it now reads.
pub fn trust_remote_file(socket: &Path, key: &SecretKey) -> Result<(), String> {
    let trust = || {
        let stored = read_stored(&file_path(socket))?;
        Keeper::new(socket, false)?.save(&HostKey {
            secret: key.clone(),
            trusted: stored.trusted()?,
            account: stored.owner.clone(),
            retired: None,
            written: None,
        })
    };
    trust().map_err(|error: CoreError| error.to_string())
}

/// Reads the remote file and the key that vouches for it, trusting none of
/// the devices, hosts or account the file names unless the key's digest is
/// the file's.
fn load(keeper: &Keeper, path: &Path) -> CoreResult<(Stored, HostKey, String)> {
    let (file, mut stored) = read_file(path)?;
    let file_key = stored.secret_key.take();
    let trusted = stored.trusted()?;
    let key = match keeper.load()? {
        Some(key)
            if key.trusted == trusted
                || key.written.as_ref() == Some(&trusted)
                || key.trusted == trusted_as_written(&file)? =>
        {
            key
        }
        Some(key) => {
            eprintln!(
                "sikemux core: {} was changed by something other than this host, so it trusts none of the devices or hosts it names",
                path.display()
            );
            stored.distrust();
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

/// Loads the core's key and devices, listens if remote access was left on,
/// and connects to the account the host is signed in to.
pub(crate) async fn start(core: &Arc<Core>, config: &ServerConfig) {
    let direct_only = config.remote_direct_only;
    let path = file_path(&config.socket);
    let loading = path.clone();
    let (socket, secret_store) = (config.socket.clone(), config.secret_store);
    let loaded = blocking(move || {
        let keeper = Keeper::new(&socket, secret_store)?;
        let (stored, key, trusted) = load(&keeper, &loading)?;
        Ok((stored, key, trusted, keeper))
    })
    .await;
    let (stored, key, trusted, keeper) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!(
                "sikemux core: remote access is unavailable, {} did not load: {error}",
                path.display()
            );
            return;
        }
    };
    let enabled = stored.enabled;
    {
        let mut inner = core.remote.inner.locked();
        inner.relays = stored
            .network
            .as_ref()
            .and_then(network::usable_relays)
            .unwrap_or_else(network::default_relays);
        inner.path = Some(path);
        inner.keeper = Some(keeper);
        inner.bound = key.account.clone();
        inner.kept = (trusted.clone(), key.account);
        inner.written = trusted;
        inner.direct_only = direct_only;
        inner.never_too_old = cfg!(debug_assertions);
        inner.secret = Some(key.secret);
        inner.retired = key.retired;
        inner.stored = stored;
        inner.accounts_api = config.accounts_api.clone();
    }
    ensure_live(core);
    leave_with_retired_key(core);
    super::share::restore(core);
    if enabled {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    }
    watch_network(core);
}

/// Reads the network now and every few hours while the core runs. Until the
/// first answer, the host uses the last copy it saved.
fn watch_network(core: &Arc<Core>) {
    let mut inner = core.remote.inner.locked();
    let Some(base) = inner.accounts_api.clone() else {
        return;
    };
    if inner.direct_only || inner.network.is_some() {
        return;
    }
    let refresh = core.remote.refresh.clone();
    let core = Arc::downgrade(core);
    inner.network = Some(tokio::spawn(async move {
        loop {
            let fetched = network::fetch(&base).await;
            let Some(core) = core.upgrade() else {
                return;
            };
            let wait = match fetched {
                Some(fetched) => {
                    apply_network(&core, fetched).await;
                    NETWORK_REFRESH
                }
                None => NETWORK_RETRY,
            };
            drop(core);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = refresh.notified() => {}
            }
        }
    }));
}

/// The network came back or changed, or the computer woke: everything that waits
/// on the network tries again now rather than sitting out its backoff.
pub(crate) async fn network_back(core: &Arc<Core>) {
    let endpoint = core
        .remote
        .inner
        .locked()
        .running
        .as_ref()
        .map(|running| running.endpoint.clone());
    if let Some(endpoint) = endpoint {
        endpoint.network_change().await;
    }
    core.remote.refresh.notify_one();
    ensure_live(core);
    super::share::network_back(core);
}

pub(crate) fn stop_network(core: &Core) {
    if let Some(task) = core.remote.inner.locked().network.take() {
        task.abort();
    }
}

/// Moves a listening endpoint onto the network's relays, and turns remote
/// access and the account off while this build is older than the server
/// allows.
async fn apply_network(core: &Arc<Core>, fetched: Network) {
    let relays = network::usable_relays(&fetched);
    let version = &core.build.version;
    let (moved, endpoint, required, was_required) = {
        let mut inner = core.remote.inner.locked();
        let required = if inner.never_too_old {
            None
        } else {
            network::too_old(
                version,
                super::host::minimum_version(&fetched.minimum_versions),
            )
            .map(|minimum| UpdateRequired {
                current: version.clone(),
                minimum,
            })
        };
        let moved = match &relays {
            Some(relays) if *relays != inner.relays => {
                let changes = network::relay_changes(&inner.relays, relays);
                inner.relays = relays.clone();
                Some(changes)
            }
            _ => None,
        };
        let was_required = std::mem::replace(&mut inner.update_required, required.clone());
        let endpoint = inner
            .running
            .as_ref()
            .map(|running| running.endpoint.clone());
        (moved, endpoint, required, was_required)
    };
    if relays.is_some() {
        let saved = core.remote.change(|stored| {
            if stored.network.as_ref() != Some(&fetched) {
                stored.network = Some(fetched);
            }
            Ok(())
        });
        if let Err(error) = saved {
            eprintln!("sikemux core: could not save the network: {error}");
        }
    }
    if let (Some((removed, added)), Some(endpoint)) = (moved, endpoint) {
        for config in added {
            endpoint.insert_relay(config.url.clone(), config).await;
        }
        for url in &removed {
            endpoint.remove_relay(url).await;
        }
    }
    match (&was_required, &required) {
        (None, Some(required)) => {
            eprintln!(
                "sikemux core: this build ({}) is older than {}, the oldest the accounts server works with; remote access and the account are off until Sikemux updates",
                required.current, required.minimum
            );
            stop(core).await;
            stop_live(core);
            core.updates.check_soon();
        }
        (Some(_), None) => {
            ensure_live(core);
            if core.remote.is_enabled() {
                if let Err(error) = listen(core).await {
                    eprintln!("sikemux core: remote access did not start: {error}");
                }
            }
        }
        _ => {}
    }
    if was_required != required {
        announce(core);
    }
}

fn update_first(required: &UpdateRequired) -> CoreError {
    CoreError::from(format!(
        "update Sikemux first: this version ({}) is older than {}, the oldest the accounts server works with",
        required.current, required.minimum
    ))
}

async fn listen(core: &Arc<Core>) -> CoreResult<()> {
    let (secret, direct_only, relays) = {
        let inner = core.remote.inner.locked();
        if let Some(required) = &inner.update_required {
            return Err(update_first(required));
        }
        if inner.running.is_some() {
            return Ok(());
        }
        let secret = inner
            .secret
            .clone()
            .ok_or_else(|| CoreError::from("remote access has no key"))?;
        (secret, inner.direct_only, inner.relays.clone())
    };
    let builder = if direct_only {
        Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(|error| CoreError::from(error.to_string()))?
    } else {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(network::relay_map(&relays)))
    };
    let endpoint = builder
        .secret_key(secret)
        .alpns(vec![
            CORE_ALPN.to_vec(),
            JOIN_ALPN.to_vec(),
            super::hosts::LINK_ALPN.to_vec(),
        ])
        .bind()
        .await
        .map_err(|error| CoreError::from(format!("remote access did not start: {error}")))?;
    let accept = tokio::spawn(accept(core.clone(), endpoint.clone()));
    let mut inner = core.remote.inner.locked();
    if inner.running.is_some() {
        accept.abort();
        return Ok(());
    }
    inner.running = Some(Running { endpoint, accept });
    drop(inner);
    core.hosts.wake.now();
    Ok(())
}

/// Stops listening and drops every device connection, handing back the
/// endpoint for the caller to close.
fn halt(core: &Core) -> Option<Endpoint> {
    let running = {
        let mut inner = core.remote.inner.locked();
        inner.pending.clear();
        inner.running.take()
    };
    core.close_device_clients(None);
    super::hosts::halt(core);
    running.map(|running| {
        running.accept.abort();
        running.endpoint
    })
}

pub(crate) async fn stop(core: &Core) {
    if let Some(endpoint) = halt(core) {
        endpoint.close().await;
    }
}

pub(crate) async fn set_enabled(core: &Arc<Core>, enabled: bool) -> CoreResult<RemoteStatus> {
    if enabled && core.remote.inner.locked().stored.owner.is_none() {
        return Err(SIGNED_OUT.into());
    }
    if let Some(required) = core
        .remote
        .inner
        .locked()
        .update_required
        .as_ref()
        .filter(|_| enabled)
    {
        return Err(update_first(required));
    }
    core.remote.change(|stored| {
        stored.enabled = enabled;
        Ok(())
    })?;
    if enabled {
        if let Err(error) = listen(core).await {
            core.remote.change(|stored| {
                stored.enabled = false;
                Ok(())
            })?;
            return Err(error);
        }
    } else {
        stop(core).await;
    }
    Ok(announce(core))
}

pub(crate) fn set_access(core: &Core, id: &str, access: DeviceAccess) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        let device = stored
            .devices
            .iter_mut()
            .find(|device| device.id == id)
            .ok_or_else(|| CoreError::from("no paired device has that id"))?;
        device.access = access;
        Ok(())
    })?;
    Ok(announce(core))
}

/// Signing in starts the live connection to the account and turns remote
/// access on. Signing out turns remote access off and sends `leave` on the
/// live connection, now or on the next one, so the account drops this host;
/// paired devices stay.
pub(crate) async fn set_owner(
    core: &Arc<Core>,
    owner: Option<String>,
    peer: &Peer,
) -> CoreResult<RemoteStatus> {
    if let Some(owner) = &owner {
        crate::accounts::check_user_id(owner).map_err(CoreError::from)?;
        core.remote.may_join(owner, peer)?;
        core.remote.bind(Some(owner.clone()))?;
    }
    let live = core.remote.inner.locked().accounts_api.is_some();
    let leaving = core.remote.change(|stored| {
        match owner {
            Some(owner) => {
                if stored.owner.as_ref() != Some(&owner) {
                    stored.account_event_id = 0;
                }
                stored.owner = Some(owner);
                stored.pending_leave = None;
                stored.removed = None;
                stored.enabled = true;
            }
            None => {
                stored.enabled = false;
                if let Some(previous) = stored.owner.take().filter(|_| live) {
                    stored.pending_leave = Some(previous);
                }
            }
        }
        Ok(stored.pending_leave.is_some())
    })?;
    if core.remote.owner().is_none() {
        unlink_all(core);
    }
    if core.remote.is_enabled() {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    } else {
        stop(core).await;
    }
    ensure_live(core);
    if leaving {
        let confirmed = async {
            loop {
                let left = core.remote.left.notified();
                if core.remote.inner.locked().stored.pending_leave.is_none() {
                    return;
                }
                left.await;
            }
        };
        let _ = tokio::time::timeout(LEAVE_WAIT, confirmed).await;
    }
    Ok(announce(core))
}

/// Starts the live connection if the host wants one and has none, or wakes
/// the one it has to look again.
pub(crate) fn ensure_live(core: &Arc<Core>) {
    let mut inner = core.remote.inner.locked();
    if let Some(live) = &inner.live {
        let _ = live.changed.send(());
        return;
    }
    let Some(base) = inner.accounts_api.clone() else {
        return;
    };
    if inner.stored.want() == Want::Stop || inner.update_required.is_some() {
        return;
    }
    let (changed, watching) = watch::channel(());
    let app = LiveApp {
        platform: super::host::PLATFORM,
        version: core.build.version.clone(),
    };
    let account = Arc::new(HostAccount(Arc::downgrade(core)));
    let outbox = inner.outbox.clone();
    let task = tokio::spawn(live::run(account, live::url(&base), app, outbox, watching));
    inner.live = Some(LiveTask { task, changed });
}

/// Takes the host that held the retired key off its account, trying again
/// until the account answers.
fn leave_with_retired_key(core: &Arc<Core>) {
    let inner = core.remote.inner.locked();
    let (Some(key), Some(base)) = (inner.retired.clone(), inner.accounts_api.clone()) else {
        return;
    };
    let app = LiveApp {
        platform: super::host::PLATFORM,
        version: core.build.version.clone(),
    };
    let account = Arc::new(RetiredHost {
        core: Arc::downgrade(core),
        key,
    });
    tokio::spawn(async move {
        let (_changed, watching) = watch::channel(());
        live::run(account, live::url(&base), app, Arc::default(), watching).await;
    });
}

pub(crate) fn stop_live(core: &Core) {
    let mut inner = core.remote.inner.locked();
    inner.link = None;
    if let Some(live) = inner.live.take() {
        live.task.abort();
    }
}

/// The host as its live connection sees it.
struct HostAccount(Weak<Core>);

impl live::Account for HostAccount {
    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        self.0.upgrade()?.remote.sign_live(nonce)
    }

    fn want(&self) -> Want {
        let Some(core) = self.0.upgrade() else {
            return Want::Stop;
        };
        let want = {
            let mut inner = core.remote.inner.locked();
            let want = inner.stored.want();
            if want == Want::Stop {
                inner.live = None;
                inner.link = None;
            }
            want
        };
        if want == Want::Stop {
            announce(&core);
        }
        want
    }

    fn cursor(&self) -> i64 {
        self.0
            .upgrade()
            .map_or(0, |core| core.remote.inner.locked().stored.account_event_id)
    }

    fn apply(&self, events: &[AccountEvent]) {
        if let Some(core) = self.0.upgrade() {
            apply_events(&core, events);
        }
    }

    fn rewind(&self, latest: i64) {
        let Some(core) = self.0.upgrade() else {
            return;
        };
        if core.remote.inner.locked().stored.account_event_id <= latest {
            return;
        }
        let rewound = core.remote.change(|stored| {
            stored.account_event_id = 0;
            Ok(())
        });
        if let Err(error) = rewound {
            eprintln!("sikemux core: could not save the account's place: {error}");
        }
    }

    fn link(&self, link: Link) {
        let Some(core) = self.0.upgrade() else {
            return;
        };
        let changed = {
            let mut inner = core.remote.inner.locked();
            let changed = inner.link.map(|(current, _)| current) != Some(link);
            if changed {
                inner.link = Some((link, epoch_ms()));
            }
            changed
        };
        if changed {
            announce(&core);
        }
        if link == Link::Live {
            core.hosts.relink.now();
        }
    }

    fn removed(&self, reason: Option<RevokeReason>) {
        if let Some(core) = self.0.upgrade() {
            let_go(&core, reason);
            announce(&core);
        }
    }

    fn pushed(&self, push: Push, result: PushResult) {
        if let Some(core) = self.0.upgrade() {
            if core.remote.note_pushed(&push, result) {
                announce(&core);
            }
        }
    }
}

/// The host as it was under its retired key, connecting only to leave.
struct RetiredHost {
    core: Weak<Core>,
    key: SecretKey,
}

impl live::Account for RetiredHost {
    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        sign_live(&self.key, nonce)
    }

    fn want(&self) -> Want {
        match self.core.upgrade() {
            Some(core) if core.remote.inner.locked().retired.is_some() => Want::Leave,
            _ => Want::Stop,
        }
    }

    fn cursor(&self) -> i64 {
        0
    }

    fn apply(&self, _: &[AccountEvent]) {}

    fn rewind(&self, _: i64) {}

    fn link(&self, _: Link) {}

    fn removed(&self, _: Option<RevokeReason>) {
        if let Some(core) = self.core.upgrade() {
            core.remote.forget_retired();
        }
    }

    fn pushed(&self, _: Push, _: PushResult) {}
}

fn revoke_reason(reason: Option<RevokeReason>) -> &'static str {
    match reason {
        Some(RevokeReason::SignedOut) => "it signed out of your account",
        Some(RevokeReason::AccountDeleted) => "your account was deleted",
        _ => "it was removed from your account",
    }
}

/// Forgets each phone the account revoked and lets go of the account if it
/// let go of this host, then moves the cursor past `events`.
fn apply_events(core: &Core, events: &[AccountEvent]) {
    let own = core.remote.core_id();
    let mut revoked = Vec::new();
    let mut released = None;
    for event in events {
        match event.r#type {
            AccountEventType::DeviceRevoked => match event.key.as_deref() {
                Some(key) if Some(key) == own.as_deref() => released = Some(event.reason),
                Some(key) => revoked.push((
                    key.to_owned(),
                    event.reason,
                    live::parse_rfc3339(&event.at).unwrap_or_else(epoch_ms),
                )),
                None => {}
            },
            AccountEventType::AccountDeleted => {
                released = Some(Some(RevokeReason::AccountDeleted));
            }
            AccountEventType::DeviceChanged if event.role == Some(DeviceRole::Host) => {
                core.hosts.relink.now();
            }
            AccountEventType::DeviceAdded => {
                if event.role == Some(DeviceRole::Host) {
                    core.hosts.relink.now();
                }
                // Events a host missed while offline come all at once; only a
                // device that joined lately is news.
                let lately = live::parse_rfc3339(&event.at)
                    .is_some_and(|at| epoch_ms().saturating_sub(at) < NEW_DEVICE_NEWS_MS);
                if let (true, Some(key), Some(core)) = (lately, &event.key, core.me.upgrade()) {
                    if Some(key) != own.as_ref() {
                        super::hosts::account::tell_joined(&core, key.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let last = events.iter().map(|event| event.id).max().unwrap_or(0);
    let mut unlinked = Vec::new();
    let forgotten = core.remote.change(|stored| {
        let mut forgotten = Vec::new();
        for (key, reason, at) in &revoked {
            if stored.links.iter().any(|link| link.id == *key) {
                unlinked.push(key.clone());
            }
            if let Some(device) = stored.forget(key) {
                forgotten.push((device, *reason));
            }
            let latest = stored.revoked.entry(key.clone()).or_default();
            *latest = (*latest).max(*at);
        }
        let forgettable = epoch_ms().saturating_sub(join::REVOCATION_MEMORY_MS);
        stored.revoked.retain(|_, at| *at >= forgettable);
        stored.account_event_id = stored.account_event_id.max(last);
        Ok(forgotten)
    });
    let forgotten = forgotten.unwrap_or_else(|error| {
        eprintln!("sikemux core: could not apply what changed on the account: {error}");
        Vec::new()
    });
    {
        let mut inner = core.remote.inner.locked();
        inner.pending.retain(|pending| {
            !revoked
                .iter()
                .any(|(key, _, _)| *key == pending.device.device_id)
        });
    }
    for (device, reason) in &forgotten {
        core.close_device_clients(Some(&device.id));
        eprintln!(
            "sikemux core: forgot {} ({}): {}",
            if device.name.is_empty() {
                "a device"
            } else {
                &device.name
            },
            device.id.get(..8).unwrap_or(&device.id),
            revoke_reason(*reason)
        );
    }
    for host in unlinked {
        super::hosts::unlinked(core, &host, "it was removed from your account");
    }
    if let Some(reason) = released {
        let_go(core, reason);
    }
    announce(core);
}

/// The account no longer has this host: forget it, keep why until the person
/// signs in again, and turn remote access off. Paired devices stay.
/// A host off its account is linked with none of the account's hosts; it
/// links with them again by itself once it signs in.
fn unlink_all(core: &Core) {
    let links = core
        .remote
        .change(|stored| Ok(std::mem::take(&mut stored.links)));
    for link in links.unwrap_or_default() {
        super::hosts::unlinked(core, &link.id, "this host left your account");
    }
}

fn let_go(core: &Core, reason: Option<RevokeReason>) {
    let released = core.remote.change(|stored| {
        let asked = stored.pending_leave.is_some();
        stored.enabled = false;
        if stored.owner.take().is_some() {
            stored.removed = Some(Removal {
                reason,
                at: epoch_ms(),
            });
            eprintln!(
                "sikemux core: signed out of the account: {}",
                match reason {
                    Some(RevokeReason::AccountDeleted) => "it was deleted",
                    Some(RevokeReason::SignedOut) => "this host signed out elsewhere",
                    _ => "this host was removed from it",
                }
            );
        }
        stored.pending_leave = None;
        Ok(asked)
    });
    let released = released.and_then(|asked| {
        if asked {
            Ok(())
        } else {
            core.remote.bind(None)
        }
    });
    unlink_all(core);
    if let Err(error) = released {
        eprintln!("sikemux core: could not save leaving the account: {error}");
    }
    if let Some(endpoint) = halt(core) {
        tokio::spawn(async move { endpoint.close().await });
    }
    core.remote.left.notify_waiters();
}

pub(crate) fn revoke(core: &Core, id: &str) -> CoreResult<RemoteStatus> {
    let linked = core.remote.change(|stored| {
        let linked = stored.links.iter().any(|link| link.id == id);
        stored.forget(id);
        Ok(linked)
    })?;
    if linked {
        super::hosts::unlink(core, id);
    }
    core.close_device_clients(Some(id));
    Ok(announce(core))
}

/// Who waits to join this host and who is paired, and the hosts linked with
/// it or asking to be, for the owner's devices.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pairing {
    pending: Vec<PendingDevice>,
    devices: Vec<DeviceInfo>,
    links: Vec<super::hosts::LinkView>,
    pending_links: Vec<PendingDevice>,
}

pub(crate) fn pairing(core: &Core) -> Pairing {
    let status = core.remote.status();
    Pairing {
        pending: status.pending,
        devices: status.devices,
        links: super::hosts::views(core),
        pending_links: core.remote.pending_links(),
    }
}

#[derive(Deserialize)]
pub(crate) struct PairingAnswer {
    id: String,
    allow: bool,
    access: DeviceAccess,
}

pub(crate) fn answer_pairing(core: &Core, answer: PairingAnswer) -> CoreResult<Pairing> {
    core.remote
        .answer(&answer.id, answer.allow.then_some(answer.access))?;
    announce(core);
    Ok(pairing(core))
}

#[derive(Deserialize)]
pub(crate) struct DeviceChange {
    id: String,
    /// None unpairs the device.
    access: Option<DeviceAccess>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalSetting {
    require_approval: bool,
}

/// Turns the lock of ADR 0018 on or off: whether each new device and host
/// waits for the person rather than being let in on its account's word.
pub(crate) fn set_require_approval(
    core: &Core,
    setting: ApprovalSetting,
) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        stored.require_approval = setting.require_approval;
        Ok(())
    })?;
    Ok(announce(core))
}

pub(crate) fn change_device(core: &Core, change: DeviceChange) -> CoreResult<Pairing> {
    match change.access {
        Some(access) => set_access(core, &change.id, access)?,
        None => revoke(core, &change.id)?,
    };
    Ok(pairing(core))
}

/// Tells the app what changed, and answers with the same status.
pub(crate) fn announce(core: &Core) -> RemoteStatus {
    if let Some(core) = core.arc() {
        tokio::spawn(async move { super::account::show(&core).await });
    }
    core.changes.changed(Topic::Hosts, None, None);
    let status = core.remote.status();
    core.broadcast_local(&Event::Remote {
        status: status.clone(),
    });
    status
}

/// Boxed because a client it serves can turn remote access on, which starts
/// this loop.
fn accept(core: Arc<Core>, endpoint: Endpoint) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(serve_device(core.clone(), incoming));
        }
    })
}

async fn serve_device(core: Arc<Core>, incoming: Incoming) {
    let Ok(connection) = incoming.await else {
        return;
    };
    if connection.alpn() == JOIN_ALPN {
        super::join::serve(core, connection).await;
        return;
    }
    if connection.alpn() == super::hosts::LINK_ALPN {
        super::hosts::serve(core, connection).await;
        return;
    }
    let id = connection.remote_id().to_string();
    if core.remote.standing_of(&id).is_none() {
        connection.close(
            crate::remote::NOT_PAIRED.into(),
            b"this device is not paired with this core",
        );
        return;
    }
    let Ok((send, recv)) = connection.accept_bi().await else {
        return;
    };
    core.remote.note_connected(&id, true);
    announce(&core);
    serve_client(
        core.clone(),
        Peer::Device { id: id.clone() },
        None,
        Box::new(recv),
        Box::new(send),
    )
    .await;
    core.remote.note_connected(&id, false);
    announce(&core);
    connection.close(0u32.into(), b"");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hosts that update keep the digest their last build took, over that
    /// build's shape of the file; they come back with their account, devices
    /// and links, and keep the canonical digest from then on.
    #[test]
    fn a_remote_file_from_an_earlier_build_stays_trusted() {
        for (file, digest) in [
            (
                include_str!("fixtures/remote/nightly3.remote.json"),
                "5c7327e3e80c6089ca305449ee08a79bb12f01bed332c26cdd637d7b1f52e862",
            ),
            (
                include_str!("fixtures/remote/nightly4.remote.json"),
                "42b087817bc6ada3d6e42d4295c06f7aef8c22abcf481f849a7f4609ebf9f7d2",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("core.sock");
            let path = file_path(&socket);
            std::fs::write(&path, file).unwrap();
            let keeper = Keeper::new(&socket, false).unwrap();
            keeper
                .save(&HostKey {
                    secret: SecretKey::generate(),
                    trusted: digest.into(),
                    account: Some("user_2nightly".into()),
                    retired: None,
                    written: None,
                })
                .unwrap();
            for _ in 0..2 {
                let (stored, _, trusted) = load(&keeper, &path).unwrap();
                assert!(stored.enabled);
                assert_eq!(stored.owner.as_deref(), Some("user_2nightly"));
                assert_eq!(stored.devices.len(), 1);
                assert_eq!(stored.links.len(), 1);
                assert_eq!(stored.joined_through.len(), 1);
                assert_eq!(keeper.load().unwrap().unwrap().trusted, trusted);
            }
            std::fs::write(&path, file.replace("\"full\"", "\"watch\"")).unwrap();
            keeper
                .save(&HostKey {
                    secret: SecretKey::generate(),
                    trusted: digest.into(),
                    account: Some("user_2nightly".into()),
                    retired: None,
                    written: None,
                })
                .unwrap();
            let (edited, _, _) = load(&keeper, &path).unwrap();
            assert!(edited.owner.is_none() && edited.devices.is_empty());
        }
    }

    /// The secret store moves ahead of the file before the file is written.
    /// A host whose write failed, or that stopped in between, comes back with
    /// its file trusted; once the file is written, its state before is not.
    #[test]
    fn a_file_behind_the_secret_store_stays_trusted_until_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("core.sock");
        let path = file_path(&socket);
        std::fs::write(&path, include_str!("fixtures/remote/nightly4.remote.json")).unwrap();
        let keeper = Keeper::new(&socket, false).unwrap();
        keeper
            .save(&HostKey {
                secret: SecretKey::generate(),
                trusted: "42b087817bc6ada3d6e42d4295c06f7aef8c22abcf481f849a7f4609ebf9f7d2".into(),
                account: Some("user_2nightly".into()),
                retired: None,
                written: None,
            })
            .unwrap();
        let (mut stored, key, written) = load(&keeper, &path).unwrap();
        stored.links.clear();
        keeper
            .save(&HostKey {
                trusted: stored.trusted().unwrap(),
                written: Some(written.clone()),
                ..key.clone()
            })
            .unwrap();
        let (stored, key, written) = load(&keeper, &path).unwrap();
        assert_eq!(stored.links.len(), 1);
        assert_eq!(stored.owner.as_deref(), Some("user_2nightly"));

        let before = std::fs::read(&path).unwrap();
        let core = Core::new(crate::protocol::BuildIdentity::default(), None).unwrap();
        {
            let mut inner = core.remote.inner.locked();
            inner.path = Some(path.clone());
            inner.keeper = Some(Keeper::new(&socket, false).unwrap());
            inner.secret = Some(key.secret.clone());
            inner.bound = key.account.clone();
            inner.kept = (written.clone(), key.account.clone());
            inner.written = written;
            inner.stored = stored;
        }
        core.remote
            .change(|stored| {
                stored.links.clear();
                Ok(())
            })
            .unwrap();
        assert_eq!(keeper.load().unwrap().unwrap().written, None);
        std::fs::write(&path, before).unwrap();
        let (put_back, _, _) = load(&keeper, &path).unwrap();
        assert!(put_back.owner.is_none() && put_back.links.is_empty());
    }

    #[test]
    fn the_file_sits_beside_the_socket() {
        assert_eq!(
            file_path(Path::new("/home/me/.config/sikemux/core.dev.sock")),
            PathBuf::from("/home/me/.config/sikemux/core.dev.sock.remote.json")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_devices_survive_a_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/core.sock.remote.json");
        let stored = Stored {
            secret_key: None,
            enabled: true,
            owner: Some("user_2abc".into()),
            account_event_id: 7,
            pending_leave: Some("user_2old".into()),
            removed: None,
            network: None,
            notifications: BTreeMap::new(),
            revoked: BTreeMap::new(),
            shares: Vec::new(),
            joined_through: BTreeMap::new(),
            links: Vec::new(),
            require_approval: false,
            devices: vec![DeviceInfo {
                id: SecretKey::generate().public().to_string(),
                name: "Phone".into(),
                platform: "ios".into(),
                access: DeviceAccess::Watch,
                paired_at: 1,
                last_seen: None,
            }],
        };
        write_stored(&path, &stored).unwrap();
        let read = read_stored(&path).unwrap();
        assert!(read.enabled);
        assert_eq!(read.devices, stored.devices);
        assert_eq!(read.owner.as_deref(), Some("user_2abc"));
        assert_eq!(read.account_event_id, 7);
        assert_eq!(read.pending_leave.as_deref(), Some("user_2old"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn host_requests_sign_the_text_the_server_checks() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../server/protocol/vectors/host-request.json"
        ))
        .unwrap();
        let text = |name: &str| vector[name].as_str().unwrap().to_owned();
        let request: crate::accounts::HostRequest = serde_json::from_value(vector.clone()).unwrap();
        assert_eq!(
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
                text("body").as_bytes()
            )),
            request.body_sha256
        );
        assert_eq!(request.message(), text("message"));
        let remote = Remote::default();
        remote.inner.locked().secret = secret_from_hex(&text("secretKey"));
        assert!(remote.sign_host_request(&request).is_err());
        let (key, signature) = remote.sign_with_key(|_| request.message()).unwrap();
        assert_eq!(key, text("key"));
        assert_eq!(signature, text("signature"));
        assert_eq!(
            format!("SikemuxHost {key}.{}.{signature}", request.time),
            text("authorization")
        );
    }

    /// The same vector the server's tests verify, so both sides sign and
    /// check exactly the same text.
    #[test]
    fn registrations_sign_the_text_the_server_checks() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../server/protocol/vectors/registration.json"
        ))
        .unwrap();
        let text = |name: &str| vector[name].as_str().unwrap().to_owned();
        let remote = Remote::default();
        remote.inner.locked().secret = secret_from_hex(&text("secretKey"));
        let (key, signature) = remote
            .sign_registration(&text("nonce"), &text("userId"))
            .unwrap();
        assert_eq!(key, text("key"));
        assert_eq!(
            crate::accounts::registration_message(&text("nonce"), &text("userId"), &key),
            text("message")
        );
        assert_eq!(signature, text("signature"));
    }

    fn phone(name: &str) -> DeviceInfo {
        DeviceInfo {
            id: SecretKey::generate().public().to_string(),
            name: name.into(),
            platform: "ios".into(),
            access: DeviceAccess::Full,
            paired_at: 1,
            last_seen: None,
        }
    }

    fn event(id: i64, kind: &str, key: Option<&str>, role: &str, reason: &str) -> AccountEvent {
        let mut event = serde_json::json!({ "id": id, "type": kind, "at": "2026-10-03T00:00:00Z" });
        if let Some(key) = key {
            event["key"] = key.into();
            event["role"] = role.into();
            event["reason"] = reason.into();
        }
        serde_json::from_value(event).unwrap()
    }

    /// A core signed in to an account with `phones` paired, saving to `dir`
    /// and listening only on loopback.
    fn signed_in(dir: &Path, phones: &[&DeviceInfo]) -> Arc<Core> {
        let core = Core::new(crate::protocol::BuildIdentity::default(), None).unwrap();
        {
            let mut inner = core.remote.inner.locked();
            inner.path = Some(dir.join("core.sock.remote.json"));
            inner.direct_only = true;
            inner.secret = Some(SecretKey::generate());
            inner.stored.owner = Some("user_2abc".into());
            inner.stored.devices = phones.iter().map(|phone| (*phone).clone()).collect();
        }
        core
    }

    fn devices(core: &Core) -> Vec<String> {
        core.remote
            .status()
            .devices
            .into_iter()
            .map(|device| device.name)
            .collect()
    }

    #[tokio::test]
    async fn a_phone_the_account_revoked_is_forgotten_and_its_request_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (kept, gone) = (phone("Kept"), phone("Gone"));
        let core = signed_in(dir.path(), &[&kept, &gone]);
        let _waiting = core.remote.ask(
            PendingDevice {
                id: "request-1".into(),
                device_id: gone.id.clone(),
                name: "Gone".into(),
                platform: "ios".into(),
                from_account: true,
                expires_at: epoch_ms() + 120_000,
            },
            "user_2abc".into(),
            None,
        );
        let events = [
            event(4, "device.revoked", Some(&gone.id), "client", "removed"),
            event(5, "device.added", Some("someone"), "client", "removed"),
            event(6, "added.later", None, "", ""),
        ];
        apply_events(&core, &events);
        apply_events(&core, &events);

        assert_eq!(devices(&core), vec!["Kept"]);
        assert!(core.remote.status().pending.is_empty());
        assert_eq!(core.remote.status().owner.as_deref(), Some("user_2abc"));
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.account_event_id, 6);
        assert_eq!(saved.devices, vec![kept]);
    }

    fn ticket_for(core: &Core, phone: &str, issued_at: u64) -> JoinTicket {
        let vector = crate::join::vector::vector();
        core.remote
            .trust_join_key(&vector.ticket.key_id, vector.public);
        vector.signed(|ticket| {
            ticket.account = "user_2abc".into();
            ticket.host = core.remote.core_id().unwrap();
            ticket.phone = phone.into();
            ticket.issued_at = issued_at as i64;
            ticket.expires_at = issued_at as i64 + 600;
        })
    }

    fn revoked_at(id: i64, key: &str, at_ms: u64) -> AccountEvent {
        let mut event = event(id, "device.revoked", Some(key), "client", "signed_out");
        event.at = live::rfc3339(at_ms);
        event
    }

    #[tokio::test]
    async fn a_join_ticket_issued_before_the_account_revoked_the_phone_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        let phone = SecretKey::generate().public().to_string();
        let issued = epoch_secs() - 30;
        let ticket = ticket_for(&core, &phone, issued);
        assert_eq!(core.remote.check_join(&ticket, &phone), Ok(None));

        apply_events(&core, &[revoked_at(1, &phone, issued * 1000 + 10_000)]);
        assert_eq!(
            core.remote.check_join(&ticket, &phone),
            Err(Refusal::Revoked)
        );
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.revoked.get(&phone), Some(&(issued * 1000 + 10_000)));

        let reissued = ticket_for(&core, &phone, issued + 20);
        assert_eq!(core.remote.check_join(&reissued, &phone), Ok(None));
    }

    #[tokio::test]
    async fn revocations_too_old_to_matter_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        let (old, recent) = (
            SecretKey::generate().public().to_string(),
            SecretKey::generate().public().to_string(),
        );
        let now = epoch_ms();
        apply_events(
            &core,
            &[
                revoked_at(1, &old, now - crate::join::REVOCATION_MEMORY_MS - 1_000),
                revoked_at(2, &recent, now - 1_000),
            ],
        );
        let kept: Vec<String> = core
            .remote
            .inner
            .locked()
            .stored
            .revoked
            .keys()
            .cloned()
            .collect();
        assert_eq!(kept, vec![recent]);
    }

    #[tokio::test]
    async fn a_signed_out_host_refuses_every_join_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        let phone = SecretKey::generate().public().to_string();
        let ticket = ticket_for(&core, &phone, epoch_secs());
        core.remote.inner.locked().stored.owner = None;
        assert_eq!(
            core.remote.check_join(&ticket, &phone),
            Err(Refusal::SignedOut)
        );
    }

    #[tokio::test]
    async fn a_paired_phone_with_a_ticket_keeps_the_access_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let mut paired = phone("Paired");
        paired.access = DeviceAccess::Watch;
        let core = signed_in(dir.path(), &[&paired]);
        let ticket = ticket_for(&core, &paired.id, epoch_secs());
        assert_eq!(
            core.remote.check_join(&ticket, &paired.id),
            Ok(Some(DeviceAccess::Watch))
        );
    }

    #[tokio::test]
    async fn the_account_letting_go_of_this_host_signs_it_out_and_keeps_its_phones() {
        let dir = tempfile::tempdir().unwrap();
        let kept = phone("Kept");
        let core = signed_in(dir.path(), &[&kept]);
        let own = core.remote.core_id().unwrap();
        apply_events(
            &core,
            &[event(9, "device.revoked", Some(&own), "host", "removed")],
        );

        let status = core.remote.status();
        assert_eq!(status.owner, None);
        assert_eq!(devices(&core), vec!["Kept"]);
        let account = status.account.unwrap();
        assert_eq!(account.state, AccountLinkState::Removed);
        assert_eq!(account.reason, Some(RevokeReason::Removed));

        apply_events(&core, &[event(10, "account.deleted", None, "", "")]);
        let account = core.remote.status().account.unwrap();
        assert_eq!(account.reason, Some(RevokeReason::Removed));
    }

    #[tokio::test]
    async fn a_deleted_account_signs_the_host_out() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        apply_events(&core, &[event(3, "account.deleted", None, "", "")]);
        let status = core.remote.status();
        assert_eq!(status.owner, None);
        assert_eq!(
            status.account.unwrap().reason,
            Some(RevokeReason::AccountDeleted)
        );
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert!(saved.removed.is_some());
    }

    /// Against a stand-in for the accounts server: a phone removed elsewhere
    /// is forgotten while connected, and signing out leaves the account.
    #[tokio::test]
    async fn the_live_connection_revokes_phones_and_signing_out_leaves() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_websockets::{Message, ServerBuilder};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let (kept, gone) = (phone("Kept"), phone("Gone"));
        let core = signed_in(dir.path(), &[&kept, &gone]);
        core.remote.inner.locked().accounts_api = Some(base);
        ensure_live(&core);

        let (stream, _) = listener.accept().await.unwrap();
        let (_, mut socket) = ServerBuilder::new().accept(stream).await.unwrap();
        let send = |value: serde_json::Value| Message::text(value.to_string());
        let nonce = "ab".repeat(32);
        socket
            .send(send(serde_json::json!({ "type": "challenge", "nonce": nonce, "expiresAt": "2026-10-03T00:00:30Z" })))
            .await
            .unwrap();
        let hello = socket.next().await.unwrap().unwrap();
        let hello: serde_json::Value = serde_json::from_str(hello.as_text().unwrap()).unwrap();
        assert_eq!(hello["key"], core.remote.core_id().unwrap());
        socket
            .send(send(
                serde_json::json!({ "type": "ready", "latest": 1, "heartbeatMs": 25000 }),
            ))
            .await
            .unwrap();
        socket
            .send(send(serde_json::json!({ "type": "events", "events": [
                { "id": 1, "type": "device.revoked", "at": "2026-10-03T00:00:00Z", "key": gone.id, "role": "client", "reason": "signed_out" }
            ] })))
            .await
            .unwrap();
        let ack = socket.next().await.unwrap().unwrap();
        assert_eq!(ack.as_text(), Some(r#"{"type":"ack","id":1}"#));
        assert_eq!(devices(&core), vec!["Kept"]);
        assert_eq!(
            core.remote.status().account.unwrap().state,
            AccountLinkState::Live
        );

        let signing_out = tokio::spawn({
            let core = core.clone();
            async move { set_owner(&core, None, &Peer::App).await }
        });
        let leave = socket.next().await.unwrap().unwrap();
        assert_eq!(leave.as_text(), Some(r#"{"type":"leave"}"#));
        socket
            .send(send(
                serde_json::json!({ "type": "revoked", "reason": "signed_out" }),
            ))
            .await
            .unwrap();
        let status = tokio::time::timeout(Duration::from_secs(2), signing_out)
            .await
            .expect("signing out waits only for the server")
            .unwrap()
            .unwrap();
        assert_eq!(status.owner, None);
        assert_eq!(status.account, None);
        assert_eq!(devices(&core), vec!["Kept"]);
        assert!(core.remote.inner.locked().stored.pending_leave.is_none());
    }

    /// A host whose key was in the file, on an account, takes the old host off
    /// the account with that key, even when the first start is offline.
    #[tokio::test]
    async fn a_key_moved_out_of_the_file_leaves_the_account() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_websockets::{Message, ServerBuilder};

        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("core.sock");
        let old = SecretKey::generate();
        std::fs::write(
            file_path(&socket_path),
            serde_json::json!({
                "secretKey": hex::encode(old.to_bytes()),
                "enabled": true,
                "devices": [],
                "owner": "user_2abc",
            })
            .to_string(),
        )
        .unwrap();
        let config = |api: String| ServerConfig {
            remote_direct_only: true,
            accounts_api: Some(api),
            ..ServerConfig::new(socket_path.clone())
        };
        let retired = || {
            Keeper::new(&socket_path, false)
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .retired
                .map(|key| key.public())
        };

        let offline = Core::new(crate::protocol::BuildIdentity::default(), None).unwrap();
        start(&offline, &config("http://127.0.0.1:9".into())).await;
        assert_eq!(retired(), Some(old.public()));
        let file = std::fs::read_to_string(file_path(&socket_path)).unwrap();
        assert!(!file.contains("secretKey"));
        assert_ne!(offline.remote.core_id().unwrap(), old.public().to_string());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let core = Core::new(crate::protocol::BuildIdentity::default(), None).unwrap();
        start(&core, &config(base)).await;
        let (stream, _) = listener.accept().await.unwrap();
        let (_, mut socket) = ServerBuilder::new().accept(stream).await.unwrap();
        let send = |value: serde_json::Value| Message::text(value.to_string());
        socket
            .send(send(serde_json::json!({ "type": "challenge", "nonce": "ab".repeat(32), "expiresAt": "2026-10-03T00:00:30Z" })))
            .await
            .unwrap();
        let hello = socket.next().await.unwrap().unwrap();
        let hello: serde_json::Value = serde_json::from_str(hello.as_text().unwrap()).unwrap();
        assert_eq!(hello["key"], old.public().to_string());
        socket
            .send(send(
                serde_json::json!({ "type": "ready", "latest": 1, "heartbeatMs": 25000 }),
            ))
            .await
            .unwrap();
        let leave = socket.next().await.unwrap().unwrap();
        assert_eq!(leave.as_text(), Some(r#"{"type":"leave"}"#));
        socket
            .send(send(
                serde_json::json!({ "type": "revoked", "reason": "signed_out" }),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while retired().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the retired key is forgotten once the account lets it go");
        assert!(core.remote.inner.locked().retired.is_none());
    }

    #[tokio::test]
    async fn signing_out_offline_keeps_the_leave_for_the_next_connection() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        core.remote.inner.locked().accounts_api = Some("http://127.0.0.1:9".into());
        let status = set_owner(&core, None, &Peer::App).await.unwrap();
        assert_eq!(status.owner, None);
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.pending_leave.as_deref(), Some("user_2abc"));

        set_owner(&core, Some("user_2new".into()), &Peer::App)
            .await
            .unwrap();
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.pending_leave, None);
        stop_live(&core);
    }

    fn network_allowing(version: &str) -> Network {
        let any = serde_json::json!({ "nightly": "0.0.0", "stable": "0.0.0" });
        serde_json::from_value(serde_json::json!({
            "relays": [{ "url": "https://relay.example/", "region": "test", "quicPort": null }],
            "minimumVersions": {
                "macos": { "nightly": version, "stable": version },
                "linux": { "nightly": version, "stable": version },
                "ios": any,
                "android": any,
            },
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_build_older_than_the_server_allows_stays_off_until_it_is_allowed_again() {
        let dir = tempfile::tempdir().unwrap();
        let build = crate::protocol::BuildIdentity {
            version: "0.5.0-nightly.1".into(),
            ..Default::default()
        };
        let core = Core::new(build, None).unwrap();
        {
            let mut inner = core.remote.inner.locked();
            inner.path = Some(dir.path().join("core.sock.remote.json"));
            inner.secret = Some(SecretKey::generate());
            inner.direct_only = true;
            inner.accounts_api = Some("http://127.0.0.1:9".into());
            inner.stored.owner = Some("user_2abc".into());
        }
        set_enabled(&core, true).await.unwrap();
        ensure_live(&core);
        assert!(core.remote.inner.locked().live.is_some());

        apply_network(&core, network_allowing("0.5.0-nightly.2")).await;
        let status = core.remote.status();
        assert_eq!(
            status.update_required,
            Some(UpdateRequired {
                current: "0.5.0-nightly.1".into(),
                minimum: "0.5.0-nightly.2".into(),
            })
        );
        assert!(status.enabled, "the switch stays on for after the update");
        assert!(core.remote.inner.locked().running.is_none());
        assert!(core.remote.inner.locked().live.is_none());
        assert!(set_enabled(&core, true).await.is_err());
        ensure_live(&core);
        assert!(core.remote.inner.locked().live.is_none());
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(
            saved.network.unwrap().relays[0].url,
            "https://relay.example/"
        );
        assert_eq!(
            core.remote.inner.locked().relays[0].url,
            "https://relay.example/"
        );

        apply_network(&core, network_allowing("0.4.0")).await;
        assert_eq!(core.remote.status().update_required, None);
        assert!(core.remote.inner.locked().running.is_some());
        assert!(core.remote.inner.locked().live.is_some());
        stop(&core).await;
        stop_live(&core);
    }

    #[tokio::test]
    async fn remote_access_follows_the_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        core.remote.inner.locked().stored.owner = None;
        let refused = set_enabled(&core, true).await.unwrap_err();
        assert_eq!(refused.to_string(), SIGNED_OUT);
        assert!(!core.remote.is_enabled());

        let status = set_owner(&core, Some("user_2abc".into()), &Peer::App)
            .await
            .unwrap();
        assert!(status.enabled, "signing in turns remote access on");
        assert!(core.remote.inner.locked().running.is_some());

        let status = set_owner(&core, None, &Peer::App).await.unwrap();
        assert!(!status.enabled, "signing out turns remote access off");
        assert!(core.remote.inner.locked().running.is_none());
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert!(!saved.enabled);
    }

    #[tokio::test]
    async fn the_account_letting_go_of_this_host_turns_remote_access_off() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        set_enabled(&core, true).await.unwrap();
        let own = core.remote.core_id().unwrap();
        apply_events(
            &core,
            &[event(1, "device.revoked", Some(&own), "host", "removed")],
        );
        assert!(!core.remote.status().enabled);
        assert!(core.remote.inner.locked().running.is_none());
    }
}
