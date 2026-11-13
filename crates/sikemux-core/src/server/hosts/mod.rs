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
pub mod chat_move;
pub(crate) mod code;
pub(crate) mod copies;
pub mod install;
mod join;
mod link;
mod listed;
mod machine;
pub mod open;
mod outbox;
pub(crate) mod probe;
mod receipts;
pub(crate) mod reopen;
pub(crate) mod route;
pub(crate) mod spawn;
pub(crate) mod team;
pub mod transfer;

use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::serving::ServeParts;
pub use sikemux_calls::hosts::LinkedHost;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::{EndpointAddr, RelayUrl};
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Semaphore;

use super::changed::Topic;
use super::kept::Kept;
use super::remote::{self, StoredLink};
use super::wake::{sooner, Wake};
use super::{Core, CoreError, CoreResult};
use crate::backoff::Backoff;
use calls::HostAsk;

pub use install::{AliasParams as HostAliasParams, SetUpWork, SetupEnded, SetupRow};
pub(crate) use join::link_host;
pub(crate) use link::{Failed, LINK_ALPN};
pub(crate) use listed::{installed_from, rows};
pub use listed::{HostRow, Reach as HostReach};
pub(crate) use outbox::Sending;
pub use probe::{Found as HostFound, Unanswered as HostUnanswered};
pub(crate) use receipts::Stands;

const DIAL_WAIT: Duration = Duration::from_secs(15);
const ASK_WAIT: Duration = Duration::from_secs(30);
/// How long a lane waits to go again after its post could not go.
const RETRY_MS: u64 = 15_000;
/// How long a post that carries a file to a host too old to fetch it may take.
const BODY_WAIT: Duration = Duration::from_secs(15 * 60);
/// How long a file being fetched may go without a byte before it is given up.
const STALL: Duration = Duration::from_secs(60);
/// The code a host closes a second connection to the same host with.
const DUPLICATE: u32 = 3;
/// Two connections this close together were dialed by both hosts at once.
const DIALED_AT_ONCE_MS: u64 = 10_000;
/// How a host refuses an ask it does not know, as older hosts do.
const UNKNOWN_ASK: &str = "this host does not know ";
const REDIAL: Backoff = Backoff {
    first: Duration::from_secs(2),
    cap: Duration::from_secs(15),
};
/// How a linked host is tried again when only its projects are wanted, while the window is open.
const REDIAL_FOR_COPIES: Backoff = Backoff {
    first: Duration::from_secs(30),
    cap: Duration::from_secs(15 * 60),
};
/// How long a host that runs workers may be out of reach before their leads
/// hear of it.
const OUT_OF_REACH_MS: u64 = 2 * 60_000;
/// Asks one linked host may have this host answer at once.
const MAX_ANSWERING: usize = 32;
const MAX_ADDRESSES: usize = 8;
const INSTALLED_FILE: &str = "host-installed-from.json";

/// A linked host out of reach, and when to try it again.
struct Down {
    since: u64,
    retry_at: u64,
    /// Dials in a row that failed.
    failures: u32,
    /// Why the last of them failed.
    last: Option<DialFailure>,
    /// Its workers' leads were told it is out of reach.
    told: bool,
}

#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
    dialing: HashSet<String>,
    /// The lanes of each host being sent now.
    flushing: HashSet<(String, String)>,
    /// The lanes of each host that wait to be sent again until then, after a post could not go.
    held: HashMap<(String, String), u64>,
    down: HashMap<String, Down>,
    /// When each agent's teammates elsewhere last heard it did other work.
    worked: HashMap<String, u64>,
    /// When the connection kept to each host was taken.
    adopted: HashMap<String, u64>,
    /// Workers whose start is being asked after, so one asks at a time.
    settling: HashSet<String>,
    /// Hosts back in reach whose teammates here were told they were away, until each says
    /// which of its agents still run.
    told_away: HashSet<String>,
}

pub(crate) struct Hosts {
    state: Mutex<State>,
    pub(crate) outbox: outbox::Outbox,
    receipts: receipts::Receipts,
    pub(crate) team: team::Team,
    /// What the other hosts hold of the account's projects.
    pub(crate) copies: copies::Copies,
    /// How big each linked host is, by its key.
    pub(crate) machines: machine::Machines,
    /// Tries hosts out of reach again and tells leads when one stays away.
    pub(crate) wake: Wake,
    /// Links with the account's other hosts, when it may have one not linked.
    pub(crate) relink: Wake,
    linking: AtomicBool,
    /// A client lists the hosts, so every linked one is kept in reach to say
    /// how it is reached.
    listed: AtomicBool,
    /// The SSH host each host was installed on from here, by its key.
    installed: Mutex<BTreeMap<String, String>>,
    installed_kept: Kept,
    /// What is being done to SSH hosts from here, by alias, until it ends well.
    setups: Mutex<HashMap<String, install::Setup>>,
    /// What each SSH host was found to run when last asked, by alias.
    probes: Mutex<probe::Probes>,
    /// The linked hosts when the hosts were last listed, to see links come and go.
    listed_links: Mutex<Option<Vec<String>>>,
    /// What each host an agent's teammates run on was last told this host shows of it, by
    /// that host's key and the agent's id.
    pub(crate) rows_told: Mutex<HashMap<(String, String), team::Said>>,
    /// Tells them the rows that changed.
    pub(crate) rows: Wake,
    /// Changed rows are gathering to be told.
    rows_due: AtomicBool,
}

impl Hosts {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let installed_kept = Kept::new(data_dir, INSTALLED_FILE);
        Self {
            state: Mutex::new(State::default()),
            outbox: outbox::Outbox::new(data_dir),
            receipts: receipts::Receipts::new(data_dir),
            team: team::Team::new(data_dir),
            copies: copies::Copies::new(data_dir),
            machines: machine::Machines::default(),
            wake: Wake::default(),
            relink: Wake::default(),
            linking: AtomicBool::new(false),
            listed: AtomicBool::new(false),
            installed: Mutex::new(installed_kept.load()),
            installed_kept,
            setups: Mutex::default(),
            probes: Mutex::default(),
            listed_links: Mutex::default(),
            rows_told: Mutex::default(),
            rows: Wake::default(),
            rows_due: AtomicBool::new(false),
        }
    }

    /// Takes the SSH hosts the app noted hosts were installed on from here,
    /// keeping any the core noted itself.
    pub(crate) fn carry_installed_from(&self, carried: BTreeMap<String, String>) {
        if carried.is_empty() {
            return;
        }
        let mut installed = self.installed.locked();
        for (key, alias) in carried {
            installed.entry(key).or_insert(alias);
        }
        self.installed_kept.save(&*installed);
    }

    /// Whether the teammates here of `host`'s agents were told it was out of reach, since it
    /// last said which of them still run.
    pub(crate) fn take_told_away(&self, host: &str) -> bool {
        self.state.locked().told_away.remove(host)
    }

    fn reach(&self, host: &str) -> listed::Reach {
        let state = self.state.locked();
        if state.connections.contains_key(host) {
            listed::Reach::Connected
        } else if state.down.get(host).is_some_and(|down| down.failures > 0) {
            listed::Reach::Away
        } else {
            listed::Reach::Connecting
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

    /// The linked hosts that failed to answer for `for_ms` or longer, each with since when.
    pub(crate) fn out_of_reach(&self, for_ms: u64) -> Vec<(String, u64)> {
        let now = epoch_ms();
        let state = self.state.locked();
        state
            .down
            .iter()
            .filter(|(host, down)| {
                down.failures > 0
                    && now.saturating_sub(down.since) >= for_ms
                    && !state.connections.contains_key(*host)
            })
            .map(|(host, down)| (host.clone(), down.since))
            .collect()
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

    /// Holds what waits in `lane` for `host` for `for_ms` before it is sent again.
    fn hold(&self, host: &str, lane: &str, for_ms: u64) {
        let until = epoch_ms() + for_ms;
        (self.state.locked().held).insert((host.to_owned(), lane.to_owned()), until);
        self.wake.at(until);
    }

    /// The network came back: dials every host out of reach and sends what waits now,
    /// rather than after the backoff its failures while offline built up.
    pub(crate) fn retry_now(&self) {
        let mut state = self.state.locked();
        for down in state.down.values_mut() {
            down.retry_at = 0;
            down.failures = 0;
        }
        state.held.clear();
        drop(state);
        self.wake.now();
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
                last: None,
                told: false,
            });
        self.wake.now();
    }

    /// How this host last fared reaching `host`, for a diagnosis of why it is out of reach.
    pub(crate) fn dial_state(&self, host: &str) -> DialState {
        let state = self.state.locked();
        if let Some(connection) = state.connections.get(host) {
            let paths = connection.paths();
            let relay = (paths.iter())
                .find(|path| path.is_selected())
                .is_some_and(|path| path.is_relay());
            return DialState::Connected { relay };
        }
        match state.down.get(host) {
            Some(down) => DialState::Down {
                since: down.since,
                failures: down.failures,
                last: down.last,
            },
            None => DialState::Unknown,
        }
    }
}

pub(crate) fn views(core: &Core) -> Vec<LinkedHost> {
    let team = core.hosts().team.all();
    core.remote()
        .links()
        .into_iter()
        .map(|link| LinkedHost {
            connected: core.hosts().connected(&link.id).is_some(),
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
pub(crate) fn find(core: &Core, wanted: &str) -> Result<StoredLink, String> {
    let links = core.remote().links();
    let wanted = wanted.trim();
    let matching: Vec<&StoredLink> = links
        .iter()
        .filter(|link| link.name.eq_ignore_ascii_case(wanted) || link.id == wanted)
        .collect();
    match matching.as_slice() {
        [link] => Ok((*link).clone()),
        [] if links.is_empty() => Err(
            "This host is linked with no other; the person links a host by signing it in to their Sikemux account, and finds it on the Devices page".into(),
        ),
        [] => Err(format!(
            "No linked host is called {wanted}; the linked hosts are {}",
            links
                .iter()
                .map(|link| match core.hosts().machines.of(&link.id) {
                    Some(size) => format!("{} ({size})", link.name),
                    None => link.name.clone(),
                })
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

pub(crate) fn name_of(core: &Core, id: &str) -> String {
    core.remote()
        .link(id)
        .map(|link| link.name)
        .unwrap_or_else(|| id.get(..8).unwrap_or(id).to_owned())
}

fn own_addresses(core: &Core) -> Vec<String> {
    core.remote()
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
        .remote()
        .relays()
        .iter()
        .filter_map(|relay| relay.url.parse::<RelayUrl>().ok())
        .fold(EndpointAddr::new(key), EndpointAddr::with_relay_url);
    Ok(addresses
        .iter()
        .filter_map(|address| address.parse::<SocketAddr>().ok())
        .chain(kit_addresses(id))
        .fold(with_relays, EndpointAddr::with_ip_addr))
}

/// Where a test kit's core also dials the host `id`: the addresses the kit names for it in
/// `dev/host-addresses.json`, since test cores on one machine have no relay to meet through.
#[cfg(feature = "host-addresses-file")]
fn kit_addresses(id: &str) -> Vec<SocketAddr> {
    let Some(text) = sikemux_process::paths::config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("dev/host-addresses.json")).ok())
    else {
        return Vec::new();
    };
    let Ok(named) = serde_json::from_str::<HashMap<String, Vec<SocketAddr>>>(&text) else {
        return Vec::new();
    };
    if !named
        .values()
        .flatten()
        .all(|address| on_this_network(address.ip()))
    {
        eprintln!("sikemux core: dev/host-addresses.json names an address off this network, so it is not used");
        return Vec::new();
    }
    named.get(id).cloned().unwrap_or_default()
}

#[cfg(not(feature = "host-addresses-file"))]
fn kit_addresses(_: &str) -> Vec<SocketAddr> {
    Vec::new()
}

#[cfg(feature = "host-addresses-file")]
fn on_this_network(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Why a dial to a linked host failed, as iroh says rather than as its words read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialFailure {
    /// Remote access is off here, so this host has no endpoint to dial from.
    Off,
    /// iroh knows no address or relay for the host.
    NoAddress,
    /// Nothing answered in time: no path to the host.
    NoAnswer,
    /// The host's core answered and said this one is not linked with it.
    NotLinked,
    /// The host's core answered and closed the connection for another reason.
    Closed,
    Other,
}

/// How this host last fared reaching a linked one.
pub(crate) enum DialState {
    Connected {
        relay: bool,
    },
    Down {
        since: u64,
        failures: u32,
        last: Option<DialFailure>,
    },
    Unknown,
}

pub(crate) struct DialError {
    pub failure: DialFailure,
    message: String,
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl DialFailure {
    fn closed(close: &iroh::endpoint::ApplicationClose) -> Self {
        match close.error_code.into_inner() == u64::from(link::NOT_LINKED) {
            true => DialFailure::NotLinked,
            false => DialFailure::Closed,
        }
    }

    fn of(error: &iroh::endpoint::ConnectError) -> Self {
        use iroh::endpoint::{
            ConnectError, ConnectWithOptsError, ConnectingError, ConnectionError,
        };
        let closed = |error: &ConnectionError| match error {
            ConnectionError::TimedOut => Some(DialFailure::NoAnswer),
            ConnectionError::ApplicationClosed(close) => Some(DialFailure::closed(close)),
            _ => None,
        };
        match error {
            ConnectError::Connect {
                source: ConnectWithOptsError::NoAddress { .. },
                ..
            } => DialFailure::NoAddress,
            ConnectError::Connect {
                source: ConnectWithOptsError::EndpointClosed { .. },
                ..
            } => DialFailure::Off,
            ConnectError::Connecting {
                source: ConnectingError::ConnectionError { source, .. },
                ..
            }
            | ConnectError::Connection { source, .. } => {
                closed(source).unwrap_or(DialFailure::Other)
            }
            _ => DialFailure::Other,
        }
    }
}

async fn dial(core: &Core, id: &str, addresses: &[String]) -> Result<Connection, DialError> {
    let failed = |failure, message: String| DialError { failure, message };
    let endpoint = core.remote().endpoint().ok_or_else(|| {
        failed(
            DialFailure::Off,
            "remote access is off on this host, so it reaches no other host".into(),
        )
    })?;
    let addr =
        addr_of(core, id, addresses).map_err(|message| failed(DialFailure::Other, message))?;
    match tokio::time::timeout(DIAL_WAIT, endpoint.connect(addr, LINK_ALPN)).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(error)) => Err(failed(DialFailure::of(&error), error.to_string())),
        Err(_) => Err(failed(
            DialFailure::NoAnswer,
            "it did not answer in time".into(),
        )),
    }
}

/// The connection to a linked host, made now if there is none.
async fn connection(core: &Arc<Core>, host: &str) -> Result<Connection, Failed> {
    if let Some(connection) = core.hosts().connected(host) {
        return Ok(connection);
    }
    let link = core
        .remote()
        .link(host)
        .ok_or_else(|| Failed::Refused(format!("{} is no longer linked", name_of(core, host))))?;
    match dial(core, host, &link.addresses).await {
        Ok(connection) => Ok(adopt(core, host, connection)),
        Err(error) => {
            core.hosts().note_down(host);
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
    asking(core, host, method, params, body, limit, None).await
}

/// A new id for a call that does work on another host, made once and passed to every
/// [`call`] that retries it.
pub(crate) fn call_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Asks a linked host to do something once: asked again with the same `id`, it answers what
/// it answered the first time instead of doing it again.
pub(crate) async fn call(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<&Path>,
    limit: Duration,
    id: &str,
) -> Result<Value, Failed> {
    asking(core, host, method, params, body, limit, Some(id)).await
}

/// Fetches the file `host` holds for a post it sent this host, for as long as its bytes keep
/// coming.
pub(crate) async fn pull(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
) -> Result<link::Received, Failed> {
    let connection = connection(core, host).await?;
    link::pull(&connection, method, params, STALL).await
}

/// What became of the call `id` this host made to `host`. A host too old to keep receipts
/// refuses, as it does any ask it does not know.
pub(crate) async fn receipt(core: &Arc<Core>, host: &str, id: &str) -> Result<Stands, Failed> {
    let params = calls::params_of::<calls::Receipt>(&calls::ReceiptParams {
        call: id.to_owned(),
    });
    let answer = ask(core, host, calls::Receipt::METHOD, params, None, ASK_WAIT).await?;
    Stands::heard(&answer)
        .ok_or_else(|| Failed::Refused(format!("{} answered unreadably", name_of(core, host))))
}

async fn asking(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<&Path>,
    limit: Duration,
    id: Option<&str>,
) -> Result<Value, Failed> {
    let connection = connection(core, host).await?;
    let answer = link::ask(&connection, method, params, body, limit, id).await;
    if let Err(Failed::NotLinked(_)) = &answer {
        unlinked_there(core, host);
    }
    answer
}

/// `host` no longer has the link with this one, though the person there could not tell this
/// host when they removed it: this host lets it go too.
fn unlinked_there(core: &Core, host: &str) {
    if core.remote().forget_link(host) {
        unlinked(core, host, "the person there removed the link");
    }
}

/// Sends a linked host something that must arrive, behind whatever already
/// waits for it.
pub(crate) fn post(core: &Core, host: &str, sending: Sending) {
    core.hosts().outbox.push(host, sending);
    match core.arc() {
        Some(core) if core.hosts().connected(host).is_some() => flush(&core, host),
        _ => core.hosts().wake.now(),
    }
}

/// Whether something sent to `host` now would go at once, rather than wait
/// behind what is queued for it.
pub(crate) fn goes_now(core: &Core, host: &str, about: &str) -> bool {
    core.hosts()
        .outbox
        .lanes(host)
        .iter()
        .all(|lane| lane != about)
        && core.hosts().reachable(host)
}

/// Sends what waits for `host` while it stays reachable, each lane in order and every lane at
/// once, so one post that cannot go yet holds up only those behind it in its lane.
fn flush(core: &Arc<Core>, host: &str) {
    let now = epoch_ms();
    let lanes: Vec<String> = {
        let mut state = core.hosts().state.locked();
        (core.hosts().outbox.lanes(host).into_iter())
            .filter(|lane| {
                let key = (host.to_owned(), lane.clone());
                let held = state.held.get(&key).is_some_and(|until| *until > now);
                !held && state.flushing.insert(key)
            })
            .collect()
    };
    for lane in lanes {
        tokio::spawn(flush_lane(core.clone(), host.to_owned(), lane));
    }
}

async fn flush_lane(core: Arc<Core>, host: String, lane: String) {
    while let Some(post) = core.hosts().outbox.front(&host, &lane) {
        let Some(connection) = core.hosts().connected(&host) else {
            break;
        };
        let body = post.body.as_deref().filter(|_| !post.pulled);
        let limit = if body.is_some() { BODY_WAIT } else { ASK_WAIT };
        let sent = link::ask(
            &connection,
            &post.method,
            post.params.clone(),
            body,
            limit,
            post.call.as_deref(),
        )
        .await;
        match sent {
            Ok(_) => {
                core.hosts().outbox.sent(&host, post.seq);
                if post.method == "question" {
                    if let Some(id) = post.params["id"].as_str() {
                        super::questions::relay_arrived(&core, id);
                    }
                }
            }
            Err(Failed::Refused(why))
                if post.pulled && why == format!("{UNKNOWN_ASK}{}", post.method) =>
            {
                core.hosts()
                    .outbox
                    .carry(&host, post.seq, calls::Commits::METHOD);
            }
            Err(Failed::Refused(why)) => {
                if core.hosts().outbox.sent(&host, post.seq) {
                    turned_away(&core, &host, &post, &why);
                }
            }
            Err(Failed::NotLinked(_)) => {
                unlinked_there(&core, &host);
                break;
            }
            Err(Failed::Retry(_) | Failed::Unreachable(_) | Failed::Unknown(_)) => {
                core.hosts().hold(&host, &lane, RETRY_MS);
                break;
            }
        }
    }
    core.hosts()
        .state
        .locked()
        .flushing
        .remove(&(host.clone(), lane));
    if core.hosts().outbox.has(&host) {
        core.hosts().wake.now();
    }
}

/// `host` turned away a post that waited for it, for good: the agent that sent it hears why.
fn turned_away(core: &Core, host: &str, post: &outbox::Post, why: &str) {
    let name = name_of(core, host);
    eprintln!(
        "sikemux core: {name} turned away a {} that waited for it: {why}",
        post.method
    );
    let commits =
        [calls::Commits::METHOD, calls::CommitsReady::METHOD].contains(&post.method.as_str());
    if commits {
        if let Some(worker) = &post.from {
            core.hosts().team.tree_unsent(worker);
        }
    }
    if let (Some(asked), "answer") = (post.back.clone(), post.method.as_str()) {
        super::questions::answer_turned_away(core, host, asked, why);
    }
    let Some(from) = post.from.clone() else {
        return;
    };
    let what = match post.method.as_str() {
        "deliver" => "Your message",
        _ if commits => "Your commits",
        _ => "What you sent",
    };
    let text = format!(
        "{what} did not reach {name}: it turned it away ({why}). Nothing more will be sent of it; what you committed stays in your checkout and goes with your next report."
    );
    route::notice(core, from, text);
}

/// Takes `connection` as the one to `host`: answers what it asks over it,
/// sends what waits for it, and notices when it closes.
/// A new connection replaces the one kept, which the other host would not have dialed past
/// unless it lost it. When both hosts dialed at once, each keeps the connection the host with
/// the lower key dialed and closes the other, so they agree. Answers the connection kept.
fn adopt(core: &Arc<Core>, host: &str, connection: Connection) -> Connection {
    core.boards().wake.now();
    let (was_down, replaced) = {
        let mut state = core.hosts().state.locked();
        let racing = (state.adopted.get(host))
            .is_some_and(|at| epoch_ms().saturating_sub(*at) < DIALED_AT_ONCE_MS);
        if let Some(kept) = state.connections.get(host).filter(|kept| {
            kept.stable_id() != connection.stable_id()
                && kept.close_reason().is_none()
                && racing
                && !replaces(core, host, kept, &connection)
        }) {
            let kept = kept.clone();
            drop(state);
            connection.close(DUPLICATE.into(), b"a connection is up already");
            return kept;
        }
        let replaced = state
            .connections
            .insert(host.to_owned(), connection.clone())
            .filter(|old| old.stable_id() != connection.stable_id());
        state.held.retain(|(held, _), _| held != host);
        state.adopted.insert(host.to_owned(), epoch_ms());
        let was_down = state.down.remove(host);
        if was_down.as_ref().is_some_and(|down| down.told) {
            state.told_away.insert(host.to_owned());
        }
        (was_down, replaced)
    };
    if let Some(replaced) = replaced {
        replaced.close(DUPLICATE.into(), b"replaced by a newer connection");
    }
    core.remote().link_seen(host, None);
    tokio::spawn(answer_asks(
        core.clone(),
        host.to_owned(),
        connection.clone(),
    ));
    tokio::spawn(until_closed(
        core.clone(),
        host.to_owned(),
        connection.clone(),
    ));
    flush(core, host);
    core.hosts().copies.reached(host);
    route::resync(core, host);
    machine::ask_machine(core, host);
    spawn::settle_pending_on(core, host);
    route::ask_after_workers(core, host);
    if was_down.is_some_and(|down| down.told) {
        route::back_in_reach(core, host);
    }
    changed(core);
    connection
}

/// Whether `new` takes the place of `old`, a connection to `host` still up.
fn replaces(core: &Core, host: &str, old: &Connection, new: &Connection) -> bool {
    if old.side() == new.side() {
        return true;
    }
    let own = core.remote().core_id().unwrap_or_default();
    let dialer = |connection: &Connection| match connection.side() {
        iroh::endpoint::Side::Client => own.as_str(),
        iroh::endpoint::Side::Server => host,
    };
    dialer(new) < dialer(old)
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
            if method == calls::PullCommits::METHOD {
                match calls::pulled(&core, &host, &method, params) {
                    Ok(path) => incoming.reply_body(&path).await,
                    Err(refusal) => incoming.reply(Err(refusal)).await,
                }
                drop(slot);
                return;
            }
            let body = incoming.body.take();
            let call = incoming.call.take();
            let answer = calls::answer(&core, &host, &method, params, body, call).await;
            incoming.reply(answer).await;
            drop(slot);
        });
    }
}

async fn until_closed(core: Arc<Core>, host: String, connection: Connection) {
    let closed = connection.closed().await;
    let current = {
        let mut state = core.hosts().state.locked();
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
        core.hosts().note_down(&host);
        if let iroh::endpoint::ConnectionError::ApplicationClosed(close) = closed {
            if DialFailure::closed(&close) == DialFailure::NotLinked {
                if let Some(down) = core.hosts().state.locked().down.get_mut(&host) {
                    down.last = Some(DialFailure::NotLinked);
                }
            }
        }
        changed(&core);
    }
}

/// A linked host opened a connection to this one.
pub(crate) async fn serve(core: Arc<Core>, connection: Connection) {
    let host = connection.remote_id().to_string();
    if core.remote().link(&host).is_some() {
        adopt(&core, &host, connection);
        return;
    }
    join::answer(core, connection).await;
}

/// The person here removed the link with `host`: the other host hears of it
/// if it can, and the agents working across the link are let go.
pub(crate) fn unlink(core: &Core, host: &str) {
    let connection = {
        let mut state = core.hosts().state.locked();
        state.connections.remove(host)
    };
    unlinked(core, host, "the person removed the link");
    if let Some(connection) = connection {
        tokio::spawn(async move {
            let _ = link::ask(
                &connection,
                "unlink",
                Value::Null,
                None,
                Duration::from_secs(5),
                None,
            )
            .await;
            connection.close(link::NOT_LINKED.into(), b"unlinked");
        });
    }
}

/// `host` is no longer linked with this one, for the reason `why`: what
/// waits for it is dropped, and the leads of workers there are told.
pub(crate) fn unlinked(core: &Core, host: &str, why: &str) {
    core.hosts().outbox.forget(host);
    core.hosts().receipts.forget(host);
    core.hosts().copies.forget(host);
    let connection = {
        let mut state = core.hosts().state.locked();
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
        let mut state = core.hosts().state.locked();
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
    core.changed(What::View);
}

/// Tries hosts out of reach again while something waits for them or agents
/// work across the link, and tells leads when a host stays out of reach.
pub(crate) fn settle(core: &Arc<Core>) {
    let links = core.remote().links();
    let linked: Vec<String> = links.iter().map(|link| link.id.clone()).collect();
    core.hosts().outbox.keep_only(&linked);
    if core.remote().endpoint().is_none() {
        let mut state = core.hosts().state.locked();
        state.down.clear();
        state.dialing.clear();
        return;
    }
    let now = epoch_ms();
    let mut next = None;
    let mut redial = Vec::new();
    let window = core.window().is_open();
    let mut tell = Vec::new();
    let mut flushes = Vec::new();
    {
        let mut state = core.hosts().state.locked();
        state.down.retain(|host, _| linked.contains(host));
        state
            .held
            .retain(|(host, _), until| linked.contains(host) && *until > now);
        next = (state.held.values()).fold(next, |next, until| sooner(next, Some(*until)));
        flushes.extend(
            (state.connections.keys())
                .filter(|host| core.hosts().outbox.has(host))
                .cloned(),
        );
        for link in &links {
            let needed = core.hosts().outbox.has(&link.id)
                || core.hosts().team.has_host(&link.id)
                || core.hosts().listed.load(Ordering::Acquire);
            let for_copies = window
                && (core.hosts().copies.holds(&link.id)
                    || !core.hosts().copies.heard_from(&link.id));
            if !(needed || for_copies)
                || state.connections.contains_key(&link.id)
                || state.dialing.contains(&link.id)
            {
                continue;
            }
            let down = state.down.entry(link.id.clone()).or_insert(Down {
                since: now,
                retry_at: now,
                failures: 0,
                last: None,
                told: false,
            });
            if !down.told && core.hosts().team.has_host(&link.id) {
                if now >= down.since + OUT_OF_REACH_MS {
                    down.told = true;
                    tell.push(link.id.clone());
                } else {
                    next = sooner(next, Some(down.since + OUT_OF_REACH_MS));
                }
            }
            if down.retry_at <= now {
                state.dialing.insert(link.id.clone());
                redial.push((
                    link.clone(),
                    if needed { REDIAL } else { REDIAL_FOR_COPIES },
                ));
            } else {
                next = sooner(next, Some(down.retry_at));
            }
        }
    }
    for (link, backoff) in redial {
        let core = core.clone();
        tokio::spawn(async move {
            let dialed = dial(&core, &link.id, &link.addresses).await;
            {
                let mut state = core.hosts().state.locked();
                state.dialing.remove(&link.id);
                if let Err(error) = &dialed {
                    if let Some(down) = state.down.get_mut(&link.id) {
                        down.last = Some(error.failure);
                        down.failures = down.failures.saturating_add(1);
                        down.retry_at =
                            epoch_ms() + backoff.after(down.failures).as_millis() as u64;
                    }
                }
            }
            match dialed {
                Ok(connection) => {
                    adopt(&core, &link.id, connection);
                }
                Err(_) => {
                    core.changed(What::Read(Topic::Hosts));
                    core.hosts().wake.now();
                }
            }
        });
    }
    for host in flushes {
        flush(core, &host);
    }
    for host in tell {
        route::out_of_reach(core, &host);
    }
    if let Some(next) = next {
        core.hosts().wake.at(next);
    }
}

/// The JSON a host answers a link ask with, from a core result.
fn answered<T: Serialize>(result: CoreResult<T>) -> link::Answer {
    result
        .map_err(|error| error.to_string())
        .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
}

/// What this host tells a host it links with about itself.
fn introduction(core: &Core) -> Value {
    json!({
        "name": super::host::info().name,
        "platform": super::host::PLATFORM,
        "addresses": own_addresses(core),
    })
}

fn not_linked(core: &Core, host: &str) -> CoreError {
    CoreError::from(format!(
        "{} is not linked with this host",
        name_of(core, host)
    ))
}

impl crate::server::part::Part for Hosts {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        use crate::server::part::on_wake;
        vec![
            on_wake(core, |core| &core.hosts().wake, settle),
            on_wake(core, |core| &core.hosts().relink, account::relink),
            on_wake(core, |core| &core.hosts().copies.wake, copies::share),
            on_wake(core, |core| &core.hosts().rows, route::tell_rows_soon),
        ]
    }
}
