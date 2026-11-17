//! The background tasks that ended in one turn, folded into one part.

use sikemux_view::chat::{EndedTask, EndedTasks, NoticeState, TaskNotice};

/// Adds `notice` to `tasks`, telling a task that ended the same way before once more rather
/// than again.
pub(crate) fn add(tasks: &mut EndedTasks, notice: TaskNotice) {
    let name = notice.name.lines().next().unwrap_or_default().trim();
    let name = if name.is_empty() {
        "Background task"
    } else {
        name
    }
    .to_owned();
    match notice.state {
        NoticeState::Completed => tasks.done += 1,
        NoticeState::Failed => tasks.failed += 1,
        NoticeState::Stopped => {
            tasks.stopped += 1;
            if notice.task_type.as_deref() == Some("monitor") {
                tasks.watches_stopped += 1;
            }
        }
    }
    let same = tasks.tasks.iter_mut().find(|task| {
        task.name == name && task.state == notice.state && task.task_type == notice.task_type
    });
    if let Some(task) = same {
        task.times += 1;
        return;
    }
    let summary = notice
        .summary
        .as_deref()
        .and_then(|summary| summary.lines().next())
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != name)
        .map(str::to_owned);
    tasks.tasks.push(EndedTask {
        name,
        state: notice.state,
        summary,
        task_type: notice.task_type,
        times: 1,
    });
}
