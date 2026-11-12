//! Drift audits. On the schedule the person sets for a project, the core starts an agent with
//! fresh eyes, one that never saw the work, at the project's root. It reads what landed since
//! the audit before and files each problem it finds as a card under the audit's own card, so the
//! findings sit on the board where anyone, Auto too, can take them up. That card, its findings
//! and the earlier findings done since make the project's health digest.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::audit::{AuditDigest, AuditFinding, AuditView};
use sikemux_view::chat::TriggerWhen;

use super::{add_card, project_at, project_view, Adding, Board, By, CardStatus, ProjectBoard};
use crate::git::Git;
use crate::server::changed::What;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::triggers::next_after;
use crate::server::wake::sooner;
use crate::server::{chat, workspace, Core};
use sikemux_wire::protocol::Event;

const TITLE: &str = "Drift audit";
/// The most audits a project keeps a record of.
const KEEP: usize = 12;
const MIN_EVERY_SECONDS: u64 = 60 * 60;

/// A project's audit schedule and the audits that ran.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Audits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    every: Option<TriggerWhen>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_at: Option<u64>,
    /// Oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runs: Vec<Run>,
}

impl Audits {
    pub(super) fn is_none(&self) -> bool {
        self.every.is_none() && self.runs.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    card: u64,
    at: u64,
    /// The project's commit it audited.
    head: String,
    commits: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEveryParams {
    pub project: String,
    /// An `every` of an hour or more, or `daily`; none stops them.
    #[serde(default)]
    pub every: Option<TriggerWhen>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditNowParams {
    pub project: String,
}

/// The person sets when the project's audits run.
pub(crate) async fn person_audit_every(
    core: &Arc<Core>,
    params: AuditEveryParams,
) -> Result<ProjectBoard, String> {
    match &params.every {
        None | Some(TriggerWhen::Daily { .. }) => {}
        Some(TriggerWhen::Every { seconds }) if *seconds >= MIN_EVERY_SECONDS => {}
        Some(_) => return Err("an audit runs every hour or more, or daily".into()),
    }
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let mut state = core.boards.state.locked();
        let board = (state.boards.entry(project.clone())).or_insert_with(|| Board::empty(&project));
        let now = epoch_ms();
        board.audits.next_at =
            (params.every.as_ref()).and_then(|every| next_after(every, now, now));
        board.audits.every = params.every;
        core.boards.save(&mut state, &project);
        drop(state);
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The person starts an audit now, outside its schedule.
pub(crate) async fn person_audit_now(
    core: &Arc<Core>,
    params: AuditNowParams,
) -> Result<ProjectBoard, String> {
    let project = project_at(core, &params.project)?;
    if running(core, &project) {
        return Err("An audit is already reading this project".into());
    }
    start(core, &project).await?;
    Ok(project_view(core, &project))
}

/// Starts the audits whose time came, and answers when the next is due.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let mut next = None;
    let mut starting = Vec::new();
    {
        let mut state = core.boards.state.locked();
        let projects: Vec<String> = state.boards.keys().cloned().collect();
        for project in projects {
            let Some(board) = state.boards.get_mut(&project) else {
                continue;
            };
            let (Some(every), Some(at)) = (board.audits.every.clone(), board.audits.next_at) else {
                continue;
            };
            if at > now {
                next = sooner(next, Some(at));
                continue;
            }
            board.audits.next_at = next_after(&every, at, now);
            next = sooner(next, board.audits.next_at);
            if !latest_running(board) {
                starting.push(project.clone());
            }
            core.boards.save(&mut state, &project);
        }
    }
    for project in starting {
        let core = core.clone();
        tokio::spawn(async move {
            if let Err(error) = start(&core, &project).await {
                eprintln!("sikemux core: the drift audit of {project} did not start: {error}");
            }
        });
    }
    next
}

fn running(core: &Core, project: &str) -> bool {
    let state = core.boards.state.locked();
    state.boards.get(project).is_some_and(latest_running)
}

fn latest_running(board: &Board) -> bool {
    (board.audits.runs.last())
        .and_then(|run| board.card(run.card).ok())
        .is_some_and(|card| card.status != CardStatus::Done)
}

/// Starts an agent that never saw the work at the project's root, on a card of its own, and
/// hands it what to read.
async fn start(core: &Arc<Core>, project: &str) -> Result<(), String> {
    workspace::launchers_known(core).await;
    let agent = crate::server::names::new_agent_id();
    let started = {
        let (core, agent, project) = (core.clone(), agent.clone(), project.to_owned());
        blocking(move || {
            let since = {
                let state = core.boards.state.locked();
                (state.boards.get(&project))
                    .and_then(|board| board.audits.runs.last())
                    .map(|run| (run.head.clone(), run.at))
            };
            let folder = Path::new(&project);
            let head = Git::at(folder)
                .text(&["rev-parse", "HEAD"])
                .map(|head| head.trim().to_owned())
                .map_err(|error| format!("git could not read the project's commit: {error}"))?;
            let commits = match &since {
                Some((from, _)) => Git::at(folder)
                    .text(&["rev-list", "--count", &format!("{from}..{head}")])
                    .ok()
                    .and_then(|count| count.trim().parse().ok())
                    .unwrap_or(0),
                None => 0,
            };
            let settings = core
                .boards
                .state
                .locked()
                .boards
                .get(&project)
                .map(|board| board.settings.clone());
            let settings = settings.unwrap_or_default();
            let (mut launch, launcher) = core.workspaces.board_launch(
                settings.launcher.as_deref(),
                folder,
                settings.provider.as_deref(),
                settings.model.clone(),
                "in the board's Auto settings",
            )?;
            core.names.give(
                Starting {
                    agent_id: &agent,
                    project: project.clone(),
                    provider: Some(&launch.provider),
                    resume_id: None,
                },
                core.open_agents(),
            );
            launch.agent_id = agent.clone();
            let chat = chat::begin(&core, launch, None, Some(launcher), false, false)
                .inspect_err(|_| core.names.forget(&agent))?;
            let adding = Adding {
                title: TITLE.to_owned(),
                ..Adding::default()
            };
            let card = match add_card(&core, &project, &By::Auto("audit"), adding, Some(&agent)) {
                Ok((card, _)) => card,
                Err(error) => {
                    let _ = workspace::close_agent(&core, agent);
                    return Err(error.into());
                }
            };
            {
                let mut state = core.boards.state.locked();
                if let Some(board) = state.boards.get_mut(&project) {
                    board.audits.runs.push(Run {
                        card,
                        at: epoch_ms(),
                        head: head.clone(),
                        commits,
                    });
                    let over = board.audits.runs.len().saturating_sub(KEEP);
                    board.audits.runs.drain(..over);
                    core.boards.save(&mut state, &project);
                }
            }
            core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
            core.changed(What::View);
            Ok((card, head, since.map(|(from, _)| from), commits))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (card, head, since, commits) = started;
    let reading = match since {
        Some(from) => format!(
            "Read what landed since the audit before: {commits} commits, `git log --oneline {from}..{head}`, and the code around them."
        ),
        None => "This is the project's first audit: read it as a whole, starting where it changes most.".to_owned(),
    };
    let task = format!(
        "You are a fresh pair of eyes on this codebase, auditing its health; you have no history of the work, on purpose. {reading}\n\nLook for drift: dead code, the same logic written twice, docs and design notes that no longer match the code, rules in your brief the code breaks, checks or tests weakened, and whatever would confuse someone new here. Change nothing.\n\nFile each problem worth fixing as a card with board_add, parent {card}: a one-line title naming the place, and details saying what is wrong and why it matters. When you are through, close card #{card} with board_update, status done, and a result of two or three sentences on the codebase's health. If you find nothing worth fixing, say so there."
    );
    chat::prompt(
        core,
        &agent,
        None,
        task,
        Vec::new(),
        Vec::new(),
        chat::Whose::NOT_PERSON,
    )
    .map_err(|error| error.to_string())
}

/// The project's audits as clients show them; none when it never had any.
pub(super) fn view(core: &Core, board: &Board) -> Option<AuditView> {
    if board.audits.is_none() {
        return None;
    }
    let runs = &board.audits.runs;
    let latest = runs.last().and_then(|run| {
        let card = board.card(run.card).ok()?;
        let before = runs.len().checked_sub(2).map(|at| &runs[at]);
        let findings = (board.under(run.card).into_iter())
            .map(|found| AuditFinding {
                card: found.id,
                title: found.title.clone(),
                done: found.status == CardStatus::Done,
            })
            .collect();
        let since = before.map(|before| before.at);
        let fixed = (runs.iter().rev().skip(1))
            .flat_map(|earlier| board.under(earlier.card))
            .filter(|found| found.status == CardStatus::Done)
            .filter(|found| since.is_none_or(|since| found.updated_at > since))
            .count();
        Some(AuditDigest {
            card: run.card,
            at: run.at,
            agent_name: card.owner_name(core),
            running: card.status != CardStatus::Done,
            commits: run.commits,
            since,
            findings,
            fixed: fixed as u32,
            summary: card.result.clone(),
        })
    });
    Some(AuditView {
        every: board.audits.every.clone(),
        next_at: board.audits.next_at,
        latest,
    })
}
