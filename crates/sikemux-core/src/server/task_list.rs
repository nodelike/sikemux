//! A project's one list of tasks: the calls open for the person, then the work on its board. The
//! Tasks view draws it and the chief's digest reads it.

use std::sync::Arc;

use sikemux_calls::board::BoardParams;
use sikemux_view::calls::{CallState, PersonCall};
use sikemux_view::task_list::{ProjectTasks, TaskFact, TaskHolder, TaskOf, TaskRow, TaskSection};

use super::board;
use super::connection::blocking;
use super::Core;

pub(crate) async fn read(core: &Arc<Core>, params: BoardParams) -> Result<ProjectTasks, String> {
    let core = core.clone();
    blocking(move || {
        let folder = board::project_at(&core, &params.project)?;
        Ok(of_folder(&core, &folder))
    })
    .await
    .map_err(|error| error.to_string())
}

pub(crate) fn of_folder(core: &Core, folder: &str) -> ProjectTasks {
    let mut rows: Vec<TaskRow> = (super::person_calls::shown(core).iter())
        .filter(|call| call.state == CallState::Open && call.project == folder)
        .map(call_row)
        .collect();
    rows.extend(board::task_rows(core, folder));
    rows.sort_by(|one, other| (one.section.cmp(&other.section)).then(other.since.cmp(&one.since)));
    ProjectTasks {
        project: folder.to_owned(),
        rows,
        teammates: super::peer::tasks(core, folder),
        shared_by: super::peer::shared_by(core, folder),
        asked: super::teammate_asks::of_project(core, folder),
    }
}

fn call_row(call: &PersonCall) -> TaskRow {
    let count = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
    TaskRow {
        of: TaskOf::Call {
            id: call.id.clone(),
        },
        section: TaskSection::ForYou,
        title: call.title.clone().unwrap_or_else(|| call.question.clone()),
        fact: Some(TaskFact::Call {
            call: call.kind,
            choices: count(call.options.len()),
            page: call.page.is_some(),
            pictures: count(
                call.options
                    .iter()
                    .filter(|option| option.picture.is_some())
                    .count(),
            ),
        }),
        holder: Some(TaskHolder {
            agent_id: call.asker.agent_id.clone(),
            name: call.asker.name.clone(),
            cli: None,
        }),
        since: call.raised_at,
        dropped: None,
        asked: false,
    }
}

/// The list as lines of the chief's digest: the queued tasks the person asked for, each by its
/// number, and how many sit in each section.
pub(crate) fn words(tasks: &ProjectTasks, ago: impl Fn(u64) -> String) -> Vec<String> {
    let asked: Vec<String> = (tasks.rows.iter())
        .filter(|row| row.asked && row.section == TaskSection::Queued)
        .filter_map(|row| match row.of {
            TaskOf::Task { id } => Some(format!("#{id} \"{}\", {}", row.title, ago(row.since))),
            TaskOf::Call { .. } => None,
        })
        .collect();
    let mut lines = Vec::new();
    if !asked.is_empty() {
        lines.push(format!(
            "What the person asked for that nobody is on (give one to an agent with agent_spawn's or agent_send's task): {}",
            asked.join("; ")
        ));
    }
    let sections = [
        (TaskSection::ForYou, "for the person"),
        (
            TaskSection::Finished,
            "finished, not yet opened by the person",
        ),
        (TaskSection::Moving, "moving"),
        (TaskSection::Queued, "queued"),
        (TaskSection::Dropped, "dropped"),
    ];
    let counts: Vec<String> = (sections.into_iter())
        .filter_map(|(section, words)| {
            let count = (tasks.rows.iter())
                .filter(|row| row.section == section)
                .count();
            (count > 0).then(|| format!("{count} {words}"))
        })
        .collect();
    if !counts.is_empty() {
        lines.push(format!("Tasks: {}", counts.join(", ")));
    }
    lines
}
