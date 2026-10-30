//! Whose work an agent's own git command would take in or throw away, for the
//! git guard to refuse it by.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::commit::{in_checkout, names};
use super::owners::{git, traced};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::messages::{listed, Reach};
use crate::server::workspace::project_of;
use crate::server::Core;

/// The other agents than `agent_id` with uncommitted lines in `paths`, as
/// given in `dir`, leaving out any outside its checkout. Lines it may have
/// written among others at work then count as its own. With no paths, those
/// with lines anywhere in the checkout, and those working in it. A checkout
/// in no project, such as a scratch repository, has no lines on record, so
/// only the agents working in it are looked for.
pub(crate) fn others(
    core: &Core,
    agent_id: &str,
    dir: &Path,
    paths: &[String],
) -> Result<Vec<String>, String> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim());
    let top = std::fs::canonicalize(&top).unwrap_or(top);
    let projects = core.workspaces.project_paths();
    let project = project_of(&top, &projects);
    let in_project = projects.iter().any(|known| {
        std::fs::canonicalize(known).unwrap_or_else(|_| known.clone()) == Path::new(&project)
    });
    let mut others = BTreeSet::new();
    if in_project {
        others.extend(recorded(core, agent_id, dir, paths)?);
    }
    if paths.is_empty() {
        others.extend(
            listed(core)
                .into_iter()
                .filter(|agent| matches!(agent.reach, Reach::Chat | Reach::Terminal))
                .filter(|agent| agent.agent_id != agent_id)
                .filter(|agent| {
                    core.edits.checkout_of(core, &agent.agent_id).as_deref() == Some(&top)
                })
                .map(|agent| agent.name.unwrap_or(agent.agent_id)),
        );
    }
    Ok(others.into_iter().collect())
}

/// The other agents than `agent_id` the edit record has uncommitted lines of
/// in `paths`, or anywhere in the checkout with none.
fn recorded(
    core: &Core,
    agent_id: &str,
    dir: &Path,
    paths: &[String],
) -> Result<BTreeSet<String>, String> {
    let (owners, diff) = traced(core, dir, false)?;
    let named: Vec<String> = paths
        .iter()
        .filter_map(|path| in_checkout(&diff.top, dir, path).ok())
        .collect();
    if named.is_empty() && !paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mine = owners.standing_for(core, agent_id);
    Ok(owners
        .files
        .iter()
        .filter(|file| named.is_empty() || names(&named, &file.path))
        .flat_map(|file| {
            let lines = file.lines.iter().map(|run| run.author);
            lines.chain(file.removed.iter().map(|removal| removal.author))
        })
        .filter(|at| !mine.contains(at))
        .map(|at| owners.authors[at].name.clone())
        .collect())
}

/// What the git shim asks before a command that could take in or throw away
/// other agents' work: `paths` it names, or none for the whole checkout.
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let paths: Vec<String> = match request.params.get("paths") {
        None | Some(Value::Null) => Vec::new(),
        Some(paths) => {
            serde_json::from_value(paths.clone()).map_err(|_| "paths must be a list of paths")?
        }
    };
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("git.guard needs the calling agent's id")?;
    let (core, dir) = (core.clone(), PathBuf::from(&request.project));
    blocking(move || Ok(others(&core, &agent_id, &dir, &paths)?))
        .await
        .map_err(|error| error.to_string())
        .map(|others| serde_json::json!({ "others": others }))
}
