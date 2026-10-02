//! Paired devices reaching the core from other machines. The core's key, the
//! on/off switch and the trusted devices live in `<socket>.remote.json`, so
//! the dev and release cores keep separate ones.

use std::collections::HashMap;
use std::fs::{DirBuilder, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::endpoint::{presets, Incoming};
use iroh::{Endpoint, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::pairing::{PairingLink, CODE_DIGITS, PAIR_ALPN};
use crate::protocol::{DeviceAccess, DeviceInfo, Event, PairingOffer, PendingDevice, RemoteStatus};
use crate::remote::CORE_ALPN;

use super::access::Peer;
use super::bonjour;
use super::connection::{blocking, serve_client};
use super::{Core, CoreError, CoreResult};

const OFFER_LIFETIME_MS: u64 = 5 * 60 * 1000;
/// Wrong codes one pairing code survives before it is withdrawn.
const OFFER_ATTEMPTS: u8 = 5;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    secret_key: Option<String>,
    enabled: bool,
    devices: Vec<DeviceInfo>,
}

struct Running {
    endpoint: Endpoint,
    accept: JoinHandle<()>,
    _advert: Option<bonjour::Advert>,
}

struct Offer {
    code: String,
    expires_at: u64,
    attempts_left: u8,
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
    offer: Option<Offer>,
    pending: Vec<Pending>,
}

impl Inner {
    fn live_offer(&self) -> Option<&Offer> {
        self.offer
            .as_ref()
            .filter(|offer| offer.attempts_left > 0 && offer.expires_at > unix_ms())
    }
}

#[derive(Default)]
pub(crate) struct Remote {
    inner: Mutex<Inner>,
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
            pairing: inner
                .live_offer()
                .zip(inner.secret.as_ref())
                .map(|(offer, secret)| PairingOffer {
                    code: offer.code.clone(),
                    expires_at: offer.expires_at,
                    link: PairingLink {
                        core: secret.public(),
                        code: offer.code.clone(),
                    }
                    .to_url(),
                }),
            pending: inner
                .pending
                .iter()
                .map(|pending| pending.device.clone())
                .collect(),
        }
    }

    pub(super) fn core_id(&self) -> Option<String> {
        self.lock()
            .secret
            .as_ref()
            .map(|secret| secret.public().to_string())
    }

    pub(super) fn open_offer(&self) -> CoreResult<()> {
        let mut inner = self.lock();
        if !inner.stored.enabled {
            return Err("turn on remote access before pairing a device".into());
        }
        let code = uuid::Uuid::new_v4().as_u128() % 10u128.pow(CODE_DIGITS as u32);
        inner.offer = Some(Offer {
            code: format!("{code:0width$}", width = CODE_DIGITS),
            expires_at: unix_ms() + OFFER_LIFETIME_MS,
            attempts_left: OFFER_ATTEMPTS,
        });
        Ok(())
    }

    pub(super) fn close_offer(&self) {
        self.lock().offer = None;
    }

    /// The open code, spending one of its attempts.
    pub(super) fn attempt(&self) -> Option<String> {
        let mut inner = self.lock();
        inner.live_offer()?;
        let offer = inner.offer.as_mut()?;
        offer.attempts_left -= 1;
        Some(offer.code.clone())
    }

    /// Withdraws `code` once a device has used it, unless another replaced it.
    pub(super) fn spend_offer(&self, code: &str) {
        let mut inner = self.lock();
        if inner.offer.as_ref().is_some_and(|offer| offer.code == code) {
            inner.offer = None;
        }
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

    pub(super) fn forget_pending(&self, id: &str) {
        self.lock()
            .pending
            .retain(|pending| pending.device.id != id);
    }

    fn add_device(&self, device: DeviceInfo) -> CoreResult<()> {
        self.change(|stored| {
            stored.devices.retain(|known| known.id != device.id);
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

    fn change(&self, edit: impl FnOnce(&mut Stored) -> CoreResult<()>) -> CoreResult<()> {
        let mut inner = self.lock();
        edit(&mut inner.stored)?;
        self.save(&inner)
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

/// Loads the core's key and devices, and listens if remote access was left on.
pub(crate) async fn start(core: &Arc<Core>, socket: &Path, direct_only: bool) {
    let path = file_path(socket);
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
        inner.path = Some(path);
        inner.direct_only = direct_only;
        inner.secret = Some(secret);
        inner.stored = stored;
    }
    if enabled {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    }
}

async fn listen(core: &Arc<Core>) -> CoreResult<()> {
    let (secret, direct_only) = {
        let inner = core.remote.lock();
        if inner.running.is_some() {
            return Ok(());
        }
        let secret = inner
            .secret
            .clone()
            .ok_or_else(|| CoreError::from("remote access has no key"))?;
        (secret, inner.direct_only)
    };
    let builder = if direct_only {
        Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(|error| CoreError::from(error.to_string()))?
    } else {
        Endpoint::builder(presets::N0)
    };
    let endpoint = builder
        .secret_key(secret)
        .alpns(vec![CORE_ALPN.to_vec(), PAIR_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| CoreError::from(format!("remote access did not start: {error}")))?;
    let accept = tokio::spawn(accept(core.clone(), endpoint.clone()));
    let advert = if direct_only {
        None
    } else {
        let port = endpoint
            .bound_sockets()
            .iter()
            .find(|address| address.is_ipv4())
            .map(|address| address.port());
        port.and_then(|port| bonjour::advertise(&endpoint.id().to_string(), port))
    };
    let mut inner = core.remote.lock();
    if inner.running.is_some() {
        accept.abort();
        return Ok(());
    }
    inner.running = Some(Running {
        endpoint,
        accept,
        _advert: advert,
    });
    Ok(())
}

pub(crate) async fn stop(core: &Arc<Core>) {
    let running = {
        let mut inner = core.remote.lock();
        inner.offer = None;
        inner.pending.clear();
        inner.running.take()
    };
    core.close_device_clients(None);
    if let Some(running) = running {
        running.accept.abort();
        running.endpoint.close().await;
    }
}

pub(crate) async fn set_enabled(core: &Arc<Core>, enabled: bool) -> CoreResult<RemoteStatus> {
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

pub(crate) fn revoke(core: &Core, id: &str) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        stored.devices.retain(|device| device.id != id);
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
    if connection.alpn() == PAIR_ALPN {
        super::pairing::serve(core, connection).await;
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
        let key = secret_from_hex(read.secret_key.as_deref().unwrap()).unwrap();
        assert_eq!(key.public(), secret.public());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_missing_file_is_remote_access_off_with_no_devices() {
        let dir = tempfile::tempdir().unwrap();
        let stored = read_stored(&dir.path().join("absent.json")).unwrap();
        assert!(!stored.enabled);
        assert!(stored.devices.is_empty());
    }
}
