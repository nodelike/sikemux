//! The runs a core restart must not lose: what runs, which the next core counts until its
//! process ends, and what waits for an agent sent away.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::sizes::Size;
use super::{Held, Kind, Need, Run, State};

const FILE: &str = "line-runs.json";

pub(super) struct Saved {
    file: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    id: u64,
    agent_id: Option<String>,
    command: String,
    cwd: String,
    pid: Option<u32>,
    started: Option<u64>,
    since: u64,
    order: u64,
    measured: Option<(String, String)>,
    expected: Option<(u64, u64)>,
    before_commit: bool,
    in_line: bool,
    /// It waits for its agent, sent away until its turn comes; else it runs.
    comes_back: bool,
    #[serde(default)]
    needs: Vec<Need>,
}

impl Saved {
    pub(super) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            file: data_dir.map(|dir| dir.join(FILE)),
        }
    }

    pub(super) fn save(&self, state: &State) {
        let Some(file) = &self.file else {
            return;
        };
        let running = (state.running.iter())
            .filter(|run| run.kind == Kind::Command)
            .filter(|run| run.pid.is_some() && run.started.is_some() && !run.released)
            .map(|run| kept(run, false));
        let away = (state.waiting.iter())
            .filter(|run| run.held == Held::ComesBack && run.kind == Kind::Command)
            .map(|run| kept(run, true));
        let runs: Vec<Kept> = running.chain(away).collect();
        let written = serde_json::to_vec(&runs)
            .map_err(std::io::Error::other)
            .and_then(|json| sikemux_process::files::write_atomic_unsynced(file, &json, false));
        if let Err(error) = written {
            eprintln!("sikemux core: the heavy commands in line were not kept: {error}");
        }
    }

    /// The runs an earlier core kept whose processes still run, and those that wait for an
    /// agent sent away.
    pub(super) fn load(&self) -> Vec<Run> {
        let kept: Vec<Kept> = (self.file.as_ref())
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        kept.into_iter()
            .filter(|run| {
                run.comes_back
                    || run.pid.zip(run.started).is_some_and(|(pid, started)| {
                        sikemux_process::procs::started(pid) == Some(started)
                    })
            })
            .map(|run| Run {
                id: run.id,
                agent_id: run.agent_id,
                command: run.command,
                cwd: run.cwd,
                callers: Vec::new(),
                held: match run.comes_back {
                    true => Held::ComesBack,
                    false => Held::Taken,
                },
                pid: run.pid,
                started: run.started,
                since: run.since,
                order: run.order,
                measured: run.measured,
                expected: run.expected.map(|(peak, took_ms)| Size { peak, took_ms }),
                before_commit: run.before_commit,
                in_line: run.in_line,
                released: false,
                after: None,
                copies_output: false,
                log: None,
                execution_id: None,
                stopped: None,
                peak: 0,
                holds: 0,
                measured_once: false,
                needs: run.needs,
                kind: Kind::Command,
                wants: None,
                told_long: false,
            })
            .collect()
    }
}

fn kept(run: &Run, comes_back: bool) -> Kept {
    Kept {
        id: run.id,
        agent_id: run.agent_id.clone(),
        command: run.command.clone(),
        cwd: run.cwd.clone(),
        pid: run.pid,
        started: run.started,
        since: run.since,
        order: run.order,
        measured: run.measured.clone(),
        expected: run.expected.map(|size| (size.peak, size.took_ms)),
        before_commit: run.before_commit,
        in_line: run.in_line,
        comes_back,
        needs: run.needs.clone(),
    }
}
