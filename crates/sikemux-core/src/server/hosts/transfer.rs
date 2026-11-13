//! A project's session sent to another host of the account. The project opens there as Open on
//! does, with the uncommitted changes laid over it when the person asks; each chat whose agent
//! keeps it in files is filed in that host's own store and added stopped, to start on that
//! host's sign-in when the person next sends to it; and the project's tabs are laid out again.
//! A copy leaves everything here. A move then closes here each chat the other host took and
//! removes its files, but only when every one of them is idle and unchanged since it went;
//! otherwise none closes here and the other host drops the ones it took, so no chat is left
//! resumable on both. A move whose uncommitted changes did not go sends no chat.

use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::hosts::{Left, SendParams, SessionSent};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map};

use super::super::connection::blocking;
use super::super::{layout, workspace, Core, CoreError, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::code::Scratch;
use super::link::{Answer, Received};
use super::open::{self, OpenAs};
use super::{ask, call, call_id, find, receipt, Failed, Stands, ASK_WAIT};
use crate::agent_catalog::{self, AgentKind};
use crate::sessions::{carry, delete, SavedRef};
use sikemux_view::layout::Tab;
use sikemux_view::workspace::{AgentBuddy, WorkspaceAgent};

/// Long enough to send a project's chats over a slow link.
const TAKE_WAIT: Duration = Duration::from_secs(10 * 60);

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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Take {
    project: String,
    from: String,
    /// The folder the chats go on in there, when not the project's own: a worktree of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work: Option<String>,
    chats: Vec<Carried>,
    tabs: Vec<Tab>,
}

/// Sends the project's session to the linked host `host`, answering what went, what stayed and
/// where it now is.
pub(crate) async fn send_session(core: &Arc<Core>, params: SendParams) -> CoreResult<SessionSent> {
    let link = find(core, &params.host)?;
    let project = core
        .workspaces()
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
        if let Some(busy) = agents.iter().find(|agent| at_work(core, &agent.id)) {
            return Err(format!(
                "{} is at work; move the session once its turn ends",
                busy.title
            )
            .into());
        }
    }
    let how = match params.worktree {
        true => OpenAs::Worktree,
        false => OpenAs::Checkout,
    };
    let opened = open::open_there(
        core,
        &link,
        project.path.clone(),
        params.uncommitted,
        how,
        params.there.clone(),
    )
    .await?;
    if let Some(why) = opened["differs"].as_str() {
        return Ok(SessionSent {
            host: link.id.clone(),
            host_name: link.name.clone(),
            project: String::new(),
            path: None,
            uncommitted: None,
            carried: Vec::new(),
            closed: Vec::new(),
            left: Vec::new(),
            differs: Some(why.to_owned()),
        });
    }
    let there = opened["project"]
        .as_str()
        .ok_or_else(|| CoreError::from(format!("{} did not open the project", link.name)))?
        .to_owned();
    let failed = (opened["uncommitted"].as_str()).filter(|said| *said != "applied");
    if let (true, Some(why)) = (params.moving, failed) {
        return Err(format!(
            "nothing moved, since the uncommitted changes did not go to {}: {why}",
            link.name
        )
        .into());
    }
    let (chats, sent, body, mut left) = {
        let core = core.clone();
        blocking(move || Ok(gather(&core, agents))).await?
    };
    let tabs = layout::view(core, &core.workspace_view(), &core.session_infos(), None)
        .projects
        .remove(&project.id)
        .map(|strip| strip.tabs)
        .unwrap_or_default();
    let request = params_of::<asks::TakeSession>(&Take {
        project: there.clone(),
        from,
        work: (opened["work"].as_str())
            .filter(|work| Some(*work) != opened["path"].as_str())
            .map(str::to_owned),
        chats,
        tabs,
    });
    let method = asks::TakeSession::METHOD;
    let answer = call_once(
        core,
        &link.id,
        method,
        request,
        Some(body.path()),
        TAKE_WAIT,
    )
    .await
    .map_err(|error| match error {
        Failed::Unknown(why) => format!(
            "{} did not say whether it took the chats ({why}); nothing here was closed",
            link.name
        ),
        error => error.to_string(),
    })?;
    let taken: HashMap<String, String> =
        serde_json::from_value(answer["agents"].clone()).unwrap_or_default();
    let (went, stayed): (Vec<Sent>, Vec<Sent>) = sent
        .into_iter()
        .partition(|chat| taken.contains_key(&chat.id));
    left.extend(stayed.into_iter().map(|chat| Left {
        title: chat.title,
        why: format!("{} could not take it", link.name),
    }));
    let (mut carried, mut closed) = (Vec::new(), Vec::new());
    if !params.moving || went.is_empty() {
        carried.extend(went.into_iter().map(|chat| chat.title));
    } else {
        let stirred: Vec<String> = {
            let (core, went) = (core.clone(), went.clone());
            blocking(move || {
                Ok((went.into_iter())
                    .filter(|chat| !chat.still(&core))
                    .map(|chat| chat.title)
                    .collect())
            })
            .await?
        };
        if stirred.is_empty() {
            for chat in went {
                match close_here(core, &chat).await {
                    Ok(()) => closed.push(chat.title.clone()),
                    Err(why) => left.push(Left {
                        title: chat.title.clone(),
                        why: format!("it is on both hosts now, as it did not close here: {why}"),
                    }),
                }
                carried.push(chat.title);
            }
        } else {
            let there: Vec<String> = (went.iter())
                .filter_map(|chat| taken.get(&chat.id).cloned())
                .collect();
            let dropped = ask(
                core,
                &link.id,
                asks::UntakeSession::METHOD,
                params_of::<asks::UntakeSession>(&Untake { agents: there }),
                None,
                ASK_WAIT,
            )
            .await;
            for chat in went {
                let why = match &dropped {
                    Err(error) => {
                        carried.push(chat.title.clone());
                        format!("it is on both hosts now, as {} kept it: {error}", link.name)
                    }
                    Ok(_) if stirred.contains(&chat.title) => {
                        "it went on here while it was on its way".to_owned()
                    }
                    Ok(_) => format!(
                        "{} went on here while it was on its way",
                        stirred.join(", ")
                    ),
                };
                left.push(Left {
                    title: chat.title,
                    why,
                });
            }
        }
    }
    Ok(SessionSent {
        host: link.id.clone(),
        host_name: link.name.clone(),
        project: there,
        path: opened["path"].as_str().map(str::to_owned),
        uncommitted: opened["uncommitted"].as_str().map(str::to_owned),
        carried,
        closed,
        left,
        differs: None,
    })
}

/// Makes the call `method` on `host` once and waits for it as long as that host is at it:
/// when the answer is lost on the way, it asks how the call stands, waiting while it runs, and
/// asks again with the same id when that host never heard of it, which never does it twice.
pub(super) async fn call_once(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: serde_json::Value,
    body: Option<&std::path::Path>,
    limit: Duration,
) -> Result<serde_json::Value, Failed> {
    let id = call_id();
    let mut tries = 0;
    loop {
        let why = match call(core, host, method, params.clone(), body, limit, &id).await {
            Err(Failed::Unknown(why)) => why,
            done => return done,
        };
        loop {
            match receipt(core, host, &id).await {
                Ok(Stands::Running) => tokio::time::sleep(STANDS_EVERY).await,
                Ok(Stands::Done(answer)) => return answer.map_err(Failed::Refused),
                Ok(Stands::Unknown) | Err(Failed::Refused(_)) => break,
                Err(_) => return Err(Failed::Unknown(why)),
            }
        }
        tries += 1;
        if tries > 2 {
            return Err(Failed::Unknown(why));
        }
    }
}

/// How often a call whose answer was lost is asked after while the other host is at it.
const STANDS_EVERY: Duration = Duration::from_secs(15);

/// The chats of `agents` that can go, their files written one after another into a scratch
/// file to send as the body, and the rest with why they stay.
fn gather(
    core: &Core,
    agents: Vec<WorkspaceAgent>,
) -> (Vec<Carried>, Vec<Sent>, Scratch, Vec<Left>) {
    let body = Scratch::new("session");
    let mut out = std::fs::File::create(body.path()).ok();
    let (mut chats, mut sent, mut left) = (Vec::new(), Vec::new(), Vec::new());
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
        let config = agent.profile_id.as_deref().and_then(|id| {
            core.workspaces()
                .accounts()?
                .profile(id)?
                .config_path
                .clone()
        });
        let cwd = agent.cwd.to_string_lossy().into_owned();
        let mut prints = Vec::new();
        let written = carry::gather(kind, &cwd, &resume, config.as_deref()).and_then(|files| {
            let out = out.as_mut().ok_or("could not make the file to send")?;
            files
                .into_iter()
                .map(|file| {
                    let print = Print::of(&file.path);
                    let bytes = std::fs::read(&file.path).map_err(|error| error.to_string())?;
                    out.write_all(&bytes).map_err(|error| error.to_string())?;
                    prints.push(print);
                    Ok((file.name, bytes.len() as u64))
                })
                .collect::<Result<Vec<_>, String>>()
        });
        match written {
            Ok(files) => {
                sent.push(Sent {
                    id: agent.id.clone(),
                    title: agent.title.clone(),
                    kind,
                    cwd: cwd.clone(),
                    resume: resume.clone(),
                    config: config.clone(),
                    prints,
                });
                chats.push(Carried {
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
                })
            }
            Err(error) => left.push(stay(error)),
        }
    }
    (chats, sent, body, left)
}

/// A saved file as it was when its chat was gathered: where, how long, when last written.
#[derive(Clone, PartialEq, Eq)]
struct Print(
    std::path::PathBuf,
    Option<u64>,
    Option<std::time::SystemTime>,
);

impl Print {
    fn of(path: &std::path::Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self(
            path.to_owned(),
            meta.as_ref().map(|meta| meta.len()),
            meta.and_then(|meta| meta.modified().ok()),
        )
    }
}

/// A chat that went: here by its id, with its saved session and its files as they went.
#[derive(Clone)]
struct Sent {
    id: String,
    title: String,
    kind: AgentKind,
    cwd: String,
    resume: String,
    config: Option<String>,
    prints: Vec<Print>,
}

impl Sent {
    /// Whether the chat is idle here and its saved files are as they went.
    fn still(&self, core: &Core) -> bool {
        !at_work(core, &self.id)
            && carry::gather(self.kind, &self.cwd, &self.resume, self.config.as_deref()).is_ok_and(
                |files| {
                    files
                        .iter()
                        .map(|file| Print::of(&file.path))
                        .collect::<Vec<_>>()
                        == self.prints
                },
            )
    }
}

/// Whether the agent `id` is in a turn, in its chat or its terminal.
fn at_work(core: &Core, id: &str) -> bool {
    core.chats().at_work(id) || core.agent_now(id).mid_turn()
}

/// Closes the chat `chat` here and removes its saved files, now that another host holds it.
async fn close_here(core: &Arc<Core>, chat: &Sent) -> Result<(), String> {
    if core.workspaces().agent(&chat.id).is_none() {
        return Ok(());
    }
    if !core.chats().stop_if_idle(&chat.id) {
        return Err("it began a turn".into());
    }
    workspace::close_agent(core, chat.id.clone()).map_err(|error| error.to_string())?;
    let saved = SavedRef {
        agent: chat.kind,
        cwd: chat.cwd.clone(),
        session_id: chat.resume.clone(),
        executable_path: None,
        config_path: chat.config.clone(),
    };
    delete::delete(saved).await
}

#[derive(Serialize, Deserialize)]
pub(super) struct Untake {
    /// The chats this host took, by their ids here.
    agents: Vec<String>,
}

/// What taking a chat here made: the session files that were not here before, and whether
/// its agent is new rather than one this host already had for it.
struct Taken {
    made: Vec<std::path::PathBuf>,
    new: bool,
}

/// The chats taken here, by their agents' ids, until a move that sent them finishes or drops
/// them.
fn taken() -> std::sync::MutexGuard<'static, HashMap<String, Taken>> {
    static TAKEN: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Taken>>> =
        std::sync::OnceLock::new();
    let taken = TAKEN.get_or_init(Default::default);
    taken
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remove_files(files: &[std::path::PathBuf]) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
}

/// The host that sent a session asks this one to drop the chats it took of it, since the move
/// did not finish there: a chat made by the take is closed, and only the files the take made are
/// removed, so a chat this host already had keeps its own.
pub(super) async fn untake_here(core: &Arc<Core>, untake: Untake) -> Answer {
    let mut kept = Vec::new();
    for id in untake.agents {
        let Some(taken) = taken().remove(&id) else {
            continue;
        };
        if taken.new {
            let Some(agent) = core.workspaces().agent(&id) else {
                remove_files(&taken.made);
                continue;
            };
            if !core.chats().stop_if_idle(&id) || workspace::close_agent(core, id).is_err() {
                kept.push(agent.title);
                continue;
            }
        }
        remove_files(&taken.made);
    }
    match kept.is_empty() {
        true => Ok(serde_json::Value::Null),
        false => Err(format!("{} began a turn here", kept.join(", "))),
    }
}

/// A linked host sends this one a project's session: its chats' files in `body`, filed here
/// and added stopped, and its tabs, laid out again.
pub(super) async fn take_here(core: &Arc<Core>, params: Take, body: Option<Received>) -> Answer {
    let take = params;
    let project = core
        .workspaces()
        .project_info(&take.project)
        .ok_or("that project is not on this host")?;
    let to = match take.work {
        Some(work) => work,
        None => project.path.to_string_lossy().into_owned(),
    };
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
        let Ok(made) = filed else {
            continue;
        };
        let held = (core.workspace_view().agents.into_iter()).find(|agent| {
            agent.project == project.id
                && agent.provider == chat.provider
                && agent.resume_id.as_deref() == Some(chat.resume_id.as_str())
        });
        if let Some(held) = held {
            taken().insert(held.id.clone(), Taken { made, new: false });
            agents.insert(chat.id, held.id);
            continue;
        }
        let mut draft = Map::new();
        draft.insert("provider".into(), chat.provider.into());
        draft.insert("project".into(), project.id.clone().into());
        draft.insert("cwd".into(), to.clone().into());
        draft.insert("title".into(), chat.title.into());
        draft.insert("renamed".into(), chat.renamed.into());
        draft.insert("resumeId".into(), chat.resume_id.into());
        draft.insert("permissionMode".into(), mode.as_str().into());
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
        match core.workspaces().put_agent_draft(draft) {
            Ok(agent) => {
                taken().insert(agent.id.clone(), Taken { made, new: true });
                agents.insert(chat.id, agent.id);
            }
            Err(_) => remove_files(&made),
        }
    }
    core.changed(What::View);
    let (laid, wanted) = layout::lay_out_carried(core, &project.id, &take.from, take.tabs, &agents)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({ "agents": agents, "tabs": laid, "wanted": wanted }))
}
