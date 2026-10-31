//! The person moves a project's rule into the AGENTS.md of the repository it
//! is kept for, where every agent and person reads it with the code and it is
//! versioned with it. The core then forgets it, so it lives in one place. The
//! file is left uncommitted for the person to review.

use std::path::Path;
use std::sync::Arc;

use sikemux_process::sync::Locked;
use sikemux_wire::memory::MemoryView;

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
        let folder = core.memory.state.locked().folder_of(&project, id)?;
        if !Path::new(&folder).join(".git").exists() {
            return Err(format!(
                "{folder} is not the top of a git checkout, so an {FILE} there would not be versioned"
            ));
        }
        let (rule, file) = {
            let mut state = core.memory.state.locked();
            let held = state
                .held
                .get_mut(&folder)
                .ok_or_else(|| format!("#{id} is no longer kept"))?;
            let at = (held.rules.iter())
                .position(|rule| rule.id == id)
                .ok_or_else(|| format!("#{id} is not a rule, and only rules go into {FILE}"))?;
            if held.rules[at].goal.is_some() {
                return Err(format!("#{id} is a goal's rule, which stays with the goal"));
            }
            if held.rules[at].sensitive {
                return Err(format!(
                    "#{id} is sensitive, so it never goes into the repository"
                ));
            }
            let rule = held.rules.remove(at);
            (rule, core.memory.save(&mut state, &folder))
        };
        if let Some(file) = file {
            file.flush();
        }
        if let Err(error) = write(&folder, &rule.text) {
            let mut state = core.memory.state.locked();
            if let Some(held) = state.held.get_mut(&folder) {
                held.rules.push(rule);
            }
            core.memory.save(&mut state, &folder);
            return Err(error);
        }
        tell_clients(&core, false);
        Ok(view(&core, &project, &peer))
    })
    .await
}

/// Adds the rule to the folder's AGENTS.md, or to the file it links to,
/// keeping the file's permissions.
fn write(folder: &str, rule: &str) -> Result<(), String> {
    let path = Path::new(folder).join(FILE);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let file = match std::fs::read_to_string(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    let permissions = std::fs::metadata(&path).ok().map(|meta| meta.permissions());
    let cannot = |error: std::io::Error| format!("{} cannot be written: {error}", path.display());
    sikemux_process::files::write_atomic(&path, with_rule(&file, rule).as_bytes(), false)
        .map_err(cannot)?;
    if let Some(permissions) = permissions {
        std::fs::set_permissions(&path, permissions).map_err(cannot)?;
    }
    Ok(())
}
