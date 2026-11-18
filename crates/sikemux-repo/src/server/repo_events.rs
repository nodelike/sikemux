//! A commit an agent made, and what follows from it.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::Path;

use super::Core;

/// A commit an agent made, which exists now.
pub struct CommitMade<'a> {
    pub agent: &'a str,
    /// The top of the checkout it was made in.
    pub top: &'a Path,
    pub sha: &'a str,
    /// The files it changed, from the top of the checkout.
    pub paths: &'a [String],
    pub message: &'a str,
}

/// The one event a commit is, however it was made: what follows from it follows from here.
pub fn commit_made(core: &Core, made: CommitMade<'_>) {
    let (project, on_branch) = core.workspaces().project_checkout(made.top);
    let subject = made.message.lines().next().unwrap_or_default();
    let commit = [(made.sha.to_owned(), subject.to_owned())];
    (core.repo_above().board_committed)(core, &project, made.agent, &commit, on_branch);
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
            subject: subject.to_owned(),
            agents,
        };
        core.checks()
            .wait_for(project.clone(), std::path::PathBuf::from(&project), commit);
    }
    (core.repo_above().turn_state_committed)(core, made.agent);
    core.collisions()
        .committed(made.agent, made.top, made.paths);
}
