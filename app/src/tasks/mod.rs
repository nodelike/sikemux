//! A project's tasks: running them, what they listen on, the checks that hold them back, and
//! adding one.

pub(crate) mod add_task;
pub(crate) mod ports;
pub(crate) mod run_check;
pub(crate) mod test_card;

use gpui_kit::{Context, Window};
use serde_json::json;
use sikemux_core::server::ports::PortOwner;
use sikemux_core::server::ports::ProjectPort;
use sikemux_core::server::tasks::RunView;
use sikemux_core::server::tasks::TaskView;

use crate::workbench::Workbench;

impl Workbench {
    /// The run of the project in front's tasks the person started that is going, and the last.
    pub fn person_runs(&self) -> (Option<&RunView>, Option<&RunView>) {
        let Some(path) = self.watched_project.as_deref() else {
            return (None, None);
        };
        let mine: Vec<&RunView> = (self.reads.runs.iter())
            .filter(|run| run.run.project == path && run.agent_id.is_none())
            .collect();
        (
            mine.iter()
                .rev()
                .find(|run| run.run.status.is_active())
                .copied(),
            mine.last().copied(),
        )
    }

    /// The port the task run `execution` listens on, when it listens on one.
    pub fn port_of_run(&self, execution: &str) -> Option<&ProjectPort> {
        self.reads.ports.iter().find(|port| {
            matches!(&port.listening.owner, PortOwner::Task { execution_id, .. } if execution_id == execution)
        })
    }

    /// Counts the seconds on the Run chip while the person's task runs with no port to show.
    pub(crate) fn time_run(&mut self, cx: &mut Context<Self>) {
        let run = self.person_runs().0;
        let counting = run.is_some_and(|run| self.port_of_run(&run.run.execution_id).is_none());
        let started_at = run.and_then(|run| run.run.started_at).map(|at| at as i64);
        self.run_clock
            .update(cx, |clock, cx| clock.set(counting, started_at, cx));
    }

    /// Makes `task` the one the project's Run button starts.
    pub fn pick_task(&mut self, task: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.watched_project.clone() {
            self.picked_tasks.insert(project, task.to_owned());
            cx.notify();
        }
    }

    /// Opens the Add a process dialog for the project whose processes the Run menu lists.
    pub fn add_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.watched_project.clone() {
            self.overlays.adding_task =
                Some(crate::tasks::add_task::AddTask::new(project, window, cx));
            cx.notify();
        }
    }

    /// Opens the task dialog on the person's own `task` of `project`, to change it.
    pub fn edit_task(
        &mut self,
        project: &str,
        task: &TaskView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.adding_task = Some(crate::tasks::add_task::AddTask::edit(
            project.to_owned(),
            task,
            window,
            cx,
        ));
        cx.notify();
    }

    pub fn stop_run(&self, execution: &str) {
        self.call_in_front("taskStop", json!({ "executionId": execution }));
    }
}
