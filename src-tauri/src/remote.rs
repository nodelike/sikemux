//! Remote access and the phones allowed in, for the Devices settings page,
//! and the projects and agents the app offers paired devices. The core keeps
//! the state; these commands forward to it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::{
    AllowedDevice, BackdropImage, ChatLauncher, DeviceAccess, ProjectInfo, PublishedChat,
    PublishedRecent, RemoteStatus,
};
use tauri::{AppHandle, Manager, State};

use crate::acp::LauncherSpec;
use crate::error::AppResult;
use crate::login_item::{self, CoreLaunch};
use crate::pty::{core_error, PtyManager};

/// What the app last published, sent again to a core it reconnects to: the
/// core keeps it in memory only.
#[derive(Default)]
pub struct PublishedWorkspace(Mutex<Option<(Vec<ProjectInfo>, Vec<ChatLauncher>)>>);

/// The backdrop the app last published, sent again like the workspace.
#[derive(Default)]
pub struct PublishedBackdrop(Mutex<Option<(bool, Option<BackdropImage>)>>);

/// The theme colours the app last published, sent again like the workspace.
#[derive(Default)]
pub struct PublishedPalette(Mutex<BTreeMap<String, String>>);

/// The chats the app lists, and the titles of its terminal agents by agent id.
type AgentList = (Vec<PublishedChat>, BTreeMap<String, String>);

/// The agents the app last listed, sent again like the workspace.
#[derive(Default)]
pub struct PublishedAgents(Mutex<Option<AgentList>>);

/// The recent chats the app last listed, sent again like the workspace.
#[derive(Default)]
pub struct PublishedRecentChats(Mutex<Option<Vec<PublishedRecent>>>);

/// The agents the app last showed, sent again like the workspace.
#[derive(Default)]
pub struct PublishedOnScreen(Mutex<Vec<String>>);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherRequest {
    id: String,
    provider: String,
    label: String,
    config_path: Option<String>,
    executable_path: Option<String>,
    #[serde(default)]
    environment_keys: Vec<String>,
    permission_mode: String,
    #[serde(default)]
    status: Option<String>,
}

#[tauri::command]
pub async fn remote_publish_workspace(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedWorkspace>,
    projects: Vec<ProjectInfo>,
    launchers: Vec<LauncherRequest>,
) -> AppResult<()> {
    let mut ready = Vec::with_capacity(launchers.len());
    for request in launchers {
        let spec = LauncherSpec {
            id: request.id,
            provider: request.provider,
            label: request.label,
            config_path: request.config_path,
            executable_path: request.executable_path,
            environment_keys: request.environment_keys,
            permission_mode: request.permission_mode,
            status: request.status,
        };
        if let Ok(launcher) = crate::acp::launcher(spec).await {
            ready.push(launcher);
        }
    }
    if let Ok(mut last) = published.0.lock() {
        *last = Some((projects.clone(), ready.clone()));
    }
    let client = manager.client().await?;
    client
        .publish_workspace(projects, ready)
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn remote_publish_backdrop(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedBackdrop>,
    texture: bool,
    image: Option<BackdropImage>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = Some((texture, image.clone()));
    }
    let client = manager.client().await?;
    client
        .publish_backdrop(texture, image)
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn remote_publish_palette(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedPalette>,
    palette: BTreeMap<String, String>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = palette.clone();
    }
    let client = manager.client().await?;
    client.publish_palette(palette).await.map_err(core_error)
}

#[tauri::command]
pub async fn remote_publish_agents(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedAgents>,
    chats: Vec<PublishedChat>,
    titles: BTreeMap<String, String>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = Some((chats.clone(), titles.clone()));
    }
    let client = manager.client().await?;
    client
        .publish_agents(chats, titles)
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn remote_publish_recent(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedRecentChats>,
    chats: Vec<PublishedRecent>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = Some(chats.clone());
    }
    let client = manager.client().await?;
    client.publish_recent(chats).await.map_err(core_error)
}

#[tauri::command]
pub async fn remote_publish_on_screen(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedOnScreen>,
    agent_ids: Vec<String>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = agent_ids.clone();
    }
    let client = manager.client().await?;
    client
        .publish_on_screen(agent_ids)
        .await
        .map_err(core_error)
}

/// Gives a core the app just connected to what it published to the last one,
/// and keeps the login item in step with its remote access switch.
pub(crate) async fn connected(
    app: &AppHandle,
    launch: Option<&CoreLaunch>,
    client: &Arc<CoreClient>,
) {
    let last = app
        .try_state::<PublishedWorkspace>()
        .and_then(|published| published.0.lock().ok().and_then(|last| last.clone()));
    if let Some((projects, launchers)) = last {
        if let Err(error) = client.publish_workspace(projects, launchers).await {
            eprintln!("Sikemux could not tell its core which agents devices may start: {error}");
        }
    }
    let palette = app
        .try_state::<PublishedPalette>()
        .and_then(|published| published.0.lock().ok().map(|last| last.clone()))
        .filter(|palette| !palette.is_empty());
    if let Some(palette) = palette {
        if let Err(error) = client.publish_palette(palette).await {
            eprintln!("Sikemux could not tell its core the theme's colours: {error}");
        }
    }
    let backdrop = app
        .try_state::<PublishedBackdrop>()
        .and_then(|published| published.0.lock().ok().and_then(|last| last.clone()));
    if let Some((texture, image)) = backdrop {
        if let Err(error) = client.publish_backdrop(texture, image).await {
            eprintln!("Sikemux could not tell its core what it draws behind panes: {error}");
        }
    }
    let agents = app
        .try_state::<PublishedAgents>()
        .and_then(|published| published.0.lock().ok().and_then(|last| last.clone()));
    if let Some((chats, titles)) = agents {
        if let Err(error) = client.publish_agents(chats, titles).await {
            eprintln!("Sikemux could not tell its core which agents it has: {error}");
        }
    }
    let recent = app
        .try_state::<PublishedRecentChats>()
        .and_then(|published| published.0.lock().ok().and_then(|last| last.clone()));
    if let Some(chats) = recent {
        if let Err(error) = client.publish_recent(chats).await {
            eprintln!("Sikemux could not tell its core which chats are recent: {error}");
        }
    }
    let on_screen = app
        .try_state::<PublishedOnScreen>()
        .and_then(|published| published.0.lock().ok().map(|last| last.clone()))
        .filter(|agent_ids| !agent_ids.is_empty());
    if let Some(agent_ids) = on_screen {
        if let Err(error) = client.publish_on_screen(agent_ids).await {
            eprintln!("Sikemux could not tell its core which agents are on screen: {error}");
        }
    }
    if let Ok(status) = client.remote_status().await {
        login_item::sync(&app.config().identifier, launch, status.enabled);
    }
}

#[tauri::command]
pub async fn remote_status(manager: State<'_, PtyManager>) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.remote_status().await.map_err(core_error)
}

/// Keeps the login item in step with remote access, which the switch turns
/// on and off, and so do signing in and out.
pub(crate) fn follow_remote_access(app: &AppHandle, enabled: bool) {
    let identifier = app.config().identifier.clone();
    let launch = app
        .try_state::<PtyManager>()
        .and_then(|manager| manager.core_launch());
    tauri::async_runtime::spawn_blocking(move || {
        login_item::sync(&identifier, launch.as_ref(), enabled);
    });
}

#[tauri::command]
pub async fn remote_set_enabled(
    manager: State<'_, PtyManager>,
    enabled: bool,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.set_remote_access(enabled).await.map_err(core_error)
}

#[tauri::command]
pub async fn remote_set_device_access(
    manager: State<'_, PtyManager>,
    id: String,
    access: DeviceAccess,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client
        .set_device_access(id, access)
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn remote_revoke_device(
    manager: State<'_, PtyManager>,
    id: String,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.revoke_device(id).await.map_err(core_error)
}

#[tauri::command]
pub async fn remote_answer_pairing(
    manager: State<'_, PtyManager>,
    id: String,
    allow: bool,
    access: DeviceAccess,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client
        .answer_pairing(id, allow, access)
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn remote_allow_devices(
    manager: State<'_, PtyManager>,
    devices: Vec<AllowedDevice>,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.allow_devices(devices).await.map_err(core_error)
}
