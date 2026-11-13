//! A new chat moved to another host of the account before its first message. That host starts
//! the same agent, on its own sign-in, in its copy of the project, with the draft, model and
//! effort the chat had, and this host closes the chat.

use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::hosts::{ChatMoved, MoveChatToHostParams};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_view::chat::{chat_draft_key, DraftExtra};
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_wire::protocol::PermissionMode;

use super::super::drafts::{self, SetDraftParams};
use super::super::{launching, layout, workspace, Core, CoreError, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::transfer::call_once;
use super::{find, Failed};

const TAKE_WAIT: Duration = Duration::from_secs(30);

#[derive(Deserialize, Serialize)]
pub(super) struct TakeChat {
    provider: String,
    project: String,
    permission_mode: PermissionMode,
    model: Option<String>,
    effort: Option<String>,
    title: Option<String>,
    text: String,
    extra: Option<DraftExtra>,
}

pub(crate) async fn move_chat(
    core: &Arc<Core>,
    params: MoveChatToHostParams,
) -> CoreResult<ChatMoved> {
    let agent = core.workspaces().agent_or_gone(&params.agent_id)?;
    if agent.terminal || agent.resume_id.is_some() || agent.worktree.is_some() {
        return Err("only a new chat moves to another host, before its first message".into());
    }
    let link = find(core, &params.host)?;
    let draft = core.drafts().chat_draft(&agent.id);
    let attached = (draft.as_ref())
        .and_then(|draft| draft.extra.as_ref())
        .is_some_and(|extra| !extra.attachments.is_empty());
    if attached {
        return Err(format!(
            "its draft has attached files, which stay on this host; take them off, move the chat, then attach them on {}",
            link.name
        )
        .into());
    }
    let taking = TakeChat {
        provider: agent.provider.clone(),
        project: params.project,
        permission_mode: agent.permission_mode,
        model: agent.model.clone(),
        effort: agent.effort.clone(),
        title: agent.renamed.then(|| agent.title.clone()),
        text: draft
            .as_ref()
            .map(|draft| draft.text.clone())
            .unwrap_or_default(),
        extra: draft.and_then(|draft| draft.extra),
    };
    let params = params_of::<asks::TakeChat>(&taking);
    let method = asks::TakeChat::METHOD;
    let answer = call_once(core, &link.id, method, params, None, TAKE_WAIT)
        .await
        .map_err(|error| match error {
            Failed::Unknown(why) => CoreError::from(format!(
                "{} did not say whether it started the chat ({why}); look there before moving it again",
                link.name
            )),
            error => CoreError::from(error.to_string()),
        })?;
    let agent_id = answer["agentId"]
        .as_str()
        .ok_or_else(|| CoreError::from(format!("{} did not start the chat", link.name)))?
        .to_owned();
    workspace::close_agent(core, agent.id)?;
    Ok(ChatMoved { agent_id })
}

/// Starts the chat another host hands over, in front in its project, and answers with its id.
pub(super) async fn take_here(core: &Arc<Core>, taking: TakeChat) -> Answer {
    let started = start_taken(core, taking).await;
    started
        .map(|agent_id| json!({ "agentId": agent_id }))
        .map_err(|error| error.to_string())
}

async fn start_taken(core: &Arc<Core>, taking: TakeChat) -> CoreResult<String> {
    let launchers = core.workspaces().view().launchers;
    let ids = launchers
        .iter()
        .map(|launcher| (launcher.id.as_str(), launcher.provider.as_str()));
    let accounts = core.workspaces().accounts();
    let launcher = workspace::launcher_for(ids, accounts.as_ref(), &taking.provider)
        .ok_or_else(|| CoreError::from(format!("{} is not set up here", taking.provider)))?;
    let choice = workspace::ChatChoice {
        launcher,
        project: taking.project.clone(),
        permission_mode: Some(taking.permission_mode),
        model: taking.model,
        effort: taking.effort,
        resume_id: None,
        cwd: None,
        title: taking.title,
        terminal: false,
    };
    let agent_id = launching::start_waiting(core, choice).await?;
    if !taking.text.is_empty() || taking.extra.is_some() {
        drafts::set(
            core,
            SetDraftParams {
                key: chat_draft_key(&agent_id),
                text: taking.text,
                extra: taking.extra,
                rev: None,
            },
        )?;
    }
    layout::open_tab(
        core,
        OpenTab::new(At::project(&taking.project), PaneKind::Agent, &agent_id),
    )?;
    Ok(agent_id)
}
