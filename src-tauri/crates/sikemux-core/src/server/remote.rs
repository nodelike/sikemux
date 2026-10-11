//! Paired devices reaching the core from other machines. The core's key, the
//! on/off switch and the trusted devices live in `<socket>.remote.json`, so
//! the dev and release cores keep separate ones.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{DirBuilder, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::endpoint::{presets, Incoming};
use iroh::{Endpoint, RelayMode, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, watch, Notify};
use tokio::task::JoinHandle;

use crate::accounts::live::{self, Link, Push, Want};
use crate::accounts::network;
use crate::accounts::protocol::{
    AccountEvent, AccountEventType, JoinTicket, LiveApp, Network, Platform, PushResult, Relay,
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

use super::access::Peer;
use super::connection::{blocking, serve_client};
use super::{Core, CoreError, CoreResult, ServerConfig};

/// How long signing out waits for the account to confirm this host left.
#[cfg(not(test))]
const LEAVE_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const LEAVE_WAIT: Duration = Duration::from_millis(300);
const NETWORK_REFRESH: Duration = Duration::from_secs(4 * 60 * 60);
const NETWORK_RETRY: Duration = Duration::from_secs(5 * 60);
const SIGNED_OUT: &str = "sign in to use Sikemux on your phone";

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    secret_key: Option<String>,
    enabled: bool,
    devices: Vec<DeviceInfo>,
    owner: Option<String>,
    /// The last account event applied.
    #[serde(default)]
    account_event_id: i64,
    /// The account this host signed out of before the server heard, which
    /// it tells on its next connection.
    #[serde(default)]
    pending_leave: Option<String>,
    /// Why the account let this host go, until it signs in again.
    #[serde(default)]
    removed: Option<Removal>,
    /// The last network the accounts server described, for when it is out of
    /// reach.
    #[serde(default)]
    network: Option<Network>,
    /// Each phone's key for its notifications and what it wants to hear
    /// about, by device id.
    #[serde(default)]
    notifications: BTreeMap<String, PhoneNotify>,
    /// When the account lately revoked each phone, in milliseconds since the
    /// epoch, by device id. A join ticket issued before then is refused.
    #[serde(default)]
    revoked: BTreeMap<String, u64>,
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
    answer: oneshot::Sender<Option<DeviceAccess>>,
}

#[derive(Default)]
struct Inner {
    path: Option<PathBuf>,
    direct_only: bool,
    secret: Option<SecretKey>,
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
}

pub(crate) fn file_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn read_stored(path: &Path) -> CoreResult<Stored> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_stored(path: &Path, stored: &Stored) -> CoreResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.exists()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(&serde_json::to_vec_pretty(stored)?)?;
    file.sync_all()?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn secret_from_hex(text: &str) -> Option<SecretKey> {
    let bytes: [u8; 32] = hex::decode(text).ok()?.try_into().ok()?;
    Some(SecretKey::from_bytes(&bytes))
}

impl Remote {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.lock().stored.enabled
    }

    /// What a paired device may do now, or `None` once it is unpaired or remote
    /// access is off.
    pub(crate) fn access_of(&self, id: &str) -> Option<DeviceAccess> {
        let inner = self.lock();
        if !inner.stored.enabled {
            return None;
        }
        inner
            .stored
            .devices
            .iter()
            .find(|device| device.id == id)
            .map(|device| device.access)
    }

    pub(crate) fn status(&self) -> RemoteStatus {
        let inner = self.lock();
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
                .map(|pending| pending.device.clone())
                .collect(),
            owner: inner.stored.owner.clone(),
            account: account_link(&inner),
            update_required: inner.update_required.clone(),
            notifications: notifications(&inner),
        }
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
                _ => unix_ms(),
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
        let mut inner = self.lock();
        inner.refused.remove(id);
        Ok(())
    }

    pub(crate) fn clear_notifications(&self, id: &str) -> CoreResult<()> {
        self.change(|stored| {
            stored.notifications.remove(id);
            Ok(())
        })?;
        let mut inner = self.lock();
        inner.pushed.remove(id);
        inner.refused.remove(id);
        Ok(())
    }

    /// This host's key and the phones it may notify now: none while it is
    /// signed out, since notifications go through the account.
    pub(crate) fn notify_targets(&self) -> Option<(String, Vec<NotifyTarget>)> {
        let inner = self.lock();
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
        let mut inner = self.lock();
        inner.secret = Some(secret);
        inner.stored.owner = owner.map(str::to_owned);
        inner.stored.devices = devices;
    }

    #[cfg(test)]
    pub(crate) fn listen_on_loopback(&self) {
        self.lock().direct_only = true;
    }

    pub(crate) fn outbox(&self) -> Arc<live::Outbox> {
        self.lock().outbox.clone()
    }

    /// Notes what the server did with a push. True when the phone's state
    /// changed, so the app should hear.
    #[cfg(test)]
    pub(crate) fn set_accounts_api(&self, base: &str) {
        self.lock().accounts_api = Some(base.into());
    }

    pub(crate) fn note_pushed(&self, push: &Push, result: PushResult) -> bool {
        let state = match result {
            PushResult::Sent => NotificationState::On,
            PushResult::NoToken => NotificationState::PhoneOff,
            PushResult::NotAllowed => NotificationState::OtherAccount,
            PushResult::Failed | PushResult::NotSetUp => NotificationState::NotReaching,
            PushResult::Throttled | PushResult::Expired | PushResult::Unknown => return false,
        };
        let mut inner = self.lock();
        if !inner.stored.notifications.contains_key(&push.to) {
            return false;
        }
        if state == NotificationState::OtherAccount {
            inner.refused.insert(push.to.clone());
        }
        let changed = inner.pushed.get(&push.to).map(|(known, _)| *known) != Some(state);
        if changed {
            inner.pushed.insert(push.to.clone(), (state, unix_ms()));
        }
        changed
    }

    /// The core's key and its signature over the registration text for
    /// `nonce` and `user_id`, which must be a challenge and an account.
    pub(super) fn sign_registration(
        &self,
        nonce: &str,
        user_id: &str,
    ) -> CoreResult<(String, String)> {
        crate::accounts::check_registration(nonce, user_id).map_err(CoreError::from)?;
        let inner = self.lock();
        let secret = inner
            .secret
            .as_ref()
            .ok_or_else(|| CoreError::from("the core's key has not loaded"))?;
        let key = secret.public().to_string();
        let message = crate::accounts::registration_message(nonce, user_id, &key);
        Ok((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
    }

    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        crate::accounts::check_live(nonce).ok()?;
        let inner = self.lock();
        let secret = inner.secret.as_ref()?;
        let key = secret.public().to_string();
        let message = crate::accounts::live_message(nonce, &key);
        Some((key, hex::encode(secret.sign(message.as_bytes()).to_bytes())))
    }

    pub(super) fn core_id(&self) -> Option<String> {
        self.lock()
            .secret
            .as_ref()
            .map(|secret| secret.public().to_string())
    }

    pub(super) fn ask(&self, device: PendingDevice) -> oneshot::Receiver<Option<DeviceAccess>> {
        let (answer, answered) = oneshot::channel();
        self.lock().pending.push(Pending { device, answer });
        answered
    }

    /// Adds an allowed device before telling it, so the answer the app gets
    /// back already lists it.
    pub(super) fn answer(&self, id: &str, access: Option<DeviceAccess>) -> CoreResult<()> {
        let pending = {
            let mut inner = self.lock();
            let index = inner
                .pending
                .iter()
                .position(|pending| pending.device.id == id)
                .ok_or_else(|| CoreError::from("that device is no longer waiting"))?;
            inner.pending.remove(index)
        };
        if let Some(access) = access {
            let device = &pending.device;
            if let Err(error) = self.add_device(DeviceInfo {
                id: device.device_id.clone(),
                name: device.name.clone(),
                platform: device.platform.clone(),
                access,
                paired_at: unix_ms(),
                last_seen: None,
            }) {
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
                let mut inner = self.lock();
                let index = inner
                    .pending
                    .iter()
                    .position(|pending| pending.device.device_id == device.id);
                index.map(|index| inner.pending.remove(index))
            };
            self.add_device(DeviceInfo {
                id: device.id,
                name: device.name,
                platform: device.platform,
                access: device.access,
                paired_at: unix_ms(),
                last_seen: None,
            })?;
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
        let inner = self.lock();
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
                now: (unix_ms() / 1000) as i64,
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
        if let Some(keys) = self.lock().join_keys.clone() {
            return keys;
        }
        TrustedKeys::for_this_build()
    }

    #[cfg(test)]
    pub(super) fn trust_join_key(&self, id: &str, key: [u8; 32]) {
        let mut keys = TrustedKeys::default();
        keys.insert(id, key);
        self.lock().join_keys = Some(keys);
    }

    /// Withdraws whatever `device_id` asked before, so a phone that tries
    /// again waits on one request.
    pub(super) fn forget_device_requests(&self, device_id: &str) {
        self.lock()
            .pending
            .retain(|pending| pending.device.device_id != device_id);
    }

    pub(super) fn forget_pending(&self, id: &str) {
        self.lock()
            .pending
            .retain(|pending| pending.device.id != id);
    }

    fn add_device(&self, device: DeviceInfo) -> CoreResult<()> {
        self.change(|stored| {
            stored.devices.retain(|known| known.id != device.id);
            stored.notifications.remove(&device.id);
            stored.devices.push(device);
            Ok(())
        })
    }

    fn save(&self, inner: &Inner) -> CoreResult<()> {
        match inner.path.as_deref() {
            Some(path) => write_stored(path, &inner.stored),
            None => Ok(()),
        }
    }

    fn change<T>(&self, edit: impl FnOnce(&mut Stored) -> CoreResult<T>) -> CoreResult<T> {
        let mut inner = self.lock();
        let result = edit(&mut inner.stored)?;
        self.save(&inner)?;
        Ok(result)
    }

    fn note_connected(&self, id: &str, connected: bool) {
        let mut inner = self.lock();
        if connected {
            *inner.connected.entry(id.to_owned()).or_default() += 1;
            let now = unix_ms();
            if let Some(device) = inner
                .stored
                .devices
                .iter_mut()
                .find(|device| device.id == id)
            {
                device.last_seen = Some(now);
            }
            if let Err(error) = self.save(&inner) {
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

/// Loads the core's key and devices, listens if remote access was left on,
/// and connects to the account the host is signed in to.
pub(crate) async fn start(core: &Arc<Core>, config: &ServerConfig) {
    let direct_only = config.remote_direct_only;
    let path = file_path(&config.socket);
    let loading = path.clone();
    let loaded = blocking(move || {
        let mut stored = read_stored(&loading)?;
        let secret = match stored.secret_key.as_deref().and_then(secret_from_hex) {
            Some(secret) => secret,
            None => {
                let secret = SecretKey::generate();
                stored.secret_key = Some(hex::encode(secret.to_bytes()));
                write_stored(&loading, &stored)?;
                secret
            }
        };
        Ok((stored, secret))
    })
    .await;
    let (stored, secret) = match loaded {
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
        let mut inner = core.remote.lock();
        inner.relays = stored
            .network
            .as_ref()
            .and_then(network::usable_relays)
            .unwrap_or_else(network::default_relays);
        inner.path = Some(path);
        inner.direct_only = direct_only;
        inner.never_too_old = cfg!(debug_assertions);
        inner.secret = Some(secret);
        inner.stored = stored;
        inner.accounts_api = config.accounts_api.clone();
    }
    ensure_live(core);
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
    let mut inner = core.remote.lock();
    let Some(base) = inner.accounts_api.clone() else {
        return;
    };
    if inner.direct_only || inner.network.is_some() {
        return;
    }
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
            tokio::time::sleep(wait).await;
        }
    }));
}

pub(crate) fn stop_network(core: &Core) {
    if let Some(task) = core.remote.lock().network.take() {
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
        let mut inner = core.remote.lock();
        let required = if inner.never_too_old {
            None
        } else {
            network::too_old(version, &fetched.minimum_versions.macos).map(|minimum| {
                UpdateRequired {
                    current: version.clone(),
                    minimum,
                }
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
        let inner = core.remote.lock();
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
        .alpns(vec![CORE_ALPN.to_vec(), JOIN_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| CoreError::from(format!("remote access did not start: {error}")))?;
    let accept = tokio::spawn(accept(core.clone(), endpoint.clone()));
    let mut inner = core.remote.lock();
    if inner.running.is_some() {
        accept.abort();
        return Ok(());
    }
    inner.running = Some(Running { endpoint, accept });
    Ok(())
}

/// Stops listening and drops every device connection, handing back the
/// endpoint for the caller to close.
fn halt(core: &Core) -> Option<Endpoint> {
    let running = {
        let mut inner = core.remote.lock();
        inner.pending.clear();
        inner.running.take()
    };
    core.close_device_clients(None);
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
    if enabled && core.remote.lock().stored.owner.is_none() {
        return Err(SIGNED_OUT.into());
    }
    if let Some(required) = core
        .remote
        .lock()
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
pub(crate) async fn set_owner(core: &Arc<Core>, owner: Option<String>) -> CoreResult<RemoteStatus> {
    if let Some(owner) = &owner {
        crate::accounts::check_user_id(owner).map_err(CoreError::from)?;
    }
    let live = core.remote.lock().accounts_api.is_some();
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
                if core.remote.lock().stored.pending_leave.is_none() {
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
    let mut inner = core.remote.lock();
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
        platform: if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Unknown
        },
        version: core.build.version.clone(),
    };
    let account = Arc::new(HostAccount(Arc::downgrade(core)));
    let outbox = inner.outbox.clone();
    let task = tokio::spawn(live::run(account, live::url(&base), app, outbox, watching));
    inner.live = Some(LiveTask { task, changed });
}

pub(crate) fn stop_live(core: &Core) {
    let mut inner = core.remote.lock();
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
            let mut inner = core.remote.lock();
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
            .map_or(0, |core| core.remote.lock().stored.account_event_id)
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
        if core.remote.lock().stored.account_event_id <= latest {
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
            let mut inner = core.remote.lock();
            let changed = inner.link.map(|(current, _)| current) != Some(link);
            if changed {
                inner.link = Some((link, unix_ms()));
            }
            changed
        };
        if changed {
            announce(&core);
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
                    live::parse_rfc3339(&event.at).unwrap_or_else(unix_ms),
                )),
                None => {}
            },
            AccountEventType::AccountDeleted => {
                released = Some(Some(RevokeReason::AccountDeleted));
            }
            _ => {}
        }
    }
    let last = events.iter().map(|event| event.id).max().unwrap_or(0);
    let forgotten = core.remote.change(|stored| {
        let mut forgotten = Vec::new();
        for (key, reason, at) in &revoked {
            if let Some(index) = stored.devices.iter().position(|device| &device.id == key) {
                forgotten.push((stored.devices.remove(index), *reason));
            }
            stored.notifications.remove(key);
            let latest = stored.revoked.entry(key.clone()).or_default();
            *latest = (*latest).max(*at);
        }
        let forgettable = unix_ms().saturating_sub(join::REVOCATION_MEMORY_MS);
        stored.revoked.retain(|_, at| *at >= forgettable);
        stored.account_event_id = stored.account_event_id.max(last);
        Ok(forgotten)
    });
    let forgotten = forgotten.unwrap_or_else(|error| {
        eprintln!("sikemux core: could not apply what changed on the account: {error}");
        Vec::new()
    });
    {
        let mut inner = core.remote.lock();
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
    if let Some(reason) = released {
        let_go(core, reason);
    }
    announce(core);
}

/// The account no longer has this host: forget it, keep why until the person
/// signs in again, and turn remote access off. Paired devices stay.
fn let_go(core: &Core, reason: Option<RevokeReason>) {
    let released = core.remote.change(|stored| {
        stored.enabled = false;
        if stored.owner.take().is_some() {
            stored.removed = Some(Removal {
                reason,
                at: unix_ms(),
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
        Ok(())
    });
    if let Err(error) = released {
        eprintln!("sikemux core: could not save leaving the account: {error}");
    }
    if let Some(endpoint) = halt(core) {
        tokio::spawn(async move { endpoint.close().await });
    }
    core.remote.left.notify_waiters();
}

pub(crate) fn revoke(core: &Core, id: &str) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        stored.devices.retain(|device| device.id != id);
        stored.notifications.remove(id);
        Ok(())
    })?;
    core.close_device_clients(Some(id));
    Ok(announce(core))
}

/// Tells the app what changed, and answers with the same status.
pub(crate) fn announce(core: &Core) -> RemoteStatus {
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
    let id = connection.remote_id().to_string();
    if core.remote.access_of(&id).is_none() {
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

    #[test]
    fn the_file_sits_beside_the_socket() {
        assert_eq!(
            file_path(Path::new("/home/me/.config/sikemux/core.dev.sock")),
            PathBuf::from("/home/me/.config/sikemux/core.dev.sock.remote.json")
        );
    }

    #[test]
    fn the_key_and_devices_survive_a_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/core.sock.remote.json");
        let secret = SecretKey::generate();
        let stored = Stored {
            secret_key: Some(hex::encode(secret.to_bytes())),
            enabled: true,
            owner: Some("user_2abc".into()),
            account_event_id: 7,
            pending_leave: Some("user_2old".into()),
            removed: None,
            network: None,
            notifications: BTreeMap::new(),
            revoked: BTreeMap::new(),
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
        let key = secret_from_hex(read.secret_key.as_deref().unwrap()).unwrap();
        assert_eq!(key.public(), secret.public());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
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
        remote.lock().secret = secret_from_hex(&text("secretKey"));
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
            let mut inner = core.remote.lock();
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
        let _waiting = core.remote.ask(PendingDevice {
            id: "request-1".into(),
            device_id: gone.id.clone(),
            name: "Gone".into(),
            platform: "ios".into(),
            from_account: true,
            expires_at: unix_ms() + 120_000,
        });
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
        let issued = unix_ms() / 1000 - 30;
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
        let now = unix_ms();
        apply_events(
            &core,
            &[
                revoked_at(1, &old, now - crate::join::REVOCATION_MEMORY_MS - 1_000),
                revoked_at(2, &recent, now - 1_000),
            ],
        );
        let kept: Vec<String> = core.remote.lock().stored.revoked.keys().cloned().collect();
        assert_eq!(kept, vec![recent]);
    }

    #[tokio::test]
    async fn a_signed_out_host_refuses_every_join_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        let phone = SecretKey::generate().public().to_string();
        let ticket = ticket_for(&core, &phone, unix_ms() / 1000);
        core.remote.lock().stored.owner = None;
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
        let ticket = ticket_for(&core, &paired.id, unix_ms() / 1000);
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
        core.remote.lock().accounts_api = Some(base);
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
            async move { set_owner(&core, None).await }
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
        assert!(core.remote.lock().stored.pending_leave.is_none());
    }

    #[tokio::test]
    async fn signing_out_offline_keeps_the_leave_for_the_next_connection() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        core.remote.lock().accounts_api = Some("http://127.0.0.1:9".into());
        let status = set_owner(&core, None).await.unwrap();
        assert_eq!(status.owner, None);
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.pending_leave.as_deref(), Some("user_2abc"));

        set_owner(&core, Some("user_2new".into())).await.unwrap();
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(saved.pending_leave, None);
        stop_live(&core);
    }

    fn network_allowing(macos: &str) -> Network {
        let any = serde_json::json!({ "nightly": "0.0.0", "stable": "0.0.0" });
        serde_json::from_value(serde_json::json!({
            "relays": [{ "url": "https://relay.example/", "region": "test", "quicPort": null }],
            "minimumVersions": {
                "macos": { "nightly": macos, "stable": macos },
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
            let mut inner = core.remote.lock();
            inner.path = Some(dir.path().join("core.sock.remote.json"));
            inner.secret = Some(SecretKey::generate());
            inner.direct_only = true;
            inner.accounts_api = Some("http://127.0.0.1:9".into());
            inner.stored.owner = Some("user_2abc".into());
        }
        set_enabled(&core, true).await.unwrap();
        ensure_live(&core);
        assert!(core.remote.lock().live.is_some());

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
        assert!(core.remote.lock().running.is_none());
        assert!(core.remote.lock().live.is_none());
        assert!(set_enabled(&core, true).await.is_err());
        ensure_live(&core);
        assert!(core.remote.lock().live.is_none());
        let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
        assert_eq!(
            saved.network.unwrap().relays[0].url,
            "https://relay.example/"
        );
        assert_eq!(core.remote.lock().relays[0].url, "https://relay.example/");

        apply_network(&core, network_allowing("0.4.0")).await;
        assert_eq!(core.remote.status().update_required, None);
        assert!(core.remote.lock().running.is_some());
        assert!(core.remote.lock().live.is_some());
        stop(&core).await;
        stop_live(&core);
    }

    #[tokio::test]
    async fn remote_access_follows_the_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let core = signed_in(dir.path(), &[]);
        core.remote.lock().stored.owner = None;
        let refused = set_enabled(&core, true).await.unwrap_err();
        assert_eq!(refused.to_string(), SIGNED_OUT);
        assert!(!core.remote.is_enabled());

        let status = set_owner(&core, Some("user_2abc".into())).await.unwrap();
        assert!(status.enabled, "signing in turns remote access on");
        assert!(core.remote.lock().running.is_some());

        let status = set_owner(&core, None).await.unwrap();
        assert!(!status.enabled, "signing out turns remote access off");
        assert!(core.remote.lock().running.is_none());
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
        assert!(core.remote.lock().running.is_none());
    }
}
