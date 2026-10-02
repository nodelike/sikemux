//! Remote access and pairing for the Devices settings page, and the projects
//! and agents the app offers paired devices. The core keeps the state; these
//! commands forward to it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::{
    BackdropImage, ChatLauncher, DeviceAccess, ProjectInfo, PublishedChat, RemoteStatus,
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

/// The chats the app last listed, sent again like the workspace.
#[derive(Default)]
pub struct PublishedChats(Mutex<Vec<PublishedChat>>);

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
}

#[tauri::command]
pub async fn remote_publish_workspace(
    app: AppHandle,
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
        };
        if let Ok(launcher) = crate::acp::launcher(&app, spec).await {
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
pub async fn remote_publish_chats(
    manager: State<'_, PtyManager>,
    published: State<'_, PublishedChats>,
    chats: Vec<PublishedChat>,
) -> AppResult<()> {
    if let Ok(mut last) = published.0.lock() {
        *last = chats.clone();
    }
    let client = manager.client().await?;
    client.publish_chats(chats).await.map_err(core_error)
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
    let chats = app
        .try_state::<PublishedChats>()
        .and_then(|published| published.0.lock().ok().map(|last| last.clone()))
        .filter(|chats| !chats.is_empty());
    if let Some(chats) = chats {
        if let Err(error) = client.publish_chats(chats).await {
            eprintln!("Sikemux could not tell its core which chats it has: {error}");
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

#[tauri::command]
pub async fn remote_set_enabled(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    enabled: bool,
) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    let status = client
        .set_remote_access(enabled)
        .await
        .map_err(core_error)?;
    login_item::sync(
        &app.config().identifier,
        manager.core_launch().as_ref(),
        status.enabled,
    );
    Ok(status)
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
pub async fn remote_open_pairing(manager: State<'_, PtyManager>) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.open_pairing().await.map_err(core_error)
}

#[tauri::command]
pub async fn remote_close_pairing(manager: State<'_, PtyManager>) -> AppResult<RemoteStatus> {
    let client = manager.client().await?;
    client.close_pairing().await.map_err(core_error)
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
