//! The person moves a project's rule into the AGENTS.md of the repository it
//! is kept for, where every agent and person reads it with the code and it is
//! versioned with it. The core then forgets it, so it lives in one place. The
//! file is left uncommitted for the person to review.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_process::sync::Locked;
use sikemux_view::memory::{MemoryView, Rule, Stays};

use super::{project_at, run, tell_clients, view, Core, MemoryEntryParams};
use crate::server::access::Peer;

const FILE: &str = "AGENTS.md";
const HEADING: &str = "## Rules kept by Sikemux";

/// AGENTS.md with the rule added under Sikemux's heading, which goes at the
/// end when the file has none yet, in the file's own line endings. A rule
/// already under the heading is not added again.
fn with_rule(file: &str, rule: &str) -> String {
    let newline = if file.contains("\r\n") { "\r\n" } else { "\n" };
    let bullet = format!(
        "- {}",
        rule.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    let bullet = bullet.as_str();
    let mut lines: Vec<&str> = file.lines().collect();
    match lines.iter().position(|line| line.trim_end() == HEADING) {
        None => {
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            if !lines.is_empty() {
                lines.push("");
            }
            lines.extend([HEADING, "", bullet]);
        }
        Some(heading) => {
            let end = lines[heading + 1..]
                .iter()
                .position(|line| line.starts_with("## "))
                .map_or(lines.len(), |at| heading + 1 + at);
            if lines[heading + 1..end]
                .iter()
                .any(|line| line.trim_end() == bullet)
            {
                return file.to_owned();
            }
            let last = lines[..end]
                .iter()
                .rposition(|line| !line.trim().is_empty())
                .unwrap_or(heading);
            let mut added = if last == heading {
                vec!["", bullet]
            } else {
                vec![bullet]
            };
            if end < lines.len() {
                added.push("");
            }
            lines.splice(last + 1..end, added);
        }
    }
    lines.join(newline) + newline
}

pub(crate) async fn promote(
    core: &Arc<Core>,
    peer: Peer,
    params: MemoryEntryParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let id = params.id;
    let core = core.clone();
    run(move || {
        let _one_at_a_time = core.memory.promotes.locked();
        let folder = core.memory.state.locked().folder_of(&project, id)?;
        let path = agents_file(&folder);
        let (text, file) = {
            let mut state = core.memory.state.locked();
            let rule = (state.held.get(&folder))
                .and_then(|held| held.rules.iter().find(|rule| rule.id == id))
                .ok_or_else(|| format!("#{id} is not a rule, and only rules go into {FILE}"))?;
            if let Some(stays) = stays(rule, &path) {
                return Err(refusal(id, &folder, stays));
            }
            let text = rule.text.clone();
            state.promoting = Some(id);
            (text, core.memory.save(&mut state, &folder))
        };
        let gone_on_disk = match file {
            Some(file) => {
                file.flush();
                file.unsaved()
                    .map_or(Ok(()), |error| Err(format!("Sikemux could not save that #{id} moved, so {FILE} was left alone: {error}")))
            }
            None => Ok(()),
        };
        let written = gone_on_disk.and_then(|()| match &path {
            Ok(path) => write(path, &text),
            Err(stays) => Err(refusal(id, &folder, *stays)),
        });
        let mut state = core.memory.state.locked();
        state.promoting = None;
        if written.is_ok() {
            if let Some(held) = state.held.get_mut(&folder) {
                held.rules.retain(|rule| rule.id != id);
            }
        }
        core.memory.save(&mut state, &folder);
        drop(state);
        written?;
        tell_clients(&core, false);
        Ok(view(&core, &project, &peer))
    })
    .await
}

/// The AGENTS.md of the checkout at `folder`, or the file it links to inside
/// the checkout.
pub(super) fn agents_file(folder: &str) -> Result<PathBuf, Stays> {
    let folder = Path::new(folder);
    if !folder.join(".git").exists() {
        return Err(Stays::NoCheckout);
    }
    let path = folder.join(FILE);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return Ok(path);
    };
    if !meta.file_type().is_symlink() {
        return Ok(path);
    }
    std::fs::canonicalize(&path)
        .ok()
        .filter(|target| target.starts_with(folder))
        .ok_or(Stays::LinkedAway)
}

/// Why `rule`, kept for a folder whose AGENTS.md is `file`, cannot move there.
pub(super) fn stays(rule: &Rule, file: &Result<PathBuf, Stays>) -> Option<Stays> {
    if rule.sensitive {
        return Some(Stays::Sensitive);
    }
    file.as_ref().err().copied()
}

fn refusal(id: u64, folder: &str, stays: Stays) -> String {
    match stays {
        Stays::Sensitive => format!("#{id} is sensitive, so it never goes into the repository"),
        Stays::NoCheckout => format!(
            "{folder} is not the top of a git checkout, so an {FILE} there would not be versioned"
        ),
        Stays::LinkedAway => format!(
            "{folder}'s {FILE} links to nothing that can be read, or outside the checkout, which the repository does not version"
        ),
    }
}

/// Adds the rule to the file, keeping its permissions. Once the new file is
/// in place the rule is in the repository, so a permission that cannot be
/// put back is only reported.
fn write(path: &Path, rule: &str) -> Result<(), String> {
    let file = match std::fs::read_to_string(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    let permissions = std::fs::metadata(path).ok().map(|meta| meta.permissions());
    sikemux_process::files::write_atomic(path, with_rule(&file, rule).as_bytes(), false)
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    if let Some(Err(error)) =
        permissions.map(|permissions| std::fs::set_permissions(path, permissions))
    {
        eprintln!(
            "sikemux core: {} kept its new permissions: {error}",
            path.display()
        );
    }
    Ok(())
}
