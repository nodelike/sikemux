//! What happens to a project's commits, told once by the code that commits,
//! lands and checks them, for the board to follow in the order it happened.

use std::collections::VecDeque;
use std::sync::Mutex;

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

/// Told events the board has not read yet, past which the oldest go.
const MAX_WAITING: usize = 256;

pub(crate) enum RepoEvent {
    /// An agent committed `sha`, in the project's own checkout when
    /// `on_branch`, else in a checkout of its own.
    Committed {
        project: String,
        agent: String,
        sha: String,
        on_branch: bool,
    },
    /// Landing made an agent's commits again on the project's branch, from
    /// `onto` up to `tip`.
    Landed {
        project: String,
        agent: String,
        onto: String,
        tip: String,
    },
    /// The project's check ran at `sha`, taking in the `covered` commits.
    Checked {
        project: String,
        command: String,
        sha: Option<String>,
        covered: Vec<String>,
        passed: bool,
        output: String,
    },
}

#[derive(Default)]
pub(crate) struct RepoEvents {
    waiting: Mutex<VecDeque<RepoEvent>>,
    told: Notify,
}

impl RepoEvents {
    pub(crate) fn tell(&self, event: RepoEvent) {
        let mut waiting = self.waiting.locked();
        if waiting.len() >= MAX_WAITING {
            waiting.pop_front();
        }
        waiting.push_back(event);
        drop(waiting);
        self.told.notify_one();
    }

    /// Every event told since the last read, oldest first, once one is.
    pub(crate) async fn next(&self) -> Vec<RepoEvent> {
        loop {
            let events: Vec<RepoEvent> = self.waiting.locked().drain(..).collect();
            if !events.is_empty() {
                return events;
            }
            self.told.notified().await;
        }
    }
}
