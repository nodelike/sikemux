//! Hiring a chat for a standing role, such as @Marketer for reach and growth: it takes the role's
//! name and a line saying what the role covers, is pinned, and keeps a small session that goes on
//! across weeks, under the project's @Chief, who sends it the work in its area.

use crate::server::changed::{ChangedCore, What};
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_view::workspace::PinnedChat;

use super::chat::{rotation, ChatCommand, Whose};
use super::connection::blocking;
use super::names::AgentName;
use super::{Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use sikemux_calls::names::HireAgentParams;

/// Hires the agent: renamed to the role, kept, pinned, and told its role. Hired by `chief`, the
/// hire is on the record as the chief's decision, which the person can overrule.
pub(crate) async fn hire(
    core: &Arc<Core>,
    params: HireAgentParams,
    chief: Option<String>,
) -> CoreResult<AgentName> {
    if core.workspaces().is_chief(&params.agent_id) {
        return Err("@Chief keeps its own seat".into());
    }
    let core = core.clone();
    blocking(move || {
        let was = core.names().name_or_id(&params.agent_id);
        let named = core.names().hire_for_role(
            &params.agent_id,
            &params.name,
            &params.role,
            core.open_agents(),
        )?;
        let role = core.names().role(&params.agent_id).unwrap_or_default();
        if let Some(agent) = core.workspaces().agent(&params.agent_id) {
            if let Some(session_id) = agent.resume_id {
                core.workspaces().pin(PinnedChat {
                    provider: agent.provider,
                    session_id,
                    title: agent.title,
                    project: agent.cwd,
                })?;
            }
        }
        if let Ok(chat) = core.chats().running(&params.agent_id) {
            chat.hired();
            let seat = rotation::hired_seat(&core, &params.agent_id).unwrap_or_default();
            let told = chat.send(ChatCommand::Prompt {
                message_id: None,
                text: String::new(),
                paths: Vec::new(),
                context: vec![crate::sessions::for_agent::sikemux_context(
                    "hired",
                    "Your standing role",
                    &seat,
                )],
                shown: true,
                whose: Whose::NOT_PERSON,
            });
            if let Err(error) = told {
                eprintln!("sikemux core: a hired chat was not told its role: {error}");
            }
        }
        if let Some(chief) = chief {
            super::board::goal::keep_settled(
                &core,
                super::board::goal::Settled {
                    project: named.project.clone(),
                    by: chief,
                    holder: params.agent_id.clone(),
                    asked: None,
                    text: format!("@{was} is hired as @{}: {role}", named.name),
                    why: None,
                },
            );
        }
        core.changed(What::View);
        Ok(named)
    })
    .await
}

#[derive(Deserialize)]
struct ToolHire {
    agent: String,
    name: String,
    role: String,
}

/// `agent_hire`: the project's chief hires one of its agents for a standing role.
pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = super::caller::agent_of(request, "agent_hire")?;
    let given: ToolHire =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let (here, asked, by) = (core.clone(), request.clone(), caller.clone());
    let agent_id = blocking(move || {
        if !here.workspaces().is_chief(&by) {
            return Err("Only the project's @Chief hires for a standing role".into());
        }
        let project = super::caller::project_of(&here, &asked);
        let agents = super::roster::listed(&here);
        let target = super::roster::resolve(&here, &agents, &given.agent, &project, &by)?;
        if target.project != project {
            return Err(format!("{} works in another project", given.agent).into());
        }
        Ok((target.agent_id, given.name, given.role))
    })
    .await
    .map_err(|error| error.to_string())?;
    let (agent_id, name, role) = agent_id;
    let named = hire(
        core,
        HireAgentParams {
            agent_id,
            name,
            role,
        },
        Some(caller),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(json!({ "agent": format!("@{}", named.name) }))
}
