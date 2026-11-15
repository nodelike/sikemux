//! A project's tasks: running them, the checks that hold them back, and adding one.

pub(crate) mod add_task;
pub(crate) mod run_check;
pub(crate) mod test_card;

use std::collections::HashSet;

use gpui_kit::{AnyElement, Context, Window};
use sikemux_calls::call;
use sikemux_calls::harness::RunSelector;
use sikemux_calls::tasks::RunView;
use sikemux_calls::tasks::TaskView;

use crate::workbench::Workbench;

/// The runs the person started here, and the card of the last one's failed tests.
#[derive(Default)]
pub struct Tasks {
    /// The runs the person started here, whose terminal opens once the run has one.
    pub to_show: HashSet<String>,
    pub cards: test_card::RunCards,
}

/// What hangs under the top bar's right end: the checks holding back a task the person started,
/// else the failed tests of their last run.
pub(crate) fn cards(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let project = bench.watched.project.as_deref()?;
    run_check::card(bench, project, cx).or_else(|| test_card::card(bench, cx))
}

impl Workbench {
    /// The run of the project in front's tasks the person started that is going, and the last.
    pub fn person_runs(&self) -> (Option<&RunView>, Option<&RunView>) {
        let Some(path) = self.watched.project.as_deref() else {
            return (None, None);
        };
        let mine: Vec<&RunView> = (self.feeds.in_front().runs.iter())
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

    /// Opens the Add a process dialog for the project in front.
    pub fn add_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.watched.project.clone() {
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
        self.target()
            .call_typed::<call::TaskStop>(RunSelector::of_execution(execution));
    }
}
