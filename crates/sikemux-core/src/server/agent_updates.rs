//! Which release of each agent's CLI this host has, which its chats run, and the newest
//! out, and the agent's own updater, run at the person's asking one agent at a time.

use crate::server::parts::Parts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;
use sikemux_process::user_environment;
use sikemux_pty::user_shell::login_shell_environment;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::agent_accounts::Installed;
use super::changed::{Topic, What};
use super::Core;
use crate::agent_catalog::{self, AgentEntry, Latest};
use crate::launch::discovery;

/// How long the newest release found stands before it is looked up again.
const LATEST_FOR: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a failed lookup waits before it is tried again.
const RETRY_LATEST_AFTER: Duration = Duration::from_secs(30 * 60);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How often the line an updater last printed is passed on while it runs.
const SAID_EVERY: Duration = Duration::from_secs(1);

/// How an update the person asked for is going. One that finished is no longer listed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Updating {
    /// Waiting for another agent's update to finish.
    Queued,
    /// The updater runs; `said` is the last line it printed.
    Running {
        said: Option<String>,
    },
    Failed {
        reason: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct UpdateAgents {
    /// The agents to update; none updates every one with a newer release out.
    #[serde(default)]
    pub kinds: Vec<String>,
}

fn updates() -> &'static Mutex<HashMap<String, Updating>> {
    static UPDATES: LazyLock<Mutex<HashMap<String, Updating>>> = LazyLock::new(Default::default);
    &UPDATES
}

struct Looked {
    at: Instant,
    latest: Option<String>,
}

fn looked() -> &'static Mutex<HashMap<&'static str, Looked>> {
    static LOOKED: LazyLock<Mutex<HashMap<&'static str, Looked>>> = LazyLock::new(Default::default);
    &LOOKED
}

/// Fills in each agent's releases and how its update goes, and looks up the newest
/// releases out again behind the answer once they are old.
pub(super) fn add_versions(core: &Arc<Core>, agents: &mut [Installed]) {
    let chats = core.chats().all();
    let updating = updates().locked().clone();
    for agent in agents.iter_mut() {
        let Some(def) = agent_catalog::agent(&agent.kind) else {
            continue;
        };
        let program = agent.available.then(|| PathBuf::from(&agent.command));
        agent.version = program
            .as_deref()
            .and_then(discovery::said)
            .and_then(|said| release(&said));
        let mut running: Vec<String> = chats
            .iter()
            .filter(|chat| chat.provider() == def.kind)
            .filter_map(|chat| release(&chat.cli_version()?))
            .collect();
        running.sort_by(|a, b| newer_first(a, b));
        running.dedup();
        agent.running = running;
        agent.latest = latest(core, def, program);
        agent.updating = updating.get(def.kind).cloned();
        agent.unlocks = unlocks(def, agent.version.as_deref(), agent.latest.as_deref());
    }
}

/// The models the release `version` cannot run and `latest` can, when both are known.
fn unlocks(def: &AgentEntry, version: Option<&str>, latest: Option<&str>) -> Vec<String> {
    let (Some(version), Some(latest)) = (version, latest) else {
        return Vec::new();
    };
    def.models_since
        .iter()
        .filter(|model| is_newer(model.since, version) && !is_newer(model.since, latest))
        .map(|model| model.label.to_owned())
        .collect()
}

/// The newest release out, as last looked up, looking again behind the answer when stale.
fn latest(core: &Arc<Core>, def: &'static AgentEntry, program: Option<PathBuf>) -> Option<String> {
    let source = def.latest.as_ref()?;
    let mut known_now = looked().locked();
    let known = known_now.get(def.kind);
    let fresh = known.is_some_and(|known| {
        let wait = match known.latest {
            Some(_) => LATEST_FOR,
            None => RETRY_LATEST_AFTER,
        };
        known.at.elapsed() < wait
    });
    let latest = known.and_then(|known| known.latest.clone());
    if fresh {
        return latest;
    }
    if matches!(source, Latest::Command(_)) && program.is_none() {
        return latest;
    }
    known_now.insert(
        def.kind,
        Looked {
            at: Instant::now(),
            latest: latest.clone(),
        },
    );
    drop(known_now);
    let core = core.clone();
    tokio::spawn(async move {
        let found = look_up(source, program.as_deref()).await;
        let mut kept = looked().locked();
        let before = kept.get(def.kind).and_then(|known| known.latest.clone());
        let at = match found {
            Some(_) => Instant::now(),
            None => Instant::now() - LATEST_FOR + RETRY_LATEST_AFTER,
        };
        let latest = found.or(before.clone());
        let moved = latest != before;
        kept.insert(def.kind, Looked { at, latest });
        drop(kept);
        if moved {
            core.changed(What::Read(Topic::AgentStatus));
        }
    });
    latest
}

async fn look_up(source: &Latest, program: Option<&Path>) -> Option<String> {
    match source {
        Latest::Npm(package) => npm_latest(package).await,
        Latest::Command(args) => command_latest(program?, args).await,
    }
}

async fn npm_latest(package: &str) -> Option<String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client =
        crate::http::shared(&CLIENT, || crate::http::builder().timeout(LOOKUP_TIMEOUT)).ok()?;
    let answer: Value = client
        .get(format!("https://registry.npmjs.org/{package}/latest"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    release(answer.get("version")?.as_str()?)
}

async fn command_latest(program: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::from(user_environment::agent_command(program));
    command
        .args(args)
        .envs(login_shell_environment())
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = tokio::time::timeout(LOOKUP_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    let answer: Value = serde_json::from_slice(&output.stdout).ok()?;
    release(answer.get("latestVersion")?.as_str()?)
}

/// Queues an update of each agent in `kinds`, or of every agent with a newer release out,
/// passing over one already queued or running.
pub(super) async fn update(core: &Arc<Core>, asked: UpdateAgents) -> super::CoreResult<()> {
    let agents = super::agent_accounts::setup(core).await?;
    let chosen: Vec<(String, PathBuf)> = agents
        .into_iter()
        .filter(|agent| agent.available)
        .filter(|agent| match asked.kinds.is_empty() {
            true => agent
                .latest
                .as_deref()
                .zip(agent.version.as_deref())
                .is_some_and(|(latest, version)| is_newer(latest, version)),
            false => asked.kinds.contains(&agent.kind),
        })
        .map(|agent| (agent.kind, PathBuf::from(agent.command)))
        .collect();
    let queued: Vec<(String, PathBuf)> = {
        let mut updates = updates().locked();
        let queued: Vec<(String, PathBuf)> = chosen
            .into_iter()
            .filter(|(kind, _)| {
                !matches!(
                    updates.get(kind),
                    Some(Updating::Queued | Updating::Running { .. })
                )
            })
            .collect();
        for (kind, _) in &queued {
            updates.insert(kind.clone(), Updating::Queued);
        }
        queued
    };
    if queued.is_empty() {
        return Ok(());
    }
    core.changed(What::Read(Topic::AgentStatus));
    for (kind, program) in queued {
        let core = core.clone();
        tokio::spawn(async move {
            static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
            let _turn = TURN.lock().await;
            let Some(def) = agent_catalog::agent(&kind) else {
                return;
            };
            set(&core, &kind, Some(Updating::Running { said: None }));
            let ended = run(&core, def, &program).await;
            discovery::forget_failures();
            set(
                &core,
                &kind,
                ended.err().map(|reason| Updating::Failed { reason }),
            );
            super::workspace::refresh_launchers(core.clone()).await;
        });
    }
    Ok(())
}

fn set(core: &Core, kind: &str, updating: Option<Updating>) {
    {
        let mut updates = updates().locked();
        match updating {
            Some(updating) => updates.insert(kind.to_owned(), updating),
            None => updates.remove(kind),
        };
    }
    core.changed(What::Read(Topic::AgentStatus));
}

/// Runs the agent's own updater, or its installer when it has none, passing on what it prints.
async fn run(core: &Core, def: &AgentEntry, program: &Path) -> Result<(), String> {
    let mut command = match (def.update, def.install) {
        (Some(args), _) => {
            let mut command = Command::from(user_environment::agent_command(program));
            command.args(args);
            command
        }
        (None, Some(install)) => {
            let mut command = Command::from(user_environment::agent_command("/bin/sh"));
            command.args(["-c", install]);
            command
        }
        (None, None) => return Err(format!("{} has no updater", def.label)),
    };
    command
        .envs(login_shell_environment())
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err("could not read what the updater says".into());
    };
    let (lines, mut heard) = tokio::sync::mpsc::unbounded_channel::<String>();
    for out in [
        Box::new(stdout) as Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        Box::new(stderr),
    ] {
        let lines = lines.clone();
        tokio::spawn(async move {
            let mut read = BufReader::new(out).lines();
            while let Ok(Some(line)) = read.next_line().await {
                let _ = lines.send(line);
            }
        });
    }
    drop(lines);
    let kind = def.kind;
    let mut last: Option<String> = None;
    let mut passed_on = Instant::now();
    let listening = async {
        while let Some(line) = heard.recv().await {
            let Some(line) = plain(&line) else {
                continue;
            };
            last = Some(line);
            if passed_on.elapsed() >= SAID_EVERY {
                passed_on = Instant::now();
                set(core, kind, Some(Updating::Running { said: last.clone() }));
            }
        }
    };
    let waited = tokio::time::timeout(UPDATE_TIMEOUT, async {
        listening.await;
        child.wait().await
    })
    .await;
    match waited {
        Err(_) => Err("the updater took over ten minutes".into()),
        Ok(Err(error)) => Err(format!("could not wait for the updater: {error}")),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(last.unwrap_or_else(|| format!("exited with {status}"))),
    }
}

/// `line` without colour codes, spinners or surrounding space, unless nothing is left.
fn plain(line: &str) -> Option<String> {
    let line = line.rsplit('\r').find(|part| !part.trim().is_empty())?;
    let detail = discovery::error_detail(strip_escapes(line).as_bytes())?;
    detail.chars().any(char::is_alphanumeric).then_some(detail)
}

fn strip_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    plain
}

/// The release a CLI names in `said`, such as `2.1.3` in "2.1.3 (Claude Code)", "grok
/// 1.0.5 (abc)" or "omp/18.1.0".
pub fn release(said: &str) -> Option<String> {
    said.split(|c: char| c.is_whitespace() || matches!(c, '/' | '(' | ')' | ',' | '@'))
        .map(|word| word.strip_prefix('v').unwrap_or(word))
        .find(|word| {
            let numbers = word.split(['-', '+']).next().unwrap_or_default();
            numbers.contains('.')
                && numbers
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_owned)
}

fn parts(release: &str) -> (Vec<u64>, Option<&str>) {
    let (numbers, pre) = match release.split_once('-') {
        Some((numbers, pre)) => (numbers, Some(pre)),
        None => (release.split('+').next().unwrap_or(release), None),
    };
    (
        numbers
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect(),
        pre,
    )
}

/// Whether release `a` comes after release `b`.
pub fn is_newer(a: &str, b: &str) -> bool {
    newer_first(a, b) == std::cmp::Ordering::Less
}

fn newer_first(a: &str, b: &str) -> std::cmp::Ordering {
    let ((a_numbers, a_pre), (b_numbers, b_pre)) = (parts(a), parts(b));
    b_numbers.cmp(&a_numbers).then(match (a_pre, b_pre) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (a_pre, b_pre) => b_pre.cmp(&a_pre),
    })
}
