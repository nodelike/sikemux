//! A project's session sent to another host of the account. The project opens there as Open on
//! does, with the uncommitted changes laid over it when the person asks; each chat whose agent
//! keeps it in files is filed in that host's own store and added stopped, to start on that
//! host's sign-in when the person next sends to it; and the project's tabs are laid out again.
//! A copy leaves everything here. A move then closes the chats here and removes their files,
//! but only once the other host has taken every chat and tab; anything short of that leaves
//! this host as it was.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::super::agent::AgentActivity;
use super::super::connection::blocking;
use super::super::{layout, workspace, Core, CoreError, CoreResult};
use super::code::Scratch;
use super::link::{Answer, Received};
use super::{ask, find, open};
use crate::agent_catalog::{self, AgentKind};
use crate::protocol::{AgentBuddy, Tab, WorkspaceAgent};
use crate::sessions::{carry, delete};

/// Long enough to send a project's chats over a slow link.
const TAKE_WAIT: Duration = Duration::from_secs(10 * 60);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendParams {
    /// The project, by id.
    project: String,
    host: String,
    #[serde(default)]
    uncommitted: bool,
    /// Moves rather than copies.
    #[serde(default, rename = "move")]
    moving: bool,
}

/// A chat on its way, as the other host is told it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Carried {
    id: String,
    provider: String,
    title: String,
    renamed: bool,
    /// The folder it worked in there, the project's or a worktree's of it.
    cwd: String,
    resume_id: String,
    model: Option<String>,
    effort: Option<String>,
    buddy: Option<AgentBuddy>,
    /// Its files, in the order the body holds them, with their sizes.
    files: Vec<(String, u64)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Take {
    project: String,
    from: String,
    chats: Vec<Carried>,
    tabs: Vec<Tab>,
}

/// What stays behind, and why.
#[derive(Serialize)]
struct Left {
    title: String,
    why: String,
}

/// Sends the project's session to the linked host `host`, answering what went, what stayed and
/// where it now is.
pub(crate) async fn send_session(core: &Arc<Core>, params: SendParams) -> CoreResult<Value> {
    let link = find(core, &params.host)?;
    let project = core
        .workspaces
        .project_info(&params.project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))?;
    let from = project.path.to_string_lossy().into_owned();
    let agents: Vec<WorkspaceAgent> = core
        .workspace_view()
        .agents
        .into_iter()
        .filter(|agent| agent.project == project.id)
        .collect();
    if params.moving {
        if let Some(busy) = agents.iter().find(|agent| at_work(core, agent)) {
            return Err(format!(
                "{} is at work; move the session once its turn ends",
                busy.title
            )
            .into());
        }
    }
    let opened = open::open_there(core, &link, project.path.clone(), params.uncommitted).await?;
    let there = opened["project"]
        .as_str()
        .ok_or_else(|| CoreError::from(format!("{} did not open the project", link.name)))?
        .to_owned();
    let (chats, body, mut left) = {
        let core = core.clone();
        blocking(move || Ok(gather(&core, agents))).await?
    };
    let tabs = layout::view(core, &core.workspace_view(), &core.session_infos(), None)
        .projects
        .remove(&project.id)
        .map(|strip| strip.tabs)
        .unwrap_or_default();
    let sent: Vec<Sent> = chats
        .iter()
        .map(|chat| Sent {
            id: chat.id.clone(),
            title: chat.title.clone(),
            cwd: chat.cwd.clone(),
            resume: chat.resume_id.clone(),
        })
        .collect();
    let request = json!({ "project": there, "from": from, "chats": chats, "tabs": tabs });
    let answer = ask(
        core,
        &link.id,
        "takeSession",
        request,
        Some(body.path()),
        TAKE_WAIT,
    )
    .await
    .map_err(|error| error.to_string())?;
    let taken: HashMap<String, String> =
        serde_json::from_value(answer["agents"].clone()).unwrap_or_default();
    for chat in &sent {
        if !taken.contains_key(&chat.id) {
            left.push(Left {
                title: chat.title.clone(),
                why: format!("{} could not take it", link.name),
            });
        }
    }
    let laid_over = !params.uncommitted || opened["uncommitted"] == "applied";
    let whole = taken.len() == sent.len() && answer["tabs"] == answer["wanted"] && laid_over;
    let moved = params.moving && whole && close_here(core, &sent).await;
    Ok(json!({
        "host": link.id,
        "hostName": link.name,
        "project": there,
        "path": opened["path"],
        "uncommitted": opened["uncommitted"],
        "carried": sent
            .iter()
            .filter(|chat| taken.contains_key(&chat.id))
            .map(|chat| &chat.title)
            .collect::<Vec<_>>(),
        "left": left,
        "moved": moved,
    }))
}

/// The chats of `agents` that can go, their files written one after another into a scratch
/// file to send as the body, and the rest with why they stay.
fn gather(core: &Core, agents: Vec<WorkspaceAgent>) -> (Vec<Carried>, Scratch, Vec<Left>) {
    let body = Scratch::new("session");
    let mut out = std::fs::File::create(body.path()).ok();
    let (mut chats, mut left) = (Vec::new(), Vec::new());
    for agent in agents {
        let stay = |why: String| Left {
            title: agent.title.clone(),
            why,
        };
        let Some(resume) = agent.resume_id.clone() else {
            left.push(stay("it never started".into()));
            continue;
        };
        let entry = agent_catalog::agent(&agent.provider);
        let Some(kind) = AgentKind::from_name(&agent.provider)
            .filter(|_| entry.is_some_and(|entry| entry.relocatable_sessions))
        else {
            let label = entry.map_or(agent.provider.as_str(), |entry| entry.label);
            left.push(stay(format!("{label} keeps its chats where it runs")));
            continue;
        };
        let config = agent
            .profile_id
            .as_deref()
            .and_then(|id| core.workspaces.accounts()?.profile(id)?.config_path.clone());
        let cwd = agent.cwd.to_string_lossy().into_owned();
        let written = carry::gather(kind, &cwd, &resume, config.as_deref()).and_then(|files| {
            let out = out.as_mut().ok_or("could not make the file to send")?;
            files
                .into_iter()
                .map(|file| {
                    let bytes = std::fs::read(&file.path).map_err(|error| error.to_string())?;
                    out.write_all(&bytes).map_err(|error| error.to_string())?;
                    Ok((file.name, bytes.len() as u64))
                })
                .collect::<Result<Vec<_>, String>>()
        });
        match written {
            Ok(files) => chats.push(Carried {
                id: agent.id,
                provider: agent.provider,
                title: agent.title,
                renamed: agent.renamed,
                cwd,
                resume_id: resume,
                model: agent.model,
                effort: agent.effort,
                buddy: agent.buddy,
                files,
            }),
            Err(error) => left.push(stay(error)),
        }
    }
    (chats, body, left)
}

/// A chat that went: here by its id, with its folder and saved session.
struct Sent {
    id: String,
    title: String,
    cwd: String,
    resume: String,
}

/// Whether `agent` is in a turn, in its chat or its terminal.
fn at_work(core: &Core, agent: &WorkspaceAgent) -> bool {
    let terminal = agent
        .pty_id
        .and_then(|pty| core.session(pty))
        .is_some_and(|session| session.agent.as_ref().is_some_and(AgentActivity::busy));
    core.chats.at_work(&agent.id) || terminal
}

/// Closes the chats `sent` here and removes their saved files, now that another host holds
/// them, answering whether every one went.
async fn close_here(core: &Arc<Core>, sent: &[Sent]) -> bool {
    let mut all = true;
    for chat in sent {
        let Some(agent) = core.workspaces.agent(&chat.id) else {
            continue;
        };
        if !core.chats.stop_if_idle(&chat.id) {
            all = false;
            continue;
        }
        let config = agent.profile_id.as_deref().and_then(|profile| {
            core.workspaces
                .accounts()?
                .profile(profile)?
                .config_path
                .clone()
        });
        if workspace::close_agent(core, chat.id.clone()).is_err() {
            all = false;
            continue;
        }
        let Some(kind) = AgentKind::from_name(&agent.provider) else {
            continue;
        };
        if delete::delete(kind, chat.cwd.clone(), chat.resume.clone(), None, config)
            .await
            .is_err()
        {
            all = false;
        }
    }
    all
}

/// A linked host sends this one a project's session: its chats' files in `body`, filed here
/// and added stopped, and its tabs, laid out again.
pub(super) async fn take_here(core: &Arc<Core>, params: Value, body: Option<Received>) -> Answer {
    let take: Take =
        serde_json::from_value(params).map_err(|error| format!("unreadable session: {error}"))?;
    let project = core
        .workspaces
        .project_info(&take.project)
        .ok_or("that project is not on this host")?;
    let to = project.path.to_string_lossy().into_owned();
    let mode = core.workspace_view().permission_mode;
    let mut reader = match &body {
        Some(body) => Some(std::fs::File::open(body.path()).map_err(|error| error.to_string())?),
        None => None,
    };
    let mut agents = HashMap::new();
    for chat in take.chats {
        let mut files = Vec::with_capacity(chat.files.len());
        for (name, size) in &chat.files {
            let reader = reader
                .as_mut()
                .ok_or("the session came without its files")?;
            let mut bytes = vec![0; usize::try_from(*size).map_err(|error| error.to_string())?];
            reader
                .read_exact(&mut bytes)
                .map_err(|error| format!("the session's files came short: {error}"))?;
            files.push((name.clone(), bytes));
        }
        let Some(kind) = AgentKind::from_name(&chat.provider) else {
            continue;
        };
        let (from, into) = (chat.cwd.clone(), to.clone());
        let filed =
            blocking(move || carry::file(kind, &from, &into, files).map_err(Into::into)).await;
        if filed.is_err() {
            continue;
        }
        let mut draft = Map::new();
        draft.insert("provider".into(), chat.provider.into());
        draft.insert("project".into(), project.id.clone().into());
        draft.insert("cwd".into(), to.clone().into());
        draft.insert("title".into(), chat.title.into());
        draft.insert("renamed".into(), chat.renamed.into());
        draft.insert("resumeId".into(), chat.resume_id.into());
        draft.insert("permissionMode".into(), mode.clone().into());
        draft.insert("asleep".into(), true.into());
        for (key, value) in [("model", chat.model), ("effort", chat.effort)] {
            if let Some(value) = value {
                draft.insert(key.into(), value.into());
            }
        }
        if let Some(buddy) = chat
            .buddy
            .and_then(|buddy| serde_json::to_value(buddy).ok())
        {
            draft.insert("buddy".into(), buddy);
        }
        if let Ok(made) = core.workspaces.put_agent(draft) {
            agents.insert(chat.id, made.id);
        }
    }
    core.view_changed();
    let (laid, wanted) = layout::lay_out_carried(core, &project.id, &take.from, take.tabs, &agents)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({ "agents": agents, "tabs": laid, "wanted": wanted }))
}
