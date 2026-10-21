//! Hosts on the same account linked with each other, so an agent on one can
//! start agents on another and work with them as with its own. Every host on
//! an account links with every other, as a phone joins one: with a ticket from
//! the account (ADR 0018). Removing a host from the account unlinks it.
//!
//! Linked hosts ask each other things over one iroh connection, which either
//! may open and both use. What must arrive, such as messages and reports,
//! waits in an outbox while the other host is out of reach and goes when it is
//! back.

pub(crate) mod account;
mod calls;
mod code;
mod join;
mod link;
mod outbox;
pub(crate) mod route;
pub(crate) mod spawn;
pub(crate) mod team;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::{EndpointAddr, RelayUrl};
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Semaphore;

use super::remote::{self, LinkedHost};
use super::wake::{sooner, Wake};
use super::{Core, CoreError, CoreResult};
use crate::backoff::Backoff;

pub(crate) use join::link_host;
pub(crate) use link::{Failed, LINK_ALPN};
pub(crate) use outbox::Sending;

const DIAL_WAIT: Duration = Duration::from_secs(15);
const ASK_WAIT: Duration = Duration::from_secs(30);
const REDIAL: Backoff = Backoff {
    first: Duration::from_secs(2),
    cap: Duration::from_secs(60),
};
/// How long a host that runs workers may be out of reach before their leads
/// hear of it.
const OUT_OF_REACH_MS: u64 = 2 * 60_000;
/// Asks one linked host may have this host answer at once.
const MAX_ANSWERING: usize = 32;
const MAX_ADDRESSES: usize = 8;

/// A linked host out of reach, and when to try it again.
struct Down {
    since: u64,
    retry_at: u64,
    /// Dials in a row that failed.
    failures: u32,
    /// Its workers' leads were told it is out of reach.
    told: bool,
}

#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
    dialing: HashSet<String>,
    flushing: HashSet<String>,
    down: HashMap<String, Down>,
    /// When each agent's teammates elsewhere last heard it did other work.
    worked: HashMap<String, u64>,
}

pub(crate) struct Hosts {
    state: Mutex<State>,
    pub(crate) outbox: outbox::Outbox,
    pub(crate) team: team::Team,
    /// Tries hosts out of reach again and tells leads when one stays away.
    pub(crate) wake: Wake,
    /// Links with the account's other hosts, when it may have one not linked.
    pub(crate) relink: Wake,
    linking: AtomicBool,
}

impl Hosts {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            outbox: outbox::Outbox::new(data_dir),
            team: team::Team::new(data_dir),
            wake: Wake::default(),
            relink: Wake::default(),
            linking: AtomicBool::new(false),
        }
    }

    fn connected(&self, host: &str) -> Option<Connection> {
        self.state.locked().connections.get(host).cloned()
    }

    /// Whether `host` is reachable, as far as this host knows: connected, or
    /// not yet found out of reach.
    pub(crate) fn reachable(&self, host: &str) -> bool {
        let state = self.state.locked();
        state.connections.contains_key(host) || !state.down.contains_key(host)
    }

    /// Whether it is time to tell an agent's teammates elsewhere that it did
    /// other work, noting that they are told now.
    pub(crate) fn worked_due(&self, agent_id: &str, every_ms: u64) -> bool {
        let now = epoch_ms();
        let mut state = self.state.locked();
        state
            .worked
            .retain(|_, at| now.saturating_sub(*at) < every_ms);
        if state.worked.contains_key(agent_id) {
            return false;
        }
        state.worked.insert(agent_id.to_owned(), now);
        true
    }

    fn note_down(&self, host: &str) {
        let now = epoch_ms();
        self.state
            .locked()
            .down
            .entry(host.to_owned())
            .or_insert(Down {
                since: now,
                retry_at: now,
                failures: 0,
                told: false,
            });
        self.wake.now();
    }
}

/// A linked host as Settings and phones list it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkView {
    id: String,
    name: String,
    platform: String,
    linked_at: u64,
    last_seen: Option<u64>,
    connected: bool,
    /// Agents of teams this host shares with it.
    agents: usize,
}

pub(crate) fn views(core: &Core) -> Vec<LinkView> {
    let team = core.hosts.team.all();
    core.remote
        .links()
        .into_iter()
        .map(|link| LinkView {
            connected: core.hosts.connected(&link.id).is_some(),
            agents: team.iter().filter(|agent| agent.host == link.id).count(),
            id: link.id,
            name: link.name,
            platform: link.platform,
            linked_at: link.linked_at,
            last_seen: link.last_seen,
        })
        .collect()
}

/// The linked host an agent names, by the name this host gives it or its key.
pub(crate) fn find(core: &Core, wanted: &str) -> Result<LinkedHost, String> {
    let links = core.remote.links();
    let wanted = wanted.trim();
    let matching: Vec<&LinkedHost> = links
        .iter()
        .filter(|link| link.name.eq_ignore_ascii_case(wanted) || link.id == wanted)
        .collect();
    match matching.as_slice() {
        [link] => Ok((*link).clone()),
        [] if links.is_empty() => Err(
            "This host is linked with no other; the person links hosts in Sikemux's Settings, under Devices".into(),
        ),
        [] => Err(format!(
            "No linked host is called {wanted}; the linked hosts are {}",
            links
                .iter()
                .map(|link| link.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "More than one linked host is called {wanted}; pass its key: {}",
            matching
                .iter()
                .map(|link| link.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// What this host calls the linked host `id`.
pub(crate) fn name_of(core: &Core, id: &str) -> String {
    core.remote
        .link(id)
        .map(|link| link.name)
        .unwrap_or_else(|| id.get(..8).unwrap_or(id).to_owned())
}

/// Where this host's endpoint can be reached directly.
fn own_addresses(core: &Core) -> Vec<String> {
    core.remote
        .endpoint()
        .map(|endpoint| {
            endpoint
                .addr()
                .ip_addrs()
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn addresses_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|address| address.parse::<SocketAddr>().is_ok())
        .take(MAX_ADDRESSES)
        .map(str::to_owned)
        .collect()
}

fn addr_of(core: &Core, id: &str, addresses: &[String]) -> Result<EndpointAddr, String> {
    let key: iroh::PublicKey = id
        .parse()
        .map_err(|_| "that is not a host's key".to_owned())?;
    let with_relays = core
        .remote
        .relays()
        .iter()
        .filter_map(|relay| relay.url.parse::<RelayUrl>().ok())
        .fold(EndpointAddr::new(key), EndpointAddr::with_relay_url);
    Ok(addresses
        .iter()
        .filter_map(|address| address.parse::<SocketAddr>().ok())
        .fold(with_relays, EndpointAddr::with_ip_addr))
}

async fn dial(core: &Core, id: &str, addresses: &[String]) -> Result<Connection, String> {
    let endpoint = core
        .remote
        .endpoint()
        .ok_or("remote access is off on this host, so it reaches no other host")?;
    let addr = addr_of(core, id, addresses)?;
    match tokio::time::timeout(DIAL_WAIT, endpoint.connect(addr, LINK_ALPN)).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("it did not answer in time".into()),
    }
}

/// The connection to a linked host, made now if there is none.
async fn connection(core: &Arc<Core>, host: &str) -> Result<Connection, Failed> {
    if let Some(connection) = core.hosts.connected(host) {
        return Ok(connection);
    }
    let link = core
        .remote
        .link(host)
        .ok_or_else(|| Failed::Refused(format!("{} is no longer linked", name_of(core, host))))?;
    match dial(core, host, &link.addresses).await {
        Ok(connection) => {
            adopt(core, host, connection.clone());
            Ok(connection)
        }
        Err(error) => {
            core.hosts.note_down(host);
            changed(core);
            Err(Failed::Unreachable(format!(
                "{} is out of reach: {error}",
                link.name
            )))
        }
    }
}

/// Asks a linked host something now, waiting at most `limit`.
pub(crate) async fn ask(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<&Path>,
    limit: Duration,
) -> Result<Value, Failed> {
    let connection = connection(core, host).await?;
    link::ask(&connection, method, params, body, limit).await
}

/// Sends a linked host something that must arrive, behind whatever already
/// waits for it.
pub(crate) fn post(core: &Core, host: &str, sending: Sending) {
    core.hosts.outbox.push(host, sending);
    match core.arc() {
        Some(core) if core.hosts.connected(host).is_some() => flush(&core, host),
        _ => core.hosts.wake.now(),
    }
}

/// Whether something sent to `host` now would go at once, rather than wait
/// behind what is queued for it.
pub(crate) fn goes_now(core: &Core, host: &str) -> bool {
    !core.hosts.outbox.has(host) && core.hosts.reachable(host)
}

/// Sends what waits for `host`, in order, while it stays reachable.
fn flush(core: &Arc<Core>, host: &str) {
    if !core.hosts.state.locked().flushing.insert(host.to_owned()) {
        return;
    }
    let (core, host) = (core.clone(), host.to_owned());
    tokio::spawn(async move {
        while let Some(post) = core.hosts.outbox.front(&host) {
            let Some(connection) = core.hosts.connected(&host) else {
                break;
            };
            match link::ask(
                &connection,
                &post.method,
                post.params.clone(),
                post.body.as_deref(),
                ASK_WAIT,
            )
            .await
            {
                Ok(_) => core.hosts.outbox.sent(&host, post.seq),
                Err(Failed::Refused(why)) => {
                    eprintln!(
                        "sikemux core: {} turned away a {} that waited for it: {why}",
                        name_of(&core, &host),
                        post.method
                    );
                    core.hosts.outbox.sent(&host, post.seq);
                }
                Err(Failed::Unreachable(_)) => break,
            }
        }
        core.hosts.state.locked().flushing.remove(&host);
        if core.hosts.outbox.has(&host) {
            core.hosts.wake.now();
        }
    });
}

/// Takes `connection` as the one to `host`: answers what it asks over it,
/// sends what waits for it, and notices when it closes.
fn adopt(core: &Arc<Core>, host: &str, connection: Connection) {
    let was_down = {
        let mut state = core.hosts.state.locked();
        state
            .connections
            .insert(host.to_owned(), connection.clone());
        state.down.remove(host)
    };
    core.remote.link_seen(host, None);
    tokio::spawn(answer_asks(
        core.clone(),
        host.to_owned(),
        connection.clone(),
    ));
    tokio::spawn(until_closed(core.clone(), host.to_owned(), connection));
    flush(core, host);
    route::resync(core, host);
    if was_down.is_some_and(|down| down.told) {
        route::back_in_reach(core, host);
    }
    changed(core);
}

async fn answer_asks(core: Arc<Core>, host: String, connection: Connection) {
    let answering = Arc::new(Semaphore::new(MAX_ANSWERING));
    while let Some(incoming) = link::next(&connection).await {
        let Ok(incoming) = incoming else {
            continue;
        };
        let Ok(slot) = answering.clone().acquire_owned().await else {
            return;
        };
        let (core, host) = (core.clone(), host.clone());
        tokio::spawn(async move {
            let mut incoming = incoming;
            let method = std::mem::take(&mut incoming.method);
            let params = std::mem::take(&mut incoming.params);
            let body = incoming.body.take();
            let answer = calls::answer(&core, &host, &method, params, body).await;
            incoming.reply(answer).await;
            drop(slot);
        });
    }
}

async fn until_closed(core: Arc<Core>, host: String, connection: Connection) {
    connection.closed().await;
    let current = {
        let mut state = core.hosts.state.locked();
        let current = state
            .connections
            .get(&host)
            .is_some_and(|known| known.stable_id() == connection.stable_id());
        if current {
            state.connections.remove(&host);
        }
        current
    };
    if current {
        core.hosts.note_down(&host);
        changed(&core);
    }
}

/// A linked host opened a connection to this one.
pub(crate) async fn serve(core: Arc<Core>, connection: Connection) {
    let host = connection.remote_id().to_string();
    if core.remote.link(&host).is_some() {
        adopt(&core, &host, connection);
        return;
    }
    join::answer(core, connection).await;
}

/// The person here removed the link with `host`: the other host hears of it
/// if it can, and the agents working across the link are let go.
pub(crate) fn unlink(core: &Core, host: &str) {
    let connection = core.hosts.connected(host);
    unlinked(core, host, "the person removed the link");
    if let Some(connection) = connection {
        tokio::spawn(async move {
            let _ = link::ask(
                &connection,
                "unlink",
                Value::Null,
                None,
                Duration::from_secs(5),
            )
            .await;
            connection.close(link::NOT_LINKED.into(), b"unlinked");
        });
    }
}

/// `host` is no longer linked with this one, for the reason `why`: what
/// waits for it is dropped, and the leads of workers there are told.
pub(crate) fn unlinked(core: &Core, host: &str, why: &str) {
    core.hosts.outbox.forget(host);
    let connection = {
        let mut state = core.hosts.state.locked();
        state.down.remove(host);
        state.connections.remove(host)
    };
    if let Some(connection) = connection {
        connection.close(link::NOT_LINKED.into(), b"unlinked");
    }
    route::host_gone(core, host, why);
    changed(core);
}

/// Remote access went off: every link connection closes with it.
pub(crate) fn halt(core: &Core) {
    let connections: Vec<Connection> = {
        let mut state = core.hosts.state.locked();
        state
            .connections
            .drain()
            .map(|(_, connection)| connection)
            .collect()
    };
    for connection in connections {
        connection.close(0u32.into(), b"remote access is off");
    }
}

/// What the link views and the agents shown with their hosts depend on
/// changed.
fn changed(core: &Core) {
    remote::announce(core);
    core.view_changed();
}

/// Tries hosts out of reach again while something waits for them or agents
/// work across the link, and tells leads when a host stays out of reach.
pub(crate) fn settle(core: &Arc<Core>) {
    let links = core.remote.links();
    let linked: Vec<String> = links.iter().map(|link| link.id.clone()).collect();
    core.hosts.outbox.keep_only(&linked);
    let now = epoch_ms();
    let mut next = None;
    let mut redial = Vec::new();
    let mut tell = Vec::new();
    {
        let mut state = core.hosts.state.locked();
        state.down.retain(|host, _| linked.contains(host));
        for link in &links {
            let wanted = core.hosts.outbox.has(&link.id) || core.hosts.team.has_host(&link.id);
            if !wanted
                || state.connections.contains_key(&link.id)
                || state.dialing.contains(&link.id)
            {
                continue;
            }
            let down = state.down.entry(link.id.clone()).or_insert(Down {
                since: now,
                retry_at: now,
                failures: 0,
                told: false,
            });
            if !down.told && core.hosts.team.has_host(&link.id) {
                if now >= down.since + OUT_OF_REACH_MS {
                    down.told = true;
                    tell.push(link.id.clone());
                } else {
                    next = sooner(next, Some(down.since + OUT_OF_REACH_MS));
                }
            }
            if down.retry_at <= now {
                state.dialing.insert(link.id.clone());
                redial.push(link.clone());
            } else {
                next = sooner(next, Some(down.retry_at));
            }
        }
    }
    if core.remote.endpoint().is_none() {
        core.hosts.state.locked().dialing.clear();
        redial.clear();
    }
    for link in redial {
        let core = core.clone();
        tokio::spawn(async move {
            let dialed = dial(&core, &link.id, &link.addresses).await;
            {
                let mut state = core.hosts.state.locked();
                state.dialing.remove(&link.id);
                if dialed.is_err() {
                    if let Some(down) = state.down.get_mut(&link.id) {
                        down.failures = down.failures.saturating_add(1);
                        down.retry_at = epoch_ms() + REDIAL.after(down.failures).as_millis() as u64;
                    }
                }
            }
            match dialed {
                Ok(connection) => adopt(&core, &link.id, connection),
                Err(_) => core.hosts.wake.now(),
            }
        });
    }
    for host in tell {
        route::out_of_reach(core, &host);
    }
    if let Some(next) = next {
        core.hosts.wake.at(next);
    }
}

/// The JSON a host answers a link ask with, from a core result.
fn answered<T: Serialize>(result: CoreResult<T>) -> link::Answer {
    result
        .map_err(|error| error.to_string())
        .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
}

fn platform() -> String {
    serde_json::to_value(super::host::PLATFORM)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// What this host tells a host it links with about itself.
fn introduction(core: &Core) -> Value {
    json!({
        "name": super::host::info().name,
        "platform": platform(),
        "addresses": own_addresses(core),
    })
}

fn not_linked(core: &Core, host: &str) -> CoreError {
    CoreError::from(format!(
        "{} is not linked with this host",
        name_of(core, host)
    ))
}
