//! Push-to-talk dictation, through `sikemux-voice`, with what it reports relayed to the
//! window as `voice` events.

use std::sync::{Arc, OnceLock};

use sikemux_voice::{Status, Voice};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

use crate::error::{AppError, AppResult};

pub const VOICE_EVENT: &str = "voice";

/// The app's dictation, made the first time it is asked for, once the app knows where its
/// data lives.
#[derive(Clone, Default)]
pub struct VoiceManager {
    voice: Arc<OnceLock<Voice>>,
}

impl VoiceManager {
    pub fn drain(&self) {
        if let Some(voice) = self.voice.get() {
            voice.shutdown();
        }
    }

    fn voice(&self, app: &AppHandle) -> AppResult<&Voice> {
        if let Some(voice) = self.voice.get() {
            return Ok(voice);
        }
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| AppError::Other(format!("voice data directory unavailable: {error}")))?
            .join("voice");
        let (events, mut heard) = mpsc::unbounded_channel();
        let relay = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = heard.recv().await {
                let _ = relay.emit_to("main", VOICE_EVENT, event);
            }
        });
        Ok(self.voice.get_or_init(|| Voice::new(dir, events)))
    }
}

#[tauri::command]
pub async fn voice_status(app: AppHandle, voice: State<'_, VoiceManager>) -> AppResult<Status> {
    Ok(voice.voice(&app)?.status())
}

#[tauri::command]
pub async fn voice_prepare(app: AppHandle, voice: State<'_, VoiceManager>) -> AppResult<()> {
    voice.voice(&app)?.prepare().map_err(AppError::Other)
}

#[tauri::command]
pub async fn voice_start(app: AppHandle, voice: State<'_, VoiceManager>) -> AppResult<()> {
    voice.voice(&app)?.start().map_err(AppError::Other)
}

#[tauri::command]
pub async fn voice_stop(app: AppHandle, voice: State<'_, VoiceManager>) -> AppResult<()> {
    voice.voice(&app)?.stop().map_err(AppError::Other)
}

#[tauri::command]
pub async fn voice_shutdown(voice: State<'_, VoiceManager>) -> AppResult<()> {
    voice.drain();
    Ok(())
}

#[tauri::command]
pub async fn voice_cancel(app: AppHandle, voice: State<'_, VoiceManager>) -> AppResult<()> {
    voice.voice(&app)?.cancel().map_err(AppError::Other)
}
