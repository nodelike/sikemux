//! Moving an agent into another project. One that has started a saved
//! session is stopped and its session filed under the new folder, so it
//! resumes there; a chat starts again in its new folder at once, and a
//! terminal agent starts again there when it is next shown.

use crate::server::parts::Parts;
use crate::server::ServerCore;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::workspace::AddProjectParams;
use super::{launching, layout, session, Core, CoreError, CoreResult};
use crate::agent_catalog::{self, AgentKind};
use crate::sessions::relocate::relocate;
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_view::workspace::WorkspaceAgent;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveParams {
    pub agent_id: String,
    /// The project, by id.
    pub project: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveToFolder {
    pub agent_id: String,
    /// The folder, which becomes a project first when it is not one.
    pub path: PathBuf,
}

fn movable(core: &Core, agent_id: &str) -> CoreResult<WorkspaceAgent> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    if agent.worktree.is_some() {
        return Err("an agent working in its own worktree stays in its project".into());
    }
    Ok(agent)
}

/// Moves an agent into the project at a folder, as `move_agent` does, making the folder a
/// project first when it is not one.
pub(crate) async fn move_to_folder(
    core: &Arc<Core>,
    params: MoveToFolder,
) -> CoreResult<WorkspaceAgent> {
    movable(core, &params.agent_id)?;
    let project = super::workspace::add_project(core, AddProjectParams::at(params.path)).await?;
    move_agent(
        core,
        MoveParams {
            agent_id: params.agent_id,
            project: project.id,
        },
    )
    .await
}

pub(crate) async fn move_agent(core: &Arc<Core>, params: MoveParams) -> CoreResult<WorkspaceAgent> {
    let agent = movable(core, &params.agent_id)?;
    let to = core
        .workspaces()
        .project_info(&params.project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))?;
    if agent.cwd == to.path {
        return Ok(agent);
    }
    let followed = layout::agent_in_front(core, &agent.id, &agent.project);
    let chat = core.chats().get(&agent.id).is_some();
    let terminal = agent
        .pty_id
        .and_then(|pty| core.session(pty))
        .filter(|session| session.is_running());
    if let Some(resume) = &agent.resume_id {
        let relocatable =
            agent_catalog::agent(&agent.provider).is_some_and(|entry| entry.relocatable_sessions);
        let kind = AgentKind::from_name(&agent.provider)
            .filter(|_| relocatable)
            .ok_or_else(|| {
                CoreError::from(format!(
                    "{} keeps its sessions where they started, so it cannot move",
                    agent.provider
                ))
            })?;
        if chat && !core.chats().stop_if_idle(&agent.id) {
            return Err("the agent is at work; move it once its turn ends".into());
        }
        if let Some(terminal) = &terminal {
            session::kill(core, terminal);
        }
        let config_path = agent.profile_id.as_deref().and_then(|id| {
            core.workspaces()
                .accounts()?
                .profile(id)?
                .config_path
                .clone()
        });
        relocate(
            kind,
            agent.cwd.to_string_lossy().into_owned(),
            to.path.to_string_lossy().into_owned(),
            resume.clone(),
            config_path,
        )
        .await
        .map_err(CoreError::from)?;
    } else if chat {
        core.chats().stop(&agent.id);
    }
    let moved = core.workspaces().edit_agent(&agent.id, |moved| {
        moved.project = to.id.clone();
        moved.cwd = to.path;
        if terminal.is_some() {
            moved.pty_id = None;
        }
    })?;
    if followed {
        layout::open_tab(
            core,
            OpenTab::new(At::project(&to.id), PaneKind::Agent, &agent.id),
        )?;
    }
    if chat {
        let (core, agent_id) = (core.clone(), agent.id.clone());
        // A chat that never ran goes on waiting for the person in its new project.
        let waits = agent.resume_id.is_none();
        tokio::spawn(async move {
            if let Err(error) = launching::start_saved(&core, &agent_id, waits).await {
                eprintln!(
                    "sikemux core: {agent_id} did not start again in its new project: {error}"
                );
            }
        });
    }
    Ok(moved)
}
