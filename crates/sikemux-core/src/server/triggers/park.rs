//! Triggers that fire when a run of `process_start` ends or prints a text, or when another agent
//! finishes or asks the person something, so an agent ends its turn instead of waiting in it.
//! They look whenever the harness journal moves, the same signal `events_wait` and `agent_wait`
//! hold a turn open for; a timeout fires them through the trigger's own time.

use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::turn::TurnWaitKind;

use super::{fire, remove_where, Firing, Trigger};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::follow::{self, Happened, Until};
use crate::server::harness::{run_printed, run_tail};
use crate::server::{roster, Core};

pub(super) const DEFAULT_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
pub(super) const MAX_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
const MIN_TIMEOUT_SECONDS: u64 = 60;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Park {
    /// The project whose journal tells of what it waits on.
    project: String,
    on: On,
    /// When it was set, in Unix milliseconds.
    pub(super) since: u64,
    /// The journal's cursor when it was set; only what happens after it counts.
    after: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
enum On {
    Process {
        execution_id: String,
        shown: String,
        /// The text it waits for the run to print; none waits for it to end.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Agent {
        agent_id: String,
        name: String,
        until: String,
    },
    /// Its heavy command's turn on the host, which the line itself says has come.
    Line { cwd: String, command: String },
    /// A thing only one agent uses at a time, `name` in `project`, which the line says is free.
    InUse { project: String, name: String },
}

/// What setting a park found: it already happened, or what to wait on and how to show it.
pub(super) enum Parked {
    Already(String),
    Waits(Park, String),
}

impl Park {
    pub(super) fn kind(&self) -> TurnWaitKind {
        match self.on {
            On::Process { .. } => TurnWaitKind::Process,
            On::Agent { .. } => TurnWaitKind::Agent,
            On::Line { .. } => TurnWaitKind::Line,
            On::InUse { .. } => TurnWaitKind::InUse,
        }
    }

    /// What its wait is kept under, so the next wait on the same command has an estimate.
    pub(super) fn waits_on(&self) -> Option<String> {
        match &self.on {
            On::Process {
                shown, text: None, ..
            } => Some(format!("process {shown}")),
            _ => None,
        }
    }

    pub(super) fn timed_out(&self, now: u64) -> String {
        let minutes = now.saturating_sub(self.since) / 60_000;
        let what = match &self.on {
            On::Process {
                shown, text: None, ..
            } => format!("{shown} had not ended"),
            On::Process {
                shown,
                text: Some(text),
                ..
            } => format!("{shown} had not printed {text:?}"),
            On::Agent { name, until, .. } => format!("@{name} had not reached {until}"),
            On::Line { command, .. } => format!("`{command}` was still in line"),
            On::InUse { name, .. } => format!("{name} was still in use"),
        };
        format!("Timed out: after {minutes} min {what}. Look with process_read or agent_read, then park again or carry on.")
    }
}

fn is_given(params: &Value, key: &str) -> bool {
    params.get(key).is_some_and(|value| !value.is_null())
}

/// Whether the call parks on a process or an agent.
pub(super) fn asked(params: &Value) -> bool {
    is_given(params, "process") || is_given(params, "agent")
}

/// How long the park waits at most, in seconds.
pub(super) fn timeout(params: &Value) -> Result<u64, String> {
    match params.get("timeout") {
        None | Some(Value::Null) => Ok(DEFAULT_TIMEOUT_SECONDS),
        Some(value) => value
            .as_u64()
            .filter(|seconds| (MIN_TIMEOUT_SECONDS..=MAX_TIMEOUT_SECONDS).contains(seconds))
            .ok_or_else(|| {
                format!("timeout must be a whole number of seconds from {MIN_TIMEOUT_SECONDS} to {MAX_TIMEOUT_SECONDS}")
            }),
    }
}

/// What the call parks on, or what already happened.
pub(super) async fn set(core: &Arc<Core>, request: &HarnessRequest) -> Result<Parked, String> {
    let params = &request.params;
    let until = params.get("until").and_then(Value::as_str).map(str::trim);
    let since = epoch_ms();
    match (params.get("process"), params.get("agent")) {
        (Some(Value::String(execution_id)), None) => {
            let run = core
                .harness()
                .run_of(execution_id.trim())
                .filter(|run| run.project == request.project)
                .ok_or("process must be an executionId process_start gave in this project")?;
            let shown = run
                .label
                .clone()
                .or_else(|| run.command.clone())
                .unwrap_or_else(|| run.execution_id.clone());
            let text = match until {
                None | Some("" | "exit") => None,
                Some("ready") => Some(run.ready_when.clone().ok_or(
                    "this run was started without readyWhen; pass the text that means ready as until",
                )?),
                Some(text) => Some(text.to_owned()),
            };
            let park = Park {
                project: request.project.clone(),
                on: On::Process {
                    execution_id: run.execution_id.clone(),
                    shown: shown.clone(),
                    text,
                },
                since,
                after: core.harness().journal_cursor(&request.project),
            };
            Ok(match met(core, &park).await {
                Some(said) => Parked::Already(said),
                None => Parked::Waits(park, shown),
            })
        }
        (None, Some(Value::String(agent))) => {
            let caller = request
                .agent_id
                .clone()
                .ok_or("trigger_add needs the calling agent's id")?;
            let until = until.filter(|until| !until.is_empty()).unwrap_or("done");
            let wait = Until::named(until)?;
            let found = {
                let (core, request, agent) = (core.clone(), request.clone(), agent.clone());
                blocking(move || {
                    follow::waited(&core, &request, &[agent], &caller).map_err(Into::into)
                })
                .await
                .map_err(|error| error.to_string())?
            };
            let waited = found.into_iter().next().ok_or("agent must name an agent")?;
            if wait.already(waited.status) {
                let status = waited.status.map_or("gone", roster::status_word);
                return Ok(Parked::Already(format!(
                    "@{} is {status} now.",
                    waited.name
                )));
            }
            let shown = format!("@{}", waited.name);
            let park = Park {
                after: core.harness().journal_cursor(&waited.project),
                project: waited.project,
                on: On::Agent {
                    agent_id: waited.agent_id,
                    name: waited.name,
                    until: until.to_owned(),
                },
                since,
            };
            Ok(Parked::Waits(park, shown))
        }
        _ => Err("Give process (an executionId) or agent (a name), not both".into()),
    }
}

impl Park {
    /// A park on `command`'s turn in line, run in `cwd` of `project`.
    pub(super) fn in_line(core: &Core, project: &str, cwd: &str, command: &str) -> Self {
        Park {
            project: project.to_owned(),
            on: On::Line {
                cwd: cwd.to_owned(),
                command: command.to_owned(),
            },
            since: epoch_ms(),
            after: core.harness().journal_cursor(project),
        }
    }

    /// A park on the thing `name` of `project`, which another holds.
    pub(super) fn for_thing(core: &Core, project: &str, name: &str) -> Self {
        Park {
            project: project.to_owned(),
            on: On::InUse {
                project: project.to_owned(),
                name: name.to_owned(),
            },
            since: epoch_ms(),
            after: core.harness().journal_cursor(project),
        }
    }

    /// Whether it waits on the line: a heavy command's turn, or a thing in use.
    pub(super) fn waits_in_line(&self) -> bool {
        matches!(self.on, On::Line { .. } | On::InUse { .. })
    }

    /// Whether it waits on the thing `name` of `project`.
    pub(super) fn is_thing(&self, at: &str, thing: &str) -> bool {
        matches!(&self.on, On::InUse { project, name } if project == at && name == thing)
    }

    /// Whether it waits on `command`'s turn in line, run in `cwd`.
    pub(super) fn is_line(&self, at: &str, run: &str) -> bool {
        matches!(&self.on, On::Line { cwd, command } if cwd == at && command == run)
    }
}

/// What happened, once what the park waits on has.
async fn met(core: &Core, park: &Park) -> Option<String> {
    match &park.on {
        On::Line { .. } | On::InUse { .. } => None,
        On::Process {
            execution_id,
            shown,
            text,
        } => {
            let Some(run) = core.harness().run_of(execution_id) else {
                return Some(format!(
                    "{shown} is gone from Sikemux's runs; start it again with process_start if it is still needed."
                ));
            };
            let printed = match text {
                Some(text) => run_printed(core, &run, text).await,
                None => false,
            };
            if !printed && run.status.is_active() {
                return None;
            }
            let how = match (&text, printed) {
                (Some(text), true) => format!("{shown} printed {text:?}."),
                (Some(text), false) => format!("{shown} ended without printing {text:?}."),
                (None, _) => format!("{shown} ended."),
            };
            let mut said = format!("{how} Status: {}", run.status.event_kind());
            if let Some(code) = run.exit_code {
                said.push_str(&format!(", exit code {code}"));
            }
            if let Some(tail) = run_tail(core, &run).await {
                said.push_str(&format!(". Last lines:\n{tail}"));
            }
            Some(said)
        }
        On::Agent {
            agent_id,
            name,
            until,
        } => {
            let until = Until::named(until).ok()?;
            let page = core
                .harness()
                .journal_since(&park.project, park.after)
                .ok()
                .filter(|page| !page.truncated);
            let Some(page) = page else {
                let status = roster::listed(core)
                    .into_iter()
                    .find(|agent| &agent.agent_id == agent_id)
                    .map(|agent| agent.standing(core));
                return until.already(status).then(|| {
                    let status = status.map_or("gone", roster::status_word);
                    format!("@{name} is {status} now. agent_read shows what it said.")
                });
            };
            let happened = page.events.iter().find_map(|entry| {
                (entry.agent_id.as_deref() == Some(agent_id.as_str()))
                    .then(|| Happened::of(&entry.kind))
                    .flatten()
                    .filter(|happened| until.heard(*happened))
            })?;
            Some(format!(
                "@{name} {}. agent_read shows what it said.",
                happened.words()
            ))
        }
    }
}

/// Fires each parked trigger whose wait is over.
async fn look(core: &Arc<Core>) {
    let parked: Vec<(u64, Park)> = {
        let state = core.triggers().state.locked();
        let waiting = state.triggers.iter().filter(|trigger| !trigger.rests());
        waiting
            .filter_map(|trigger| Some((trigger.id, trigger.park.clone()?)))
            .collect()
    };
    for (id, park) in parked {
        let Some(said) = met(core, &park).await else {
            continue;
        };
        let Some(mut trigger) = remove_where(core, |trigger: &Trigger| trigger.id == id) else {
            continue;
        };
        if let Some(on) = park.waits_on() {
            core.waits()
                .took(&on, epoch_ms().saturating_sub(park.since));
        }
        trigger.prompt = format!("{}\n\n{said}", trigger.prompt);
        tokio::spawn(fire(
            core.clone(),
            Firing {
                trigger,
                late: None,
                last: true,
                request: None,
            },
        ));
    }
}

/// Looks at the parked triggers each time the harness journal moves, for as long as the core runs.
pub(super) async fn serve(core: Arc<Core>) {
    let mut changed = core.harness().subscribe();
    loop {
        look(&core).await;
        if changed.changed().await.is_err() {
            return;
        }
    }
}
