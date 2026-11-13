//! A goal's report. On the schedule set for it, every so many minutes or whenever the goal moves,
//! the core writes down how the goal stands in one fixed shape, from the same views every client
//! draws, so nobody has to ask its lead for status. The person hears of a report only when it
//! says something the one before did not.

use crate::server::board_state::BoardParts;
pub use sikemux_calls::board::GoalReportsParams;
pub use sikemux_calls::board::ReportEveryParams;
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::slice::{GateState, GoalReport, ReportEvery, ReportLine, SliceState};

use super::{card_view, project_at, project_view, BoardCard, Card, ProjectBoard};
use crate::server::chat::reset::when;
use crate::server::connection::blocking;
use crate::server::notify::GoalMoved;
use crate::server::wake::sooner;
use crate::server::Core;

/// How many reports a goal keeps.
const KEEP: usize = 12;
/// The least time between two reports a goal's moves write.
const MIN_GAP_MS: u64 = 60_000;
const MIN_MINUTES: u32 = 5;
const MAX_MINUTES: u32 = 24 * 60;
/// How many slices of each kind a notification names before it counts the rest.
const NAMED: usize = 2;

/// The person sets how often goal `id` is reported on. A report is written at once, so the page
/// shows one straight away.
pub async fn person_report_every(
    core: &Arc<Core>,
    params: ReportEveryParams,
) -> Result<ProjectBoard, String> {
    if let Some(ReportEvery::Minutes { minutes }) = params.every {
        if !(MIN_MINUTES..=MAX_MINUTES).contains(&minutes) {
            return Err(format!(
                "a report comes every {MIN_MINUTES} to {MAX_MINUTES} minutes"
            ));
        }
    }
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let set = core.boards().edit(&project, params.id, |card| {
            card.goal.report_on(params.every);
        });
        set.ok_or_else(|| format!("There is no card #{} on this project's board", params.id))?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Goal `id`'s reports, oldest first.
pub fn reports(core: &Core, params: GoalReportsParams) -> Result<Vec<GoalReport>, String> {
    let project = project_at(core, &params.project)?;
    let state = core.boards().state.locked();
    let board = (state.boards.get(&project))
        .ok_or_else(|| format!("There is no card #{} on this project's board", params.id))?;
    Ok(board.card(params.id)?.goal.reports().to_vec())
}

/// Writes the reports that are due, and answers when the next one is, with the goals whose new
/// report the person should hear of.
pub fn due(core: &Core) -> (Option<u64>, Vec<GoalMoved>) {
    let now = epoch_ms();
    let mut next = None;
    let mut moved = Vec::new();
    let mut state = core.boards().state.locked();
    let projects: Vec<String> = state.boards.keys().cloned().collect();
    for project in projects {
        let Some(board) = state.boards.get(&project) else {
            continue;
        };
        let mut written = Vec::new();
        for goal in (board.cards.iter()).filter(|card| card.parent.is_none()) {
            let Some(every) = goal.goal.report_every() else {
                continue;
            };
            let last = goal.goal.last_report();
            if let (ReportEvery::Minutes { minutes }, Some(last)) = (every, last) {
                let at = last.at + u64::from(minutes) * 60_000;
                if at > now {
                    next = sooner(next, Some(at));
                    continue;
                }
            }
            let report = write(core, board, goal, last, now);
            let changed = last.is_none_or(|last| shape(last) != shape(&report));
            let due_at = match (every, last) {
                (_, None) => Some(now),
                (ReportEvery::Minutes { .. }, Some(_)) => {
                    let finished = super::slice::goal_done(board, goal) && !changed;
                    (!finished).then_some(now)
                }
                (ReportEvery::Change, Some(last)) => changed.then(|| last.at + MIN_GAP_MS),
            };
            match due_at {
                Some(at) if at <= now => {
                    if let ReportEvery::Minutes { minutes } = every {
                        next = sooner(next, Some(now + u64::from(minutes) * 60_000));
                    }
                    written.push((
                        goal.id,
                        goal.title.clone(),
                        report,
                        changed && last.is_some(),
                    ));
                }
                at => next = sooner(next, at),
            }
        }
        if written.is_empty() {
            continue;
        }
        let Some(board) = state.boards.get_mut(&project) else {
            continue;
        };
        for (id, title, report, tell) in written {
            if tell {
                moved.push(GoalMoved {
                    project: project.clone(),
                    goal: id,
                    title,
                    landed: Vec::new(),
                    finished: false,
                    slip: None,
                    report: Some(words(&report)),
                });
            }
            if let Ok(card) = board.card_mut(id) {
                card.goal.reported(report, KEEP);
            }
        }
        core.boards().save(&mut state, &project);
    }
    (next, moved)
}

/// How goal `goal` stands now, against the report before.
fn write(
    core: &Core,
    board: &super::Board,
    goal: &Card,
    last: Option<&GoalReport>,
    now: u64,
) -> GoalReport {
    let view = card_view(core, board, goal);
    let since = last.map(|last| last.at);
    let mut report = GoalReport {
        at: now,
        since,
        needs_you: Vec::new(),
        done: Vec::new(),
        moving: Vec::new(),
        waiting: Vec::new(),
        next: view.next.clone(),
        eta: view.eta.clone(),
        eta_before: last.and_then(|last| last.eta.as_ref()?.at),
    };
    for held in board.held_by_goal(goal) {
        let held = match held.id == goal.id {
            true => view.clone(),
            false => card_view(core, board, held),
        };
        let Some(mut line) = line(&held) else {
            continue;
        };
        let bucket = match line.state {
            SliceState::NeedsYou => &mut report.needs_you,
            SliceState::Done => {
                let before = line.at.zip(since).is_some_and(|(at, since)| at <= since);
                if before {
                    continue;
                }
                &mut report.done
            }
            SliceState::Moving | SliceState::Failed => &mut report.moving,
            SliceState::Waiting => &mut report.waiting,
        };
        line.new = last.is_some_and(|last| !was(last, &line));
        bucket.push(line);
    }
    report
}

fn line(card: &BoardCard) -> Option<ReportLine> {
    let slice = card.slice.as_ref()?;
    let waiting = (slice.gates.iter()).find(|gate| gate.state == GateState::Waiting);
    let at = match slice.state {
        SliceState::Done => slice.gates.iter().filter_map(|gate| gate.at).max(),
        _ => slice.wait.as_ref().map(|wait| wait.since),
    };
    Some(ReportLine {
        card: card.id,
        promise: slice.promise.clone(),
        owner_name: card.owner_name.clone(),
        state: slice.state,
        gate: waiting.map(|gate| gate.gate),
        environment: waiting.and_then(|gate| gate.environment.clone()),
        wait: slice.wait.clone(),
        at,
        new: false,
    })
}

/// Whether `line` stood the same way in `report`.
fn was(report: &GoalReport, line: &ReportLine) -> bool {
    lines(report).any(|before| {
        before.card == line.card
            && before.state == line.state
            && before.gate == line.gate
            && before.environment == line.environment
    })
}

fn lines(report: &GoalReport) -> impl Iterator<Item = &ReportLine> {
    (report.needs_you.iter())
        .chain(&report.done)
        .chain(&report.moving)
        .chain(&report.waiting)
}

/// What a report says, without the clock: a later report with the same shape says nothing new.
/// A slice that got done leaves the rest, which is change enough, so what is done is left out.
fn shape(report: &GoalReport) -> Vec<String> {
    let mut shape: Vec<String> = lines(report)
        .filter(|line| line.state != SliceState::Done)
        .map(|line| {
            format!(
                "{}:{:?}:{:?}:{:?}",
                line.card, line.state, line.gate, line.environment
            )
        })
        .collect();
    shape.extend(
        (report.next.iter())
            .map(|next| format!("next {}:{:?}:{:?}", next.card, next.gate, next.environment)),
    );
    if let Some(eta) = &report.eta {
        shape.push(format!("slip {:?}", eta.slip));
    }
    shape
}

/// A report in a sentence or two, as the person's notification says it: what waits on them
/// first, then what got done, then when the goal should be through.
fn words(report: &GoalReport) -> String {
    let mut said = Vec::new();
    if !report.needs_you.is_empty() {
        said.push(format!("On you: {}", named(&report.needs_you)));
    }
    if !report.done.is_empty() {
        said.push(format!("Done: {}", named(&report.done)));
    }
    if !report.moving.is_empty() {
        said.push(format!("Moving: {}", named(&report.moving)));
    }
    if let Some(eta) = &report.eta {
        let through = match eta.at {
            Some(at) => format!("Through {}", when(at)),
            None => "No ETA".to_owned(),
        };
        said.push(match eta.slip {
            Some(slip) => format!("{through}, later than said: {}", slip.words()),
            None => through,
        });
    }
    match said.is_empty() {
        true => "No change".to_owned(),
        false => said.join(". "),
    }
}

fn named(lines: &[ReportLine]) -> String {
    let mut names: Vec<String> = (lines.iter().take(NAMED))
        .map(|line| format!("#{} {}", line.card, line.promise))
        .collect();
    if lines.len() > NAMED {
        names.push(format!("{} more", lines.len() - NAMED));
    }
    names.join(", ")
}
