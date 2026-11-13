//! `agent_spawn` with `resume` for a worker that closed on a linked host: the host it ran on
//! takes up its session again, in its checkout there, as the same lead's worker.

use crate::server::parts::Parts;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

use crate::cli::protocol::{is_tool_agent_id, HarnessRequest};
use sikemux_wire::protocol::AgentStatus;

use super::super::connection::blocking;
use super::super::names::Adopted;
use super::super::saved;
use super::super::spawn::{self as local, ForHost};
use super::super::workspace::LauncherId;
use super::super::worktree;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::route::pin;
use super::spawn::{self as remote, LeadParams, MAX_PROMPT_BYTES, SPAWN_WAIT};
use super::team::Foreign;
use super::{call, find, name_of, Failed};

/// The closed worker of `lead` called `wanted`, by name or agent id, that ran on a linked host.
pub(crate) fn closed_elsewhere(core: &Core, lead: &str, wanted: &str) -> Option<Foreign> {
    let wanted = wanted.trim().trim_start_matches('@');
    core.hosts().team.closed_worker(lead, wanted, None)
}

/// Reopens the worker `wanted` that closed on `host`, or on whichever host it ran on, and hands
/// it `prompt` once it is ready.
pub(crate) async fn reopen(
    core: &Arc<Core>,
    request: &HarnessRequest,
    host: Option<&str>,
    wanted: &str,
    prompt: String,
) -> Result<Value, String> {
    let lead_id = request
        .agent_id
        .clone()
        .ok_or("agent_spawn needs the calling agent's id")?;
    let wanted = wanted.trim().trim_start_matches('@');
    let link = host.map(|host| find(core, host)).transpose()?;
    let closed = core
        .hosts()
        .team
        .closed_worker(&lead_id, wanted, link.as_ref().map(|link| link.id.as_str()))
        .ok_or_else(|| match &link {
            Some(link) => format!("none of your workers that closed on {} is called {wanted}; resume with host takes the worker's name or agent id", link.name),
            None => format!("none of your workers that closed on a linked host is called {wanted}"),
        })?;
    let link = match link {
        Some(link) => link,
        None => find(core, &closed.host)?,
    };
    let reports = !matches!(request.params.get("report"), Some(Value::Bool(false)));
    let lead = {
        let (core, lead_id, project) = (core.clone(), lead_id.clone(), request.project.clone());
        blocking(move || {
            local::lead_of(&core, &lead_id, &project)
                .map(|(lead, _)| lead)
                .map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let named = core.names().adopt(
        Adopted {
            agent_id: &closed.agent_id,
            name: &closed.name,
            project: closed.project.clone(),
            parent: Some(&lead_id),
            provider: &closed.cli,
            reports,
            depth: 0,
        },
        core.open_agents(),
    );
    let id = super::call_id();
    core.hosts().team.add(Foreign {
        name: named.name.clone(),
        status: AgentStatus::Starting,
        said: None,
        active_at: Some(epoch_ms()),
        landed: None,
        pending: Some(id.clone()),
        ..closed.clone()
    });
    pin(core);
    let params = params_of::<asks::Reopen>(&ReopenParams {
        agent_id: closed.agent_id.clone(),
        name: named.name.clone(),
        lead: LeadParams {
            agent_id: lead_id.clone(),
            name: core.names().name_or_id(&lead_id),
            cli: lead.provider.clone(),
            mode: lead.permission_mode.map(|mode| mode.as_str().to_owned()),
            depth: core.names().depth(&lead_id),
        },
        reports,
        prompt,
    });
    let shown = |name: &str| format!("{name} on {}", link.name);
    match call(
        core,
        &link.id,
        asks::Reopen::METHOD,
        params,
        None,
        SPAWN_WAIT,
        &id,
    )
    .await
    {
        Ok(answer) => {
            let name = remote::started_as(core, &closed.agent_id, &named.name, &answer);
            Ok(json!({
                "name": shown(&name),
                "agentId": closed.agent_id,
                "host": link.name,
                "cli": closed.cli,
                "resumed": answer["resumed"],
                "reportsBack": reports,
                "note": format!(
                    "{} is reopening its session on {} in its checkout there, and gets your prompt once it is ready. Each time its turn ends, its new commits come back to a branch in your checkout{}.",
                    shown(&name),
                    link.name,
                    if reports { " and its final answer comes to you as a message" } else { "" },
                ),
            }))
        }
        Err(Failed::Unknown(_) | Failed::Retry(_)) => {
            remote::settle_later(core, &link.id, &closed.agent_id);
            Ok(json!({
                "name": shown(&named.name),
                "agentId": closed.agent_id,
                "host": link.name,
                "started": "not known yet",
                "note": format!("{} did not answer in time, so whether {} reopened is not known yet. Do not reopen it again: Sikemux asks {} until it knows, and tells you.", link.name, shown(&named.name), link.name),
            }))
        }
        Err(error) => {
            remote::not_started(core, &closed.agent_id);
            Err(format!(
                "{} did not reopen on {}: {error}",
                closed.name, link.name
            ))
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReopenParams {
    agent_id: String,
    name: String,
    lead: LeadParams,
    reports: bool,
    prompt: String,
}

/// A linked host asks this one to take up again the session of a worker of its agent that
/// closed here.
pub(super) async fn reopen_here(
    core: &Arc<Core>,
    host: &str,
    params: ReopenParams,
) -> Result<Value, String> {
    if !is_tool_agent_id(&params.agent_id) || !is_tool_agent_id(&params.lead.agent_id) {
        return Err("that is not an agent id".into());
    }
    if params.prompt.trim().is_empty() || params.prompt.len() > MAX_PROMPT_BYTES {
        return Err("the prompt is empty or too long".into());
    }
    if core.open_agents().contains(&params.agent_id) {
        return Err("that agent still runs here".into());
    }
    core.names().settle(core.open_agents());
    let ran = core
        .names()
        .closed_worker(&params.lead.agent_id, &params.agent_id)
        .ok_or("this host no longer keeps the session of that worker")?;
    let (saved, resumed) = {
        let (core, ran) = (core.clone(), ran.clone());
        blocking(move || {
            let saved = saved::locate(&core, &ran.session_id, Some(&ran.provider))?;
            if saved.open.is_some() {
                return Err("another agent here has its session open".into());
            }
            let resumed = worktree::agent::resumed_in(&saved.project, None)?;
            Ok((saved, resumed))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let project = core.workspaces().project_of(&resumed.cwd);
    remote::take_lead(core, host, &params.lead, &project);
    let lead = &params.lead;
    let session = saved.session_id.clone();
    let plan = ForHost {
        agent_id: params.agent_id.clone(),
        name: params.name.clone(),
        lead_id: lead.agent_id.clone(),
        lead_name: format!("{} on {}", lead.name, name_of(core, host)),
        lead_cli: if lead.cli.is_empty() {
            "agent".to_owned()
        } else {
            lead.cli.clone()
        },
        mode: lead.mode.clone(),
        provider: ran.provider.clone(),
        model: None,
        effort: None,
        reports: params.reports,
        cwd: resumed.cwd,
        prompt: params.prompt,
        launcher: (ran.profile.as_deref())
            .map(|profile| LauncherId::on(&ran.provider, profile).to_string()),
        resume: Some(saved),
    };
    let core = core.clone();
    let mut answer = blocking(move || {
        let answer = local::start_for_host(&core, plan)?;
        super::changed(&core);
        Ok(answer)
    })
    .await
    .map_err(|error| error.to_string())?;
    answer["resumed"] = session.into();
    Ok(answer)
}
