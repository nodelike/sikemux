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
/// end when the file has none yet, in the file's own line endings.
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
        let (folder, text) = {
            let mut state = core.memory.state.locked();
            let folder = state.folder_of(&project, id)?;
            let rule = state
                .held
                .get(&folder)
                .and_then(|held| held.rules.iter().find(|rule| rule.id == id))
                .ok_or_else(|| format!("#{id} is not a rule, and only rules go into {FILE}"))?;
            if rule.goal.is_some() {
                return Err(format!("#{id} is a goal's rule, which stays with the goal"));
            }
            if rule.sensitive {
                return Err(format!(
                    "#{id} is sensitive, so it never goes into the repository"
                ));
            }
            let text = rule.text.clone();
            if !state.promoting.insert(id) {
                return Err(format!("#{id} is already being written into {FILE}"));
            }
            (folder, text)
        };
        let written = write(&folder, &text);
        let mut state = core.memory.state.locked();
        state.promoting.remove(&id);
        written?;
        if let Some(held) = state.held.get_mut(&folder) {
            held.rules.retain(|rule| rule.id != id);
        }
        core.memory.save(&mut state, &folder);
        drop(state);
        tell_clients(&core, false);
        Ok(view(&core, &project, &peer))
    })
    .await
}

fn write(folder: &str, rule: &str) -> Result<(), String> {
    if !Path::new(folder).join(".git").exists() {
        return Err(format!(
            "{folder} is not the top of a git checkout, so an {FILE} there would not be versioned"
        ));
    }
    let path = Path::new(folder).join(FILE);
    let file = match std::fs::read_to_string(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    sikemux_process::files::write_atomic(&path, with_rule(&file, rule).as_bytes(), false)
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))
}
