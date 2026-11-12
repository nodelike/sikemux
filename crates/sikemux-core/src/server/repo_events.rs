//! What happens to a project's commits, told once by the code that commits,
//! lands and checks them, for the board to follow in the order it happened.

use crate::server::parts::Parts;
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use sikemux_process::sync::Locked;
use sikemux_view::slice::{CheckRun, Verdict};
use tokio::sync::Notify;

use super::board::DeployRan;
use super::Core;

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
    /// A deploy of `agent`'s ended, which its slice keeps.
    Deployed {
        project: String,
        agent: String,
        ran: DeployRan,
    },
    /// The project's check started on the `covered` commits, the newest last,
    /// here or in CI as `run` says.
    CheckStarted {
        project: String,
        covered: Vec<String>,
        run: CheckRun,
    },
    /// The project's check ended at `sha`, taking in the `covered` commits:
    /// with no verdict when it did not run, for the reason in `output`.
    Checked {
        project: String,
        command: String,
        sha: Option<String>,
        covered: Vec<String>,
        verdict: Option<Verdict>,
        output: String,
    },
}

/// A commit an agent made, which exists now.
pub(crate) struct CommitMade<'a> {
    pub agent: &'a str,
    /// The top of the checkout it was made in.
    pub top: &'a Path,
    pub sha: &'a str,
    /// The files it changed, from the top of the checkout.
    pub paths: &'a [String],
    pub message: &'a str,
}

/// The one event a commit is, however it was made: the board hears of it in order with the rest
/// of the project's events, and what else follows from it follows from here.
pub(crate) fn commit_made(core: &Core, made: CommitMade<'_>) {
    super::board::backup::committed(core, made.agent);
    let (project, on_branch) = core.workspaces().project_checkout(made.top);
    if on_branch {
        let mut agents: Vec<String> = crate::trailers::agents(made.message)
            .into_iter()
            .map(|agent| agent.chat)
            .collect();
        if agents.is_empty() {
            agents.push(made.agent.to_owned());
        }
        let commit = super::checks::Commit {
            sha: made.sha.to_owned(),
            subject: made.message.lines().next().unwrap_or_default().to_owned(),
            agents,
            asked: false,
        };
        core.checks()
            .wait_for(project.clone(), std::path::PathBuf::from(&project), commit);
        super::board::roles::landed(core, &project);
        super::chief::landed(core, &project, made.agent);
    }
    core.repo_events().tell(RepoEvent::Committed {
        project,
        agent: made.agent.to_owned(),
        sha: made.sha.to_owned(),
        on_branch,
    });
    core.collisions()
        .committed(made.agent, made.top, made.paths);
}

impl RepoEvent {
    fn project(&self) -> &str {
        match self {
            RepoEvent::Committed { project, .. }
            | RepoEvent::Landed { project, .. }
            | RepoEvent::Deployed { project, .. }
            | RepoEvent::CheckStarted { project, .. }
            | RepoEvent::Checked { project, .. } => project,
        }
    }
}

/// What the board reads at once: the events told, oldest first, and the
/// projects some of whose events were let go of unread.
pub(crate) struct Told {
    pub events: Vec<RepoEvent>,
    pub lost: BTreeSet<String>,
}

#[derive(Default)]
struct Waiting {
    events: VecDeque<RepoEvent>,
    lost: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct RepoEvents {
    waiting: Mutex<Waiting>,
    told: Notify,
}

impl RepoEvents {
    pub(crate) fn tell(&self, event: RepoEvent) {
        let mut waiting = self.waiting.locked();
        if waiting.events.len() >= MAX_WAITING {
            if let Some(oldest) = waiting.events.pop_front() {
                eprintln!(
                    "sikemux core: the board fell behind and let go of an event in {}",
                    oldest.project()
                );
                waiting.lost.insert(oldest.project().to_owned());
            }
        }
        waiting.events.push_back(event);
        drop(waiting);
        self.told.notify_one();
    }

    /// The events of `projects` were not followed through, so the board
    /// treats them as lost.
    pub(crate) fn lost(&self, projects: impl IntoIterator<Item = String>) {
        self.waiting.locked().lost.extend(projects);
        self.told.notify_one();
    }

    /// What was told since the last read, once anything was.
    pub(crate) async fn next(&self) -> Told {
        loop {
            {
                let mut waiting = self.waiting.locked();
                if !waiting.events.is_empty() || !waiting.lost.is_empty() {
                    return Told {
                        events: waiting.events.drain(..).collect(),
                        lost: std::mem::take(&mut waiting.lost),
                    };
                }
            }
            self.told.notified().await;
        }
    }
}
