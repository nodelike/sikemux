use tauri::AppHandle;
use tauri_plugin_updater::{Updater, UpdaterExt};

use crate::error::{AppError, AppResult};

const STABLE_ENDPOINT: &str =
    "https://github.com/nodelike/sikemux/releases/latest/download/latest.json";
const PREVIEW_ENDPOINT: &str =
    "https://github.com/nodelike/sikemux/releases/download/preview/latest.json";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

fn updater(app: &AppHandle, channel: &str) -> AppResult<Updater> {
    let endpoint = match channel {
        "stable" => STABLE_ENDPOINT,
        "preview" => PREVIEW_ENDPOINT,
        _ => return Err(AppError::BadArg("update channel must be stable or preview")),
    };
    let url = endpoint
        .parse()
        .map_err(|error| AppError::Other(format!("update endpoint: {error}")))?;
    app.updater_builder()
        .endpoints(vec![url])
        .and_then(|builder| builder.build())
        .map_err(|error| AppError::Other(format!("updater: {error}")))
}

#[tauri::command]
pub async fn update_check(app: AppHandle, channel: String) -> AppResult<Option<UpdateInfo>> {
    Ok(updater(&app, &channel)?
        .check()
        .await
        .map_err(|error| AppError::Other(format!("update check: {error}")))?
        .map(|update| UpdateInfo {
            version: update.version,
            current_version: update.current_version,
            notes: update.body,
            date: update.date.map(|date| date.to_string()),
        }))
}

#[tauri::command]
pub async fn update_install(app: AppHandle, channel: String) -> AppResult<()> {
    let update = updater(&app, &channel)?
        .check()
        .await
        .map_err(|error| AppError::Other(format!("update check: {error}")))?
        .ok_or_else(|| AppError::Other("no update is available".into()))?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| AppError::Other(format!("update install: {error}")))
}
