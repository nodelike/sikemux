//! Things only so many agents use at once, such as the app on a fixed port or a webhook
//! listener. A run holds what it needs while it runs, taking all of it at once or waiting
//! holding none of it; an agent can also hold one for its turn with the `hold` tool.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;

use super::{Held, Kind, Run, State};
use crate::tasks::config::{self, ConfigLoad};
use crate::tasks::resources::{self, Resource};

/// How long a turn's hold lasts after the agent's last tool call.
const IDLE_MS: u64 = 10 * 60_000;
/// How long a thing is held while someone waits before its holder's lead, or the person, hears.
const LONG_MS: u64 = 30 * 60_000;

/// One thing a run needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Need {
    /// What tells it apart on this host: its project and name.
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<u16>,
}

impl Need {
    fn of(project: &str, resource: Resource) -> Self {
        Need {
            key: format!("{project}#{}", resource.name),
            name: resource.name,
            label: resource.label,
            count: resource.count,
            port: resource.port,
        }
    }
}

/// The things `names` stands for in the `sikemux.json` nearest `dir`, as `project` holds them:
/// one thing for every checkout of it.
pub(crate) fn resolve(project: &str, dir: &Path, names: &[String]) -> Result<Vec<Need>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let folder = dir
        .ancestors()
        .find(|folder| folder.join(config::CONFIG_FILE).is_file())
        .unwrap_or(dir);
    let listed = match config::load(folder) {
        ConfigLoad::Valid { config, .. } => config.resources,
        _ => Vec::new(),
    };
    names
        .iter()
        .map(|name| match name.as_str() {
            resources::SIMULATOR => {
                Err("simulator is held through sim_attach, which waits its own turn".to_owned())
            }
            resources::SCREEN => Err("no agent can hold the person's screen yet".to_owned()),
            name => (listed.iter())
                .find(|resource| resource.name == name)
                .cloned()
                .map(|resource| Need::of(project, resource))
                .ok_or_else(|| {
                    format!("{name} is not among the resources in this project's sikemux.json")
                }),
        })
        .collect()
}

/// Why a run cannot have one of the things it needs now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wanted {
    /// Another holds it: the first holder's agent, when it has one.
    InUse { label: String, by: Option<String> },
    /// A program outside Sikemux listens on its port.
    PortTaken {
        label: String,
        port: u16,
        by: String,
    },
}

impl Wanted {
    pub(crate) fn words(&self, name_of: &dyn Fn(&str) -> String) -> String {
        match self {
            Wanted::InUse {
                label,
                by: Some(agent),
            } => format!("{label} is in use by @{}", name_of(agent)),
            Wanted::InUse { label, by: None } => format!("{label} is in use"),
            Wanted::PortTaken { label, port, by } => {
                format!("{label} needs port {port}, which {by} outside Sikemux listens on")
            }
        }
    }

    pub(crate) fn holder(&self) -> Option<&str> {
        match self {
            Wanted::InUse { by, .. } => by.as_deref(),
            _ => None,
        }
    }
}

/// What the line knows of the host when it hands things out.
#[derive(Default)]
pub(crate) struct Around {
    /// Programs outside Sikemux found listening on the ports of things, by port.
    pub(crate) outside: HashMap<u16, String>,
}

/// Whether `run` can have everything it needs now, or what it waits for.
pub(crate) fn unfree(state: &State, run: &Run, around: &Around) -> Option<Wanted> {
    for need in &run.needs {
        let held: Vec<&Run> = holders(state, &need.key).collect();
        if held.len() >= need.count as usize {
            return Some(Wanted::InUse {
                label: need.label.clone(),
                by: held.iter().find_map(|run| run.agent_id.clone()),
            });
        }
        if let Some(port) = need.port.filter(|_| held.is_empty()) {
            if port_taken(port) {
                let by = (around.outside.get(&port).cloned())
                    .unwrap_or_else(|| "another program".to_owned());
                return Some(Wanted::PortTaken {
                    label: need.label.clone(),
                    port,
                    by,
                });
            }
        }
    }
    None
}

/// The running runs that hold the thing `key`.
pub(crate) fn holders<'a>(state: &'a State, key: &'a str) -> impl Iterator<Item = &'a Run> {
    (state.running.iter()).filter(move |run| run.needs.iter().any(|need| need.key == key))
}

/// Whether something already listens on `port` on this host.
fn port_taken(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// The ports of things runs wait for that something outside listens on, each with what it is,
/// as the system names it.
pub(crate) fn outside_holders(ports: &[u16]) -> HashMap<u16, String> {
    ports
        .iter()
        .filter(|port| port_taken(**port))
        .filter_map(|port| Some((*port, listener_of(*port)?)))
        .collect()
}

fn listener_of(port: u16) -> Option<String> {
    let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .into_iter()
        .find(|lsof| Path::new(lsof).is_file())?;
    let output = sikemux_process::user_environment::command(lsof)
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let pid = text.lines().find_map(|line| line.strip_prefix('p'))?;
    let name = text.lines().find_map(|line| line.strip_prefix('c'))?;
    Some(format!("`{name}` (pid {pid})"))
}

/// Why `agent_id` may not wait for `key`: it holds something for its turn that an agent holding
/// `key` for its turn waits for, so each would wait on the other. That other agent's id.
pub(crate) fn would_deadlock(state: &State, agent_id: &str, key: &str) -> Option<String> {
    let mine: Vec<&str> = (state.running.iter())
        .filter(|run| run.kind == Kind::Hold && run.is_for(agent_id))
        .flat_map(|run| run.needs.iter().map(|need| need.key.as_str()))
        .collect();
    if mine.is_empty() {
        return None;
    }
    holders(state, key)
        .filter(|run| run.kind == Kind::Hold)
        .filter_map(|run| run.agent_id.as_deref())
        .find(|other| {
            (state.waiting.iter())
                .filter(|run| run.kind == Kind::Hold && run.is_for(other))
                .any(|run| {
                    run.needs
                        .iter()
                        .any(|need| mine.contains(&need.key.as_str()))
                })
        })
        .map(str::to_owned)
}

/// The turn holds whose agent is not working: no tool call of any kind started or ended for a
/// while, none still running, and no heavy command or process run of its alive. By run.
pub(crate) fn idle_holds(state: &State, now: u64) -> Vec<u64> {
    let idle = super::debug_ms("SIKEMUX_LINE_IDLE_MS").unwrap_or(IDLE_MS);
    let busy = |agent: &str| {
        state
            .open_calls
            .get(agent)
            .is_some_and(|open| !open.is_empty())
            || (state.running.iter().chain(&state.waiting))
                .any(|run| run.kind != Kind::Hold && run.is_for(agent))
    };
    (state.running.iter())
        .filter(|run| run.kind == Kind::Hold && run.held == Held::Turn)
        .filter(|run| {
            let agent = run.agent_id.as_deref().unwrap_or_default();
            let last = state
                .called
                .get(agent)
                .copied()
                .unwrap_or(run.since)
                .max(run.since);
            now.saturating_sub(last) >= idle && !busy(agent)
        })
        .map(|run| run.id)
        .collect()
}

/// The runs that held a thing long while someone waits for it, not told of yet.
pub(crate) fn held_long(state: &State, now: u64) -> Vec<u64> {
    let long = super::debug_ms("SIKEMUX_LINE_LONG_MS").unwrap_or(LONG_MS);
    (state.running.iter())
        .filter(|run| !run.needs.is_empty() && !run.told_long)
        .filter(|run| now.saturating_sub(run.since) >= long)
        .filter(|run| {
            (state.waiting.iter()).any(|waiting| {
                (waiting.needs.iter()).any(|need| run.needs.iter().any(|held| held.key == need.key))
            })
        })
        .map(|run| run.id)
        .collect()
}

/// How many wait for any of the things `run` holds.
pub(crate) fn waiting_for(state: &State, run: &Run) -> u32 {
    (state.waiting.iter())
        .filter(|waiting| {
            (waiting.needs.iter()).any(|need| run.needs.iter().any(|held| held.key == need.key))
        })
        .count() as u32
}

/// The labels of what `run` holds, as one phrase.
pub(crate) fn labels(run: &Run) -> String {
    let labels: Vec<&str> = run.needs.iter().map(|need| need.label.as_str()).collect();
    match labels.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// A turn's hold, when its agent asks for `need` and nothing stands in the way: held now.
pub(crate) fn turn_hold(id: u64, agent_id: &str, project: &str, need: Need) -> Run {
    let now = epoch_ms();
    Run {
        id,
        agent_id: Some(agent_id.to_owned()),
        command: need.name.clone(),
        cwd: project.to_owned(),
        callers: Vec::new(),
        held: Held::ComesBack,
        pid: None,
        started: None,
        since: now,
        order: now,
        measured: None,
        expected: None,
        before_commit: false,
        in_line: false,
        released: false,
        after: None,
        copies_output: false,
        log: None,
        execution_id: None,
        stopped: None,
        peak: 0,
        holds: 0,
        measured_once: false,
        needs: vec![need],
        kind: Kind::Hold,
        wants: None,
        told_long: false,
    }
}
