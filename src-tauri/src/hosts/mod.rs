//! The account's other hosts, which the app reaches directly over iroh as a
//! device of its own with full access, as the phone does (ADR 0017), beside
//! its own core over the socket. Each host the app joined keeps one
//! connection, taken up again whenever it drops.
//!
//! A remote host's terminals reach the page under ids of their own: the
//! host's slot in the high bits and its session id below, so every id the page
//! keys by stays unique and this host's own sessions keep their ids.

pub(crate) mod install;
mod sink;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_core::accounts::network;
use sikemux_core::accounts::protocol::{JoinTicket, Platform};
use sikemux_core::client::{ClientError, CoreClient};
use sikemux_core::device::Device;
use sikemux_core::join::{JoinHello, JoinReply};
use sikemux_core::protocol::{DeviceView, SessionId, SessionInfo};
use sikemux_core::remote::{PublicKey, SecretKey};
use sikemux_process::sync::Locked;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

use crate::error::{AppError, AppResult};
use crate::pty::streams::StreamTable;
use crate::pty::{core_error, PtyManager};

use sink::RemoteSink;

/// Core session ids start from the clock in milliseconds, which fits in 42
/// bits until 2109. Above them sit the host's slot and, at the top of what a
/// JavaScript number holds exactly (2^53), a bit no session of this host has.
const SLOT_SHIFT: u32 = 42;
const SESSION_MASK: SessionId = (1 << SLOT_SHIFT) - 1;
const REMOTE: SessionId = 1 << 52;
const MAX_SLOT: u64 = (1 << (52 - SLOT_SHIFT)) - 1;
/// How long a request waits for a host that is reconnecting.
const REACH_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(30);
const HOST_VIEW_EVENT: &str = "host_view";

#[cfg(not(test))]
const KEY_SERVICE: &str = "sikemux-device";
#[cfg(test)]
const KEY_SERVICE: &str = "sikemux-device-test";

/// Which host a session id the page holds lives on: 0 is this one.
pub(crate) fn slot_of(id: SessionId) -> u64 {
    if id & REMOTE == 0 {
        0
    } else {
        (id & !REMOTE) >> SLOT_SHIFT
    }
}

fn page_id(slot: u64, id: SessionId) -> SessionId {
    REMOTE | (slot << SLOT_SHIFT) | (id & SESSION_MASK)
}

/// A session's id in its own host's core.
pub(crate) fn core_session(id: SessionId) -> SessionId {
    if id & REMOTE == 0 {
        id
    } else {
        id & SESSION_MASK
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    id: String,
    name: String,
    platform: String,
    slot: u64,
}

/// `hosts.json`: the hosts the app joined, and the slot each host key ever
/// had. A slot stays with its key after the host is left, so a tab kept from
/// that host never reaches another one, and finds it again if it is rejoined.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostsFile {
    hosts: Vec<Saved>,
    slots: BTreeMap<String, u64>,
    /// What the app no longer reads stays saved until the core has copied it.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

pub(crate) struct Host {
    saved: Saved,
    /// What this host is linked under now, which the person may change.
    name: Mutex<String>,
    key: PublicKey,
    pub(crate) streams: Arc<StreamTable>,
    client: watch::Sender<Option<Arc<CoreClient>>>,
    /// The sessions its last view listed, under the page's ids.
    sessions: Mutex<Vec<SessionInfo>>,
    forgotten: AtomicBool,
}

impl Host {
    pub(crate) fn id(&self) -> &str {
        &self.saved.id
    }

    /// Takes the name the host is linked under, answering whether it changed.
    fn renamed(&self, name: &str) -> bool {
        let mut current = self.name.locked();
        let changed = *current != name;
        if changed {
            name.clone_into(&mut current);
        }
        changed
    }

    pub(crate) fn page_id(&self, id: SessionId) -> SessionId {
        page_id(self.saved.slot, id)
    }

    fn name(&self) -> String {
        self.name.locked().clone()
    }

    fn saved(&self) -> Saved {
        Saved {
            name: self.name(),
            ..self.saved.clone()
        }
    }

    pub(crate) fn current(&self) -> Option<Arc<CoreClient>> {
        self.client
            .borrow()
            .clone()
            .filter(|client| client.is_connected())
    }

    /// The live connection, waiting a while for one that is being made.
    pub(crate) async fn client(&self) -> AppResult<Arc<CoreClient>> {
        let mut changes = self.client.subscribe();
        let reached = tokio::time::timeout(REACH_TIMEOUT, async {
            loop {
                if let Some(client) = changes
                    .borrow_and_update()
                    .clone()
                    .filter(|client| client.is_connected())
                {
                    return Some(client);
                }
                if self.forgotten.load(Ordering::Acquire) || changes.changed().await.is_err() {
                    return None;
                }
            }
        })
        .await;
        reached
            .ok()
            .flatten()
            .ok_or_else(|| AppError::Pty(format!("{} is out of reach right now", self.name())))
    }

    /// Keeps the host's view, with its sessions under the page's ids, and
    /// answers it for the page.
    fn viewed(&self, mut view: DeviceView) -> DeviceView {
        for session in &mut view.sessions {
            session.id = self.page_id(session.id);
        }
        *self.sessions.locked() = view.sessions.clone();
        view
    }
}

#[derive(Default)]
pub struct Hosts {
    app: OnceLock<AppHandle>,
    device: tokio::sync::OnceCell<Arc<Device>>,
    hosts: Mutex<Vec<Arc<Host>>>,
    slots: Mutex<BTreeMap<String, u64>>,
    rest: Mutex<serde_json::Map<String, serde_json::Value>>,
    saving: Mutex<()>,
    following: tokio::sync::Mutex<()>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostView<'a> {
    host: &'a str,
    view: DeviceView,
}

fn hosts_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("hosts.json"))
}

/// Dev builds keep their own key, as they run their own core.
fn key_account() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "default"
    }
}

fn keychain_error(error: impl std::fmt::Display) -> AppError {
    AppError::Other(format!("this app's device key is unavailable: {error}"))
}

/// This app's key on the network, made the first time it is needed.
async fn device_key() -> AppResult<SecretKey> {
    tokio::task::spawn_blocking(|| {
        let saved = sikemux_keychain::read(KEY_SERVICE, key_account()).map_err(keychain_error)?;
        let bytes = saved
            .and_then(|text| hex::decode(text.trim()).ok())
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        if let Some(bytes) = bytes {
            return Ok(SecretKey::from_bytes(&bytes));
        }
        let key = SecretKey::generate();
        sikemux_keychain::write(KEY_SERVICE, key_account(), &hex::encode(key.to_bytes()))
            .map_err(keychain_error)?;
        Ok(key)
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
}

impl Hosts {
    /// Takes up every host the app joined before. Nothing goes on the network
    /// while there is none.
    pub fn start(&self, app: &AppHandle) {
        let _ = self.app.set(app.clone());
        let file: HostsFile = hosts_file(app)
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        *self.slots.locked() = file.slots;
        *self.rest.locked() = file.rest;
        for saved in file.hosts {
            self.add(saved);
        }
    }

    /// The key this app is known by on the account, once it has gone on the
    /// network.
    pub(crate) fn own_key(&self) -> Option<String> {
        self.device.get().map(|device| device.id())
    }

    fn app(&self) -> AppResult<&AppHandle> {
        self.app
            .get()
            .ok_or_else(|| AppError::Other("hosts are not set up yet".into()))
    }

    async fn device(&self) -> AppResult<Arc<Device>> {
        self.device
            .get_or_try_init(|| async {
                let key = device_key().await?;
                let relays = network::fetch(&sikemux_core::accounts::api_base())
                    .await
                    .and_then(|network| network::usable_relays(&network))
                    .unwrap_or_default();
                Device::bind(key, relays)
                    .await
                    .map(Arc::new)
                    .map_err(core_error)
            })
            .await
            .cloned()
    }

    fn add(&self, saved: Saved) -> Option<Arc<Host>> {
        let key = saved.id.parse().ok()?;
        let host = Arc::new(Host {
            name: Mutex::new(saved.name.clone()),
            saved,
            key,
            streams: Arc::default(),
            client: watch::Sender::new(None),
            sessions: Mutex::default(),
            forgotten: AtomicBool::new(false),
        });
        self.hosts.locked().push(host.clone());
        let app = self.app.get()?.clone();
        tauri::async_runtime::spawn(stay_connected(app, host.clone()));
        Some(host)
    }

    fn save(&self) {
        let Some(file) = self.app.get().and_then(hosts_file) else {
            return;
        };
        let _saving = self.saving.locked();
        let saved = HostsFile {
            hosts: self
                .hosts
                .locked()
                .iter()
                .map(|host| host.saved())
                .collect(),
            slots: self.slots.locked().clone(),
            rest: self.rest.locked().clone(),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&saved) {
            if let Some(directory) = file.parent() {
                let _ = std::fs::create_dir_all(directory);
            }
            let _ = std::fs::write(file, bytes);
        }
    }

    pub(crate) fn by_id(&self, id: &str) -> AppResult<Arc<Host>> {
        self.hosts
            .locked()
            .iter()
            .find(|host| host.saved.id == id)
            .cloned()
            .ok_or_else(|| AppError::Other("this app has not joined that host".into()))
    }

    /// The host a session id the page holds lives on, and its id there. Ids
    /// of this host's own sessions answer `None`.
    pub(crate) fn of_session(&self, id: SessionId) -> AppResult<Option<(Arc<Host>, SessionId)>> {
        let slot = slot_of(id);
        if slot == 0 {
            return Ok(None);
        }
        let host = self
            .hosts
            .locked()
            .iter()
            .find(|host| host.saved.slot == slot)
            .cloned()
            .ok_or_else(|| AppError::Pty("that terminal's host was removed".into()))?;
        Ok(Some((host, core_session(id))))
    }

    /// The slot `id` had before, or one no host key ever had.
    fn slot_for(&self, id: &str) -> AppResult<u64> {
        let mut slots = self.slots.locked();
        if let Some(slot) = slots.get(id) {
            return Ok(*slot);
        }
        let slot = slots.values().max().copied().unwrap_or(0) + 1;
        if slot > MAX_SLOT {
            return Err(AppError::Other(
                "this app has joined as many hosts as it can".into(),
            ));
        }
        slots.insert(id.to_owned(), slot);
        Ok(slot)
    }
}

/// Keeps one connection to the host, made again whenever it drops, until the
/// host forgets this app or the app forgets the host.
async fn stay_connected(app: AppHandle, host: Arc<Host>) {
    let Some(hosts) = app.try_state::<Hosts>() else {
        return;
    };
    let mut wait = RETRY_FIRST;
    while !host.forgotten.load(Ordering::Acquire) {
        let device = match hosts.device().await {
            Ok(device) => device,
            Err(error) => {
                eprintln!("Sikemux could not go on the network to reach other hosts: {error}");
                tokio::time::sleep(RETRY_MOST).await;
                continue;
            }
        };
        let (closed, mut on_closed) = tokio::sync::mpsc::unbounded_channel();
        let sink = Arc::new(RemoteSink::new(app.clone(), host.clone(), closed));
        match device.connect(host.key, sink).await {
            Ok((client, link)) => {
                let client = Arc::new(client);
                let reconnected = host.client.send_replace(Some(client.clone())).is_some();
                if reconnected {
                    crate::pty::reattach(&client, &host.streams).await;
                }
                crate::acp::reconnected(&app, Some(host.id()), Some(&client)).await;
                wait = RETRY_FIRST;
                let _ = on_closed.recv().await;
                link.close(0u32.into(), b"closed");
            }
            Err(ClientError::NotPaired) => {
                forget(&hosts, &host);
                hosts.save();
                follow_links(&app);
                return;
            }
            Err(error) => {
                eprintln!("Sikemux could not reach {}: {error}", host.name());
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(RETRY_MOST);
            }
        }
    }
}

fn forget(hosts: &Hosts, host: &Arc<Host>) {
    host.forgotten.store(true, Ordering::Release);
    hosts.hosts.locked().retain(|kept| !Arc::ptr_eq(kept, host));
    crate::pty::sink::report_all_exited(&host.streams);
    host.client.send_replace(None);
}

/// A host this one is linked with, as the core lists it.
#[derive(Deserialize)]
struct Linked {
    id: String,
    name: String,
    platform: String,
}

/// Joins every host this host is linked with, which is every other host on
/// the account (ADR 0018), and lets go of those it no longer is.
pub(crate) fn follow_links(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (Some(hosts), Some(manager)) =
            (app.try_state::<Hosts>(), app.try_state::<PtyManager>())
        else {
            return;
        };
        if let Err(error) = hosts.follow(&manager).await {
            eprintln!("Sikemux could not reach the account's other hosts: {error}");
        }
    });
}

impl Hosts {
    async fn follow(&self, manager: &PtyManager) -> AppResult<()> {
        let _one_at_a_time = self.following.lock().await;
        let core = manager.client().await?;
        let pairing = core
            .call("pairing", serde_json::Value::Null)
            .await
            .map_err(core_error)?;
        let links: Vec<Linked> = serde_json::from_value(pairing["links"].clone())?;
        let gone: Vec<Arc<Host>> = self
            .hosts
            .locked()
            .iter()
            .filter(|host| !links.iter().any(|link| link.id == host.saved.id))
            .cloned()
            .collect();
        for host in &gone {
            forget(self, host);
        }
        let mut joined = !gone.is_empty();
        for link in links {
            if let Ok(known) = self.by_id(&link.id) {
                if known.renamed(&link.name) {
                    joined = true;
                }
                continue;
            }
            match self.join(&core, link).await {
                Ok(()) => joined = true,
                Err(error) => eprintln!("Sikemux could not join another host: {error}"),
            }
        }
        if joined {
            self.save();
        }
        Ok(())
    }

    /// Joins the linked host as a device of this app's own, with a ticket
    /// the account gives this host for it.
    async fn join(&self, core: &CoreClient, link: Linked) -> AppResult<()> {
        let key: PublicKey = link
            .id
            .parse()
            .map_err(|_| AppError::BadArg("a host is named by its key"))?;
        let device = self.device().await?;
        let ticket: JoinTicket = crate::account::app_join_ticket(core, &device, &link.id).await?;
        let hello = JoinHello {
            ticket,
            name: String::new(),
            platform: serde_json::to_value(Platform::Macos)?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        match device
            .join(key, &hello)
            .await
            .map_err(|error| AppError::Other(error.to_string()))?
        {
            JoinReply::Allowed { .. } => {}
            JoinReply::Denied => {
                return Err(AppError::Other(format!(
                    "{} did not allow this app",
                    link.name
                )))
            }
            JoinReply::Refused { reason } => {
                return Err(AppError::Other(format!(
                    "{} would not take this app's ticket ({reason})",
                    link.name
                )))
            }
        }
        self.add(Saved {
            slot: self.slot_for(&link.id)?,
            id: link.id,
            name: link.name,
            platform: link.platform,
        });
        Ok(())
    }
}

/// The sessions `id` listed last, waiting a while for a host that is
/// reconnecting.
pub(crate) async fn sessions(hosts: &Hosts, id: &str) -> AppResult<Vec<SessionInfo>> {
    let host = hosts.by_id(id)?;
    host.client().await?;
    let sessions = host.sessions.locked().clone();
    Ok(sessions)
}

/// Where requests go: this host's own core, or another the app joined.
pub(crate) async fn client(
    manager: &PtyManager,
    hosts: &Hosts,
    host: Option<&str>,
) -> AppResult<Arc<CoreClient>> {
    match host {
        None => manager.client().await,
        Some(id) => hosts.by_id(id)?.client().await,
    }
}

impl Hosts {
    pub(crate) fn emit_view(&self, host: &Host, view: DeviceView) {
        let view = host.viewed(view);
        if let Ok(app) = self.app() {
            let _ = app.emit_to(
                "main",
                HOST_VIEW_EVENT,
                HostView {
                    host: host.id(),
                    view,
                },
            );
        }
    }
}
