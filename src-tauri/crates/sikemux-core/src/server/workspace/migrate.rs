//! Carries over a workspace saved before the core kept agents: the projects
//! devices added, how sleeping chats ran, and the projects, agents, spaces and
//! pinned chats the app kept in its own state then. Agents kept before the
//! core chose buddy colours get theirs.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use super::{check_agent, dress, is_chat_provider, Kept, Saved};
use crate::protocol::{
    ChatAccount, CodeHostChoice, PinnedChat, ProjectInfo, SpaceInfo, WorkspaceAgent,
};

/// The app's data folder in this build, which is the core's when the app
/// started it, and the only core that may read the app's state.
const APP_DATA_DIR: &str = if cfg!(debug_assertions) {
    "com.nodelike.sikemux.dev"
} else {
    "com.nodelike.sikemux"
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Earlier {
    #[serde(default)]
    added: Vec<ProjectInfo>,
    chats: Option<Vec<EarlierChat>>,
    #[serde(default)]
    asleep: Vec<EarlierAsleep>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EarlierChat {
    agent_id: String,
    asleep: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EarlierAsleep {
    agent_id: String,
    resume_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    account: Option<ChatAccount>,
    #[serde(default)]
    fallbacks: Vec<ChatAccount>,
}

pub(super) fn carry_over(saved: &mut Saved, read: &Value, data_dir: Option<&Path>) {
    let earlier: Earlier = serde_json::from_value(read.clone()).unwrap_or_default();
    let app = data_dir
        .filter(|dir| dir.file_name() == Some(APP_DATA_DIR.as_ref()))
        .and_then(|_| app_state());
    if let Some(app) = &app {
        from_app(saved, app, &earlier);
    }
    for project in earlier.added {
        if !saved
            .projects
            .iter()
            .any(|known| known.path == project.path)
        {
            saved.projects.push(project);
        }
    }
}

/// Gives every agent kept before the core chose buddy colours one of its own.
pub(super) fn dress_agents(saved: &mut Saved) {
    for index in 0..saved.agents.len() {
        let mut agent = saved.agents[index].agent.clone();
        dress(&mut agent, &saved.agents);
        saved.agents[index].agent = agent;
    }
}

/// What the app last saved of its workspace.
fn app_state() -> Option<Value> {
    let path = sikemux_process::paths::config_file("state", "sqlite3")?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let json: String = connection
        .query_row(
            "SELECT core_json FROM workspace_snapshots WHERE slot = 0",
            [],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&json).ok()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn from_app(saved: &mut Saved, app: &Value, earlier: &Earlier) {
    let prefs = &app["prefs"];
    saved.spaces = prefs["spaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|space| {
            Some(SpaceInfo {
                id: text(space, "id")?,
                name: text(space, "name").unwrap_or_default(),
                icon: text(space, "icon").unwrap_or_default(),
            })
        })
        .collect();
    let space_of = |cwd: &str| {
        prefs["projectSpaces"]
            .get(cwd)
            .and_then(Value::as_str)
            .filter(|space| saved.spaces.iter().any(|known| known.id == *space))
            .map(str::to_owned)
    };
    let code_hosts_of = |cwd: &str| -> BTreeMap<String, CodeHostChoice> {
        prefs["pluginSettings"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(plugin, settings)| {
                let repo = text(&settings["repoByProject"], cwd);
                let account = text(&settings["accountByProject"], cwd);
                (repo.is_some() || account.is_some())
                    .then(|| (plugin.clone(), CodeHostChoice { repo, account }))
            })
            .collect()
    };
    let sessions = app["sessions"].as_array().cloned().unwrap_or_default();
    let projects: Vec<ProjectInfo> = sessions
        .iter()
        .filter(|session| session["kind"] == "project")
        .filter_map(|session| {
            let cwd = text(session, "cwd")?;
            Some(ProjectInfo {
                id: text(session, "id")?,
                name: text(session, "name").unwrap_or_else(|| cwd.clone()),
                space: space_of(&cwd),
                code_hosts: code_hosts_of(&cwd),
                path: cwd.into(),
                pinned: session["pinned"].as_bool().unwrap_or(false),
                identity: None,
            })
        })
        .filter(|project| project.path.is_absolute())
        .collect();
    let earlier_projects = std::mem::replace(&mut saved.projects, projects);
    for project in earlier_projects {
        if !saved
            .projects
            .iter()
            .any(|known| known.path == project.path)
        {
            saved.projects.push(project);
        }
    }

    let mut project_of_agent = HashMap::new();
    for (session, windows) in app["windowsBySession"].as_object().into_iter().flatten() {
        for window in windows.as_array().into_iter().flatten() {
            let mut pending = vec![&window["root"]];
            while let Some(node) = pending.pop() {
                if let Some(children) = node["children"].as_array() {
                    pending.extend(children);
                } else if node["kind"] == "agent" {
                    if let Some(id) = node["id"].as_str() {
                        project_of_agent.insert(id.to_owned(), session.clone());
                    }
                }
            }
        }
    }

    for agent in app["agents"].as_array().into_iter().flatten() {
        let Some(id) = text(agent, "id") else {
            continue;
        };
        let Some(project) = project_of_agent
            .get(&id)
            .and_then(|session| saved.project(session))
        else {
            continue;
        };
        let provider = text(agent, "type").unwrap_or_default();
        let permission_mode = text(agent, "permissionMode").unwrap_or_else(|| {
            match agent["skipPermissions"].as_bool() {
                Some(true) => "bypass".into(),
                _ => "workspace-write".into(),
            }
        });
        let pty_id = agent["ptyId"].as_u64();
        let asleep = earlier
            .chats
            .iter()
            .flatten()
            .find(|chat| chat.agent_id == id)
            .map(|chat| chat.asleep)
            .unwrap_or(is_chat_provider(&provider) && pty_id.is_none());
        let stopped = earlier.asleep.iter().find(|asleep| asleep.agent_id == id);
        let mut record = WorkspaceAgent {
            cwd: text(agent, "cwd")
                .map(Into::into)
                .unwrap_or_else(|| project.path.clone()),
            project: project.id.clone(),
            title: text(agent, "title").unwrap_or_else(|| provider.clone()),
            renamed: agent["renamed"].as_bool().unwrap_or(false),
            lead: text(agent, "lead"),
            resume_id: text(agent, "resumeId"),
            permission_mode,
            profile_id: text(agent, "profileId"),
            model: text(agent, "model"),
            model_provider: text(agent, "modelProvider"),
            effort: text(agent, "effort"),
            keep_alive: agent["keepAlive"].as_bool().unwrap_or(false),
            asleep,
            worktree: agent
                .get("worktree")
                .filter(|tree| tree.is_object())
                .cloned(),
            buddy: agent
                .get("buddy")
                .filter(|buddy| buddy.is_object())
                .cloned(),
            pty_id,
            created_at: 0,
            provider,
            id,
        };
        if let Some(stopped) = stopped {
            record.resume_id = record.resume_id.or(stopped.resume_id.clone());
            record.model = record.model.or(stopped.model.clone());
            record.effort = record.effort.or(stopped.effort.clone());
        }
        if check_agent(&record, saved).is_ok() {
            saved.agents.push(Kept {
                agent: record,
                account: stopped.and_then(|stopped| stopped.account.clone()),
                fallbacks: stopped
                    .map(|stopped| stopped.fallbacks.clone())
                    .unwrap_or_default(),
            });
        }
    }

    saved.pinned = prefs["pinnedChats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|chat| {
            Some(PinnedChat {
                provider: text(chat, "agent")?,
                session_id: text(chat, "id")?,
                title: text(chat, "title").unwrap_or_default(),
                project: text(chat, "project")?.into(),
            })
        })
        .collect();
    if saved.permission_mode.is_none() {
        saved.permission_mode = text(prefs, "defaultAgentPermissionMode");
    }
}
