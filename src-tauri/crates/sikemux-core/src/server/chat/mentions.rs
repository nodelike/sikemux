//! What a message tells its agent about each agent it names with `@`. A client sends
//! only who it names, as a context item with no text, and the core writes the note.
//! An earlier chat the person names is opened again first, in a tab of its own.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::protocol::ChatContext;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::{display, launching, saved, CoreError, CoreResult};
use crate::server::{names, Core};

const AGENT_URI: &str = "sikemux://agent/";
/// An earlier chat of the project, as `<cli>/<session id>`, to open again for the message.
const EARLIER_URI: &str = "sikemux://earlier/";
/// An earlier chat opened again, by the id of the agent that now runs it.
const WOKEN_URI: &str = "sikemux://woken/";

/// Whether `context` names an earlier chat to open again before the message goes.
pub(crate) fn wakes(context: &[ChatContext]) -> bool {
    context
        .iter()
        .any(|item| item.text.is_empty() && item.uri.starts_with(EARLIER_URI))
}

/// Opens again each earlier chat `context` names for the message `caller` is
/// about to get, and writes what the message tells it about the agent now
/// running it. A chat that cannot be opened again fails the message.
pub(crate) async fn wake(
    core: &Arc<Core>,
    caller: &str,
    context: &mut [ChatContext],
) -> CoreResult<()> {
    for item in context.iter_mut() {
        if !item.text.is_empty() {
            continue;
        }
        let Some((cli, session)) = item
            .uri
            .strip_prefix(EARLIER_URI)
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        *item = woken(core, caller, cli, session).await?;
    }
    Ok(())
}

async fn woken(
    core: &Arc<Core>,
    caller: &str,
    cli: &str,
    session: &str,
) -> CoreResult<ChatContext> {
    let found = {
        let (owner, session, cli) = (core.clone(), session.to_owned(), cli.to_owned());
        blocking(move || saved::locate(&owner, &session, Some(&cli)).map_err(CoreError::from))
            .await?
    };
    if let Some((agent_id, _)) = &found.open {
        let name = core
            .names
            .name(agent_id)
            .unwrap_or_else(|| agent_id.clone());
        let text = note(core, caller, agent_id).ok_or_else(|| {
            CoreError::from(format!(
                "\"{}\" closed before the message went",
                found.title
            ))
        })?;
        return Ok(ChatContext {
            uri: format!("{AGENT_URI}{agent_id}"),
            title: format!("@{name}"),
            text,
        });
    }
    let launcher = core.workspaces.launcher(&found.cli)?;
    let project = core.workspaces.project_for(&found.project)?;
    let profile = launcher.id.split_once(':').map(|(_, profile)| profile);
    let draft = json!({
        "provider": found.cli,
        "project": project,
        "cwd": found.project,
        "title": found.title,
        "resumeId": found.session_id,
        "permissionMode": launcher.permission_mode,
        "profileId": profile,
    });
    let Value::Object(draft) = draft else {
        unreachable!("an object literal")
    };
    let agent = core.workspaces.put_agent(draft)?;
    let (name, kept) = {
        let (owner, agent_id, cwd) = (core.clone(), agent.id.clone(), agent.cwd.clone());
        let (provider, resume) = (found.cli.clone(), found.session_id.clone());
        blocking(move || {
            let starting = Starting {
                agent_id: &agent_id,
                project: names::project_of(&cwd, &owner.workspaces.project_paths()),
                provider: Some(&provider),
                resume_id: Some(&resume),
            };
            let kept = owner.names.reopen(&starting, owner.open_agents());
            let named = owner.name_agent(&agent_id, &cwd, Some(&provider), Some(&resume));
            Ok((named.name, kept))
        })
        .await?
    };
    if let Some(Some(buddy)) = &kept {
        let mut patch = serde_json::Map::new();
        patch.insert("buddy".into(), buddy.clone());
        core.workspaces.update_agent(&agent.id, patch)?;
    }
    core.view_changed();
    {
        let (core, agent_id) = (core.clone(), agent.id.clone());
        tokio::spawn(async move {
            if let Err(error) = launching::start_saved(&core, &agent_id, false).await {
                eprintln!(
                    "sikemux core: an earlier chat named in a message did not start: {error}"
                );
            }
        });
    }
    let named_as = if kept.is_some() {
        "under the name it had"
    } else {
        "under this new name"
    };
    let opened = format!(
        "Sikemux opened its earlier chat \"{}\" again for this message, {named_as}.",
        found.title
    );
    let text = note(core, caller, &agent.id)
        .map(|note| {
            let mut lines: Vec<String> = note.lines().map(str::to_owned).collect();
            lines.insert(1, opened);
            for line in &mut lines {
                if line.starts_with("Status: ") {
                    *line = "Status: Starting".to_owned();
                }
            }
            lines.join("\n")
        })
        .ok_or_else(|| CoreError::from(format!("\"{}\" could not be opened again", found.title)))?;
    Ok(ChatContext {
        uri: format!("{WOKEN_URI}{}", agent.id),
        title: format!("@{name} · {}", found.title),
        text,
    })
}

/// Writes the note of each agent `context` names that came without one, for the
/// message `caller` is about to get. A named agent the core no longer has is dropped.
pub(crate) fn fill(core: &Core, caller: &str, context: &mut Vec<ChatContext>) {
    context.retain_mut(|item| {
        if !item.text.is_empty() {
            return true;
        }
        let Some(id) = item.uri.strip_prefix(AGENT_URI) else {
            return true;
        };
        match note(core, caller, id) {
            Some(note) => {
                item.text = note;
                true
            }
            None => false,
        }
    });
}

fn note(core: &Core, caller: &str, id: &str) -> Option<String> {
    let mate = core.workspaces.agent(id)?;
    let name = core.names.name(id)?;
    let elsewhere = core
        .workspaces
        .agent(caller)
        .filter(|caller| caller.project != mate.project)
        .map(|_| {
            core.workspaces
                .project_info(&mate.project)
                .map_or_else(|| mate.project.clone(), |project| project.name)
        });
    let cli = crate::agent_catalog::agent(&mate.provider)
        .map_or(mate.provider.as_str(), |entry| entry.label);
    let mut lines = vec![match &elsewhere {
        Some(project) => {
            format!("@{name} is an agent in another project, {project}, not the person.")
        }
        None => format!("@{name} is another agent in this project, not the person."),
    }];
    lines.push(format!("Agent id: {id}"));
    lines.push(format!("CLI: {cli}"));
    lines.push(format!("Status: {}", status(core, id)));
    lines.push(format!("Working on: {}", mate.title));
    let to = if elsewhere.is_some() { id } else { &name };
    lines.push(format!("To message it, call agent_send with to \"{to}\"."));
    Some(lines.join("\n"))
}

/// The word every client shows beside the agent, or Asleep while it is not running.
fn status(core: &Core, id: &str) -> &'static str {
    if let Some(chat) = core
        .chat_infos()
        .into_iter()
        .find(|chat| chat.agent_id == id)
    {
        return if chat.asleep {
            "Asleep"
        } else {
            display::group_label(chat.group)
        };
    }
    core.session_infos()
        .into_iter()
        .find(|session| session.agent_id.as_deref() == Some(id))
        .and_then(|session| session.group)
        .map_or("Asleep", display::group_label)
}
