//! The TCP ports the core's terminals, tasks and agents listen on, or anything
//! they started. Nothing tells a program when another starts listening, so
//! the core looks again when a session starts or ends, and every few seconds
//! while something runs and a client that asked is still connected. Clients
//! hear a `ports` change when what it found differs.

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "linux")]
mod linux;

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::protocol::SessionId;
use crate::server::changed::Topic;
use crate::tasks::config::{self, ConfigLoad};

use super::connection::{blocking, ClientId};
use super::wake::Wake;
use super::{Core, CoreResult};

#[cfg(target_os = "macos")]
use darwin as system;
#[cfg(target_os = "linux")]
use linux as system;

const MAX_ANCESTRY: usize = 64;
const LOOK_AGAIN_MS: u64 = 5_000;
/// A server binds its port a moment after its process starts.
const SETTLE_MS: u64 = 1_500;
const LOCAL_HOSTS: [&str; 5] = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Listener {
    pub pid: u32,
    pub port: u16,
    pub address: IpAddr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PortOwner {
    /// A terminal, in the app's pane `pane_id` when it has one.
    Terminal {
        session_id: SessionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    /// A task run, which `agent_id` started when an agent did.
    Task {
        session_id: SessionId,
        execution_id: String,
        task_id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// An agent, in its terminal `session_id` or in a chat.
    Agent {
        agent_id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<SessionId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListeningPort {
    pub port: u16,
    pub address: String,
    pub pid: u32,
    pub process: String,
    /// The project whose terminal, task or agent listens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub owner: PortOwner,
}

/// A port as a project shows it, with its preview first.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectPort {
    #[serde(flatten)]
    pub listening: ListeningPort,
    pub url: String,
    /// What `sikemux.json`'s preview points at.
    pub preview: bool,
}

#[derive(Default)]
struct State {
    found: Vec<ListeningPort>,
    /// The clients that asked.
    watchers: HashSet<ClientId>,
}

#[derive(Default)]
pub(crate) struct Ports {
    state: Mutex<State>,
    pub(crate) wake: Wake,
}

impl Ports {
    /// A session started or ended: look now, and again once a server it
    /// started has had time to bind.
    pub(crate) fn changed(&self) {
        if !self.state.locked().watchers.is_empty() {
            self.wake.now();
            self.wake.at(epoch_ms() + SETTLE_MS);
        }
    }
}

/// Whether a client that asked is still connected.
fn watched(core: &Core) -> bool {
    let connected: HashSet<ClientId> = core.clients().iter().map(|client| client.id).collect();
    let mut state = core.ports.state.locked();
    state.watchers.retain(|id| connected.contains(id));
    !state.watchers.is_empty()
}

fn address_rank(address: &IpAddr) -> u8 {
    match address {
        IpAddr::V4(ip) if ip.is_unspecified() => 0,
        IpAddr::V6(ip) if ip.is_unspecified() => 1,
        IpAddr::V4(ip) if ip.is_loopback() => 2,
        IpAddr::V6(ip) if ip.is_loopback() => 3,
        _ => 4,
    }
}

#[derive(Clone)]
enum Found {
    Session(SessionId),
    Agent(String),
}

fn find_owner(
    pid: u32,
    core_pid: u32,
    parents: &HashMap<u32, u32>,
    sessions: &HashMap<u32, SessionId>,
    agent_of_root: &mut impl FnMut(u32) -> Option<String>,
) -> Option<Found> {
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        if let Some(&session) = sessions.get(&current) {
            return Some(Found::Session(session));
        }
        let parent = *parents.get(&current)?;
        if parent == core_pid {
            return agent_of_root(current).map(Found::Agent);
        }
        current = parent;
    }
    None
}

/// One listener per port, owned by the session or agent its process runs
/// under. Servers that bind both stacks, or fork workers sharing a socket,
/// show up as several listeners for the same port.
fn attribute(
    listeners: Vec<Listener>,
    core_pid: u32,
    parents: &HashMap<u32, u32>,
    sessions: &HashMap<u32, SessionId>,
    mut agent_of_root: impl FnMut(u32) -> Option<String>,
) -> Vec<(Listener, Found)> {
    let mut owners: HashMap<u32, Option<Found>> = HashMap::new();
    let mut roots: HashMap<u32, Option<String>> = HashMap::new();
    let mut best: HashMap<u16, (Listener, Found)> = HashMap::new();
    for listener in listeners {
        let owner = owners
            .entry(listener.pid)
            .or_insert_with(|| {
                find_owner(listener.pid, core_pid, parents, sessions, &mut |root| {
                    roots
                        .entry(root)
                        .or_insert_with(|| agent_of_root(root))
                        .clone()
                })
            })
            .clone();
        let Some(owner) = owner else { continue };
        let replace = best.get(&listener.port).is_none_or(|(kept, _)| {
            (address_rank(&listener.address), listener.pid)
                < (address_rank(&kept.address), kept.pid)
        });
        if replace {
            best.insert(listener.port, (listener, owner));
        }
    }
    let mut found: Vec<_> = best.into_values().collect();
    found.sort_by_key(|(listener, _)| listener.port);
    found
}

fn agent_label(core: &Core, agent_id: &str) -> String {
    core.workspaces
        .title(agent_id)
        .or_else(|| core.names.name(agent_id))
        .unwrap_or_else(|| "agent".into())
}

fn describe(
    core: &Core,
    found: Found,
    owners: &HashMap<SessionId, super::prepare::Owner>,
) -> Option<(Option<String>, PortOwner)> {
    let agent = |agent_id: String, session_id| {
        let project = core.names.get(&agent_id).map(|name| name.project);
        let label = agent_label(core, &agent_id);
        (
            project,
            PortOwner::Agent {
                agent_id,
                label,
                session_id,
            },
        )
    };
    match found {
        Found::Agent(agent_id) => Some(agent(agent_id, None)),
        Found::Session(session_id) => {
            let owner = owners.get(&session_id)?;
            if let (Some(execution_id), Some(task)) = (&owner.task_execution_id, &owner.task) {
                return Some((
                    owner.project.clone(),
                    PortOwner::Task {
                        session_id,
                        execution_id: execution_id.clone(),
                        task_id: task.task_id.clone(),
                        label: task.label.clone(),
                        agent_id: task.agent_id.clone(),
                    },
                ));
            }
            if let Some(agent_id) = &owner.agent_id {
                return Some(agent(agent_id.clone(), Some(session_id)));
            }
            Some((
                owner.project.clone(),
                PortOwner::Terminal {
                    session_id,
                    pane_id: owner.pane_id.clone(),
                },
            ))
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn scan(core: &Core) -> Vec<ListeningPort> {
    let core_pid = std::process::id();
    let mut owners = HashMap::new();
    let mut pids = HashMap::new();
    for session in core.all_sessions() {
        if !session.is_running() {
            continue;
        }
        if let Some(pid) = session.pid() {
            pids.insert(pid, session.id);
        }
        owners.insert(session.id, session.owner().clone());
    }
    let parents = system::descendants(core_pid);
    let all: Vec<u32> = parents.keys().copied().collect();
    let listeners = system::listeners(&all);
    // Chat agents run outside a PTY, so their root process carries its agent id
    // in the environment it was launched with.
    attribute(listeners, core_pid, &parents, &pids, |root| {
        system::environment_value(root, sikemux_process::vars::AGENT_ID)
    })
    .into_iter()
    .filter_map(|(listener, found)| {
        let (project, owner) = describe(core, found, &owners)?;
        Some(ListeningPort {
            port: listener.port,
            address: listener.address.to_string(),
            pid: listener.pid,
            process: system::process_name(listener.pid),
            project,
            owner,
        })
    })
    .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn scan(_core: &Core) -> Vec<ListeningPort> {
    Vec::new()
}

/// Looks again and tells clients when what listens changed.
fn look(core: &Core) -> Vec<ListeningPort> {
    let found = scan(core);
    let mut state = core.ports.state.locked();
    if state.found != found {
        state.found = found.clone();
        drop(state);
        core.changes.changed(Topic::Ports, None, None);
    }
    found
}

/// Where a server `agent_id` started listens, by itself or in a task it ran.
pub(crate) fn agent_url(core: &Core, agent_id: &str) -> Option<String> {
    look(core)
        .into_iter()
        .find(|listening| match &listening.owner {
            PortOwner::Agent {
                agent_id: owner, ..
            } => owner == agent_id,
            PortOwner::Task {
                agent_id: owner, ..
            } => owner.as_deref() == Some(agent_id),
            PortOwner::Terminal { .. } => false,
        })
        .map(|listening| format!("http://localhost:{}", listening.port))
}

fn anything_runs(core: &Core) -> bool {
    core.running_sessions() > 0 || core.chats.count() > 0
}

/// The work [`Ports::wake`] runs: a look while a client watches, and the
/// next one a few seconds later while anything runs.
pub(crate) fn settle(core: &Arc<Core>) {
    if !watched(core) {
        return;
    }
    let looking = core.clone();
    tokio::task::spawn_blocking(move || {
        look(&looking);
        if anything_runs(&looking) {
            looking.ports.wake.at(epoch_ms() + LOOK_AGAIN_MS);
        }
    });
}

/// The port a local preview URL points at.
fn preview_port(url: &str) -> Option<u16> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if !LOCAL_HOSTS.contains(&host) {
        return None;
    }
    parsed.port_or_known_default()
}

pub(crate) fn for_project(found: Vec<ListeningPort>, project: &Path) -> Vec<ProjectPort> {
    let project_text = project.to_string_lossy();
    let preview_url = match config::load(project) {
        ConfigLoad::Valid { config, .. } => config.preview.and_then(|preview| preview.url),
        _ => None,
    };
    let preview = preview_url.as_deref().and_then(preview_port);
    let mut ports: Vec<ProjectPort> = found
        .into_iter()
        .filter(|port| port.project.as_deref() == Some(&project_text))
        .map(|listening| {
            let is_preview = Some(listening.port) == preview;
            let url = match (&preview_url, is_preview) {
                (Some(url), true) => url.clone(),
                _ => format!("http://localhost:{}/", listening.port),
            };
            ProjectPort {
                listening,
                url,
                preview: is_preview,
            }
        })
        .collect();
    ports.sort_by_key(|port| (!port.preview, port.listening.port));
    ports
}

/// What the project's terminals, tasks and agents listen on, or everything
/// the core's sessions do, looked up now. The core keeps looking for as long
/// as the client that asked stays connected.
pub(crate) async fn project_ports(
    core: &Arc<Core>,
    client: ClientId,
    project: Option<String>,
) -> CoreResult<Vec<ProjectPort>> {
    let first = core.ports.state.locked().watchers.insert(client);
    if first {
        core.ports.wake.at(epoch_ms() + LOOK_AGAIN_MS);
    }
    let core = core.clone();
    blocking(move || {
        let found = look(&core);
        Ok(match project {
            Some(project) => for_project(found, Path::new(&project)),
            None => found
                .into_iter()
                .map(|listening| ProjectPort {
                    url: format!("http://localhost:{}/", listening.port),
                    listening,
                    preview: false,
                })
                .collect(),
        })
    })
    .await
}

/// Whether something on this computer, not only what the core started,
/// already listens on `port`. Connecting reaches servers on loopback or on
/// every address, and unlike listening it never raises the macOS firewall
/// prompt.
pub(crate) fn in_use(port: u16) -> bool {
    let loopback = [
        IpAddr::from([127, 0, 0, 1]),
        IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
    ];
    loopback.into_iter().any(|ip| {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::new(ip, port),
            Duration::from_millis(200),
        )
        .is_ok()
    })
}

/// What listens now, for a check before a task starts.
pub(crate) fn listening_now(core: &Core) -> Vec<ListeningPort> {
    scan(core)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn listener(pid: u32, port: u16, address: IpAddr) -> Listener {
        Listener { pid, port, address }
    }

    const CORE: u32 = 1;
    const ANY4: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    const ANY6: IpAddr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    const LOOP4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    fn owners(found: &[(Listener, Found)]) -> Vec<(u16, String)> {
        found
            .iter()
            .map(|(listener, owner)| {
                let owner = match owner {
                    Found::Session(id) => format!("session {id}"),
                    Found::Agent(agent) => format!("agent {agent}"),
                };
                (listener.port, owner)
            })
            .collect()
    }

    #[test]
    fn a_server_under_a_shell_belongs_to_that_terminal() {
        let parents = HashMap::from([(10, CORE), (11, 10), (12, 11)]);
        let found = attribute(
            vec![listener(12, 5173, LOOP4)],
            CORE,
            &parents,
            &HashMap::from([(10, 7)]),
            |_| panic!("terminal roots never read an environment"),
        );
        assert_eq!(owners(&found), vec![(5173, "session 7".into())]);
    }

    #[test]
    fn a_server_under_a_chat_agent_is_named_by_its_root_environment() {
        let parents = HashMap::from([(20, CORE), (21, 20), (22, 21), (30, CORE), (31, 30)]);
        let mut reads = Vec::new();
        let found = attribute(
            vec![
                listener(22, 3000, ANY4),
                listener(21, 3001, ANY4),
                listener(31, 9000, ANY4),
            ],
            CORE,
            &parents,
            &HashMap::new(),
            |root| {
                reads.push(root);
                (root == 20).then(|| "agent-1".to_owned())
            },
        );
        assert_eq!(
            owners(&found),
            vec![
                (3000, "agent agent-1".into()),
                (3001, "agent agent-1".into())
            ]
        );
        reads.sort_unstable();
        assert_eq!(reads, vec![20, 30]);
    }

    #[test]
    fn processes_outside_any_terminal_or_agent_are_ignored() {
        let parents = HashMap::from([(40, 999)]);
        let found = attribute(
            vec![listener(40, 8080, ANY4), listener(50, 8081, ANY4)],
            CORE,
            &parents,
            &HashMap::new(),
            |_| None,
        );
        assert!(found.is_empty());
    }

    #[test]
    fn one_port_bound_twice_is_one_row_with_the_widest_address() {
        let parents = HashMap::from([(10, CORE), (11, 10), (12, 11)]);
        let found = attribute(
            vec![
                listener(12, 4000, LOOP4),
                listener(12, 4000, ANY6),
                listener(11, 4000, ANY4),
                listener(12, 80, ANY4),
            ],
            CORE,
            &parents,
            &HashMap::from([(10, 3)]),
            |_| None,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0.port, 80);
        assert_eq!(found[1].0, listener(11, 4000, ANY4));
    }

    #[test]
    fn a_parent_loop_does_not_hang() {
        let parents = HashMap::from([(5, 6), (6, 5)]);
        let found = attribute(
            vec![listener(5, 1234, ANY4)],
            CORE,
            &parents,
            &HashMap::new(),
            |_| None,
        );
        assert!(found.is_empty());
    }
}
