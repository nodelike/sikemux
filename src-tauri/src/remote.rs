//! Remote access and the phones allowed in, for the Devices settings page,
//! and how the app looks to paired devices. The core keeps the state; these
//! commands forward to it.

use std::sync::{Arc, Mutex};

use sikemux_core::client::CoreClient;
use sikemux_core::protocol::Request;
use sikemux_process::sync::Locked;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::login_item::{self, CoreLaunch};
use crate::pty::{core_error, PtyManager};

/// What the app last published of each kind, sent again to a core it
/// reconnects to, which keeps it in memory only.
#[derive(Default)]
pub struct Published(Mutex<Vec<Request>>);

const PUBLISH_BUDDIES: &str = "publishBuddies";

fn is_publication(request: &Request) -> bool {
    match request {
        Request::PublishBackdrop { .. }
        | Request::PublishPalette { .. }
        | Request::PublishOnScreen { .. } => true,
        Request::Call { method, .. } => method == PUBLISH_BUDDIES,
        _ => false,
    }
}

fn same_publication(kept: &Request, request: &Request) -> bool {
    match (kept, request) {
        (Request::Call { method: kept, .. }, Request::Call { method, .. }) => kept == method,
        _ => std::mem::discriminant(kept) == std::mem::discriminant(request),
    }
}

/// How the app looks and what it shows, for paired devices: the backdrop,
/// the theme's colours, the buddies and the agents on screen.
#[tauri::command]
pub async fn remote_publish(
    manager: State<'_, PtyManager>,
    published: State<'_, Published>,
    request: Request,
) -> AppResult<()> {
    if !is_publication(&request) {
        return Err(AppError::Other(
            "only a publication can be published".into(),
        ));
    }
    {
        let mut kept = published.0.locked();
        kept.retain(|kept| !same_publication(kept, &request));
        kept.push(request.clone());
    }
    let client = manager.client().await?;
    client.request(request).await.map_err(core_error)?;
    Ok(())
}

/// Gives a core the app just connected to what it published to the last one,
/// and keeps the login item in step with its remote access switch.
pub(crate) async fn connected(
    app: &AppHandle,
    launch: Option<&CoreLaunch>,
    client: &Arc<CoreClient>,
) {
    let kept = app
        .try_state::<Published>()
        .map(|published| published.0.locked().clone())
        .unwrap_or_default();
    for request in kept {
        if let Err(error) = client.request(request).await {
            eprintln!("Sikemux could not tell its core again what it published: {error}");
        }
    }
    if let Ok(status) = client.remote_status().await {
        login_item::sync(&app.config().identifier, launch, status.enabled);
    }
    crate::hosts::follow_links(app);
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
