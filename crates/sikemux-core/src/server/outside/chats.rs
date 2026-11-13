//! What an outside program may do with chats, as the person would in the app: list them, read
//! one, wait for it, and with full access start one, send it a message or stop its turn.

use crate::server::device_view::DeviceViewCore;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_wire::protocol::{AgentStatus, DeviceAccess};

use super::Caller;
use crate::server::caller::text_param;
use crate::server::chat::calls::{prompt_chat, PromptChatParams};
use crate::server::connection::blocking;
use crate::server::roster::{self, Listed, Reach};
use crate::server::workspace::ChatChoice;
use crate::server::Core;

const MAX_TEXT_BYTES: usize = 100_000;
const MAX_WAIT: Duration = Duration::from_secs(50);
const WAIT_STEP: Duration = Duration::from_millis(500);

/// One tool an outside program sees.
pub(crate) struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub writes: bool,
    pub schema: fn() -> Value,
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "chat_list",
        description: "List Sikemux's projects, the agents it can start, and its chats with each one's status. Pass project to see one project's chats.",
        writes: false,
        schema: || object(json!({ "project": text("A project's name, id or folder") }), &[]),
    },
    Tool {
        name: "chat_read",
        description: "Read what a chat said: its latest turns, or what came after cursor.",
        writes: false,
        schema: || object(json!({
            "chat": text("The chat's name or agentId, from chat_list"),
            "cursor": text("A cursor an earlier chat_read gave, to read only what came after it"),
        }), &["chat"]),
    },
    Tool {
        name: "chat_wait",
        description: "Wait until a chat finishes its turn or needs the person, up to timeoutMs (at most 50000). Then read it with chat_read.",
        writes: false,
        schema: || object(json!({
            "chat": text("The chat's name or agentId"),
            "timeoutMs": { "type": "integer", "minimum": 0, "maximum": 50000 },
        }), &["chat"]),
    },
    Tool {
        name: "chat_start",
        description: "Start a new chat in a project with a first message. It runs as if the person started it in Sikemux, and shows there.",
        writes: true,
        schema: || object(json!({
            "project": text("The project's name, id or folder, from chat_list"),
            "prompt": text("The first message"),
            "agent": text("Which agent to start, such as claude or codex; the first ready one when left out"),
            "model": text("A model the agent offers, when not its default"),
        }), &["project", "prompt"]),
    },
    Tool {
        name: "chat_send",
        description: "Send a chat a message, as the person typing it. A chat in the middle of a turn gets it next.",
        writes: true,
        schema: || object(json!({
            "chat": text("The chat's name or agentId"),
            "text": text("The message"),
        }), &["chat", "text"]),
    },
    Tool {
        name: "chat_stop",
        description: "Stop the turn a chat is running, as the person pressing stop.",
        writes: true,
        schema: || object(json!({ "chat": text("The chat's name or agentId") }), &["chat"]),
    },
];

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// The tools `access` may call.
pub(crate) fn tools(access: DeviceAccess) -> impl Iterator<Item = &'static Tool> {
    TOOLS
        .iter()
        .filter(move |tool| !tool.writes || access == DeviceAccess::Full)
}

pub(crate) async fn call(
    core: &Arc<Core>,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| format!("Sikemux has no tool named {name}"))?;
    if tool.writes && caller.access != DeviceAccess::Full {
        return Err(format!(
            "{name} needs full access, and the person let {} only read; they can change that in Sikemux's settings",
            caller.name
        ));
    }
    match name {
        "chat_list" => list(core, args).await,
        "chat_read" => read(core, args).await,
        "chat_wait" => wait(core, args).await,
        "chat_start" => start(core, args).await,
        "chat_send" => send(core, args).await,
        "chat_stop" => stop(core, args).await,
        _ => Err(format!("Sikemux has no tool named {name}")),
    }
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    text_param(args, key, MAX_TEXT_BYTES)?.ok_or_else(|| format!("{key} is required"))
}

/// The project `named` means: its id, name or folder, or the folder of a project's worktree.
fn project(core: &Core, named: &str) -> Result<String, String> {
    let named = named.trim();
    let projects = core.workspace_view_standing().projects;
    if let Some(project) = projects
        .iter()
        .find(|project| project.id == named || project.name.eq_ignore_ascii_case(named))
    {
        return Ok(project.id.clone());
    }
    let path = std::fs::canonicalize(named).ok();
    path.map(|path| core.workspaces().project_of(&path))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| format!("No project in Sikemux is named {named}; chat_list lists them"))
}

fn chat(core: &Core, named: &str) -> Result<Listed, String> {
    let target = roster::resolve(core, &roster::listed(core), named, "", "")
        .map_err(|_| format!("No chat here is called {named}; chat_list lists them"))?;
    if target.reach == Reach::Remote {
        return Err("That chat runs on a linked host; reach it from that host".into());
    }
    if core.names().is_blind(&target.agent_id) {
        return Err("That agent reviews a card blind, so nothing outside may reach it".into());
    }
    Ok(target)
}

async fn list(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let named = text_param(args, "project", 4096)?.map(str::to_owned);
    let core = core.clone();
    blocking(move || {
        let only = named.map(|named| project(&core, &named)).transpose()?;
        let view = core.workspace_view_standing();
        let projects: Vec<Value> = view
            .projects
            .iter()
            .map(|project| json!({ "id": project.id, "name": project.name, "path": project.path }))
            .collect();
        let agents: Vec<Value> = view
            .launchers
            .iter()
            .map(|launcher| json!({ "agent": launcher.id, "name": launcher.label, "status": launcher.status }))
            .collect();
        Ok(json!({
            "projects": projects,
            "agents": agents,
            "chats": roster::rows(&core, only.as_deref(), ""),
        }))
    })
    .await
    .map_err(|error| error.to_string())
}

async fn read(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let named = required(args, "chat")?.to_owned();
    let cursor = text_param(args, "cursor", 200)?.map(str::to_owned);
    let core = core.clone();
    blocking(move || {
        let target = chat(&core, &named)?;
        crate::server::follow::read_target(&core, &target, cursor.as_deref()).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

fn busy(status: AgentStatus) -> bool {
    matches!(status, AgentStatus::Working | AgentStatus::Starting)
}

async fn wait(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let named = required(args, "chat")?.to_owned();
    let limit = match args.get("timeoutMs") {
        None | Some(Value::Null) => MAX_WAIT,
        Some(value) => value
            .as_u64()
            .map(Duration::from_millis)
            .filter(|limit| *limit <= MAX_WAIT)
            .ok_or("timeoutMs must be an integer from 0 to 50000")?,
    };
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let standing = {
            let (core, named) = (core.clone(), named.clone());
            blocking(move || {
                let target = chat(&core, &named)?;
                Ok((
                    target.name.clone(),
                    target.agent_id.clone(),
                    target.standing(&core),
                ))
            })
            .await
            .map_err(|error| error.to_string())?
        };
        let (name, agent_id, status) = standing;
        let done = !busy(status);
        if done || tokio::time::Instant::now() >= deadline {
            return Ok(json!({
                "chat": name,
                "agentId": agent_id,
                "status": roster::status_word(status),
                "timedOut": !done,
            }));
        }
        tokio::time::sleep(WAIT_STEP.min(deadline - tokio::time::Instant::now())).await;
    }
}

async fn start(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let prompt = required(args, "prompt")?.to_owned();
    let named = required(args, "project")?.to_owned();
    let agent = text_param(args, "agent", 64)?.map(|agent| agent.trim().to_lowercase());
    let model = text_param(args, "model", 200)?.map(|model| model.trim().to_owned());
    let choice = {
        let core = core.clone();
        blocking(move || {
            let project = project(&core, &named)?;
            let view = core.workspace_view_standing();
            let ready =
                |status: &Option<String>| status.as_deref().is_none_or(|status| status == "ready");
            let launcher = match &agent {
                Some(agent) => view.launchers.iter().find(|launcher| {
                    launcher.id.eq_ignore_ascii_case(agent)
                        || launcher.provider.eq_ignore_ascii_case(agent)
                        || launcher.label.eq_ignore_ascii_case(agent)
                }),
                None => view
                    .launchers
                    .iter()
                    .find(|launcher| ready(&launcher.status)),
            }
            .ok_or_else(|| match &agent {
                Some(agent) => format!("Sikemux has no agent named {agent}; chat_list lists them"),
                None => "No agent is ready to start in Sikemux".to_owned(),
            })?;
            Ok(ChatChoice {
                launcher: launcher.id.clone(),
                project,
                permission_mode: None,
                model,
                effort: None,
                resume_id: None,
                cwd: None,
                title: None,
                terminal: false,
            })
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let agent_id = crate::server::launching::start_waiting(core, choice)
        .await
        .map_err(|error| error.to_string())?;
    prompt_chat(core, PromptChatParams::new(agent_id.clone(), prompt))
        .await
        .map_err(|error| error.to_string())?;
    let name = core.names().get(&agent_id).map(|named| named.name);
    Ok(json!({
        "chat": name,
        "agentId": agent_id,
        "note": "The chat started and has the message. chat_wait waits for its turn to end; chat_read reads what it said.",
    }))
}

async fn send(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let text = required(args, "text")?.to_owned();
    let named = required(args, "chat")?.to_owned();
    let target = {
        let core = core.clone();
        blocking(move || chat(&core, &named).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    if target.reach == Reach::Stopped {
        return Err("That chat has stopped; start a new one".into());
    }
    prompt_chat(core, PromptChatParams::new(target.agent_id.clone(), text))
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({ "chat": target.name, "agentId": target.agent_id, "sent": true }))
}

async fn stop(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let named = required(args, "chat")?.to_owned();
    let core = core.clone();
    blocking(move || {
        let target = chat(&core, &named)?;
        crate::server::chat::cancel(&core, &target.agent_id)?;
        Ok(json!({ "chat": target.name, "agentId": target.agent_id, "stopped": true }))
    })
    .await
    .map_err(|error| error.to_string())
}
