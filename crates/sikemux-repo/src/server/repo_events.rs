//! A commit an agent made, and what follows from it.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::Path;

use sikemux_git::Git;
use sikemux_view::chat::LinesChanged;

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
        let lines = lines_changed(made.top, &format!("{}^", made.sha), made.sha);
        (core.repo_above().chief_landed)(core, &project, made.agent, made.message, lines);
    }
    (core.repo_above().turn_state_committed)(core, made.agent);
    core.collisions()
        .committed(made.agent, made.top, made.paths);
}

/// How many lines the change from `from` to `tip` added and removed; none when it changed no
/// lines, as a change of only binary or renamed files does.
pub(crate) fn lines_changed(repo: &Path, from: &str, tip: &str) -> Option<LinesChanged> {
    let numstat = Git::quick(repo)
        .trimmed(&["diff", "-M", "--numstat", from, tip])
        .ok()?;
    let mut lines = LinesChanged { adds: 0, dels: 0 };
    for line in numstat.lines() {
        let mut counts = line.split('\t');
        lines.adds += counts
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        lines.dels += counts
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
    }
    (lines.adds + lines.dels > 0).then_some(lines)
}
